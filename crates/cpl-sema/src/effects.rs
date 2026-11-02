//! Effect checking: the `effect` stage (`spec/toolchain.md` 3.1, 3.4;
//! `spec/semantics.md` 4.1-4.5, S26-S30, S62).
//!
//! | Rule | Diagnostic |
//! | - | - |
//! | an operation call `c.op(...)` in a function body needs `c.op` in the function's declared bound; no clause or `effects {}` is the empty bound (4.4, S26) | `undeclared_effect` |
//! | a call `g(..., using c1, ..., using cm)` in a body needs every effect of `g`'s *declared* bound, with `g`'s `using` parameters replaced by `c1..cm` (S28's σ), in the bound (4.4) | `undeclared_effect` |
//! | a handle anywhere but an operation receiver or a `using` argument (S27) | `capability_as_value` |
//! | a handle as an operand of `==` or `!=` (S27) | `capability_equality` |
//! | a `using` parameter that appears in no operation call and no `using` argument (S29) | `unused_capability` (W) |
//!
//! `undeclared_effect` carries `symbol` (the calling function), `facet:
//! "effects"`, `effect` (the offending `c.op` after substitution), and
//! `callee` (the operation's or function's qualified name), and spans the
//! call; the handle diagnostics carry `name` and `type` (the capability)
//! and span the handle; `unused_capability` carries `name` and spans the
//! parameter (9.4, S63).
//!
//! `capability_equality` is reported once per handle operand, at that
//! operand: S27's table maps each *occurrence* of a handle as an operand
//! of `==`/`!=` to the code, S62 makes every misplaced handle its own S27
//! diagnostic (the generator analogue, `gen.int == gen.int`, carries two),
//! 9.4 gives the code a single `name`, and S63 (1) spans "the operand that
//! failed a check". So `db == other` carries two and `n == db` one.
//!
//! The stage also fills `EffectBound::inferred` of every function: the
//! effects its body performs (its operation calls and the substituted
//! bounds of the functions it calls; contract expressions excluded, 4.4).
//!
//! The codes of the declared bound are reported where the bound is
//! resolved (`crate::sig`: `effect_param_not_capability`,
//! `unknown_effect_operation`, `duplicate_effect`, and the signature rules
//! `using_param_not_capability` and `capability_in_value_position` of
//! 1.8); `using_argument_not_capability` is the typer's, because only the
//! syntax tells a bare name from another expression (S28), and so is the
//! `capability_as_value` of every `using` argument inside a contract
//! expression (5.1: the only report for it, whatever its form, so it
//! spans the whole argument and is never beside S28's diagnostics); an
//! operation call in a contract expression is the contract stage's
//! `effect_in_contract` (4.5, `crate::contracts`).
//!
//! # No cascades (S62)
//!
//! * A call whose `using` arguments were rejected (a count other than the
//!   callee's, an argument that is not the bare name of a handle, a handle
//!   of another capability, any `using` argument inside a contract
//!   expression) has no substitution σ and is not checked against the
//!   bound. Bareness is read from the AST: the typer turns `(db)` and
//!   `db(key)` into the handle's `Local` as well.
//! * An effect that an erroneous entry of the function's own `effects`
//!   clause may have meant to declare is not `undeclared_effect`: `p.op`
//!   with an operation `p`'s capability lacks (`unknown_effect_operation`)
//!   covers every operation call through `p`, and `x.op` whose `x` is no
//!   `using` parameter (`effect_param_not_capability`) covers every
//!   operation named `op`. Fixing the clause is the one fix of both.
//! * A `using` parameter is not `unused_capability` when a call of its
//!   body lacks a valid `using` argument for a parameter of the same
//!   capability (`using_arity_mismatch`, `using_argument_not_capability`,
//!   `type_mismatch`): the fix of that error is to pass it.
//! * A `using` parameter whose type is not a capability was
//!   `using_param_not_capability` and has the error type; a positional
//!   parameter of a capability type was `capability_in_value_position`.
//!   Neither is a handle here. A function without a body (`missing_body`)
//!   reports no `unused_capability`.

use crate::ast_walk::{self, ContractClause, SyntaxVisitor};
use crate::bodies::{visit_def, Position, Site, Visitor};
use crate::checker::{blame, diag, Checker, Stage};
use crate::codes;
use crate::resolve::Res;
use cpl_diag::{Diagnostic, Facet, Span};
use cpl_hir::{
    BindingKind, Def, DefId, DefKind, EffectRef, Expr, ExprKind, FnBody, FnDef, LocalId, Ty,
};
use cpl_syntax::ast;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// Check every function's effects (see the module docs).
pub(crate) fn check_effects(cx: &mut Checker<'_>) {
    let mut diags: Vec<Diagnostic> = Vec::new();
    let mut failed: BTreeSet<DefId> = BTreeSet::new();
    let mut inferred: Vec<(DefId, BTreeSet<EffectRef>)> = Vec::new();
    for def in cx.program.defs() {
        let DefKind::Fn(f) = &def.kind else { continue };
        if matches!(f.body, FnBody::Builtin(_)) {
            // A std stub: no body, no `using` parameter.
            continue;
        }
        let mut w = Walk::new(cx, def, f);
        visit_def(def, &mut w);
        w.unused_capabilities();
        if w.diags.iter().any(Diagnostic::is_error) {
            failed.insert(def.id);
        }
        diags.append(&mut w.diags);
        inferred.push((def.id, w.inferred));
    }
    for (def, set) in inferred {
        if let DefKind::Fn(f) = &mut cx.program.defs[def.0 as usize].kind {
            f.effects.inferred = set;
        }
    }
    for def in failed {
        cx.fail(def);
    }
    cx.emit_all(Stage::Effect, diags);
}

/// The walk of one function.
struct Walk<'c, 'a, 'h> {
    cx: &'c Checker<'a>,
    def: &'h Def,
    f: &'h FnDef,
    /// The declared bound; empty for a pure function (S26).
    bound: BTreeSet<EffectRef>,
    /// `using` parameters with an `unknown_effect_operation` entry (S62).
    uncertain_params: BTreeSet<LocalId>,
    /// Operations of `effect_param_not_capability` entries (S62).
    uncertain_ops: BTreeSet<String>,
    /// The spans of the `using` arguments written as a bare name (S28).
    bare: HashSet<Span>,
    /// The `using` parameters the syntax uses (S29).
    used: BTreeSet<LocalId>,
    /// The declaration has no body (`missing_body`): its uses are unknown.
    bodiless: bool,
    /// What the body performs.
    inferred: BTreeSet<EffectRef>,
    /// The capabilities of `using` arguments missing or rejected at calls
    /// of the body (S62, see the module docs).
    missing: BTreeSet<DefId>,
    diags: Vec<Diagnostic>,
}

impl<'c, 'a, 'h> Walk<'c, 'a, 'h> {
    fn new(cx: &'c Checker<'a>, def: &'h Def, f: &'h FnDef) -> Walk<'c, 'a, 'h> {
        let (module, index) = cx.decl_ast(def.id);
        let decl = &module.declarations[index];
        let mut syntax = Uses {
            cx,
            using_args: Vec::new(),
            bare: HashSet::new(),
            used: BTreeSet::new(),
        };
        ast_walk::visit_decl(cx, decl, &mut syntax);
        let mut w = Walk {
            cx,
            def,
            f,
            bound: f.effects.declared.clone().unwrap_or_default(),
            uncertain_params: BTreeSet::new(),
            uncertain_ops: BTreeSet::new(),
            bare: syntax.bare,
            used: syntax.used,
            bodiless: matches!(&decl.kind, ast::DeclKind::Fn(fd) if fd.body.is_none()),
            inferred: BTreeSet::new(),
            missing: BTreeSet::new(),
            diags: Vec::new(),
        };
        if let ast::DeclKind::Fn(fd) = &decl.kind {
            w.erroneous_entries(fd);
        }
        w
    }

    /// The entries of the `effects` clause that the declared bound dropped
    /// because they were diagnosed (`crate::sig`).
    fn erroneous_entries(&mut self, decl: &ast::FnDecl) {
        let Some(clause) = decl.effects() else { return };
        for e in &clause.effects {
            match self.f.using.iter().find(|u| u.name == e.param.text) {
                Some(u) => {
                    if let Ty::Capability(cap) = &u.ty {
                        if !self.has_operation(*cap, &e.operation.text) {
                            self.uncertain_params.insert(u.local);
                        }
                    }
                }
                None => {
                    self.uncertain_ops.insert(e.operation.text.clone());
                }
            }
        }
    }

    /// Whether capability `cap` has an operation `name`.
    fn has_operation(&self, cap: DefId, name: &str) -> bool {
        self.cx
            .program
            .def(cap)
            .kind
            .as_capability()
            .is_some_and(|c| c.ops.iter().any(|o| o.name == name))
    }

    /// The `using` parameter an expression names (a handle, S27), with its
    /// capability.
    fn handle(&self, e: &Expr) -> Option<(LocalId, DefId)> {
        let ExprKind::Local(id) = e.kind else {
            return None;
        };
        let local = self.f.locals.get(id.0 as usize)?;
        match (&local.kind, &local.ty) {
            (BindingKind::Using, Ty::Capability(cap)) => Some((id, *cap)),
            _ => None,
        }
    }

    fn name(&self, id: LocalId) -> String {
        self.f
            .locals
            .get(id.0 as usize)
            .map_or_else(String::new, |l| l.name.clone())
    }

    /// Whether `effect` lies in the bound, or an erroneous entry of the
    /// clause may have meant it (S62).
    fn allowed(&self, effect: &EffectRef) -> bool {
        self.bound.contains(effect)
            || self.uncertain_params.contains(&effect.param)
            || self.uncertain_ops.contains(&effect.op)
    }

    /// S28's σ for a call of `g`: `g`'s `using` parameters to the caller's
    /// handles, when every `using` argument is the bare name of a handle of
    /// exactly the parameter's capability and their count is `g`'s
    /// (otherwise the call was diagnosed and has no substitution, S62).
    fn substitution(&self, g: &FnDef, using: &[Expr]) -> Option<BTreeMap<LocalId, LocalId>> {
        if using.len() != g.using.len() {
            return None;
        }
        let mut sigma = BTreeMap::new();
        for (param, arg) in g.using.iter().zip(using) {
            sigma.insert(param.local, self.argument(param, arg)?);
        }
        Some(sigma)
    }

    /// The handle a `using` argument passes to `param`: the bare name of a
    /// handle of exactly `param`'s capability (S28); `None` for an argument
    /// the typer rejected.
    fn argument(&self, param: &cpl_hir::ParamDef, arg: &Expr) -> Option<LocalId> {
        let (id, cap) = self.handle(arg)?;
        (self.bare.contains(&arg.span) && param.ty == Ty::Capability(cap)).then_some(id)
    }

    fn facet(site: &Site<'_>) -> Facet {
        if site.contract {
            Facet::Contract
        } else {
            Facet::Body
        }
    }

    /// `capability_as_value` (S27) for a handle in a value position.
    fn as_value(&mut self, e: &Expr, id: LocalId, cap: DefId, site: &Site<'_>) {
        let name = self.name(id);
        let ty = self.cx.program.display_ty(&Ty::Capability(cap));
        let message = format!(
            "`{name}` is a capability handle, not a value: it can only receive an operation call (`{name}.op(...)`) or be passed as `using {name}`"
        );
        let d = diag(codes::CAPABILITY_AS_VALUE, Some(e.span), message)
            .with_data("name", json!(name))
            .with_data("type", json!(ty));
        let facet = Self::facet(site);
        self.diags.push(blame(d, self.cx, self.def.id, Some(facet)));
    }

    /// `capability_equality` (S27) at one handle operand of `==`/`!=`.
    fn compared(&mut self, e: &Expr, id: LocalId, cap: DefId, site: &Site<'_>) {
        let name = self.name(id);
        let ty = self.cx.program.display_ty(&Ty::Capability(cap));
        let d = diag(
            codes::CAPABILITY_EQUALITY,
            Some(e.span),
            format!("capability handles cannot be compared: `{name}` is a handle, not a value"),
        )
        .with_data("name", json!(name))
        .with_data("type", json!(ty));
        let facet = Self::facet(site);
        self.diags.push(blame(d, self.cx, self.def.id, Some(facet)));
    }

    /// `undeclared_effect` at a call (4.4); `through` names the called
    /// function of a forwarded effect.
    fn undeclared(&mut self, e: &Expr, effect: &EffectRef, callee: String, through: Option<&str>) {
        let shown = self.f.display_effect(effect);
        let fname = self.def.name.display();
        let pure = self
            .f
            .effects
            .declared
            .as_ref()
            .is_none_or(BTreeSet::is_empty);
        let action = match through {
            Some(g) => format!("calling `{g}` performs `{shown}`"),
            None => format!("it calls the operation `{shown}`"),
        };
        let message = if pure {
            format!(
                "`{fname}` is pure (it declares no effects), but {action}; declare it with `effects {{ {shown} }}`"
            )
        } else {
            format!(
                "`{fname}` does not declare `{shown}` in its `effects` clause, but {action}; add `{shown}` to the clause"
            )
        };
        let d = diag(codes::UNDECLARED_EFFECT, Some(e.span), message)
            .with_data("effect", json!(shown))
            .with_data("callee", json!(callee));
        self.diags
            .push(blame(d, self.cx, self.def.id, Some(Facet::Effects)));
    }

    /// `unused_capability` (S29) for every `using` parameter that appears
    /// in no operation call and no `using` argument of the declaration.
    fn unused_capabilities(&mut self) {
        if self.bodiless {
            return;
        }
        for p in &self.f.using {
            let Ty::Capability(cap) = &p.ty else { continue };
            if self.used.contains(&p.local) || self.missing.contains(cap) {
                continue;
            }
            let d = diag(
                codes::UNUSED_CAPABILITY,
                Some(p.span),
                format!(
                    "`using` parameter `{}` is never used: no operation call or `using` argument names it",
                    p.name
                ),
            )
            .with_data("name", json!(p.name));
            self.diags
                .push(blame(d, self.cx, self.def.id, Some(Facet::Signature)));
        }
    }
}

impl<'h> Visitor<'h> for Walk<'_, '_, 'h> {
    fn expr(&mut self, e: &'h Expr, site: Site<'h>) {
        match &e.kind {
            ExprKind::Local(_) => {
                let Some((id, cap)) = self.handle(e) else {
                    return;
                };
                match site.position {
                    Position::Value => self.as_value(e, id, cap, &site),
                    Position::Compared => self.compared(e, id, cap, &site),
                    // A `using` argument inside a contract expression is the
                    // typer's `capability_as_value` (5.1), spanning the
                    // whole argument whatever its form.
                    Position::Receiver | Position::UsingArg => {}
                }
            }
            ExprKind::CapCall {
                handle,
                capability,
                op,
                ..
            } => {
                if site.contract {
                    // `effect_in_contract` (crate::contracts).
                    return;
                }
                let Some((id, _)) = self.handle(handle) else {
                    return;
                };
                let Some(op_def) = self
                    .cx
                    .program
                    .def(*capability)
                    .kind
                    .as_capability()
                    .and_then(|c| c.ops.get(*op as usize))
                else {
                    return;
                };
                let effect = EffectRef {
                    param: id,
                    op: op_def.name.clone(),
                };
                self.inferred.insert(effect.clone());
                if !self.allowed(&effect) {
                    let callee = format!(
                        "{}.{}",
                        self.cx.program.display_path(*capability),
                        op_def.name
                    );
                    self.undeclared(e, &effect, callee, None);
                }
            }
            ExprKind::Call { callee, using, .. } => {
                let Some(g) = self.cx.program.def(*callee).kind.as_fn() else {
                    return;
                };
                if site.contract {
                    // A `using` argument in a contract is `capability_as_value`
                    // (5.1), so no σ exists (S62); without one the call was
                    // `using_arity_mismatch`.
                    return;
                }
                for (j, p) in g.using.iter().enumerate() {
                    let passed = using.get(j).and_then(|a| self.argument(p, a));
                    if let (None, Ty::Capability(cap)) = (passed, &p.ty) {
                        self.missing.insert(*cap);
                    }
                }
                let Some(bound) = &g.effects.declared else {
                    return;
                };
                if bound.is_empty() {
                    return;
                }
                let Some(sigma) = self.substitution(g, using) else {
                    return;
                };
                let gname = self.cx.program.display_path(*callee);
                let shown = self.cx.program.def(*callee).name.display();
                for effect in bound {
                    let Some(&handle) = sigma.get(&effect.param) else {
                        continue;
                    };
                    let effect = EffectRef {
                        param: handle,
                        op: effect.op.clone(),
                    };
                    self.inferred.insert(effect.clone());
                    if !self.allowed(&effect) {
                        self.undeclared(e, &effect, gname.clone(), Some(&shown));
                    }
                }
            }
            _ => {}
        }
    }
}

/// What a declaration's syntax says about its handles: the `using`
/// arguments written as a bare name (S28), and the `using` parameters it
/// uses (S29): the root of a member access (`db.get(id)`, an operation
/// call; `db.get` without a call is the typer's `method_not_called` and
/// counts as well), through grouping parentheses (`(db).get(id)` is an
/// operation call on `db`: 3.8 types `( e )` as `e`, and S27 puts no
/// bareness rule on a receiver), and every name inside a `using` argument
/// (`using db`, and the rejected `using (db)` and `using db(key)`). The
/// `effects` clause is not a use.
struct Uses<'c, 'a> {
    cx: &'c Checker<'a>,
    /// The spans of the `using` arguments met so far (pre-order: a call
    /// comes before its arguments).
    using_args: Vec<Span>,
    bare: HashSet<Span>,
    used: BTreeSet<LocalId>,
}

impl Uses<'_, '_> {
    fn note(&mut self, span: Span) {
        if let Some(Res::Local(id)) = self.cx.names.exprs.get(&span) {
            self.used.insert(*id);
        }
    }
}

/// `a` contains `b` (both in one file).
fn contains(a: Span, b: Span) -> bool {
    a.file == b.file && a.start <= b.start && b.end <= a.end
}

impl<'a> SyntaxVisitor<'a> for Uses<'_, '_> {
    fn expr(&mut self, e: &'a ast::Expr, _clause: Option<ContractClause<'a>>) {
        match &e.kind {
            ast::ExprKind::Member(m) => {
                let mut target: &ast::Expr = &m.target;
                while let ast::ExprKind::Paren(inner) = &target.kind {
                    target = &**inner;
                }
                if let ast::ExprKind::Name(n) = &target.kind {
                    self.note(n.span);
                }
            }
            ast::ExprKind::Call(c) => {
                for u in &c.args.using {
                    if let ast::ExprKind::Name(_) = &u.expr.kind {
                        self.bare.insert(u.expr.span);
                    }
                    self.using_args.push(u.expr.span);
                }
            }
            ast::ExprKind::Name(_) if self.using_args.iter().any(|u| contains(*u, e.span)) => {
                self.note(e.span);
            }
            _ => {}
        }
    }
}
