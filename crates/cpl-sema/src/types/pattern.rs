//! `match` typing, pattern typing (`spec/semantics.md` 3.12), and the
//! constructor sets of S25 for the exhaustiveness checker
//! ([`crate::exhaust`], 3.13).

use super::infer::{is_poison, subst};
use super::typer::{Out, Typer};
use crate::checker::diag;
use crate::codes;
use crate::exhaust::{self, CtorKey, CtorSig, Pat, Space};
use crate::refine::Refinements;
use crate::resolve::{Ctor, Res};
use cpl_diag::Span;
use cpl_hir::{Arm, Block, DefKind, Expr, ExprKind, Pattern, PatternKind, Program, RefKind, Ty};
use cpl_syntax::ast;
use num_bigint::BigInt;
use serde_json::json;

impl<'t, 'c, 'a> Typer<'t, 'c, 'a> {
    /// `match s { arms }` (3.8): the scrutinee fully resolved, each pattern
    /// against its type, the arm bodies as the branches of an `if` (S19,
    /// S20), exhaustiveness and reachability (3.13), and divergence (S24).
    pub fn match_expr(&mut self, e: &ast::Expr, m: &ast::MatchExpr, exp: Option<&Ty>) -> Out {
        let scrutinee = self.infer(&m.scrutinee);
        let st = self.zonk(&scrutinee.ty);
        self.require_resolved(&st);
        let mut st = self.zonk(&st);
        if is_poison(&st) {
            st = Ty::Error;
        }
        let mut arms = Vec::new();
        let mut pats: Vec<Option<Pat>> = Vec::new();
        let mut arm_ty: Option<Ty> = exp.cloned();
        let mut all_div = true;
        for arm in &m.arms {
            let (pattern, pat) = self.pattern(&arm.pattern, &st);
            pats.push(pat);
            let (body, div) = match &arm.body {
                ast::ArmBody::Block(b) => {
                    let o = self.block(b, arm_ty.as_ref());
                    (o.block, o.div)
                }
                ast::ArmBody::Expr(x) => {
                    let ex = match &arm_ty {
                        Some(t) => {
                            let t = t.clone();
                            self.check(x, &t)
                        }
                        None => self.infer(x),
                    };
                    let ty = ex.ty.clone();
                    (
                        Block {
                            stmts: Vec::new(),
                            tail: Some(Box::new(ex)),
                            ty,
                            span: x.span,
                        },
                        false,
                    )
                }
            };
            // S20: the first non-diverging arm sets the type.
            if arm_ty.is_none() && !div {
                arm_ty = Some(body.ty.clone());
            }
            all_div &= div;
            arms.push(Arm {
                pattern,
                body,
                span: arm.span,
            });
        }
        let space = TySpace {
            program: self.program,
            refinements: &self.tcx.cx.refinements,
        };
        let empty_set = space.sigma(&st).is_some_and(|s| s.is_empty());
        let div = if m.arms.is_empty() {
            !st.is_error() && empty_set
        } else {
            all_div
        };
        if !st.is_error() {
            let check = exhaust::check_match(&space, &st, &pats);
            if !check.missing.is_empty() {
                let ty = self.render(&st);
                self.error(
                    diag(
                        codes::NON_EXHAUSTIVE_MATCH,
                        Some(e.span),
                        format!(
                            "this `match` does not cover {}",
                            check
                                .missing
                                .iter()
                                .map(|w| format!("`{w}`"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    )
                    .with_data("missing", json!(check.missing))
                    .with_data("type", json!(ty)),
                );
            }
            for i in check.unreachable {
                self.error(diag(
                    codes::UNREACHABLE_ARM,
                    Some(m.arms[i].span),
                    "this arm is never reached: the arms before it match every value it matches",
                ));
            }
        }
        let ty = match exp {
            Some(x) => x.clone(),
            None if div => Ty::Error,
            None => arm_ty.unwrap_or(Ty::Error),
        };
        Out {
            e: Expr::new(
                ExprKind::Match {
                    scrutinee: Box::new(scrutinee),
                    arms,
                },
                ty,
                e.span,
            ),
            done: exp.is_some() || div,
            div,
        }
    }

    /// `p ⇐ T` (3.12): the HIR pattern and its form for the exhaustiveness
    /// checker (`None` for an erroneous pattern, which binds the error
    /// type and takes part in no check, S62).
    pub fn pattern(&mut self, p: &ast::Pattern, t: &Ty) -> (Pattern, Option<Pat>) {
        let s = self.strip(t);
        let poison = is_poison(&s);
        let wild = |t: &Ty| Pattern {
            kind: PatternKind::Wildcard,
            ty: t.clone(),
            span: p.span,
        };
        match &p.kind {
            ast::PatternKind::Wildcard => (wild(t), Some(Pat::Wild)),
            ast::PatternKind::Binding(n) => match self.tcx.cx.names.binders.get(&n.span).copied() {
                Some(local) => {
                    self.set_local_ty(local, if poison { Ty::Error } else { t.clone() });
                    (
                        Pattern {
                            kind: PatternKind::Bind(local),
                            ty: t.clone(),
                            span: p.span,
                        },
                        Some(Pat::Wild),
                    )
                }
                None => (wild(t), Some(Pat::Wild)),
            },
            ast::PatternKind::Int { negative, digits } => {
                let mut v: BigInt = digits.parse().unwrap_or_default();
                if *negative {
                    v = -v;
                }
                self.literal_pattern(
                    p.span,
                    t,
                    &s,
                    Ty::Int,
                    "Int",
                    PatternKind::Int(v.clone()),
                    CtorKey::Int(v),
                )
            }
            ast::PatternKind::Text(x) => self.literal_pattern(
                p.span,
                t,
                &s,
                Ty::Text,
                "Text",
                PatternKind::Text(x.clone()),
                CtorKey::Text(x.clone()),
            ),
            ast::PatternKind::Bool(b) => self.literal_pattern(
                p.span,
                t,
                &s,
                Ty::Bool,
                "Bool",
                PatternKind::Bool(*b),
                CtorKey::Bool(*b),
            ),
            ast::PatternKind::Ctor(cp) => self.ctor_pattern(p, cp, t, &s),
        }
    }

    #[allow(clippy::too_many_arguments)] // one row of the 3.12 table per literal kind
    fn literal_pattern(
        &mut self,
        span: Span,
        t: &Ty,
        s: &Ty,
        want: Ty,
        kind_name: &str,
        kind: PatternKind,
        key: CtorKey,
    ) -> (Pattern, Option<Pat>) {
        if is_poison(s) {
            return (
                Pattern {
                    kind: PatternKind::Wildcard,
                    ty: t.clone(),
                    span,
                },
                None,
            );
        }
        if *s != want {
            self.pattern_type_mismatch(span, t, kind_name);
            return (
                Pattern {
                    kind: PatternKind::Wildcard,
                    ty: t.clone(),
                    span,
                },
                None,
            );
        }
        (
            Pattern {
                kind,
                ty: t.clone(),
                span,
            },
            Some(Pat::Ctor(key, Vec::new())),
        )
    }

    fn ctor_pattern(
        &mut self,
        p: &ast::Pattern,
        cp: &ast::CtorPattern,
        t: &Ty,
        s: &Ty,
    ) -> (Pattern, Option<Pat>) {
        let wild = Pattern {
            kind: PatternKind::Wildcard,
            ty: t.clone(),
            span: p.span,
        };
        let subs: &[ast::Pattern] = cp.args.as_deref().unwrap_or(&[]);
        if is_poison(s) {
            self.bind_errors_in(subs);
            return (wild, None);
        }
        let res = self.tcx.cx.names.patterns.get(&cp.path.span).cloned();
        let ctor = match res {
            Some(Res::Ctor(c)) => Some(c),
            Some(Res::DeferredCtor { .. }) => self.resolve_ctor(&cp.path.name, Some(t)),
            _ => None,
        };
        let Some(ctor) = ctor else {
            self.bind_errors_in(subs);
            return (wild, None);
        };
        match ctor {
            Ctor::Variant { def, index } => {
                let (Ty::Adt(d, args), Some(en)) = (s, self.program.def(def).kind.as_enum()) else {
                    let name = self.program.def(def).name.display();
                    self.pattern_type_mismatch(p.span, t, &name);
                    self.bind_errors_in(subs);
                    return (wild, None);
                };
                if *d != def {
                    let name = self.program.def(def).name.display();
                    self.pattern_type_mismatch(p.span, t, &name);
                    self.bind_errors_in(subs);
                    return (wild, None);
                }
                let payload: Vec<Ty> = en
                    .variants
                    .get(index as usize)
                    .map(|v| v.payload.iter().map(|x| subst(x, args)).collect())
                    .unwrap_or_default();
                self.add_ref(def, RefKind::Ctor, cp.path.span);
                if payload.len() != subs.len() {
                    self.pattern_arity_mismatch(
                        p.span,
                        &cp.path.name.text,
                        payload.len(),
                        subs.len(),
                    );
                    self.bind_errors_in(subs);
                    return (wild, None);
                }
                let mut hir = Vec::new();
                let mut pats = Vec::new();
                let mut ok = true;
                for (sub, pt) in subs.iter().zip(&payload) {
                    let (h, q) = self.pattern(sub, pt);
                    hir.push(h);
                    match q {
                        Some(q) => pats.push(q),
                        None => ok = false,
                    }
                }
                (
                    Pattern {
                        kind: PatternKind::Ctor {
                            def,
                            variant: index,
                            args: hir,
                        },
                        ty: t.clone(),
                        span: p.span,
                    },
                    ok.then_some(Pat::Ctor(CtorKey::Variant(index), pats)),
                )
            }
            Ctor::Wrapper(def) => {
                if *s != Ty::Wrapper(def) {
                    let name = self.program.def(def).name.display();
                    self.pattern_type_mismatch(p.span, t, &name);
                    self.bind_errors_in(subs);
                    return (wild, None);
                }
                self.add_ref(def, RefKind::Ctor, cp.path.span);
                if subs.len() != 1 {
                    self.pattern_arity_mismatch(p.span, &cp.path.name.text, 1, subs.len());
                    self.bind_errors_in(subs);
                    return (wild, None);
                }
                let inner = self
                    .program
                    .def(def)
                    .kind
                    .as_wrapper()
                    .map_or(Ty::Error, |w| w.inner.clone());
                let (h, q) = self.pattern(&subs[0], &inner);
                (
                    Pattern {
                        kind: PatternKind::Unwrap {
                            def,
                            arg: Box::new(h),
                        },
                        ty: t.clone(),
                        span: p.span,
                    },
                    q.map(|q| Pat::Ctor(CtorKey::Single, vec![q])),
                )
            }
        }
    }

    /// The bindings of erroneous sub-patterns have the error type.
    fn bind_errors_in(&mut self, subs: &[ast::Pattern]) {
        for sub in subs {
            match &sub.kind {
                ast::PatternKind::Binding(n) => {
                    if let Some(local) = self.tcx.cx.names.binders.get(&n.span).copied() {
                        self.set_local_ty(local, Ty::Error);
                    }
                }
                ast::PatternKind::Ctor(cp) => {
                    if let Some(args) = &cp.args {
                        self.bind_errors_in(args);
                    }
                }
                _ => {}
            }
        }
    }

    fn pattern_type_mismatch(&mut self, span: Span, t: &Ty, actual: &str) {
        let expected = self.render(t);
        self.error(
            diag(
                codes::PATTERN_TYPE_MISMATCH,
                Some(span),
                format!("this pattern matches a `{actual}`, but the value is a `{expected}`"),
            )
            .with_data("expected", json!(expected))
            .with_data("actual", json!(actual)),
        );
    }

    fn pattern_arity_mismatch(&mut self, span: Span, name: &str, expected: usize, actual: usize) {
        self.error(
            diag(
                codes::PATTERN_ARITY_MISMATCH,
                Some(span),
                format!(
                    "`{name}` has {expected} payload{}, the pattern gives {actual}",
                    if expected == 1 { "" } else { "s" }
                ),
            )
            .with_data("expected", json!(expected))
            .with_data("actual", json!(actual)),
        );
    }
}

/// The constructor sets of S25 over HIR types (resolved, without
/// variables).
pub(crate) struct TySpace<'p> {
    pub program: &'p Program,
    pub refinements: &'p Refinements,
}

impl Space for TySpace<'_> {
    type Ty = Ty;

    fn sigma(&self, ty: &Ty) -> Option<Vec<CtorSig<Ty>>> {
        let t = self.refinements.strip(ty);
        match &t {
            Ty::Bool => Some(vec![
                CtorSig {
                    key: CtorKey::Bool(true),
                    name: Some("true".into()),
                    fields: Vec::new(),
                },
                CtorSig {
                    key: CtorKey::Bool(false),
                    name: Some("false".into()),
                    fields: Vec::new(),
                },
            ]),
            Ty::Adt(def, args) => match &self.program.def(*def).kind {
                DefKind::Enum(en) => Some(
                    en.variants
                        .iter()
                        .enumerate()
                        .map(|(i, v)| CtorSig {
                            key: CtorKey::Variant(i as u32),
                            name: Some(v.name.clone()),
                            fields: v.payload.iter().map(|p| subst(p, args)).collect(),
                        })
                        .collect(),
                ),
                DefKind::Struct(sd) => Some(vec![CtorSig {
                    key: CtorKey::Single,
                    name: None,
                    fields: sd.fields.iter().map(|f| subst(&f.ty, args)).collect(),
                }]),
                _ => None,
            },
            Ty::Wrapper(def) => Some(vec![CtorSig {
                key: CtorKey::Single,
                name: Some(self.program.def(*def).name.display()),
                fields: vec![self
                    .program
                    .def(*def)
                    .kind
                    .as_wrapper()
                    .map_or(Ty::Error, |w| w.inner.clone())],
            }]),
            _ => None,
        }
    }
}
