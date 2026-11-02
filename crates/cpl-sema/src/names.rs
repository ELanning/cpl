//! The resolution walk (`spec/toolchain.md` 3.1 `resolve`; `spec/semantics.md`
//! 2.1-2.5, 3.11, 3.12, S8, S9, S10, S43, S62, S67): every body, contract
//! clause, refinement predicate, loop clause, and property range is walked
//! once with its local scopes, every name is resolved as far as resolution
//! can go without types, and the result is recorded in
//! [`Resolutions`] (`Checker::names`) for the body typer.
//!
//! Diagnostics of this walk (all `resolve` category):
//!
//! | Code | Rule |
//! | - | - |
//! | `unresolved_name`, `unresolved_module`, `private_item` | 2.2, 2.4 |
//! | `not_a_value` | a module, alias, `gen`, or function in value position (S11; module-level properties such as `gen.int` excepted, S54) |
//! | `not_a_constructor`, `ambiguous_constructor` | S10, decided here only where the outcome does not depend on the expected type |
//! | `shadowed_binding`, `duplicate_pattern_binding` | S8, 3.12 |
//! | `unused_binding` (W) | S67 |
//! | `self_bound` (W), `self_outside_where`, `result_outside_ensures`, `result_name_reserved` | S9 |
//! | `assign_to_immutable` | 3.11 |
//! | `return_outside_function`, `propagate_outside_function` | 3.8, 3.11, 3.14 (in test and property bodies; in a contract expression the contract stage's codes take precedence, 5.1) |
//! | `unknown_field`, `duplicate_field`, `missing_field`, `not_a_constructor` on struct literals | 3.8 |
//! | `private_in_public_contract` | S4 |
//! | `unresolved_type`, `type_argument_count_mismatch`, `capability_in_value_position` on annotations and generic arguments | 2.2, S12, S13 |
//!
//! # Constructors and the expected type
//!
//! S10 step 1 resolves an unqualified constructor by the expected type,
//! which only the typer knows. The walk decides a constructor here when the
//! outcome cannot depend on it: in an infer-only position of S56 (the
//! initializer of an unannotated `let`/`var`, a `match` scrutinee, a
//! `forall` range, a receiver, the left operand of `==`, `!=`, `++`, and the
//! first element of a list literal in one of these positions), when no
//! declaration of the program has a constructor of that name (then nothing
//! can resolve it), and when the program's only constructor of that name is
//! also the unique lexical candidate. Every other unqualified constructor
//! is a `Res::DeferredCtor` for the typer.

use crate::checker::{blame, diag, Checker, Stage};
use crate::codes;
use crate::refine::chain_segments;
use crate::resolve::{
    value_type_errors, Ctor, CtorStep3, PathOutcome, Res, ResOut, Resolutions, Resolver,
    TypeTarget, ValueTarget,
};
use crate::scope::{LocalInfo, LocalScopes};
use cpl_diag::{Diagnostic, Facet, Span};
use cpl_hir::{BindingKind, DefId, DefKind, LocalId, RefKind, Reference, Ty};
use cpl_syntax::ast::{self, DeclKind, FnClause};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

/// Walk every declaration's bodies and clauses.
pub(crate) fn resolve_bodies(cx: &mut Checker<'_>) {
    for index in 0..cx.program.defs.len() {
        let def = DefId(index as u32);
        let (ast, decl_index) = cx.decl_ast(def);
        let decl = &ast.declarations[decl_index];
        let output = match &decl.kind {
            DeclKind::Fn(f) => Some(walk_fn(cx, def, f)),
            DeclKind::Type(t) => match &t.kind {
                ast::TypeDeclKind::Refinement(r) => Some(walk_refinement(cx, def, t, r)),
                _ => None,
            },
            DeclKind::Capability(c) => Some(walk_capability(cx, def, c)),
            DeclKind::Struct(s) if s.invariant.is_some() => Some(walk_struct_invariant(cx, def, s)),
            DeclKind::Test(t) => Some(walk_test(cx, def, t)),
            DeclKind::Property(p) => Some(walk_property(cx, def, p)),
            _ => None,
        };
        if let Some(output) = output {
            apply(cx, def, output);
        }
    }
}

/// What walking one declaration produced.
struct Output {
    diags: Vec<Diagnostic>,
    names: Resolutions,
    refs: Vec<(Span, DefId, bool)>,
    calls: BTreeSet<DefId>,
    /// The final local tables: the declaration's, or one per capability
    /// operation.
    locals: Locals,
}

enum Locals {
    Single(Vec<LocalInfo>),
    PerOp(Vec<Vec<LocalInfo>>),
}

fn apply(cx: &mut Checker<'_>, def: DefId, out: Output) {
    let failed = out.diags.iter().any(Diagnostic::is_error);
    cx.emit_all(Stage::Resolve, out.diags);
    let n = out.names;
    cx.names.exprs.extend(n.exprs);
    cx.names.patterns.extend(n.patterns);
    cx.names.struct_paths.extend(n.struct_paths);
    cx.names.assigns.extend(n.assigns);
    cx.names.binders.extend(n.binders);
    cx.names.types.extend(n.types);
    cx.names.holes.extend(n.holes);
    cx.names.hidden.extend(n.hidden);
    for (span, to, interface) in out.refs {
        cx.program.references.push(Reference {
            from: def,
            to,
            kind: RefKind::TypeUse,
            span,
            interface,
        });
    }
    let to_defs = |locals: &[LocalInfo]| -> Vec<cpl_hir::LocalDef> {
        locals
            .iter()
            .enumerate()
            .map(|(i, l)| l.to_def(i as u32))
            .collect()
    };
    match (&mut cx.program.defs[def.0 as usize].kind, out.locals) {
        (DefKind::Fn(f), Locals::Single(l)) => f.locals = to_defs(&l),
        (DefKind::Refinement(r), Locals::Single(l)) => r.locals = to_defs(&l),
        (DefKind::Struct(s), Locals::Single(l)) => {
            if let Some(inv) = &mut s.invariant {
                inv.locals = to_defs(&l);
            }
        }
        (DefKind::Test(t), Locals::Single(l)) => t.locals = to_defs(&l),
        (DefKind::Property(p), Locals::Single(l)) => p.locals = to_defs(&l),
        (DefKind::Capability(c), Locals::PerOp(per)) => {
            for (op, l) in c.ops.iter_mut().zip(per) {
                op.locals = to_defs(&l);
            }
        }
        _ => {}
    }
    if failed {
        cx.fail(def);
    }
    cx.calls.entry(def).or_default().extend(out.calls);
}

fn infos(locals: &[cpl_hir::LocalDef]) -> Vec<LocalInfo> {
    locals
        .iter()
        .map(|l| LocalInfo {
            name: l.name.clone(),
            kind: l.kind,
            mutable: l.mutable,
            span: l.span,
            ty: l.ty.clone(),
            read: false,
        })
        .collect()
}

fn walk_fn(cx: &Checker<'_>, def: DefId, f: &ast::FnDecl) -> Output {
    let fd = cx.program.def(def).kind.as_fn().expect("a fn definition");
    let generics = fd.generics.clone();
    let resolver = cx.resolver(def, &generics);
    let has_ensures = f.ensures().next().is_some();
    let mut w = Walker::new(
        cx,
        resolver,
        def,
        Owner::Fn,
        has_ensures,
        LocalScopes::new(infos(&fd.locals)),
    );
    let public = cx.program.def(def).is_pub;
    w.scopes.push();
    for p in fd.params.iter().chain(&fd.using) {
        w.scopes.enter(p.local);
    }
    for clause in &f.clauses {
        match clause {
            FnClause::Contract(c) => {
                if c.kind == ast::ContractKind::Ensures {
                    w.scopes.push();
                    if let Some(result) = fd.result_local {
                        w.scopes.enter(result);
                    }
                    w.in_contract(c.span, &c.expr, public, |w| w.expr(&c.expr, Expect::Maybe));
                    w.close_frame();
                } else {
                    w.in_contract(c.span, &c.expr, public, |w| w.expr(&c.expr, Expect::Maybe));
                }
            }
            FnClause::Decreases(d) => {
                w.in_contract(d.span, &d.expr, public, |w| w.expr(&d.expr, Expect::Maybe));
            }
            FnClause::Effects(_) => {}
        }
    }
    if let Some(body) = &f.body {
        w.block(body);
    }
    w.close_frame();
    w.finish_single()
}

fn walk_capability(cx: &Checker<'_>, def: DefId, c: &ast::CapabilityDecl) -> Output {
    let cd = cx
        .program
        .def(def)
        .kind
        .as_capability()
        .expect("a capability");
    let public = cx.program.def(def).is_pub;
    let mut diags = Vec::new();
    let mut names = Resolutions::default();
    let mut refs = Vec::new();
    let mut calls = BTreeSet::new();
    let mut per_op = Vec::new();
    for (op, od) in c.operations.iter().zip(&cd.ops) {
        let resolver = cx.resolver(def, &[]);
        let has_ensures = op
            .clauses
            .iter()
            .any(|c| c.kind == ast::ContractKind::Ensures);
        let mut w = Walker::new(
            cx,
            resolver,
            def,
            Owner::Op,
            has_ensures,
            LocalScopes::new(infos(&od.locals)),
        );
        w.scopes.push();
        for p in &od.params {
            w.scopes.enter(p.local);
        }
        for clause in &op.clauses {
            if clause.kind == ast::ContractKind::Ensures {
                w.scopes.push();
                if let Some(result) = od.result_local {
                    w.scopes.enter(result);
                }
                w.in_contract(clause.span, &clause.expr, public, |w| {
                    w.expr(&clause.expr, Expect::Maybe)
                });
                w.close_frame();
            } else {
                w.in_contract(clause.span, &clause.expr, public, |w| {
                    w.expr(&clause.expr, Expect::Maybe)
                });
            }
        }
        w.close_frame();
        let out = w.finish_single();
        diags.extend(out.diags);
        merge(&mut names, out.names);
        refs.extend(out.refs);
        calls.extend(out.calls);
        match out.locals {
            Locals::Single(l) => per_op.push(l),
            Locals::PerOp(_) => unreachable!("a single walker"),
        }
    }
    Output {
        diags,
        names,
        refs,
        calls,
        locals: Locals::PerOp(per_op),
    }
}

fn merge(into: &mut Resolutions, from: Resolutions) {
    into.exprs.extend(from.exprs);
    into.patterns.extend(from.patterns);
    into.struct_paths.extend(from.struct_paths);
    into.assigns.extend(from.assigns);
    into.binders.extend(from.binders);
    into.types.extend(from.types);
    into.holes.extend(from.holes);
    into.hidden.extend(from.hidden);
}

fn walk_refinement(
    cx: &Checker<'_>,
    def: DefId,
    t: &ast::TypeDecl,
    r: &ast::RefinementDecl,
) -> Output {
    let rd = cx
        .program
        .def(def)
        .kind
        .as_refinement()
        .expect("a refinement");
    let resolver = cx.resolver(def, &[]);
    let public = t.is_pub;
    let span = where_clause_span(cx, r);
    let mut w = Walker::new(
        cx,
        resolver,
        def,
        Owner::Refinement,
        false,
        LocalScopes::new(infos(&rd.locals)),
    );
    w.scopes.push();
    if !rd.locals.is_empty() {
        w.scopes.enter(rd.self_local);
    }
    w.in_contract(span, &r.predicate, public, |w| {
        w.expr(&r.predicate, Expect::Maybe)
    });
    w.close_frame();
    w.finish_single()
}

fn walk_struct_invariant(cx: &Checker<'_>, def: DefId, s: &ast::StructDecl) -> Output {
    let rd = cx.program.def(def).kind.checked_predicate().unwrap();
    let clause = s.invariant.as_ref().unwrap();
    let resolver = cx.resolver(def, cx.program.def(def).kind.generics());
    let mut w = Walker::new(
        cx,
        resolver,
        def,
        Owner::Refinement,
        false,
        LocalScopes::new(infos(&rd.locals)),
    );
    w.scopes.push();
    w.scopes.enter(rd.self_local);
    w.in_contract(clause.span, &clause.expr, s.is_pub, |w| {
        w.expr(&clause.expr, Expect::Maybe)
    });
    w.close_frame();
    w.finish_single()
}

/// The span of a refinement's `where` clause, from the keyword to the end
/// of the predicate (semantics S63 (2)): the AST keeps no span for the
/// keyword, so it is found in the source between the base and the predicate.
pub(crate) fn where_clause_span(cx: &Checker<'_>, r: &ast::RefinementDecl) -> Span {
    let file = r.predicate.span.file;
    let source = cx.project.source_map.file(file);
    let from = r.base.span.end;
    let to = r.predicate.span.start;
    let between = source.slice(from, to);
    match between.find("where") {
        Some(offset) => Span::new(file, from + offset as u32, r.predicate.span.end),
        None => r.predicate.span,
    }
}

fn walk_test(cx: &Checker<'_>, def: DefId, t: &ast::TestDecl) -> Output {
    let resolver = cx.resolver(def, &[]);
    let mut w = Walker::new(
        cx,
        resolver,
        def,
        Owner::Test,
        false,
        LocalScopes::default(),
    );
    w.block(&t.body);
    w.finish_single()
}

fn walk_property(cx: &Checker<'_>, def: DefId, p: &ast::PropertyDecl) -> Output {
    let resolver = cx.resolver(def, &[]);
    let mut w = Walker::new(
        cx,
        resolver,
        def,
        Owner::Property,
        false,
        LocalScopes::default(),
    );
    w.scopes.push();
    // The ranges see no binder (S43, stdlib S30).
    for clause in &p.for_clauses {
        w.in_contract(clause.span, &clause.range, false, |w| {
            w.expr(&clause.range, Expect::Maybe)
        });
    }
    for clause in &p.for_clauses {
        w.bind(&clause.binder, BindingKind::For, false);
    }
    w.block(&p.body);
    w.close_frame();
    w.finish_single()
}

/// Whether the expected type of an expression is known to be absent (an
/// infer-only position of S56) or may exist.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Expect {
    No,
    Maybe,
}

/// Whether a name is a callee (`f(...)`, where a function or module is
/// legal) or a value.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Pos {
    Value,
    Callee,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Owner {
    Fn,
    Op,
    Refinement,
    Test,
    Property,
}

/// The contract expression being walked (5.1).
#[derive(Clone, Debug)]
struct ContractCtx {
    span: Span,
    text: String,
    /// The clause belongs to a public declaration's contract facet (S4).
    public: bool,
}

struct Walker<'c, 'a> {
    cx: &'c Checker<'a>,
    r: Resolver<'c>,
    def: DefId,
    owner: Owner,
    has_ensures: bool,
    scopes: LocalScopes,
    contract: Option<ContractCtx>,
    /// (clause span, callee) pairs already reported private (S4).
    private_reported: BTreeSet<(Span, DefId)>,
    diags: Vec<Diagnostic>,
    names: Resolutions,
    refs: Vec<(Span, DefId, bool)>,
    calls: BTreeSet<DefId>,
}

impl<'c, 'a> Walker<'c, 'a> {
    fn new(
        cx: &'c Checker<'a>,
        r: Resolver<'c>,
        def: DefId,
        owner: Owner,
        has_ensures: bool,
        scopes: LocalScopes,
    ) -> Walker<'c, 'a> {
        Walker {
            cx,
            r,
            def,
            owner,
            has_ensures,
            scopes,
            contract: None,
            private_reported: BTreeSet::new(),
            diags: Vec::new(),
            names: Resolutions::default(),
            refs: Vec::new(),
            calls: BTreeSet::new(),
        }
    }

    fn finish_single(self) -> Output {
        Output {
            diags: self.diags,
            names: self.names,
            refs: self.refs,
            calls: self.calls,
            locals: Locals::Single(self.scopes.locals),
        }
    }

    fn absorb(&mut self, out: ResOut) {
        self.diags.extend(out.diags);
        let interface = self.contract.is_some();
        for (span, def) in out.uses {
            self.refs.push((span, def, interface));
        }
    }

    /// Walk a contract expression (5.1) of `span`.
    fn in_contract(
        &mut self,
        span: Span,
        expr: &ast::Expr,
        public: bool,
        f: impl FnOnce(&mut Self),
    ) {
        let saved = self.contract.replace(ContractCtx {
            span,
            text: crate::collect::clause_text(expr),
            public,
        });
        f(self);
        self.contract = saved;
    }

    /// `return` and `?` belong to `fn` bodies; in a contract expression the
    /// contract stage reports them (5.1 takes precedence).
    fn outside_fn(&self) -> bool {
        self.contract.is_none() && matches!(self.owner, Owner::Test | Owner::Property)
    }

    // ---- bindings --------------------------------------------------------------

    /// Bind a local (S8, S9) and record it.
    fn bind(&mut self, n: &ast::Name, kind: BindingKind, mutable: bool) -> LocalId {
        let name = n.text.as_str();
        if name == "result" && self.has_ensures {
            self.diags.push(
                diag(
                    codes::RESULT_NAME_RESERVED,
                    Some(n.span),
                    "`result` names the returned value in `ensures` clauses; a local of a declaration with `ensures` cannot be named `result`",
                )
                .with_data("name", json!(name)),
            );
        } else if let Some(earlier) = self.scopes.lookup(name) {
            // The earlier binding is not also `unused_binding`: whichever of
            // the two the fix renames, the reads that follow decide it.
            self.scopes.info_mut(earlier).read = true;
            let earlier_span = self.scopes.info(earlier).span;
            self.diags.push(
                diag(
                    codes::SHADOWED_BINDING,
                    Some(n.span),
                    format!("`{name}` is already bound in this function; choose another name"),
                )
                .with_data("name", json!(name))
                .with_note(format!("`{name}` is first bound here"), Some(earlier_span)),
            );
        } else if name == "self" {
            self.diags.push(
                diag(
                    codes::SELF_BOUND,
                    Some(n.span),
                    "`self` is the value of a refinement's `where` predicate; binding it elsewhere reads like that",
                )
                .with_data("name", json!(name)),
            );
        }
        let id = self.scopes.bind(LocalInfo {
            name: name.to_string(),
            kind,
            mutable,
            span: n.span,
            ty: Ty::Error,
            read: false,
        });
        self.names.binders.insert(n.span, id);
        id
    }

    /// Close the innermost frame: `unused_binding` for its unread `let`,
    /// `var`, and pattern bindings (S67).
    fn close_frame(&mut self) {
        for id in self.scopes.pop() {
            let info = self.scopes.info(LocalId(id));
            let diagnosed = matches!(
                info.kind,
                BindingKind::Let | BindingKind::Var | BindingKind::Pattern
            );
            if diagnosed && !info.read && !info.name.starts_with('_') {
                self.diags.push(
                    diag(
                        codes::UNUSED_BINDING,
                        Some(info.span),
                        format!("`{}` is never read", info.name),
                    )
                    .with_data("name", json!(info.name)),
                );
            }
        }
    }

    /// A local in scope, marked read.
    fn read_local(&mut self, name: &str) -> Option<LocalId> {
        let id = self.scopes.lookup(name)?;
        self.scopes.info_mut(id).read = true;
        Some(id)
    }

    // ---- names -------------------------------------------------------------------

    /// A lower-case name of an expression (2.2, S9, S11).
    fn value_name(&mut self, n: &ast::Name, pos: Pos) -> Res {
        if let Some(id) = self.read_local(&n.text) {
            if pos == Pos::Callee {
                if let Some(item) = self.r.peek_value_item(&n.text) {
                    self.names.hidden.insert(n.span, item);
                }
            }
            return Res::Local(id);
        }
        if n.text == "result" {
            self.diags.push(
                diag(
                    codes::RESULT_OUTSIDE_ENSURES,
                    Some(self.contract.as_ref().map_or(n.span, |c| c.span)),
                    "`result` is bound only in `ensures` clauses",
                )
                .with_data("name", json!("result")),
            );
            return Res::Error;
        }
        if n.text == "self" {
            self.diags.push(
                diag(
                    codes::SELF_OUTSIDE_WHERE,
                    Some(n.span),
                    "`self` is bound only in a refinement's `where` predicate",
                )
                .with_data("name", json!("self")),
            );
            return Res::Error;
        }
        let outcome = match self.r.lookup_value(&n.text) {
            ValueTarget::Fn(d) => PathOutcome::Fn(d),
            ValueTarget::Check => PathOutcome::Check,
            ValueTarget::Module(p) => PathOutcome::Module(p),
            ValueTarget::Error => PathOutcome::Error,
            ValueTarget::NotFound => {
                let mut d = crate::resolve::unresolved_name(n.span, &n.text, None);
                if let Some(hint) = self.r.member_hint(&n.text) {
                    d = d.with_note(hint, None);
                }
                self.diags.push(d);
                PathOutcome::Error
            }
        };
        self.outcome(outcome, n.span, &n.text, pos)
    }

    /// The `Res` of a non-local chain or name in a position.
    fn outcome(&mut self, outcome: PathOutcome, span: Span, name: &str, pos: Pos) -> Res {
        match outcome {
            PathOutcome::Fn(d) => {
                if pos == Pos::Callee {
                    self.check_private_callee(d);
                    self.calls.insert(d);
                    Res::Fn(d)
                } else if self.cx.decls[d.0 as usize].annots.module_property {
                    Res::Fn(d)
                } else {
                    self.not_a_value(span, name, "a function; call it with arguments");
                    Res::Error
                }
            }
            PathOutcome::Check => {
                if pos == Pos::Callee {
                    Res::Check
                } else {
                    self.not_a_value(span, name, "the intrinsic `check<R>(value)`");
                    Res::Error
                }
            }
            PathOutcome::Ctor(c) => Res::Ctor(c),
            PathOutcome::Type(d) => {
                if matches!(self.r.program.def(d).kind, DefKind::Wrapper(_)) {
                    Res::Ctor(Ctor::Wrapper(d))
                } else {
                    self.diags
                        .push(crate::resolve::not_a_constructor(span, name));
                    Res::Error
                }
            }
            PathOutcome::TypeParam => {
                self.diags
                    .push(crate::resolve::not_a_constructor(span, name));
                Res::Error
            }
            PathOutcome::Module(p) => {
                if pos == Pos::Callee {
                    Res::Module(p)
                } else {
                    self.not_a_value(span, name, "a module path");
                    Res::Error
                }
            }
            PathOutcome::Error => Res::Error,
        }
    }

    fn not_a_value(&mut self, span: Span, name: &str, what: &str) {
        self.diags.push(
            diag(
                codes::NOT_A_VALUE,
                Some(span),
                format!("`{name}` is not a value: it is {what}"),
            )
            .with_data("name", json!(name)),
        );
    }

    /// S4: a public declaration's contract may call only public and prelude
    /// functions.
    fn check_private_callee(&mut self, callee: DefId) {
        let Some(ctx) = &self.contract else { return };
        if !ctx.public || self.r.program.def(callee).is_pub {
            return;
        }
        if !self.private_reported.insert((ctx.span, callee)) {
            return;
        }
        let callee_name = self.r.program.display_path(callee);
        let d = diag(
            codes::PRIVATE_IN_PUBLIC_CONTRACT,
            Some(ctx.span),
            format!(
                "the contract of public `{}` calls private `{callee_name}`; make it `pub` or move the condition into the body",
                self.r.program.def(self.def).name.display()
            ),
        )
        .with_data("clause", json!(ctx.text))
        .with_data("callee", json!(callee_name));
        self.diags
            .push(blame(d, self.cx, self.def, Some(Facet::Contract)));
    }

    /// An unqualified constructor in an expression (S10; see the module
    /// docs for what is decided here).
    fn ctor_name(&mut self, n: &ast::Name, expect: Expect) -> Res {
        let mut out = ResOut::default();
        let res = match expect {
            Expect::No => self.r.lexical_ctor(n, &mut out),
            Expect::Maybe => {
                let global = self.r.ctors.named(&n.text);
                if global.is_empty() {
                    out.diags.extend(self.r.no_ctor(n));
                    Res::Error
                } else {
                    let lexical = self.r.lexical_ctors(&n.text);
                    if global.len() == 1 && lexical.len() == 1 && lexical[0] == global[0] {
                        Res::Ctor(lexical[0])
                    } else {
                        Res::DeferredCtor {
                            name: n.text.clone(),
                            candidates: lexical,
                        }
                    }
                }
            }
        };
        self.absorb(out);
        res
    }

    /// An unqualified constructor in a pattern: resolved against the
    /// scrutinee type by the typer (2.5), decided here only when nothing in
    /// the program could match: an unknown name is `unresolved_name`, a
    /// diagnosed import nothing (S62), and a type is left to the typer's
    /// `not_a_constructor` (S10 step 3, which uses the import it goes
    /// through, 1.6).
    fn pattern_ctor(&mut self, n: &ast::Name) -> Res {
        let global = self.r.ctors.named(&n.text);
        if global.is_empty() {
            return match self.r.ctor_step3(&n.text) {
                CtorStep3::Nothing => {
                    self.diags
                        .push(crate::resolve::unresolved_name(n.span, &n.text, None));
                    Res::Error
                }
                CtorStep3::Erroneous => Res::Error,
                CtorStep3::Type => Res::DeferredCtor {
                    name: n.text.clone(),
                    candidates: Vec::new(),
                },
            };
        }
        let lexical = self.r.lexical_ctors(&n.text);
        if global.len() == 1 && lexical.len() == 1 && lexical[0] == global[0] {
            Res::Ctor(lexical[0])
        } else {
            Res::DeferredCtor {
                name: n.text.clone(),
                candidates: lexical,
            }
        }
    }

    /// A member expression: a qualified chain (2.4) or a value member.
    fn member(&mut self, e: &ast::Expr, pos: Pos) {
        let ast::ExprKind::Member(m) = &e.kind else {
            return;
        };
        let Some(segs) = chain_segments(e) else {
            // The target is an expression: a receiver (infer-only, S56).
            self.expr(&m.target, Expect::No);
            return;
        };
        let root = segs[0];
        if !root.is_upper() {
            if let Some(id) = self.read_local(&root.text) {
                self.names.exprs.insert(root.span, Res::Local(id));
                return;
            }
            if root.text == "result" || root.text == "self" {
                let res = self.value_name(root, Pos::Value);
                self.names.exprs.insert(root.span, res);
                return;
            }
        }
        let mut out = ResOut::default();
        let outcome = self.r.resolve_global_chain(&segs, &mut out);
        self.absorb(out);
        let name = segs
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(".");
        let res = self.outcome(outcome, e.span, &name, pos);
        self.names.exprs.insert(e.span, res);
    }

    // ---- expressions -------------------------------------------------------------

    fn expr(&mut self, e: &ast::Expr, expect: Expect) {
        use ast::ExprKind as K;
        match &e.kind {
            K::Literal(_) => {}
            K::Name(n) => {
                let res = if n.is_upper() {
                    self.ctor_name(n, expect)
                } else {
                    self.value_name(n, Pos::Value)
                };
                self.names.exprs.insert(e.span, res);
            }
            K::Hole(_) => {
                self.names.holes.insert(e.span, self.scopes.visible());
            }
            K::Forall(f) | K::Parallel(f) => {
                self.expr(&f.collection, Expect::No);
                self.scopes.push();
                self.bind(
                    &f.binder,
                    if matches!(e.kind, K::Parallel(_)) {
                        BindingKind::Parallel
                    } else {
                        BindingKind::Forall
                    },
                    false,
                );
                self.expr(&f.predicate, Expect::Maybe);
                self.close_frame();
            }
            K::Binary(b) => {
                let left = match b.op {
                    ast::BinaryOp::Eq | ast::BinaryOp::Ne | ast::BinaryOp::Concat => Expect::No,
                    _ => Expect::Maybe,
                };
                self.expr(&b.lhs, left);
                self.expr(&b.rhs, Expect::Maybe);
            }
            K::Unary(u) => self.expr(&u.operand, Expect::Maybe),
            K::Member(_) => self.member(e, Pos::Value),
            K::Call(c) => self.call(c, expect),
            K::Propagate(inner) => {
                if self.outside_fn() {
                    self.diags.push(diag(
                        codes::PROPAGATE_OUTSIDE_FUNCTION,
                        Some(e.span),
                        "`?` propagates an error out of a function; tests and properties are not functions",
                    ));
                }
                self.expr(inner, Expect::Maybe);
            }
            K::Paren(inner) => self.expr(inner, expect),
            K::List(l) => {
                for (i, el) in l.elements.iter().enumerate() {
                    self.expr(&el.expr, if i == 0 { expect } else { Expect::Maybe });
                }
            }
            K::Struct(s) => self.struct_lit(s),
            K::If(i) => self.if_expr(i),
            K::Match(m) => {
                self.expr(&m.scrutinee, Expect::No);
                for arm in &m.arms {
                    self.scopes.push();
                    let mut bound = BTreeMap::new();
                    self.pattern(&arm.pattern, &mut bound);
                    match &arm.body {
                        ast::ArmBody::Block(b) => self.block(b),
                        ast::ArmBody::Expr(x) => self.expr(x, Expect::Maybe),
                    }
                    self.close_frame();
                }
            }
        }
    }

    fn call(&mut self, c: &ast::CallExpr, expect: Expect) {
        match &c.callee.kind {
            ast::ExprKind::Name(n) if n.is_upper() => {
                let res = self.ctor_name(n, expect);
                self.names.exprs.insert(c.callee.span, res);
            }
            ast::ExprKind::Name(n) => {
                let res = self.value_name(n, Pos::Callee);
                self.names.exprs.insert(c.callee.span, res);
            }
            ast::ExprKind::Member(_) => self.member(&c.callee, Pos::Callee),
            _ => self.expr(&c.callee, Expect::No),
        }
        if let Some(g) = &c.generic_args {
            for t in &g.args {
                self.body_type(t);
            }
        }
        for a in &c.args.positional {
            self.expr(&a.expr, Expect::Maybe);
        }
        for u in &c.args.using {
            self.expr(&u.expr, Expect::Maybe);
        }
    }

    fn if_expr(&mut self, i: &ast::IfExpr) {
        self.expr(&i.condition, Expect::Maybe);
        self.block(&i.then_block);
        match &i.else_branch {
            Some(ast::ElseBranch::Block(b)) => self.block(b),
            Some(ast::ElseBranch::If(nested)) => self.if_expr(nested),
            None => {}
        }
    }

    fn struct_lit(&mut self, s: &ast::StructLiteral) {
        let mut out = ResOut::default();
        let target = self.r.type_path(&s.path, &mut out);
        let mut fields_of: Option<(DefId, Vec<String>)> = None;
        let res = match target {
            TypeTarget::Def(d) => {
                out.uses.push((s.path.span, d));
                match &self.r.program.def(d).kind {
                    DefKind::Struct(sd) => {
                        fields_of = Some((d, sd.fields.iter().map(|f| f.name.clone()).collect()));
                        Res::Struct(d)
                    }
                    _ => {
                        out.push(
                            diag(
                                codes::NOT_A_CONSTRUCTOR,
                                Some(s.path.span),
                                format!(
                                    "`{}` is not a struct; a struct literal names a struct type",
                                    s.path.dotted()
                                ),
                            )
                            .with_data("name", json!(s.path.dotted())),
                        );
                        Res::Error
                    }
                }
            }
            TypeTarget::Param(_) => {
                out.push(
                    diag(
                        codes::NOT_A_CONSTRUCTOR,
                        Some(s.path.span),
                        format!("type parameter `{}` has no struct literal", s.path.dotted()),
                    )
                    .with_data("name", json!(s.path.dotted())),
                );
                Res::Error
            }
            TypeTarget::Error => Res::Error,
        };
        self.absorb(out);
        self.names.struct_paths.insert(s.path.span, res);
        if let Some(spread) = &s.entries.spread {
            self.expr(spread, Expect::Maybe);
        }
        for f in &s.entries.fields {
            self.expr(&f.value, Expect::Maybe);
        }
        let Some((def, fields)) = fields_of else {
            return;
        };
        let type_name = self.r.program.def(def).name.display();
        let mut seen: BTreeMap<&str, Span> = BTreeMap::new();
        for f in &s.entries.fields {
            let name = f.name.text.as_str();
            if !fields.iter().any(|x| x == name) {
                self.diags.push(
                    diag(
                        codes::UNKNOWN_FIELD,
                        Some(f.span),
                        format!("struct `{type_name}` has no field `{name}`"),
                    )
                    .with_data("name", json!(name))
                    .with_data("type", json!(type_name)),
                );
            } else if let Some(first) = seen.get(name) {
                self.diags.push(
                    diag(
                        codes::DUPLICATE_FIELD,
                        Some(f.span),
                        format!("field `{name}` is given twice"),
                    )
                    .with_data("name", json!(name))
                    .with_data("type", json!(type_name))
                    .with_note("the first value is here", Some(*first)),
                );
            } else {
                seen.insert(name, f.span);
            }
        }
        if s.entries.spread.is_none() {
            let missing: Vec<&String> = fields
                .iter()
                .filter(|x| !seen.contains_key(x.as_str()))
                .collect();
            if !missing.is_empty() {
                self.diags.push(
                    diag(
                        codes::MISSING_FIELD,
                        Some(s.span),
                        format!(
                            "struct `{type_name}` is missing {}",
                            missing
                                .iter()
                                .map(|m| format!("`{m}`"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    )
                    .with_data("missing", json!(missing))
                    .with_data("type", json!(type_name)),
                );
            }
        }
    }

    /// A type annotation or generic argument in a body or clause.
    fn body_type(&mut self, t: &ast::Type) {
        let mut out = ResOut::default();
        let ty = self.r.resolve_type(t, &mut out);
        value_type_errors(t, &ty, self.r.program, self.r.file.is_std, &mut out.diags);
        self.absorb(out);
        self.names.types.insert(t.span, ty);
    }

    // ---- statements --------------------------------------------------------------

    fn block(&mut self, b: &ast::Block) {
        self.scopes.push();
        for s in &b.statements {
            self.stmt(s);
        }
        if let Some(tail) = &b.tail {
            self.expr(&tail.expr, Expect::Maybe);
        }
        self.close_frame();
    }

    fn stmt(&mut self, s: &ast::Statement) {
        match s {
            ast::Statement::Let(l) => {
                let expect = if l.ty.is_some() {
                    Expect::Maybe
                } else {
                    Expect::No
                };
                self.expr(&l.value, expect);
                if let Some(t) = &l.ty {
                    self.body_type(t);
                }
                if let ast::LetBinding::Name(n) = &l.binding {
                    self.bind(n, BindingKind::Let, false);
                }
            }
            ast::Statement::Var(v) => {
                let expect = if v.ty.is_some() {
                    Expect::Maybe
                } else {
                    Expect::No
                };
                self.expr(&v.value, expect);
                if let Some(t) = &v.ty {
                    self.body_type(t);
                }
                self.bind(&v.name, BindingKind::Var, true);
            }
            ast::Statement::Assign(a) => {
                self.expr(&a.value, Expect::Maybe);
                self.assign_target(a);
            }
            ast::Statement::Return(r) => {
                if self.outside_fn() {
                    self.diags.push(diag(
                        codes::RETURN_OUTSIDE_FUNCTION,
                        Some(r.span),
                        "`return` exits a function; tests and properties are not functions",
                    ));
                }
                if let Some(v) = &r.value {
                    self.expr(v, Expect::Maybe);
                }
            }
            ast::Statement::Assert(a) => self.expr(&a.condition, Expect::Maybe),
            ast::Statement::While(w) => {
                self.expr(&w.condition, Expect::Maybe);
                for clause in &w.clauses {
                    let (span, e) = match clause {
                        ast::WhileClause::Invariant(i) => (i.span, &i.expr),
                        ast::WhileClause::Decreases(d) => (d.span, &d.expr),
                    };
                    self.in_contract(span, e, false, |w| w.expr(e, Expect::Maybe));
                }
                self.block(&w.body);
            }
            ast::Statement::Expr(x) => self.expr(&x.expr, Expect::Maybe),
        }
    }

    /// An assignment target (3.11): a `var` of the enclosing body.
    fn assign_target(&mut self, a: &ast::AssignStmt) {
        let t = &a.target;
        let res = match self.scopes.lookup(&t.text) {
            Some(id) if self.scopes.info(id).kind == BindingKind::Var => Res::Local(id),
            Some(id) => {
                let info = self.scopes.info(id);
                let what = match info.kind {
                    BindingKind::Parameter | BindingKind::Using => "a parameter",
                    BindingKind::For => "a property binder",
                    BindingKind::Forall => "a `forall` binder",
                    BindingKind::Parallel => "a `parallel for` binder",
                    BindingKind::Pattern => "a pattern binding",
                    BindingKind::Result => "the returned value",
                    BindingKind::SelfValue => "the refined value",
                    _ => "a `let` binding",
                };
                let span = info.span;
                self.diags.push(
                    diag(
                        codes::ASSIGN_TO_IMMUTABLE,
                        Some(a.span),
                        format!(
                            "cannot assign to `{}`: it is {what}; declare it with `var`",
                            t.text
                        ),
                    )
                    .with_data("name", json!(t.text))
                    .with_note(format!("`{}` is bound here", t.text), Some(span)),
                );
                Res::Error
            }
            None => {
                self.diags
                    .push(crate::resolve::unresolved_name(t.span, &t.text, None));
                Res::Error
            }
        };
        self.names.assigns.insert(t.span, res);
    }

    // ---- patterns ----------------------------------------------------------------

    fn pattern(&mut self, p: &ast::Pattern, bound: &mut BTreeMap<String, Span>) {
        match &p.kind {
            ast::PatternKind::Binding(n) => {
                if let Some(first) = bound.get(&n.text) {
                    self.diags.push(
                        diag(
                            codes::DUPLICATE_PATTERN_BINDING,
                            Some(n.span),
                            format!("`{}` is bound twice in one pattern", n.text),
                        )
                        .with_data("name", json!(n.text))
                        .with_note("the first binding is here", Some(*first)),
                    );
                } else {
                    bound.insert(n.text.clone(), n.span);
                    self.bind(n, BindingKind::Pattern, false);
                }
            }
            ast::PatternKind::Ctor(c) => {
                let res = if c.path.qualifier.is_empty() {
                    self.pattern_ctor(&c.path.name)
                } else {
                    let segs: Vec<&ast::Name> = c
                        .path
                        .qualifier
                        .iter()
                        .chain(std::iter::once(&c.path.name))
                        .collect();
                    let mut out = ResOut::default();
                    let outcome = self.r.resolve_global_chain(&segs, &mut out);
                    self.absorb(out);
                    self.outcome(outcome, c.path.span, &c.path.dotted(), Pos::Value)
                };
                self.names.patterns.insert(c.path.span, res);
                if let Some(args) = &c.args {
                    for a in args {
                        self.pattern(a, bound);
                    }
                }
            }
            _ => {}
        }
    }
}
