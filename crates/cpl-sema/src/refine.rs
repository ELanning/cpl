//! Refinement types (`spec/semantics.md` 3.3, 1.8, S55, S65, S66; stdlib
//! S29; toolchain T39): the declared base of every refinement, the
//! `cyclic_type_definition` check, and the structural identity classes
//! ([`RefinementClass`]) that make two refinements with the same base and
//! the same *resolved predicate* one type.
//!
//! # Resolved predicates (S55)
//!
//! Two predicates are the same when their syntax trees are identical once
//! grouping parentheses are removed, literals are equal by value, `self` is
//! `self`, and every name resolves to the same declaration. The checker
//! compares [`PKey`]s: a tree built from the AST with every name replaced by
//! its resolution (a `DefId`, a constructor, `self`, or the binder's
//! position for `forall`, pattern, and `let` binders, which therefore need
//! no renaming). Type arguments and struct-literal paths compare by the
//! declaration they name. An unqualified constructor compares by its unique
//! lexical candidate when it has one and by its name otherwise: its
//! expected type is fixed by the identical surrounding tree, so the name
//! decides (S10 step 1).
//!
//! # For the body typer
//!
//! [`Refinements::base`], [`Refinements::strip`], [`Refinements::chain`],
//! and [`Refinements::erase`] are the functions of 3.3. The predicate of a
//! refinement is typed with `RefinementDef::locals[0]` (`self`, of the base
//! type) in scope; its names are in `Checker::names` like any clause's.
//! `RefinementClassDef::predicate` holds a placeholder (`ExprKind::Error`)
//! until the typer stores the typed predicate of the class's first member.

use crate::checker::{diag, Checker, Stage};
use crate::codes;
use crate::resolve::{Ctor, PathOutcome, ResOut, Resolver, TypeTarget, ValueTarget};
use cpl_diag::{Note, Span};
use cpl_hir::{DefId, DefKind, Expr, ExprKind, RefinementClass, RefinementClassDef, Ty};
use cpl_syntax::ast;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// What the checker knows about one refinement declaration.
#[derive(Clone, Debug)]
pub(crate) struct RefinementInfo {
    /// The declared base; `None` when the refinement lies on or depends on
    /// a cycle, or its base did not resolve.
    pub base: Option<Ty>,
    /// The identity class; `None` exactly when `base` is `None`.
    pub class: Option<RefinementClass>,
    /// The type paths of the base, for references and the exportability
    /// check of 1.5.
    pub base_uses: Vec<(Span, DefId)>,
}

/// Every refinement of the program.
#[derive(Clone, Debug, Default)]
pub(crate) struct Refinements {
    info: HashMap<DefId, RefinementInfo>,
}

impl Refinements {
    pub fn get(&self, def: DefId) -> Option<&RefinementInfo> {
        self.info.get(&def)
    }

    /// The type a use of refinement `def` denotes: `Ty::Refined(class,
    /// def)`, or `Ty::Error` for a refinement without a class (S62, S66).
    pub fn ty_of(&self, def: DefId) -> Ty {
        match self.info.get(&def).and_then(|i| i.class) {
            Some(class) => Ty::Refined(class, def),
            None => Ty::Error,
        }
    }

    /// `base(R)`: the declared base of refinement `def`.
    pub fn base(&self, def: DefId) -> Option<&Ty> {
        self.info.get(&def).and_then(|i| i.base.as_ref())
    }

    /// `strip(T)` (3.3): the root base of a refinement, `T` itself
    /// otherwise; `Ty::Error` for a refinement without a base.
    pub fn strip(&self, ty: &Ty) -> Ty {
        let mut ty = ty.clone();
        let mut guard = 0;
        while let Ty::Refined(_, def) = ty {
            guard += 1;
            match self.base(def) {
                Some(b) if guard < 1000 => ty = b.clone(),
                _ => return Ty::Error,
            }
        }
        ty
    }

    /// `chain(R)` (3.3): `[R, base(R), ...]` down to but excluding the root
    /// base, following the declarations as written (S55: the declaration
    /// named at the use site).
    pub fn chain(&self, def: DefId) -> Vec<DefId> {
        let mut out = vec![def];
        let mut current = def;
        while let Some(Ty::Refined(_, next)) = self.base(current) {
            if out.contains(next) {
                break;
            }
            out.push(*next);
            current = *next;
        }
        out
    }

    /// `erase(T)` (3.3): `strip` at every position (struct fields are not
    /// entered: structs are nominal).
    #[allow(dead_code)] // the typer erases with its inference variables resolved (`types::typer`)
    pub fn erase(&self, ty: &Ty) -> Ty {
        match self.strip(ty) {
            Ty::List(t) => Ty::List(Box::new(self.erase(&t))),
            Ty::Gen(t) => Ty::Gen(Box::new(self.erase(&t))),
            Ty::Adt(d, args) => Ty::Adt(d, args.iter().map(|a| self.erase(a)).collect()),
            other => other,
        }
    }
}

/// Resolve every refinement's base, report `cyclic_type_definition`, and
/// assign identity classes (Program::refinement_classes). Runs after
/// collection and import resolution, before the other signatures (which
/// need `Ty::Refined`).
pub(crate) fn resolve_refinements(cx: &mut Checker<'_>) {
    let refinements: Vec<DefId> = cx
        .program
        .defs()
        .filter(|d| matches!(d.kind, DefKind::Refinement(_)))
        .map(|d| d.id)
        .collect();
    if refinements.is_empty() {
        return;
    }

    // The dependency graph: R -> every refinement its base mentions.
    let mut deps: BTreeMap<DefId, BTreeSet<DefId>> = BTreeMap::new();
    for &r in &refinements {
        let (ast, index) = cx.decl_ast(r);
        let base = refinement_decl(&ast.declarations[index]).map(|rd| &rd.base);
        let mut out = BTreeSet::new();
        if let Some(base) = base {
            let resolver = cx.resolver(r, &[]);
            scan_type_deps(&resolver, base, &mut out);
        }
        out.retain(|d| matches!(cx.program.def(*d).kind, DefKind::Refinement(_)));
        deps.insert(r, out);
    }

    let sccs = tarjan(&refinements, &deps);
    let mut cyclic: BTreeSet<DefId> = BTreeSet::new();
    for scc in &sccs {
        let is_cycle = scc.len() > 1 || deps[&scc[0]].contains(&scc[0]);
        if is_cycle {
            cyclic.extend(scc.iter().copied());
        }
    }
    // Report every declaration of every cycle (S66), in DefId order.
    for &r in &cyclic {
        let cycle = find_cycle(r, &deps, &cyclic);
        let names: Vec<String> = cycle
            .iter()
            .map(|d| cx.program.def(*d).name.display())
            .collect();
        let name = cx.program.def(r).name.display();
        let span = cx.decls[r.0 as usize].name_span;
        let mut d = diag(
            codes::CYCLIC_TYPE_DEFINITION,
            Some(span),
            format!("refinement `{name}` has no root base: its base chain reaches `{name}` again"),
        )
        .with_data("type", json!(name));
        d.notes
            .push(Note::new(format!("cycle: {}", names.join(" -> "))));
        for &other in &cycle {
            if other != r {
                d.notes.push(Note::at(
                    format!(
                        "`{}` is declared here",
                        cx.program.def(other).name.display()
                    ),
                    cx.decls[other.0 as usize].name_span,
                ));
            }
        }
        cx.emit(Stage::Resolve, d);
        cx.fail(r);
        cx.refinements.info.insert(
            r,
            RefinementInfo {
                base: None,
                class: None,
                base_uses: Vec::new(),
            },
        );
    }

    // Classes, dependencies first (Tarjan emits an SCC after every SCC it
    // reaches).
    let mut keys: Vec<(Ty, PKey)> = Vec::new();
    let mut classes: Vec<RefinementClassDef> = Vec::new();
    for scc in &sccs {
        for &r in scc {
            if cyclic.contains(&r) {
                continue;
            }
            let (ast, index) = cx.decl_ast(r);
            let Some(rd) = refinement_decl(&ast.declarations[index]) else {
                continue;
            };
            let blocked = deps[&r].iter().any(|d| cx.refinements.base(*d).is_none());
            let (base, uses, diags, key) = {
                let resolver = cx.resolver(r, &[]);
                let mut out = ResOut::default();
                let base = resolver.resolve_type(&rd.base, &mut out);
                let key = KeyBuilder::new(&resolver).expr(&rd.predicate);
                (base, out.uses, out.diags, key)
            };
            let failed = diags.iter().any(|d| d.is_error());
            cx.emit_all(Stage::Resolve, diags);
            if failed {
                cx.fail(r);
            }
            if blocked || failed || base.is_error() {
                cx.refinements.info.insert(
                    r,
                    RefinementInfo {
                        base: None,
                        class: None,
                        base_uses: uses,
                    },
                );
                continue;
            }
            let class = match keys.iter().position(|(b, k)| *b == base && *k == key) {
                Some(i) => {
                    classes[i].members.push(r);
                    RefinementClass(i as u32)
                }
                None => {
                    keys.push((base.clone(), key));
                    classes.push(RefinementClassDef {
                        base: base.clone(),
                        predicate: Expr::new(ExprKind::Error, Ty::Bool, rd.predicate.span),
                        members: vec![r],
                    });
                    RefinementClass(classes.len() as u32 - 1)
                }
            };
            cx.refinements.info.insert(
                r,
                RefinementInfo {
                    base: Some(base),
                    class: Some(class),
                    base_uses: uses,
                },
            );
        }
    }
    cx.program.refinement_classes = classes;
}

/// The refinement part of a `type R = B where p` declaration.
pub(crate) fn refinement_decl(decl: &ast::Declaration) -> Option<&ast::RefinementDecl> {
    match &decl.kind {
        ast::DeclKind::Type(t) => match &t.kind {
            ast::TypeDeclKind::Refinement(r) => Some(r),
            _ => None,
        },
        _ => None,
    }
}

/// Every definition a type expression's paths name (no diagnostics).
fn scan_type_deps(resolver: &Resolver<'_>, t: &ast::Type, out: &mut BTreeSet<DefId>) {
    if let TypeTarget::Def(d) = resolver.peek_type(&t.path) {
        out.insert(d);
    }
    if let Some(args) = &t.generic_args {
        for a in &args.args {
            scan_type_deps(resolver, a, out);
        }
    }
}

/// Tarjan's strongly connected components, each in DefId order; the list
/// is in reverse topological order (dependencies first).
pub(crate) fn tarjan(nodes: &[DefId], edges: &BTreeMap<DefId, BTreeSet<DefId>>) -> Vec<Vec<DefId>> {
    struct State<'e> {
        edges: &'e BTreeMap<DefId, BTreeSet<DefId>>,
        index: HashMap<DefId, usize>,
        low: HashMap<DefId, usize>,
        on_stack: BTreeSet<DefId>,
        stack: Vec<DefId>,
        next: usize,
        out: Vec<Vec<DefId>>,
    }
    fn visit(s: &mut State<'_>, v: DefId) {
        s.index.insert(v, s.next);
        s.low.insert(v, s.next);
        s.next += 1;
        s.stack.push(v);
        s.on_stack.insert(v);
        let succ: Vec<DefId> = s
            .edges
            .get(&v)
            .map(|e| e.iter().copied().collect())
            .unwrap_or_default();
        for w in succ {
            if !s.edges.contains_key(&w) {
                continue;
            }
            if !s.index.contains_key(&w) {
                visit(s, w);
                let lw = s.low[&w];
                let lv = s.low.get_mut(&v).expect("visited");
                *lv = (*lv).min(lw);
            } else if s.on_stack.contains(&w) {
                let iw = s.index[&w];
                let lv = s.low.get_mut(&v).expect("visited");
                *lv = (*lv).min(iw);
            }
        }
        if s.low[&v] == s.index[&v] {
            let mut scc = Vec::new();
            while let Some(w) = s.stack.pop() {
                s.on_stack.remove(&w);
                scc.push(w);
                if w == v {
                    break;
                }
            }
            scc.sort();
            s.out.push(scc);
        }
    }
    let mut s = State {
        edges,
        index: HashMap::new(),
        low: HashMap::new(),
        on_stack: BTreeSet::new(),
        stack: Vec::new(),
        next: 0,
        out: Vec::new(),
    };
    for &n in nodes {
        if !s.index.contains_key(&n) {
            visit(&mut s, n);
        }
    }
    s.out
}

/// A shortest cycle from `start` back to itself inside `within`, as the
/// list `[start, ..., start]`.
pub(crate) fn find_cycle(
    start: DefId,
    edges: &BTreeMap<DefId, BTreeSet<DefId>>,
    within: &BTreeSet<DefId>,
) -> Vec<DefId> {
    let mut prev: HashMap<DefId, DefId> = HashMap::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(start);
    let mut seen = BTreeSet::new();
    while let Some(v) = queue.pop_front() {
        for &w in edges.get(&v).into_iter().flatten() {
            if !within.contains(&w) {
                continue;
            }
            if w == start {
                let mut middle = Vec::new();
                let mut cur = v;
                while cur != start {
                    middle.push(cur);
                    cur = prev[&cur];
                }
                middle.reverse();
                let mut path = vec![start];
                path.extend(middle);
                path.push(start);
                return path;
            }
            if seen.insert(w) {
                prev.insert(w, v);
                queue.push_back(w);
            }
        }
    }
    vec![start, start]
}

// ---- resolved predicates (S55) ------------------------------------------------

/// A resolved type in a predicate: by declaration (S55).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TKey {
    Def(DefId, Vec<TKey>),
    Param(u32),
    Unresolved(String),
}

/// A resolved pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PatKey {
    Wildcard,
    Int(bool, String),
    Text(String),
    Bool(bool),
    Bind(u32),
    Ctor(Box<PKey>, Option<Vec<PatKey>>),
}

/// A resolved block (an `if` branch or a `match` arm body).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BKey {
    stmts: Vec<SKey>,
    tail: Option<PKey>,
}

/// A resolved statement of a predicate sub-block (S32: only `let` is
/// admitted; the others are keyed by kind for completeness).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SKey {
    Let(Option<u32>, Option<TKey>, PKey),
    Var(u32, Option<TKey>, PKey),
    Assign(String, PKey),
    Return(Option<PKey>),
    Assert(PKey),
    While(PKey, Vec<PKey>, BKey),
    Concurrent(BKey),
    Expr(PKey),
    Context(u32, PKey),
}

/// A resolved expression tree (see the module docs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PKey {
    Int(String),
    Text(String),
    Bool(bool),
    SelfValue,
    /// A `forall`, pattern, or `let` binder, by binding order.
    Binder(u32),
    Item(DefId),
    Check,
    Ctor(Ctor),
    Type(DefId),
    Module(Vec<String>),
    /// A name that did not resolve (its diagnostic comes from the
    /// resolution walk).
    Name(String),
    Hole(String),
    Unary(ast::UnaryOp, Box<PKey>),
    Binary(ast::BinaryOp, Box<PKey>, Box<PKey>),
    Member(Box<PKey>, String),
    Call(Box<PKey>, Vec<TKey>, Vec<PKey>, Vec<PKey>),
    Propagate(Box<PKey>),
    Forall(Box<PKey>, Box<PKey>),
    Parallel(Box<PKey>, Box<PKey>),
    List(Vec<PKey>),
    Struct(TKey, Option<Box<PKey>>, Vec<(String, PKey)>),
    If(Box<PKey>, Box<BKey>, Option<Box<PKey>>),
    Block(Box<BKey>),
    Match(Box<PKey>, Vec<(PatKey, PKey)>),
}

/// Builds [`PKey`]s in the scope of one refinement's file.
pub(crate) struct KeyBuilder<'r, 'c> {
    resolver: &'r Resolver<'c>,
    binders: Vec<(String, u32)>,
    next: u32,
}

impl<'r, 'c> KeyBuilder<'r, 'c> {
    pub fn new(resolver: &'r Resolver<'c>) -> KeyBuilder<'r, 'c> {
        KeyBuilder {
            resolver,
            binders: Vec::new(),
            next: 0,
        }
    }

    fn bind(&mut self, name: &str) -> u32 {
        let id = self.next;
        self.next += 1;
        self.binders.push((name.to_string(), id));
        id
    }

    fn binder(&self, name: &str) -> Option<u32> {
        self.binders
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, i)| *i)
    }

    fn ty(&self, t: &ast::Type) -> TKey {
        let args: Vec<TKey> = t
            .generic_args
            .as_ref()
            .map(|g| g.args.iter().map(|a| self.ty(a)).collect())
            .unwrap_or_default();
        match self.resolver.peek_type(&t.path) {
            TypeTarget::Def(d) => TKey::Def(d, args),
            TypeTarget::Param(i) => TKey::Param(i),
            TypeTarget::Error => TKey::Unresolved(t.path.dotted()),
        }
    }

    fn path_key(&self, segs: &[&ast::Name]) -> PKey {
        let mut scratch = ResOut::default();
        match self.resolver.resolve_global_chain(segs, &mut scratch) {
            PathOutcome::Fn(d) => PKey::Item(d),
            PathOutcome::Check => PKey::Check,
            PathOutcome::Ctor(c) => PKey::Ctor(c),
            PathOutcome::Type(d) => PKey::Type(d),
            PathOutcome::Module(p) => PKey::Module(p),
            PathOutcome::TypeParam | PathOutcome::Error => PKey::Name(
                segs.iter()
                    .map(|s| s.text.as_str())
                    .collect::<Vec<_>>()
                    .join("."),
            ),
        }
    }

    fn name(&self, n: &ast::Name) -> PKey {
        if n.is_upper() {
            let candidates = self.resolver.lexical_ctors(&n.text);
            if candidates.len() == 1 {
                return PKey::Ctor(candidates[0]);
            }
            return PKey::Name(n.text.clone());
        }
        if let Some(b) = self.binder(&n.text) {
            return PKey::Binder(b);
        }
        if n.text == "self" {
            return PKey::SelfValue;
        }
        match self.resolver.lookup_value(&n.text) {
            ValueTarget::Fn(d) => PKey::Item(d),
            ValueTarget::Check => PKey::Check,
            ValueTarget::Module(p) => PKey::Module(p),
            ValueTarget::Error | ValueTarget::NotFound => PKey::Name(n.text.clone()),
        }
    }

    pub fn expr(&mut self, e: &ast::Expr) -> PKey {
        use ast::ExprKind as K;
        match &e.kind {
            K::Literal(l) => match &l.kind {
                ast::LiteralKind::Int(d) => PKey::Int(d.clone()),
                ast::LiteralKind::Text(t) => PKey::Text(t.clone()),
                ast::LiteralKind::Bool(b) => PKey::Bool(*b),
            },
            K::Name(n) => self.name(n),
            K::Hole(n) => PKey::Hole(n.text.clone()),
            K::Paren(inner) => self.expr(inner),
            K::Unary(u) => PKey::Unary(u.op, Box::new(self.expr(&u.operand))),
            K::Binary(b) => {
                let l = self.expr(&b.lhs);
                let r = self.expr(&b.rhs);
                PKey::Binary(b.op, Box::new(l), Box::new(r))
            }
            K::Propagate(inner) => PKey::Propagate(Box::new(self.expr(inner))),
            K::Member(_) => match chain_segments(e) {
                Some(segs)
                    if self.binder(&segs[0].text).is_none()
                        && !(segs[0].text == "self" && !segs[0].is_upper()) =>
                {
                    self.path_key(&segs)
                }
                _ => {
                    let K::Member(m) = &e.kind else {
                        unreachable!("a member expression")
                    };
                    PKey::Member(Box::new(self.expr(&m.target)), m.member.text.clone())
                }
            },
            K::Call(c) => {
                let callee = self.expr(&c.callee);
                let generics = c
                    .generic_args
                    .as_ref()
                    .map(|g| g.args.iter().map(|a| self.ty(a)).collect())
                    .unwrap_or_default();
                let args = c
                    .args
                    .positional
                    .iter()
                    .map(|a| self.expr(&a.expr))
                    .collect();
                let using = c.args.using.iter().map(|a| self.expr(&a.expr)).collect();
                PKey::Call(Box::new(callee), generics, args, using)
            }
            K::Forall(f) | K::Parallel(f) => {
                let collection = self.expr(&f.collection);
                let mark = self.binders.len();
                self.bind(&f.binder.text);
                let predicate = self.expr(&f.predicate);
                self.binders.truncate(mark);
                if matches!(e.kind, K::Parallel(_)) {
                    PKey::Parallel(Box::new(collection), Box::new(predicate))
                } else {
                    PKey::Forall(Box::new(collection), Box::new(predicate))
                }
            }
            K::List(l) => PKey::List(l.elements.iter().map(|el| self.expr(&el.expr)).collect()),
            K::Struct(s) => {
                let path = ast::Type {
                    span: s.path.span,
                    path: s.path.clone(),
                    generic_args: None,
                };
                let spread = s.entries.spread.as_ref().map(|b| Box::new(self.expr(b)));
                let fields = s
                    .entries
                    .fields
                    .iter()
                    .map(|f| (f.name.text.clone(), self.expr(&f.value)))
                    .collect();
                PKey::Struct(self.ty(&path), spread, fields)
            }
            K::If(i) => self.if_expr(i),
            K::Match(m) => {
                let scrutinee = self.expr(&m.scrutinee);
                let arms = m
                    .arms
                    .iter()
                    .map(|arm| {
                        let mark = self.binders.len();
                        let pattern = self.pattern(&arm.pattern);
                        let body = match &arm.body {
                            ast::ArmBody::Block(b) => PKey::Block(Box::new(self.block(b))),
                            ast::ArmBody::Expr(x) => self.expr(x),
                        };
                        self.binders.truncate(mark);
                        (pattern, body)
                    })
                    .collect();
                PKey::Match(Box::new(scrutinee), arms)
            }
        }
    }

    fn if_expr(&mut self, i: &ast::IfExpr) -> PKey {
        let cond = self.expr(&i.condition);
        let then = self.block(&i.then_block);
        let otherwise = i.else_branch.as_ref().map(|e| {
            Box::new(match e {
                ast::ElseBranch::Block(b) => PKey::Block(Box::new(self.block(b))),
                ast::ElseBranch::If(nested) => self.if_expr(nested),
            })
        });
        PKey::If(Box::new(cond), Box::new(then), otherwise)
    }

    fn block(&mut self, b: &ast::Block) -> BKey {
        let mark = self.binders.len();
        let mut stmts = Vec::new();
        for s in &b.statements {
            stmts.push(match s {
                ast::Statement::Let(l) => {
                    let value = self.expr(&l.value);
                    let ty = l.ty.as_ref().map(|t| self.ty(t));
                    let id = match &l.binding {
                        ast::LetBinding::Name(n) => Some(self.bind(&n.text)),
                        ast::LetBinding::Wildcard(_) => None,
                    };
                    SKey::Let(id, ty, value)
                }
                ast::Statement::Var(v) => {
                    let value = self.expr(&v.value);
                    let ty = v.ty.as_ref().map(|t| self.ty(t));
                    SKey::Var(self.bind(&v.name.text), ty, value)
                }
                ast::Statement::Assign(a) => {
                    SKey::Assign(a.target.text.clone(), self.expr(&a.value))
                }
                ast::Statement::Return(r) => SKey::Return(r.value.as_ref().map(|v| self.expr(v))),
                ast::Statement::Assert(a) => SKey::Assert(self.expr(&a.condition)),
                ast::Statement::While(w) => {
                    let cond = self.expr(&w.condition);
                    let clauses = w
                        .clauses
                        .iter()
                        .map(|c| match c {
                            ast::WhileClause::Invariant(i) => self.expr(&i.expr),
                            ast::WhileClause::Decreases(d) => self.expr(&d.expr),
                        })
                        .collect();
                    SKey::While(cond, clauses, self.block(&w.body))
                }
                ast::Statement::Concurrent(c) => SKey::Concurrent(self.block(&c.body)),
                ast::Statement::Context(c) => {
                    let value = self.expr(&c.value);
                    SKey::Context(self.bind(&c.name.text), value)
                }
                ast::Statement::Expr(x) => SKey::Expr(self.expr(&x.expr)),
            });
        }
        let tail = b.tail.as_ref().map(|t| self.expr(&t.expr));
        self.binders.truncate(mark);
        BKey { stmts, tail }
    }

    fn pattern(&mut self, p: &ast::Pattern) -> PatKey {
        match &p.kind {
            ast::PatternKind::Wildcard => PatKey::Wildcard,
            ast::PatternKind::Int { negative, digits } => PatKey::Int(*negative, digits.clone()),
            ast::PatternKind::Text(t) => PatKey::Text(t.clone()),
            ast::PatternKind::Bool(b) => PatKey::Bool(*b),
            ast::PatternKind::Binding(n) => PatKey::Bind(self.bind(&n.text)),
            ast::PatternKind::Ctor(c) => {
                let path = if c.path.qualifier.is_empty() {
                    // Resolved against the scrutinee type, which the
                    // identical surrounding tree fixes: the name decides.
                    PKey::Name(c.path.name.text.clone())
                } else {
                    let segs: Vec<&ast::Name> = c
                        .path
                        .qualifier
                        .iter()
                        .chain(std::iter::once(&c.path.name))
                        .collect();
                    self.path_key(&segs)
                };
                let args = c
                    .args
                    .as_ref()
                    .map(|a| a.iter().map(|p| self.pattern(p)).collect());
                PatKey::Ctor(Box::new(path), args)
            }
        }
    }
}

/// The segments of a pure name chain `a.b.c` (`Member(Member(Name a, b),
/// c)`), or `None` when the chain's root is not a name.
pub(crate) fn chain_segments(e: &ast::Expr) -> Option<Vec<&ast::Name>> {
    match &e.kind {
        ast::ExprKind::Name(n) => Some(vec![n]),
        ast::ExprKind::Member(m) => {
            let mut segs = chain_segments(&m.target)?;
            segs.push(&m.member);
            Some(segs)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tarjan_orders_dependencies_first() {
        let a = DefId(0);
        let b = DefId(1);
        let c = DefId(2);
        let mut edges = BTreeMap::new();
        edges.insert(a, [b].into_iter().collect());
        edges.insert(b, [c].into_iter().collect());
        edges.insert(c, BTreeSet::new());
        let sccs = tarjan(&[a, b, c], &edges);
        assert_eq!(sccs, vec![vec![c], vec![b], vec![a]]);
    }

    #[test]
    fn cycles_are_found() {
        let a = DefId(0);
        let b = DefId(1);
        let mut edges = BTreeMap::new();
        edges.insert(a, [b].into_iter().collect());
        edges.insert(b, [a].into_iter().collect());
        let within: BTreeSet<DefId> = [a, b].into_iter().collect();
        assert_eq!(find_cycle(a, &edges, &within), vec![a, b, a]);
        assert_eq!(find_cycle(b, &edges, &within), vec![b, a, b]);
        let mut self_loop = BTreeMap::new();
        self_loop.insert(a, [a].into_iter().collect());
        let only: BTreeSet<DefId> = [a].into_iter().collect();
        assert_eq!(find_cycle(a, &self_loop, &only), vec![a, a]);
    }
}
