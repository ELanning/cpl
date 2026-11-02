//! The per-declaration typer state, the entry points for each kind of
//! definition, statement frames (3.7: inference variables are scoped to
//! their statement, clause, or `for` range), assignability (3.4) with the
//! compile-time proof rule (3.5, S16, S65), and the final zonking of the
//! HIR.

use super::holes::HoleSite;
use super::infer::{is_poison, mentions_error, render, Vars};
use super::{DefOutput, KindOutput, TypeCx};
use crate::checker::{diag, Stage};
use crate::codes;
use crate::consteval::{self, Fail};
use crate::resolve::Resolver;
use cpl_diag::{Diagnostic, Span};
use cpl_hir::{
    Binder, Block, DefId, Expr, ExprKind, GenExpr, LocalDef, LocalId, Pattern, PatternKind,
    Program, RefKind, Reference, RefinementProof, StdDefs, Stmt, Ty,
};
use cpl_syntax::ast::{self, DeclKind, FnClause};
use serde_json::json;
use std::collections::{HashMap, HashSet};

/// What kind of definition is being typed.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Owner {
    Fn,
    /// A capability operation's clauses.
    Op,
    Refinement,
    Test,
    Property,
}

/// How a failed assignability test is reported.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum How {
    /// `type_mismatch` at the expression.
    Normal,
    /// A non-tail expression statement: `unused_value` (3.11, S68).
    Unused,
    /// A clause that must have a type: `type_mismatch` at the clause (S63 (2)).
    Clause(Span),
}

/// What [`Typer::unit_operand`] decided for a missing operand that stands
/// for the constant expression `Unit` (3.10, 3.11).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum UnitOperand {
    /// The expected type is `Unit` (rule 1 of 3.4) or erroneous (S62):
    /// nothing to report.
    Unit,
    /// The expected type is a refinement of `Unit`: the proof rule of 3.5
    /// ran, and `proved` is false when it reported its diagnostic.
    Refinement { proved: bool },
    /// Rule 4 of 3.4: the caller reports its own mismatch (`type_mismatch`
    /// with the note "missing tail expression", `return_value_missing`).
    Mismatch,
}

/// One typed expression.
pub(crate) struct Out {
    pub e: Expr,
    /// The form already applied the expected type (an `if`, a `match`, a
    /// hole, a diverging form, S19): no whole-form `≤` test follows.
    pub done: bool,
    /// The expression diverges (S24).
    pub div: bool,
}

impl Out {
    pub fn new(e: Expr) -> Out {
        Out {
            e,
            done: false,
            div: false,
        }
    }

    pub fn done(e: Expr) -> Out {
        Out {
            e,
            done: true,
            div: false,
        }
    }
}

/// A hole until its statement ends (3.8: its expected type must be fully
/// resolved then).
pub(crate) struct PendingHole {
    pub name: String,
    pub span: Span,
    pub ty: Ty,
    pub frame: u32,
    pub state: HoleState,
    /// The invariants of the enclosing `while` loops, innermost first.
    pub invariants: Vec<String>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum HoleState {
    /// Has an expected type (possibly with variables still to resolve).
    Pending,
    /// `hole_type_unknown` was reported (S64: no report accompanies it).
    Unknown,
    /// In a contract expression: the contract stage's `hole_in_contract`
    /// alone (S64).
    Contract,
}

/// The typer of one definition (a function, a test, a property, a
/// refinement predicate, or the clauses of a capability's operations).
pub(crate) struct Typer<'t, 'c, 'a> {
    pub tcx: &'t TypeCx<'c, 'a>,
    pub program: &'c Program,
    pub r: Resolver<'c>,
    pub def: DefId,
    pub owner: Owner,
    pub generics: &'c [String],
    /// The declared return type of a function (`Unit` elsewhere).
    pub ret: Ty,
    pub std: StdDefs,
    /// The local table being typed (parameters first).
    pub locals: Vec<LocalDef>,
    pub vars: Vars,
    /// The open statement frames: (frame id, first variable created in it).
    frames: Vec<(u32, usize)>,
    next_frame: u32,
    pub holes: Vec<PendingHole>,
    /// The type of every expression that created inference variables, by
    /// span (the `type` of `cannot_infer_type`).
    pub creators: HashMap<Span, Ty>,
    reported_infer: HashSet<Span>,
    /// The invariant texts of the enclosing `while` loops, outermost first.
    pub whiles: Vec<Vec<String>>,
    /// Inside a contract expression (5.1).
    pub contract: bool,
    /// References are interface references (T17).
    pub interface: bool,
    /// The next expression may be a generator (S14: a `for` range or a
    /// `std.gen` argument); consumed by `Typer::expr`.
    pub gen_ok: bool,
    pub diags: Vec<(Stage, Diagnostic)>,
    pub refs: Vec<Reference>,
    /// The last `coerce` reported a failure (read right after a tail check).
    pub last_coerce_failed: bool,
}

impl<'t, 'c, 'a> Typer<'t, 'c, 'a> {
    pub fn new(
        tcx: &'t TypeCx<'c, 'a>,
        def: DefId,
        owner: Owner,
        locals: Vec<LocalDef>,
        ret: Ty,
    ) -> Typer<'t, 'c, 'a> {
        let cx = tcx.cx;
        let generics: &'c [String] = cx.program.def(def).kind.generics();
        Typer {
            tcx,
            program: &cx.program,
            r: cx.resolver(def, generics),
            def,
            owner,
            generics,
            ret,
            std: cx.program.std_defs,
            locals,
            vars: Vars::default(),
            frames: Vec::new(),
            next_frame: 0,
            holes: Vec::new(),
            creators: HashMap::new(),
            reported_infer: HashSet::new(),
            whiles: Vec::new(),
            contract: false,
            interface: false,
            gen_ok: false,
            diags: Vec::new(),
            refs: Vec::new(),
            last_coerce_failed: false,
        }
    }

    // ---- small helpers ---------------------------------------------------------

    pub fn unit(&self) -> Ty {
        Ty::unit(&self.std)
    }

    pub fn error(&mut self, d: Diagnostic) {
        self.diags.push((Stage::Type, d));
    }

    pub fn emit(&mut self, stage: Stage, d: Diagnostic) {
        self.diags.push((stage, d));
    }

    pub fn fresh(&mut self, span: Span) -> Ty {
        let frame = self.frame();
        self.vars.fresh(span, frame)
    }

    /// Record the type of an expression that created variables.
    pub fn created(&mut self, span: Span, ty: &Ty) {
        self.creators.insert(span, ty.clone());
    }

    pub fn resolve(&self, t: &Ty) -> Ty {
        self.vars.resolve(t)
    }

    pub fn zonk(&self, t: &Ty) -> Ty {
        self.vars.zonk(t)
    }

    /// `strip(T)` (3.3) after resolving variables at the top level.
    pub fn strip(&self, t: &Ty) -> Ty {
        let t = self.vars.resolve(t);
        if matches!(t, Ty::Refined(..)) {
            self.tcx.cx.refinements.strip(&t)
        } else {
            t
        }
    }

    /// `erase(T)` (3.3) with variables resolved at every level.
    pub fn erase(&self, t: &Ty) -> Ty {
        match self.strip(t) {
            Ty::List(e) => Ty::List(Box::new(self.erase(&e))),
            Ty::Gen(e) => Ty::Gen(Box::new(self.erase(&e))),
            Ty::Adt(d, args) => Ty::Adt(d, args.iter().map(|a| self.erase(a)).collect()),
            other => other,
        }
    }

    pub fn render(&self, t: &Ty) -> String {
        render(self.program, &self.zonk(t), self.generics, false)
    }

    /// Render two types; qualified when the bare renderings coincide but
    /// the types differ (S65).
    pub fn render_pair(&self, a: &Ty, b: &Ty) -> (String, String) {
        let (a, b) = (self.zonk(a), self.zonk(b));
        let ra = render(self.program, &a, self.generics, false);
        let rb = render(self.program, &b, self.generics, false);
        if ra == rb && a != b {
            (
                render(self.program, &a, self.generics, true),
                render(self.program, &b, self.generics, true),
            )
        } else {
            (ra, rb)
        }
    }

    pub fn add_ref(&mut self, to: DefId, kind: RefKind, span: Span) {
        self.refs
            .push(super::reference(self.def, to, kind, span, self.interface));
    }

    pub fn local_ty(&self, id: LocalId) -> Ty {
        self.locals
            .get(id.0 as usize)
            .map_or(Ty::Error, |l| l.ty.clone())
    }

    pub fn set_local_ty(&mut self, id: LocalId, ty: Ty) {
        if let Some(l) = self.locals.get_mut(id.0 as usize) {
            l.ty = ty;
        }
    }

    pub fn error_expr(&self, span: Span) -> Expr {
        Expr::new(ExprKind::Error, Ty::Error, span)
    }

    /// The canonical text of an expression (toolchain 5.2).
    pub fn text(&self, e: &ast::Expr) -> String {
        crate::collect::clause_text(e)
    }

    // ---- frames (3.7) ----------------------------------------------------------

    /// The innermost open frame.
    pub fn frame(&self) -> u32 {
        self.frames.last().map_or(0, |f| f.0)
    }

    pub fn push_frame(&mut self) {
        self.next_frame += 1;
        self.frames.push((self.next_frame, self.vars.info.len()));
    }

    /// Close the innermost frame: holes whose expected type is still not
    /// fully resolved are `hole_type_unknown` (3.8, S56), and every
    /// variable the statement created that is still unbound is
    /// `cannot_infer_type` at the expression that created it (3.7).
    pub fn pop_frame(&mut self) {
        let Some((frame, first_var)) = self.frames.pop() else {
            return;
        };
        let parent = self.frames.last().map(|f| f.0);
        for i in 0..self.holes.len() {
            if self.holes[i].frame != frame || self.holes[i].state != HoleState::Pending {
                continue;
            }
            let mut unbound = Vec::new();
            self.vars.unbound(&self.holes[i].ty, &mut unbound);
            if unbound.is_empty() {
                continue;
            }
            let outer = unbound
                .iter()
                .any(|v| self.vars.info[*v as usize].frame != frame);
            match parent {
                Some(p) if outer => self.holes[i].frame = p,
                _ => {
                    for v in unbound {
                        self.vars.bind_error(v);
                    }
                    let (name, span) = (self.holes[i].name.clone(), self.holes[i].span);
                    self.holes[i].state = HoleState::Unknown;
                    self.hole_type_unknown(&name, span);
                }
            }
        }
        // Only variables created since the frame opened can belong to it
        // (those of frames closed inside it are bound already).
        for v in first_var..self.vars.info.len() {
            if self.vars.info[v].frame != frame {
                continue;
            }
            let t = self
                .vars
                .resolve(&Ty::Param(super::infer::VAR_BASE + v as u32));
            if let Some(rep) = super::infer::var_of(&t) {
                if self.vars.info[rep as usize].frame == frame {
                    let span = self.vars.info[rep as usize].span;
                    self.cannot_infer(span);
                    self.vars.bind_error(rep);
                }
            }
        }
    }

    /// `cannot_infer_type` at `span` (once per span), with the creating
    /// expression's type rendered with `?` for its unknown parts.
    pub fn cannot_infer(&mut self, span: Span) {
        if !self.reported_infer.insert(span) {
            return;
        }
        let ty = self
            .creators
            .get(&span)
            .map(|t| self.render(t))
            .unwrap_or_else(|| "?".to_string());
        let d = diag(
            codes::CANNOT_INFER_TYPE,
            Some(span),
            format!("cannot infer the type of this expression (`{ty}`)"),
        )
        .with_data("type", json!(ty))
        .with_note("add a type annotation", None);
        self.error(d);
    }

    /// Report `cannot_infer_type` for every unbound variable of `t` now
    /// (a position that needs a fully resolved type: a `match` scrutinee,
    /// a receiver) and bind them to the error type.
    pub fn require_resolved(&mut self, t: &Ty) {
        let mut unbound = Vec::new();
        self.vars.unbound(t, &mut unbound);
        for v in unbound {
            let span = self.vars.info[v as usize].span;
            self.cannot_infer(span);
            self.vars.bind_error(v);
        }
    }

    pub fn hole_type_unknown(&mut self, name: &str, span: Span) {
        let d = diag(
            codes::HOLE_TYPE_UNKNOWN,
            Some(span),
            format!(
                "the hole `?{name}` has no expected type here; give the binding an annotation or move the hole to a typed position"
            ),
        )
        .with_data("hole", json!(name));
        let d = crate::checker::blame(d, self.tcx.cx, self.def, Some(cpl_diag::Facet::Body));
        self.emit(Stage::Hole, d);
    }

    /// Type a root (a body, clause, or range) in its own frame.
    pub fn root<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        self.push_frame();
        let r = f(self);
        self.pop_frame();
        r
    }

    /// A clause of type `Bool` (`requires`, `ensures`, `invariant`,
    /// `where`): a contract expression (5.1) whose type mismatch spans the
    /// clause (S63 (2)).
    pub fn clause_bool(&mut self, e: &ast::Expr, span: Span) -> Expr {
        let saved = self.contract;
        self.contract = true;
        let out = self.root(|t| t.check_how(e, &Ty::Bool, How::Clause(span)));
        self.contract = saved;
        out
    }

    /// A `decreases` clause: `strip(T) ≡ Int` (5.1, 3.11).
    pub fn clause_measure(&mut self, e: &ast::Expr, span: Span) -> Expr {
        let saved = self.contract;
        self.contract = true;
        let out = self.root(|t| {
            let x = t.infer(e);
            let s = t.strip(&x.ty);
            if is_poison(&s) || t.vars.unify(&s, &Ty::Int) {
                t.widen_to_root(x)
            } else {
                let actual = t.render(&x.ty);
                t.error(
                    diag(
                        codes::TYPE_MISMATCH,
                        Some(span),
                        format!("a `decreases` measure is an `Int`, found `{actual}`"),
                    )
                    .with_data("expected", json!("Int"))
                    .with_data("actual", json!(actual)),
                );
                x
            }
        });
        self.contract = saved;
        out
    }

    // ---- check and infer (3.7) ---------------------------------------------------

    pub fn infer(&mut self, e: &ast::Expr) -> Expr {
        self.expr(e, None).e
    }

    pub fn check(&mut self, e: &ast::Expr, expected: &Ty) -> Expr {
        self.check_how(e, expected, How::Normal)
    }

    /// Check mode: the expected type flows into the form, then `A ≤ E`
    /// unless the form applied it itself (S19).
    pub fn check_how(&mut self, e: &ast::Expr, expected: &Ty, how: How) -> Expr {
        let out = self.expr(e, Some(expected));
        self.last_coerce_failed = false;
        if out.done {
            return out.e;
        }
        self.coerce(out.e, e, expected, how)
    }

    // ---- assignability (3.4) -----------------------------------------------------

    /// `A ≤ E` for the typed expression `e` (3.4): identity, refinement to
    /// base, widening along the chain (`Widen` nodes), the compile-time
    /// proof rule (a `Refine` node), or the failure diagnostic.
    pub fn coerce(&mut self, e: Expr, ast: &ast::Expr, expected: &Ty, how: How) -> Expr {
        self.last_coerce_failed = false;
        let a = self.zonk(&e.ty);
        let ex = self.zonk(expected);
        if is_poison(&a) {
            self.vars.bind_errors(&ex);
            return e;
        }
        if is_poison(&ex) {
            self.vars.bind_errors(&a);
            return e;
        }
        // Rule 1.
        if self.vars.unify(&a, &ex) {
            return e;
        }
        let ex_top = self.resolve(&ex);
        if let Ty::Refined(_, edef) = ex_top {
            // Rule 2': widening along chain(A).
            if let Ty::Refined(_, adef) = a {
                let chain = self.tcx.cx.refinements.chain(adef);
                if let Some(pos) = chain
                    .iter()
                    .position(|d| self.tcx.cx.refinements.ty_of(*d) == ex_top)
                {
                    return self.widen_steps(e, &chain[..pos]);
                }
            }
            // Rule 3: the proof rule.
            let sa = self.strip(&a);
            let se = self.strip(&ex_top);
            if self.vars.unify(&sa, &se) {
                return self.prove(e, Some(ast), &ex_top, edef, how);
            }
        } else if let Ty::Refined(..) = a {
            // Rule 2: refinement to base, top level only.
            let sa = self.strip(&a);
            if self.vars.unify(&sa, &ex) {
                return self.widen_to_root(e);
            }
        }
        // Rule 4. Neither side's unknowns are worth a `cannot_infer_type`
        // after the mismatch (S62).
        self.mismatch(&e, &a, &ex, how);
        self.vars.bind_errors(&ex);
        self.vars.bind_errors(&a);
        e
    }

    /// A missing operand (a block without a tail, 3.10; a `return` without
    /// a value, 3.11) stands for the constant expression `Unit` checked
    /// against `expected` by 3.4: rule 1 accepts `Unit` itself; for a
    /// refinement of `Unit` rule 3 applies, so the proof rule of 3.5 (S16)
    /// evaluates the chain's predicates on `Unit` and reports
    /// `refinement_false` (or `constant_evaluation_fault`,
    /// `cannot_prove_refinement`) spanning `span`, with `note`; any other
    /// type is rule 4, which the caller reports. The HIR keeps the operand
    /// absent: the value is `Unit` either way, and the proof is a static
    /// fact with no runtime check (3.5).
    pub fn unit_operand(&mut self, span: Span, expected: &Ty, note: &str) -> UnitOperand {
        let unit = self.unit();
        let ex = self.zonk(expected);
        if is_poison(&ex) || self.vars.unify(&ex, &unit) {
            return UnitOperand::Unit;
        }
        let top = self.resolve(&ex);
        let Ty::Refined(_, edef) = top else {
            return UnitOperand::Mismatch;
        };
        let base = self.strip(&top);
        if is_poison(&base) {
            return UnitOperand::Unit;
        }
        if !self.vars.unify(&base, &unit) {
            return UnitOperand::Mismatch;
        }
        let operand = Expr::new(
            ExprKind::Ctor {
                def: self.std.unit,
                variant: 0,
                args: Vec::new(),
            },
            unit,
            span,
        );
        let before = self.diags.len();
        self.last_coerce_failed = false;
        self.prove(operand, None, &top, edef, How::Normal);
        let proved = !self.last_coerce_failed;
        if self.diags.len() > before {
            if let Some((_, d)) = self.diags.last_mut() {
                d.notes.push(cpl_diag::Note::new(note));
            }
        }
        UnitOperand::Refinement { proved }
    }

    /// Report the failure of `A ≤ E` (rule 4).
    pub fn mismatch(&mut self, e: &Expr, a: &Ty, ex: &Ty, how: How) {
        self.last_coerce_failed = true;
        let (expected, actual) = self.render_pair(ex, a);
        let d = match how {
            How::Unused => diag(
                codes::UNUSED_VALUE,
                Some(e.span),
                format!("this statement's value of type `{actual}` is unused; an expression statement must have type `Unit`"),
            )
            .with_data("type", json!(actual))
            .with_note("use `let _ = ...` to discard", None),
            How::Normal | How::Clause(_) => {
                let span = match how {
                    How::Clause(s) => s,
                    _ => e.span,
                };
                let mut d = diag(
                    codes::TYPE_MISMATCH,
                    Some(span),
                    format!("expected `{expected}`, found `{actual}`"),
                )
                .with_data("expected", json!(expected))
                .with_data("actual", json!(actual));
                let (ea, ee) = (self.erase(a), self.erase(ex));
                if !mentions_error(&ea) && ea == ee {
                    d = d.with_note(
                        "refinements convert only at the top level (`List<Positive>` is not a `List<Int>`); rebuild the value",
                        None,
                    );
                }
                d
            }
        };
        self.error(d);
    }

    /// Widen a refinement-typed expression to its root base (rule 2).
    pub fn widen_to_root(&mut self, e: Expr) -> Expr {
        match self.resolve(&e.ty) {
            Ty::Refined(_, d) => {
                let chain = self.tcx.cx.refinements.chain(d);
                self.widen_steps(e, &chain)
            }
            _ => e,
        }
    }

    /// One `Widen` per refinement of `steps`, from the expression's type
    /// down (rule 2').
    pub fn widen_steps(&mut self, mut e: Expr, steps: &[DefId]) -> Expr {
        for &from in steps {
            let base = self
                .tcx
                .cx
                .refinements
                .base(from)
                .cloned()
                .unwrap_or(Ty::Error);
            let span = e.span;
            e = Expr::new(
                ExprKind::Widen {
                    expr: Box::new(e),
                    from,
                },
                base,
                span,
            );
        }
        e
    }

    /// The compile-time proof rule (3.5, S16, S65) for `e` at the
    /// refinement `target` (declared by `edef` at the use site). `ast` is
    /// the written expression, `None` for the `Unit` a missing operand
    /// stands for ([`Typer::unit_operand`]).
    fn prove(
        &mut self,
        e: Expr,
        ast: Option<&ast::Expr>,
        target: &Ty,
        edef: DefId,
        how: How,
    ) -> Expr {
        let a = self.zonk(&e.ty);
        let ename = self.program.def(edef).name.display();
        // The written operand's canonical text, for the notes.
        let written = |t: &Self| ast.map_or_else(|| "Unit".to_string(), |x| t.text(x));
        if let Ty::Refined(_, adef) = a {
            // S65: a distinct refinement of the same root base.
            let written = written(self);
            let (expected, actual) = self.render_pair(target, &a);
            let apred = self.predicate_text(adef);
            let epred = self.predicate_text(edef);
            let d = diag(
                codes::CANNOT_PROVE_REFINEMENT,
                Some(span_of(&e, how)),
                format!("`{actual}` (`{apred}`) is not the refinement `{expected}` (`{epred}`), and membership cannot be proved at compile time"),
            )
            .with_data("expected", json!(expected))
            .with_data("actual", json!(actual))
            .with_note(
                format!(
                    "the predicates differ: `{apred}` and `{epred}`; declare the predicate as `{epred}` or use `check<{ename}>({written})`"
                ),
                None,
            );
            self.last_coerce_failed = true;
            self.error(d);
            return e;
        }
        if !consteval::is_constant(&e) {
            let (expected, actual) = self.render_pair(target, &a);
            let d = diag(
                codes::CANNOT_PROVE_REFINEMENT,
                Some(span_of(&e, how)),
                format!("cannot prove that this `{actual}` is a `{expected}`: only a constant expression is proved against a refinement"),
            )
            .with_data("expected", json!(expected))
            .with_data("actual", json!(actual))
            .with_note(format!("use `check<{ename}>(...)`"), None);
            self.last_coerce_failed = true;
            self.error(d);
            return e;
        }
        let value = match consteval::eval_constant(self.program, &e) {
            Ok(v) => v,
            Err(Fail::Fault(code)) => {
                let text = written(self);
                self.constant_fault(&e, how, code, &text);
                return e;
            }
            Err(Fail::Unsupported(note)) => {
                self.unprovable(&e, how, target, &a, note);
                return e;
            }
        };
        let chain = self.tcx.cx.refinements.chain(edef);
        for &r in chain.iter().rev() {
            let Some(p) = self.tcx.predicate(r) else {
                let name = self.program.def(r).name.display();
                self.unprovable(
                    &e,
                    how,
                    target,
                    &a,
                    format!("the predicate of `{name}` depends on this proof; use `check<{ename}>(...)`"),
                );
                return e;
            };
            if !p.ok {
                // The predicate has its own error (S62): nothing more.
                return e;
            }
            match consteval::eval_predicate(self.program, r, &p.expr, p.self_local, value.clone()) {
                Ok(true) => {}
                Ok(false) => {
                    let clause = self.predicate_text(r);
                    let expected = self.render(target);
                    let shown = consteval::value_text(self.program, &value);
                    let d = diag(
                        codes::REFINEMENT_FALSE,
                        Some(span_of(&e, how)),
                        format!("`{shown}` is not a `{expected}`: it does not satisfy `{clause}`"),
                    )
                    .with_data("expected", json!(expected))
                    .with_data("actual", consteval::value_json(self.program, &value))
                    .with_data("clause", json!(clause));
                    self.last_coerce_failed = true;
                    self.error(d);
                    return e;
                }
                Err(Fail::Fault(code)) => {
                    let clause = self.predicate_text(r);
                    self.constant_fault(&e, how, code, &clause);
                    return e;
                }
                Err(Fail::Unsupported(note)) => {
                    self.unprovable(&e, how, target, &a, note);
                    return e;
                }
            }
        }
        let span = e.span;
        Expr::new(
            ExprKind::Refine {
                expr: Box::new(e),
                refined: edef,
                proof: RefinementProof::Constant,
            },
            target.clone(),
            span,
        )
    }

    fn predicate_text(&self, r: DefId) -> String {
        self.program
            .def(r)
            .kind
            .as_refinement()
            .map_or(String::new(), |rd| rd.predicate.text.clone())
    }

    fn unprovable(&mut self, e: &Expr, how: How, target: &Ty, a: &Ty, note: String) {
        let (expected, actual) = self.render_pair(target, a);
        let d = diag(
            codes::CANNOT_PROVE_REFINEMENT,
            Some(span_of(e, how)),
            format!("cannot prove at compile time that this value is a `{expected}`"),
        )
        .with_data("expected", json!(expected))
        .with_data("actual", json!(actual))
        .with_note(note, None);
        self.last_coerce_failed = true;
        self.error(d);
    }

    fn constant_fault(&mut self, e: &Expr, how: How, code: &str, clause: &str) {
        let d = diag(
            codes::CONSTANT_EVALUATION_FAULT,
            Some(span_of(e, how)),
            format!("evaluating `{clause}` at compile time raises the fault `{code}`"),
        )
        .with_data("fault", json!(code))
        .with_data("clause", json!(clause));
        self.last_coerce_failed = true;
        self.error(d);
    }

    // ---- finishing ---------------------------------------------------------------

    /// Zonk the HIR parts, build the hole reports, and collect the output.
    pub fn finish(mut self, kind: KindOutput, site: HoleSite) -> DefOutput {
        let mut kind = kind;
        match &mut kind {
            KindOutput::Nothing => {}
            KindOutput::Fn {
                requires,
                ensures,
                decreases,
                body,
                ..
            } => {
                for e in requires.iter_mut().chain(ensures.iter_mut()) {
                    self.zonk_expr(e);
                }
                if let Some(e) = decreases {
                    self.zonk_expr(e);
                }
                if let Some(b) = body {
                    self.zonk_block(b);
                }
            }
            KindOutput::Refinement { predicate, .. } => self.zonk_expr(predicate),
            KindOutput::Capability { ops } => {
                for (requires, ensures, _) in ops.iter_mut() {
                    for e in requires.iter_mut().chain(ensures.iter_mut()) {
                        self.zonk_expr(e);
                    }
                }
            }
            KindOutput::Test { body, .. } => self.zonk_block(body),
            KindOutput::Property { binders, body, .. } => {
                for b in binders.iter_mut() {
                    b.elem_ty = self.vars.zonk_final(&b.elem_ty);
                    self.zonk_gen(&mut b.gen);
                }
                self.zonk_block(body);
            }
        }
        for l in self.locals.iter_mut() {
            l.ty = self.vars.zonk_final(&l.ty);
        }
        let locals = std::mem::take(&mut self.locals);
        match &mut kind {
            KindOutput::Fn { locals: l, .. }
            | KindOutput::Refinement { locals: l, .. }
            | KindOutput::Test { locals: l, .. }
            | KindOutput::Property { locals: l, .. } => *l = locals.clone(),
            // One typer per operation (`type_capability`).
            KindOutput::Capability { ops } => {
                if let Some(op) = ops.last_mut() {
                    op.2 = locals.clone();
                }
            }
            KindOutput::Nothing => {}
        }
        self.locals = locals;
        let holes = super::holes::build(&self, site);
        let failed = self.diags.iter().any(|(_, d)| d.is_error());
        DefOutput {
            kind,
            diags: self.diags,
            refs: self.refs,
            holes,
            failed,
        }
    }

    fn zonk_expr(&self, e: &mut Expr) {
        e.ty = self.vars.zonk_final(&e.ty);
        match &mut e.kind {
            ExprKind::Lit(_) | ExprKind::Local(_) | ExprKind::Hole { .. } | ExprKind::Error => {}
            ExprKind::Ctor { args, .. } | ExprKind::List(args) => {
                for a in args {
                    self.zonk_expr(a);
                }
            }
            ExprKind::Wrap { arg, .. } | ExprKind::Check { arg, .. } => self.zonk_expr(arg),
            ExprKind::Field { base, .. } => self.zonk_expr(base),
            ExprKind::Call {
                generic_args,
                args,
                using,
                ..
            } => {
                for g in generic_args.iter_mut() {
                    *g = self.vars.zonk_final(g);
                }
                for a in args.iter_mut().chain(using.iter_mut()) {
                    self.zonk_expr(a);
                }
            }
            ExprKind::CapCall { handle, args, .. } => {
                self.zonk_expr(handle);
                for a in args {
                    self.zonk_expr(a);
                }
            }
            ExprKind::Propagate(x) => self.zonk_expr(x),
            ExprKind::Unary { operand, .. } => self.zonk_expr(operand),
            ExprKind::Binary { lhs, rhs, .. } => {
                self.zonk_expr(lhs);
                self.zonk_expr(rhs);
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
                self.zonk_expr(collection);
                self.zonk_expr(predicate);
            }
            ExprKind::StructLit { spread, fields, .. } => {
                if let Some(s) = spread {
                    self.zonk_expr(s);
                }
                for (_, f) in fields {
                    self.zonk_expr(f);
                }
            }
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => {
                self.zonk_expr(cond);
                self.zonk_block(then_block);
                if let Some(b) = else_block {
                    self.zonk_block(b);
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                self.zonk_expr(scrutinee);
                for arm in arms {
                    self.zonk_pattern(&mut arm.pattern);
                    self.zonk_block(&mut arm.body);
                }
            }
            ExprKind::Widen { expr, .. } | ExprKind::Refine { expr, .. } => self.zonk_expr(expr),
            ExprKind::Gen(g) => self.zonk_gen(g),
        }
    }

    fn zonk_gen(&self, g: &mut GenExpr) {
        match g {
            GenExpr::List(x) | GenExpr::Option(x) => self.zonk_gen(x),
            GenExpr::Range(lo, hi) => {
                self.zonk_expr(lo);
                self.zonk_expr(hi);
            }
            _ => {}
        }
    }

    fn zonk_block(&self, b: &mut Block) {
        b.ty = self.vars.zonk_final(&b.ty);
        for s in &mut b.stmts {
            match s {
                Stmt::Let { value, .. } | Stmt::Assign { value, .. } => self.zonk_expr(value),
                Stmt::Discard(e) | Stmt::Expr(e) | Stmt::Assert { expr: e, .. } => {
                    self.zonk_expr(e)
                }
                Stmt::Return(e) => {
                    if let Some(e) = e {
                        self.zonk_expr(e);
                    }
                }
                Stmt::While {
                    cond,
                    invariants,
                    decreases,
                    body,
                } => {
                    self.zonk_expr(cond);
                    for c in invariants.iter_mut() {
                        self.zonk_expr(&mut c.expr);
                    }
                    if let Some(c) = decreases {
                        self.zonk_expr(&mut c.expr);
                    }
                    self.zonk_block(body);
                }
            }
        }
        if let Some(t) = &mut b.tail {
            self.zonk_expr(t);
        }
    }

    fn zonk_pattern(&self, p: &mut Pattern) {
        p.ty = self.vars.zonk_final(&p.ty);
        match &mut p.kind {
            PatternKind::Ctor { args, .. } => {
                for a in args {
                    self.zonk_pattern(a);
                }
            }
            PatternKind::Unwrap { arg, .. } => self.zonk_pattern(arg),
            _ => {}
        }
    }
}

/// The span of a proof diagnostic: the expression, or the clause.
fn span_of(e: &Expr, how: How) -> Span {
    match how {
        How::Clause(s) => s,
        _ => e.span,
    }
}

// ---- entry points --------------------------------------------------------------

fn decl_of(tcx: &TypeCx<'_, '_>, def: DefId) -> (std::sync::Arc<ast::Module>, usize) {
    tcx.cx.decl_ast(def)
}

/// A function: its clauses (contract expressions, interface references),
/// then its body (3.14).
pub(crate) fn type_fn(tcx: &TypeCx<'_, '_>, def: DefId) -> DefOutput {
    let fd = tcx.cx.program.def(def).kind.as_fn().expect("a fn");
    let (module, index) = decl_of(tcx, def);
    let DeclKind::Fn(f) = &module.declarations[index].kind else {
        return DefOutput::default();
    };
    let mut t = Typer::new(tcx, def, Owner::Fn, fd.locals.clone(), fd.ret.clone());
    let mut requires = Vec::new();
    let mut ensures = Vec::new();
    let mut decreases = None;
    t.interface = true;
    for clause in &f.clauses {
        match clause {
            FnClause::Contract(c) => {
                let e = t.clause_bool(&c.expr, c.span);
                match c.kind {
                    ast::ContractKind::Requires => requires.push(e),
                    ast::ContractKind::Ensures => ensures.push(e),
                }
            }
            FnClause::Decreases(d) => decreases = Some(t.clause_measure(&d.expr, d.span)),
            FnClause::Effects(_) => {}
        }
    }
    t.interface = false;
    let body = match (&fd.body, &f.body) {
        (cpl_hir::FnBody::Block(_), Some(b)) => {
            let ret = fd.ret.clone();
            Some(t.root(|t| t.block(b, Some(&ret)).block))
        }
        _ => None,
    };
    t.finish(
        KindOutput::Fn {
            requires,
            ensures,
            decreases,
            body,
            locals: Vec::new(),
        },
        HoleSite::FnBody,
    )
}

/// A refinement's `where` predicate: `⇐ Bool` with `self` of the base type.
pub(crate) fn type_refinement(tcx: &TypeCx<'_, '_>, def: DefId) -> DefOutput {
    let rd = tcx
        .cx
        .program
        .def(def)
        .kind
        .checked_predicate()
        .expect("a refinement");
    let (module, index) = decl_of(tcx, def);
    let expr = match &module.declarations[index].kind {
        DeclKind::Struct(s) => &s.invariant.as_ref().unwrap().expr,
        _ => match crate::refine::refinement_decl(&module.declarations[index]) {
            Some(r) => &r.predicate,
            None => return DefOutput::default(),
        },
    };
    let mut t = Typer::new(tcx, def, Owner::Refinement, rd.locals.clone(), Ty::Bool);
    t.interface = true;
    let predicate = t.clause_bool(expr, rd.predicate.span);
    t.finish(
        KindOutput::Refinement {
            predicate,
            locals: Vec::new(),
        },
        HoleSite::FnBody,
    )
}

/// A capability's operation clauses (S31): each operation has its own
/// local table.
pub(crate) fn type_capability(tcx: &TypeCx<'_, '_>, def: DefId) -> DefOutput {
    let cd = tcx
        .cx
        .program
        .def(def)
        .kind
        .as_capability()
        .expect("a capability");
    let (module, index) = decl_of(tcx, def);
    let DeclKind::Capability(c) = &module.declarations[index].kind else {
        return DefOutput::default();
    };
    let mut out = DefOutput::default();
    let mut ops = Vec::new();
    for (op, od) in c.operations.iter().zip(&cd.ops) {
        let mut t = Typer::new(tcx, def, Owner::Op, od.locals.clone(), od.ret.clone());
        t.interface = true;
        let mut requires = Vec::new();
        let mut ensures = Vec::new();
        for clause in &op.clauses {
            let e = t.clause_bool(&clause.expr, clause.span);
            match clause.kind {
                ast::ContractKind::Requires => requires.push(e),
                ast::ContractKind::Ensures => ensures.push(e),
            }
        }
        let o = t.finish(
            KindOutput::Capability {
                ops: vec![(requires, ensures, Vec::new())],
            },
            HoleSite::FnBody,
        );
        out.diags.extend(o.diags);
        out.refs.extend(o.refs);
        out.failed |= o.failed;
        if let KindOutput::Capability { ops: typed } = o.kind {
            ops.extend(typed);
        }
    }
    out.kind = KindOutput::Capability { ops };
    out
}

/// A test body (3.14: `⇐ Unit`).
pub(crate) fn type_test(tcx: &TypeCx<'_, '_>, def: DefId) -> DefOutput {
    let td = tcx.cx.program.def(def).kind.as_test().expect("a test");
    let (module, index) = decl_of(tcx, def);
    let DeclKind::Test(test) = &module.declarations[index].kind else {
        return DefOutput::default();
    };
    let mut t = Typer::new(
        tcx,
        def,
        Owner::Test,
        td.locals.clone(),
        Ty::unit(&tcx.cx.program.std_defs),
    );
    let unit = t.unit();
    let body = t.root(|t| t.block(&test.body, Some(&unit)).block);
    t.finish(
        KindOutput::Test {
            body,
            locals: Vec::new(),
        },
        HoleSite::TestBody,
    )
}

/// A property: its `for` ranges (contract expressions of type `Gen<T>`,
/// S14, 3.14, T18), then its body (`⇐ Unit`).
pub(crate) fn type_property(tcx: &TypeCx<'_, '_>, def: DefId) -> DefOutput {
    let pd = tcx
        .cx
        .program
        .def(def)
        .kind
        .as_property()
        .expect("a property");
    let (module, index) = decl_of(tcx, def);
    let DeclKind::Property(p) = &module.declarations[index].kind else {
        return DefOutput::default();
    };
    let mut t = Typer::new(
        tcx,
        def,
        Owner::Property,
        pd.locals.clone(),
        Ty::unit(&tcx.cx.program.std_defs),
    );
    let mut binders = Vec::new();
    for clause in &p.for_clauses {
        t.interface = true;
        t.contract = true;
        let (gen, elem) = t.root(|t| t.for_range(&clause.range));
        t.contract = false;
        t.interface = false;
        let local = t
            .tcx
            .cx
            .names
            .binders
            .get(&clause.binder.span)
            .copied()
            .unwrap_or(LocalId(0));
        t.set_local_ty(local, elem.clone());
        binders.push(Binder {
            local,
            gen,
            elem_ty: elem,
            span: clause.span,
        });
    }
    let unit = t.unit();
    let body = t.root(|t| t.block(&p.body, Some(&unit)).block);
    t.finish(
        KindOutput::Property {
            binders,
            body,
            locals: Vec::new(),
        },
        HoleSite::TestBody,
    )
}
