//! Signatures (`spec/semantics.md` 1.5, 1.8, 2.6, 3.1-3.2, 4.1, S7, S12,
//! S13, S27; toolchain 3.3, 6.5, T17): every type a declaration's signature
//! writes is resolved to a [`Ty`] and stored in its [`DefKind`], with the
//! well-formedness rules of 1.8 that concern signatures:
//!
//! | Rule | Diagnostic |
//! | - | - |
//! | value types in parameter, return, field, payload, wrapper, refinement-base, operation, and type-argument positions (S13, S27) | `capability_in_value_position`, `not_a_value_type` |
//! | `using` parameters are capabilities | `using_param_not_capability` |
//! | members unique per declaration (1.4) | `duplicate_field`, `duplicate_variant`, `duplicate_operation`, `duplicate_parameter`, `duplicate_type_parameter` |
//! | every type parameter used | `unused_type_parameter` (error on `struct`/`enum`, warning on `fn`) |
//! | public signatures exportable (1.5) | `private_type_in_public_signature` |
//! | `self`/`result` parameters (S9) | `self_bound`, `result_name_reserved` |
//! | the effect bound (4.1, 4.4) | `effect_param_not_capability`, `unknown_effect_operation`, `duplicate_effect` |
//!
//! Every type path of a signature becomes an interface `TypeUse`
//! reference (T17). Clauses get their canonical text and a placeholder
//! expression; parameters, `using` parameters, and `result` are the first
//! entries of the local tables, recorded in `Resolutions::binders`.

use crate::checker::{blame, diag, diag_with, Checker, Stage};
use crate::codes;
use crate::collect::{generic_names, placeholder_clause};
use crate::resolve::{value_type_errors, ResOut, Resolver};
use crate::scope::LocalInfo;
use cpl_diag::{Diagnostic, Facet, Severity, Span};
use cpl_hir::{
    BindingKind, DefId, DefKind, EffectBound, EffectRef, LocalId, ParamDef, RefKind, Reference, Ty,
};
use cpl_syntax::ast::{self, DeclKind, FnClause};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

/// Everything one declaration's signature produces.
#[derive(Default)]
struct Sig {
    diags: Vec<Diagnostic>,
    uses: Vec<(Span, DefId)>,
    /// Uses in exported positions of a public declaration (1.5).
    exported: Vec<(Span, DefId)>,
    binders: Vec<(Span, LocalId)>,
}

impl Sig {
    fn absorb(&mut self, out: ResOut, exported: bool) {
        self.diags.extend(out.diags);
        if exported {
            self.exported.extend(out.uses.iter().copied());
        }
        self.uses.extend(out.uses);
    }
}

/// Resolve the signature of every definition.
pub(crate) fn resolve_signatures(cx: &mut Checker<'_>) {
    for index in 0..cx.program.defs.len() {
        let def = DefId(index as u32);
        let (ast, decl_index) = cx.decl_ast(def);
        let decl = &ast.declarations[decl_index];
        let mut sig = Sig::default();
        let kind = {
            let generics = cx.program.def(def).kind.generics().to_vec();
            let resolver = cx.resolver(def, &generics);
            let is_pub = cx.program.def(def).is_pub;
            build(cx, &resolver, def, decl, is_pub, &mut sig)
        };
        cx.program.defs[index].kind = kind;
        // Exportability (1.5): one diagnostic per private type mentioned.
        if cx.program.def(def).is_pub {
            for &(span, used) in &sig.exported {
                if cx.program.def(used).is_pub {
                    continue;
                }
                let qualified = cx.program.display_path(used);
                let d = diag(
                    codes::PRIVATE_TYPE_IN_PUBLIC_SIGNATURE,
                    Some(span),
                    format!(
                        "private type `{qualified}` appears in the signature of public `{}`",
                        cx.program.def(def).name.display()
                    ),
                )
                .with_data("type", json!(qualified));
                sig.diags.push(blame(d, cx, def, Some(Facet::Signature)));
            }
        }
        for (span, local) in sig.binders {
            cx.names.binders.insert(span, local);
        }
        for (span, to) in sig.uses {
            cx.program.references.push(Reference {
                from: def,
                to,
                kind: RefKind::TypeUse,
                span,
                interface: true,
            });
        }
        if sig.diags.iter().any(Diagnostic::is_error) {
            cx.fail(def);
        }
        cx.emit_all(Stage::Resolve, sig.diags);
    }
}

fn build(
    cx: &Checker<'_>,
    r: &Resolver<'_>,
    def: DefId,
    decl: &ast::Declaration,
    is_pub: bool,
    sig: &mut Sig,
) -> DefKind {
    let mut kind = cx.program.def(def).kind.clone();
    let in_std = r.file.is_std;
    match (&decl.kind, &mut kind) {
        (DeclKind::Fn(f), DefKind::Fn(fd)) => {
            duplicate_type_params(&f.generic_params, sig);
            let has_ensures = f.ensures().next().is_some();
            let mut locals: Vec<LocalInfo> = Vec::new();
            let mut names: BTreeMap<String, Span> = BTreeMap::new();
            for p in &f.params.positional {
                let mut out = ResOut::default();
                let ty = r.resolve_type(&p.ty, &mut out);
                value_type_errors(&p.ty, &ty, r.program, in_std, &mut out.diags);
                sig.absorb(out, is_pub);
                param_name_rules(&p.name, p.span, has_ensures, &mut names, sig);
                let local = push_local(
                    &mut locals,
                    &p.name.text,
                    BindingKind::Parameter,
                    p.span,
                    ty.clone(),
                );
                sig.binders.push((p.name.span, local));
                fd.params.push(ParamDef {
                    local,
                    name: p.name.text.clone(),
                    ty,
                    span: p.span,
                });
            }
            for u in &f.params.using {
                let mut out = ResOut::default();
                let mut ty = r.resolve_type(&u.ty, &mut out);
                if !matches!(ty, Ty::Capability(_) | Ty::Error) {
                    out.push(
                        diag(
                            codes::USING_PARAM_NOT_CAPABILITY,
                            Some(u.span),
                            format!(
                                "`using` parameter `{}` has type `{}`, which is not a capability",
                                u.name.text,
                                r.program.display_ty_in(&ty, &fd.generics)
                            ),
                        )
                        .with_data("name", json!(u.name.text))
                        .with_data("type", json!(r.program.display_ty_in(&ty, &fd.generics))),
                    );
                    ty = Ty::Error;
                }
                sig.absorb(out, is_pub);
                param_name_rules(&u.name, u.span, has_ensures, &mut names, sig);
                let local = push_local(
                    &mut locals,
                    &u.name.text,
                    BindingKind::Using,
                    u.span,
                    ty.clone(),
                );
                sig.binders.push((u.name.span, local));
                fd.using.push(ParamDef {
                    local,
                    name: u.name.text.clone(),
                    ty,
                    span: u.span,
                });
            }
            let mut out = ResOut::default();
            fd.ret = r.resolve_type(&f.ret, &mut out);
            value_type_errors(&f.ret, &fd.ret, r.program, in_std, &mut out.diags);
            sig.absorb(out, is_pub);
            if has_ensures {
                fd.result_local = Some(push_local(
                    &mut locals,
                    "result",
                    BindingKind::Result,
                    f.ret.span,
                    fd.ret.clone(),
                ));
            }
            for clause in &f.clauses {
                match clause {
                    FnClause::Contract(c) => {
                        let cl = placeholder_clause(&c.expr, c.span);
                        match c.kind {
                            ast::ContractKind::Requires => fd.requires.push(cl),
                            ast::ContractKind::Ensures => fd.ensures.push(cl),
                        }
                    }
                    FnClause::Decreases(d) => {
                        fd.decreases = Some(placeholder_clause(&d.expr, d.span))
                    }
                    FnClause::Effects(e) => {
                        fd.effects = EffectBound {
                            declared: Some(effect_bound(r, e, fd, sig)),
                            inferred: BTreeSet::new(),
                        }
                    }
                }
            }
            // Unused type parameters: a warning on fn (1.8).
            let mut used = BTreeSet::new();
            for p in fd.params.iter().chain(&fd.using) {
                params_in(&p.ty, &mut used);
            }
            params_in(&fd.ret, &mut used);
            unused_type_params(&f.generic_params, &used, Severity::Warning, sig);
            fd.locals = locals
                .iter()
                .enumerate()
                .map(|(i, l)| l.to_def(i as u32))
                .collect();
        }
        (DeclKind::Type(t), DefKind::Wrapper(w)) => {
            if let ast::TypeDeclKind::Wrapper(wd) = &t.kind {
                let mut out = ResOut::default();
                w.inner = r.resolve_type(&wd.payload, &mut out);
                value_type_errors(&wd.payload, &w.inner, r.program, in_std, &mut out.diags);
                sig.absorb(out, is_pub);
            }
        }
        (DeclKind::Type(t), DefKind::Refinement(rd)) => {
            if let ast::TypeDeclKind::Refinement(ra) = &t.kind {
                let info = cx.refinements.get(def);
                rd.base = info.and_then(|i| i.base.clone()).unwrap_or(Ty::Error);
                // The clause spans from `where` (semantics S63 (2)).
                rd.predicate =
                    placeholder_clause(&ra.predicate, crate::names::where_clause_span(cx, ra));
                if let Some(info) = info {
                    if is_pub {
                        sig.exported.extend(info.base_uses.iter().copied());
                    }
                    sig.uses.extend(info.base_uses.iter().copied());
                }
                value_type_errors(&ra.base, &rd.base, r.program, in_std, &mut sig.diags);
                rd.locals = vec![LocalInfo {
                    name: "self".to_string(),
                    kind: BindingKind::SelfValue,
                    mutable: false,
                    span: ra.predicate.span,
                    ty: rd.base.clone(),
                    read: false,
                }
                .to_def(0)];
            }
        }
        (DeclKind::Struct(s), DefKind::Struct(sd)) => {
            if let Some(inv) = &mut sd.invariant {
                inv.base = Ty::Adt(
                    def,
                    (0..sd.generics.len())
                        .map(|i| Ty::Param(i as u32))
                        .collect(),
                );
                inv.locals = vec![LocalInfo {
                    name: "self".to_string(),
                    kind: BindingKind::SelfValue,
                    mutable: false,
                    span: s.invariant.as_ref().unwrap().expr.span,
                    ty: inv.base.clone(),
                    read: false,
                }
                .to_def(0)];
            }
            duplicate_type_params(&s.generic_params, sig);
            let mut seen: BTreeMap<&str, Span> = BTreeMap::new();
            let mut used = BTreeSet::new();
            for (field, fdef) in s.fields.iter().zip(sd.fields.iter_mut()) {
                if let Some(first) = seen.get(field.name.text.as_str()) {
                    sig.diags.push(
                        duplicate(
                            codes::DUPLICATE_FIELD,
                            field.span,
                            *first,
                            &field.name.text,
                            "field",
                        )
                        .with_data("type", json!(s.name.text)),
                    );
                } else {
                    seen.insert(&field.name.text, field.span);
                }
                let mut out = ResOut::default();
                fdef.ty = r.resolve_type(&field.ty, &mut out);
                value_type_errors(&field.ty, &fdef.ty, r.program, in_std, &mut out.diags);
                sig.absorb(out, is_pub);
                params_in(&fdef.ty, &mut used);
            }
            unused_type_params(&s.generic_params, &used, Severity::Error, sig);
        }
        (DeclKind::Enum(e), DefKind::Enum(ed)) => {
            duplicate_type_params(&e.generic_params, sig);
            let mut seen: BTreeMap<&str, Span> = BTreeMap::new();
            let mut used = BTreeSet::new();
            for (variant, vdef) in e.variants.iter().zip(ed.variants.iter_mut()) {
                if let Some(first) = seen.get(variant.name.text.as_str()) {
                    sig.diags.push(duplicate(
                        codes::DUPLICATE_VARIANT,
                        variant.span,
                        *first,
                        &variant.name.text,
                        "variant",
                    ));
                } else {
                    seen.insert(&variant.name.text, variant.span);
                }
                for (payload, pty) in variant.payload.iter().zip(vdef.payload.iter_mut()) {
                    let mut out = ResOut::default();
                    *pty = r.resolve_type(payload, &mut out);
                    value_type_errors(payload, pty, r.program, in_std, &mut out.diags);
                    sig.absorb(out, is_pub);
                    params_in(pty, &mut used);
                }
            }
            unused_type_params(&e.generic_params, &used, Severity::Error, sig);
        }
        (DeclKind::Capability(c), DefKind::Capability(cd)) => {
            let mut seen: BTreeMap<&str, Span> = BTreeMap::new();
            for (op, odef) in c.operations.iter().zip(cd.ops.iter_mut()) {
                if let Some(first) = seen.get(op.name.text.as_str()) {
                    sig.diags.push(duplicate(
                        codes::DUPLICATE_OPERATION,
                        op.span,
                        *first,
                        &op.name.text,
                        "operation",
                    ));
                } else {
                    seen.insert(&op.name.text, op.span);
                }
                let has_ensures = op
                    .clauses
                    .iter()
                    .any(|c| c.kind == ast::ContractKind::Ensures);
                let mut locals: Vec<LocalInfo> = Vec::new();
                let mut names: BTreeMap<String, Span> = BTreeMap::new();
                for p in &op.params.positional {
                    let mut out = ResOut::default();
                    let ty = r.resolve_type(&p.ty, &mut out);
                    value_type_errors(&p.ty, &ty, r.program, in_std, &mut out.diags);
                    sig.absorb(out, is_pub);
                    param_name_rules(&p.name, p.span, has_ensures, &mut names, sig);
                    let local = push_local(
                        &mut locals,
                        &p.name.text,
                        BindingKind::Parameter,
                        p.span,
                        ty.clone(),
                    );
                    sig.binders.push((p.name.span, local));
                    odef.params.push(ParamDef {
                        local,
                        name: p.name.text.clone(),
                        ty,
                        span: p.span,
                    });
                }
                let mut out = ResOut::default();
                odef.ret = r.resolve_type(&op.ret, &mut out);
                value_type_errors(&op.ret, &odef.ret, r.program, in_std, &mut out.diags);
                sig.absorb(out, is_pub);
                if has_ensures {
                    odef.result_local = Some(push_local(
                        &mut locals,
                        "result",
                        BindingKind::Result,
                        op.ret.span,
                        odef.ret.clone(),
                    ));
                }
                for clause in &op.clauses {
                    let cl = placeholder_clause(&clause.expr, clause.span);
                    match clause.kind {
                        ast::ContractKind::Requires => odef.requires.push(cl),
                        ast::ContractKind::Ensures => odef.ensures.push(cl),
                    }
                }
                odef.locals = locals
                    .iter()
                    .enumerate()
                    .map(|(i, l)| l.to_def(i as u32))
                    .collect();
            }
        }
        (DeclKind::Type(t), DefKind::BuiltinType(_)) => {
            if let ast::TypeDeclKind::Opaque(o) = &t.kind {
                duplicate_type_params(&o.generic_params, sig);
            }
        }
        _ => {}
    }
    kind
}

/// Append a parameter-like binding to a local table.
fn push_local(
    locals: &mut Vec<LocalInfo>,
    name: &str,
    kind: BindingKind,
    span: Span,
    ty: Ty,
) -> LocalId {
    locals.push(LocalInfo {
        name: name.to_string(),
        kind,
        mutable: false,
        span,
        ty,
        read: false,
    });
    LocalId(locals.len() as u32 - 1)
}

/// `duplicate_parameter` (1.4, counting `using` parameters), `self_bound`,
/// and `result_name_reserved` (S9) for one parameter.
fn param_name_rules(
    name: &ast::Name,
    span: Span,
    has_ensures: bool,
    names: &mut BTreeMap<String, Span>,
    sig: &mut Sig,
) {
    if let Some(first) = names.get(&name.text) {
        sig.diags.push(duplicate(
            codes::DUPLICATE_PARAMETER,
            span,
            *first,
            &name.text,
            "parameter",
        ));
    } else {
        names.insert(name.text.clone(), span);
    }
    if name.text == "self" {
        sig.diags.push(
            diag(
                codes::SELF_BOUND,
                Some(span),
                "binding `self` hides nothing but reads like a refinement's `self`; choose another name",
            )
            .with_data("name", json!("self")),
        );
    }
    if name.text == "result" && has_ensures {
        sig.diags.push(
            diag(
                codes::RESULT_NAME_RESERVED,
                Some(span),
                "`result` names the returned value in `ensures` clauses; a parameter of a declaration with `ensures` cannot be named `result`",
            )
            .with_data("name", json!("result")),
        );
    }
}

/// A two-location duplicate (S63 (4)): the later occurrence, with the
/// earlier in `notes`.
fn duplicate(
    code: cpl_diag::DiagCode,
    span: Span,
    first: Span,
    name: &str,
    what: &str,
) -> Diagnostic {
    diag(
        code,
        Some(span),
        format!("{what} `{name}` is declared twice"),
    )
    .with_data("name", json!(name))
    .with_note(format!("the first {what} `{name}` is here"), Some(first))
}

fn duplicate_type_params(g: &Option<ast::GenericParams>, sig: &mut Sig) {
    let Some(g) = g else { return };
    let mut seen: BTreeMap<&str, Span> = BTreeMap::new();
    for p in &g.params {
        if let Some(first) = seen.get(p.text.as_str()) {
            sig.diags.push(duplicate(
                codes::DUPLICATE_TYPE_PARAMETER,
                p.span,
                *first,
                &p.text,
                "type parameter",
            ));
        } else {
            seen.insert(&p.text, p.span);
        }
    }
}

/// `unused_type_parameter` for every type parameter (the first of its
/// name) that no type in `used` mentions.
fn unused_type_params(
    g: &Option<ast::GenericParams>,
    used: &BTreeSet<u32>,
    severity: Severity,
    sig: &mut Sig,
) {
    let Some(g) = g else { return };
    let names = generic_names(&Some(g.clone()));
    for (i, p) in g.params.iter().enumerate() {
        let first = names.iter().position(|n| *n == p.text) == Some(i);
        if first && !used.contains(&(i as u32)) {
            sig.diags.push(
                diag_with(
                    codes::UNUSED_TYPE_PARAMETER,
                    severity,
                    Some(p.span),
                    format!("type parameter `{}` is never used", p.text),
                )
                .with_data("name", json!(p.text)),
            );
        }
    }
}

/// Every `Ty::Param` index in a type.
pub(crate) fn params_in(ty: &Ty, out: &mut BTreeSet<u32>) {
    match ty {
        Ty::Param(i) => {
            out.insert(*i);
        }
        Ty::List(t) | Ty::Gen(t) => params_in(t, out),
        Ty::Adt(_, args) => {
            for a in args {
                params_in(a, out);
            }
        }
        _ => {}
    }
}

/// The declared effect bound of a function (4.1): each `p.op` names a
/// `using` parameter and an operation of its capability.
fn effect_bound(
    r: &Resolver<'_>,
    clause: &ast::EffectsClause,
    fd: &cpl_hir::FnDef,
    sig: &mut Sig,
) -> BTreeSet<EffectRef> {
    let mut declared = BTreeSet::new();
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    for e in &clause.effects {
        let param = e.param.text.as_str();
        let op = e.operation.text.as_str();
        if !seen.insert((param.to_string(), op.to_string())) {
            sig.diags.push(
                diag(
                    codes::DUPLICATE_EFFECT,
                    Some(e.span),
                    format!("effect `{param}.{op}` is listed twice"),
                )
                .with_data("effect", json!(format!("{param}.{op}"))),
            );
            continue;
        }
        match fd.using.iter().find(|u| u.name == param) {
            Some(u) => {
                // A `using` parameter that is not a capability was reported
                // (`using_param_not_capability`) and has the error type.
                if let Ty::Capability(cap) = &u.ty {
                    let has = r
                        .program
                        .def(*cap)
                        .kind
                        .as_capability()
                        .is_some_and(|c| c.ops.iter().any(|o| o.name == op));
                    if has {
                        declared.insert(EffectRef {
                            param: u.local,
                            op: op.to_string(),
                        });
                    } else {
                        let cap_name = r.program.def(*cap).name.display();
                        sig.diags.push(
                            diag(
                                codes::UNKNOWN_EFFECT_OPERATION,
                                Some(e.span),
                                format!("capability `{cap_name}` has no operation `{op}`"),
                            )
                            .with_data("name", json!(op))
                            .with_data("type", json!(cap_name)),
                        );
                    }
                }
            }
            None => {
                let mut d = diag(
                    codes::EFFECT_PARAM_NOT_CAPABILITY,
                    Some(e.span),
                    format!(
                        "`{param}` in `{param}.{op}` is not a `using` parameter of this function"
                    ),
                )
                .with_data("name", json!(param));
                if let Some(p) = fd.params.iter().find(|p| p.name == param) {
                    d = d.with_data("type", json!(r.program.display_ty_in(&p.ty, &fd.generics)));
                }
                sig.diags.push(d);
            }
        }
    }
    declared
}
