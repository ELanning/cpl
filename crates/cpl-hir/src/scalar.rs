//! The scalar subset of CPL that the native engine's scalar tier (`spec/cabi.md` 1.3a) and the hosts' speculative
//! forms compile on unboxed values: its types (`Q`), which functions are eligible (`plan`), and the call graph
//! facts eligibility needs. Shared by `cpl-emit-c` (`fast.rs`) and `cpl-emit-host`; each prints its own code.
//!
//! The scalar types: `Int` and `Bool` (refinements of them included); records, structs without type parameters whose
//! fields are `Int`, `Bool`, records, and (`Caps::texts`) `Text` (a struct with an invariant included, but never built
//! by a literal); and as the backend's `Caps` have them, `Text`, enums with `Int` and `Bool` payloads, `List<E>` for an
//! element type `E` (a leaf, a record, or a list of a leaf), and `Option<E>` of a non-list element (locals and
//! expressions only); and with `Caps::words` (the native engine), a value of any other struct or enum as a borrowed
//! word (`Q::Word`), only read. Eligibility is `local_check` (see its `Check`) closed over calls, and a function with a
//! `decreases` clause must be its own recursive group.

use crate::{Block, DefId, DefKind, Expr, ExprKind, FnBody, Lit, Pattern, PatternKind, Program, Stmt, Ty};
use cpl_syntax::ast::{BinaryOp, UnaryOp};
use num_bigint::BigInt;
use std::collections::{BTreeMap, BTreeSet};

/// The root base of a refinement chain (semantics 3.3; `cpl_eval::types::strip`): the type itself when it is not a
/// refinement, `Ty::Error` for a chain that does not end.
fn strip(p: &Program, ty: &Ty) -> Ty {
    let mut current = ty;
    for _ in 0..256 {
        match current {
            Ty::Refined(_, def) => match p.defs.get(def.0 as usize).and_then(|d| d.kind.as_refinement()) {
                Some(r) => current = &r.base,
                None => return Ty::Error,
            },
            other => return other.clone(),
        }
    }
    Ty::Error
}

/// Which lists a backend's scalar forms hold: none; `List<Int>`, `List<Bool>` and options of them (`Flat`, the hosts);
/// or every element type of `El` (`Full`, the native engine).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lists {
    None,
    Flat,
    Full,
}

/// What a backend's scalar forms hold beyond `Int`, `Bool`, and records: which lists, and whether enums (an enum
/// without type parameters whose payloads are `Int` and `Bool`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Caps {
    pub lists: Lists,
    pub enums: bool,
    /// `Text` values, record fields of `Text` included
    pub texts: bool,
    /// `Q::Word`: a value of any other struct or enum, held as the generic form holds it and only read
    pub words: bool,
}

impl Caps {
    /// The native engine's scalar tier.
    pub const NATIVE: Caps = Caps { lists: Lists::Full, enums: true, texts: true, words: true };
    /// The hosts' speculative forms: the native engine's but words.
    pub const HOSTS: Caps = Caps { lists: Lists::Full, enums: true, texts: true, words: false };
    /// Records and scalars only.
    pub const BASIC: Caps = Caps { lists: Lists::None, enums: false, texts: false, words: false };
    /// Records, scalars, and flat lists.
    pub const FLAT: Caps = Caps { lists: Lists::Flat, enums: false, texts: false, words: false };
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Leaf {
    Int,
    Bool,
}

/// The element type of a list (or the payload of an option): a leaf, a record, or a list of a leaf.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum El {
    Int,
    Bool,
    Rec(DefId),
    List(Leaf),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Q {
    Int,
    Bool,
    Rec(DefId),
    List(El),
    /// never of a list: an element list read out of an option would be a borrowed word with no owner here
    Opt(El),
    /// an enum whose payloads are `Int` and `Bool` (`Caps::enums`)
    Enum(DefId),
    /// `Text` (`Caps::texts`)
    Text,
    /// A value of a struct or enum with no form above (a tree's node, an option of one), held as the generic form
    /// holds it, borrowed from a parameter (`Caps::words`): read by field, matched, passed to a scalar form, never
    /// built, compared, returned, or kept in a list or record. A parameter's value outlives the call, and every
    /// word read out of one is held by it.
    Word,
}

pub fn el_of_leaf(l: Leaf) -> El {
    match l {
        Leaf::Int => El::Int,
        Leaf::Bool => El::Bool,
    }
}

/// The scalar type of a value of element type `e`.
pub fn of_el(e: El) -> Q {
    match e {
        El::Int => Q::Int,
        El::Bool => Q::Bool,
        El::Rec(d) => Q::Rec(d),
        El::List(l) => Q::List(el_of_leaf(l)),
    }
}

pub fn leaf(p: &Program, ty: &Ty) -> Option<Leaf> {
    match strip(p, ty) {
        Ty::Int => Some(Leaf::Int),
        Ty::Bool => Some(Leaf::Bool),
        _ => None,
    }
}


/// The scalar type of `ty`, if it has one; lists and options only as `lists` has them.
pub fn qty(p: &Program, ty: &Ty, caps: Caps) -> Option<Q> {
    let lists = caps.lists;
    let record = |d: DefId, depth: u32| record_q(p, d, depth, caps.texts);
    match strip(p, ty) {
        Ty::Text if caps.texts => return Some(Q::Text),
        Ty::Adt(d, args) if args.is_empty() => {
            if let Some(q) = record(d, 0) {
                return Some(q);
            }
            if caps.enums {
                if let Some(q) = enumeration(p, d) {
                    return Some(q);
                }
            }
        }
        _ => {}
    }
    let el = |t: &Ty| -> Option<El> {
        let e = match strip(p, t) {
            Ty::Int => El::Int,
            Ty::Bool => El::Bool,
            Ty::List(inner) => El::List(leaf(p, &inner)?),
            Ty::Adt(d, args) if args.is_empty() && d != p.std_defs.option => match record(d, 0)? {
                Q::Rec(r) => El::Rec(r),
                _ => return None,
            },
            _ => return None,
        };
        let fits = match lists {
            Lists::Full => true,
            _ => matches!(e, El::Int | El::Bool),
        };
        fits.then_some(e)
    };
    let q = match strip(p, ty) {
        Ty::Int => Some(Q::Int),
        Ty::Bool => Some(Q::Bool),
        Ty::List(e) if lists != Lists::None => el(&e).map(Q::List),
        Ty::Adt(d, args) if lists != Lists::None && d == p.std_defs.option && args.len() == 1 => {
            el(&args[0]).filter(|e| !matches!(e, El::List(_))).map(Q::Opt)
        }
        _ => None,
    };
    if q.is_some() || !caps.words {
        return q;
    }
    match strip(p, ty) {
        Ty::Adt(d, _) if d != p.std_defs.unit && matches!(p.def(d).kind, DefKind::Struct(_) | DefKind::Enum(_)) => {
            Some(Q::Word)
        }
        _ => None,
    }
}

/// `Q::Rec(d)` for a struct `d` without type parameters whose fields are `Int`, `Bool`, records, and with `texts`,
/// `Text` (an invariant allowed: see `Check`, which refuses a literal of such a struct).
fn record_q(p: &Program, d: DefId, depth: u32, texts: bool) -> Option<Q> {
    {
        if depth > 8 {
            return None;
        }
        let DefKind::Struct(s) = &p.defs.get(d.0 as usize)?.kind else {
            return None;
        };
        // a struct with an invariant is a record too: a scalar form never builds one (`Check` refuses its
        // literal), so every value of it is a copy of one the generic form made and checked
        if !s.generics.is_empty() || s.fields.is_empty() {
            return None;
        }
        for f in &s.fields {
            match strip(p, &f.ty) {
                Ty::Int | Ty::Bool => {}
                Ty::Text if texts => {}
                Ty::Adt(n, args) if args.is_empty() => {
                    record_q(p, n, depth + 1, texts)?;
                }
                _ => return None,
            }
        }
        Some(Q::Rec(d))
    }
}

/// `Q::Enum(d)` for an enum `d` without type parameters (not `Unit`) whose every payload is an `Int` or a `Bool`.
fn enumeration(p: &Program, d: DefId) -> Option<Q> {
    let DefKind::Enum(e) = &p.defs.get(d.0 as usize)?.kind else {
        return None;
    };
    if !e.generics.is_empty() || d == p.std_defs.unit || d == p.std_defs.option || e.variants.is_empty() {
        return None;
    }
    let ok = e
        .variants
        .iter()
        .all(|v| v.payload.iter().all(|t| matches!(strip(p, t), Ty::Int | Ty::Bool)));
    ok.then_some(Q::Enum(d))
}

/// The payload types of variant `variant` of the enum `d` (each `Q::Int` or `Q::Bool`).
pub fn variant_fields(p: &Program, d: DefId, variant: u32) -> Vec<Q> {
    match &p.def(d).kind {
        DefKind::Enum(e) => e
            .variants
            .get(variant as usize)
            .map(|v| {
                v.payload
                    .iter()
                    .map(|t| if matches!(strip(p, t), Ty::Bool) { Q::Bool } else { Q::Int })
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

pub fn fields(p: &Program, rec: DefId) -> Vec<Q> {
    match &p.def(rec).kind {
        DefKind::Struct(s) => s
            .fields
            .iter()
            .map(|f| qty(p, &f.ty, Caps::NATIVE).expect("a record's field"))
            .collect(),
        _ => Vec::new(),
    }
}

pub fn small_literal(n: &BigInt) -> Option<i64> {
    i64::try_from(n).ok()
}

pub fn builtin(p: &Program, callee: DefId) -> Option<&str> {
    match &p.defs.get(callee.0 as usize)?.kind.as_fn()?.body {
        FnBody::Builtin(id) => Some(id.as_str()),
        _ => None,
    }
}

/// Whether `e` is `xs.get(i)`, `xs.first()`, or `xs.last()` of a list of lists (its Option has no scalar form; only
/// `unwrap_or` reads it, by `list_element_or`).
pub fn nested_read(p: &Program, e: &Expr) -> bool {
    let ExprKind::Call { callee, args, using, .. } = &e.kind else {
        return false;
    };
    using.is_empty()
        && matches!(
            (builtin(p, *callee), args.len()),
            (Some("list_get"), 2) | (Some("list_first"), 1) | (Some("list_last"), 1)
        )
        && matches!(qty(p, &args[0].ty, Caps::NATIVE), Some(Q::List(El::List(_))))
        && (args.len() == 1 || qty(p, &args[1].ty, Caps::NATIVE) == Some(Q::Int))
}

/// The parts of `value` when it is `t ++ a ++ b ...` on the text local `t` (left-associated, `t` leftmost) and no part
/// holds a block (none can assign `t`): each part appended to `t` in place, after all are evaluated (each sees the
/// old `t`).
pub fn text_append_chain<'e>(p: &Program, local: crate::LocalId, value: &'e Expr) -> Option<Vec<&'e Expr>> {
    if !matches!(strip(p, &value.ty), Ty::Text) {
        return None;
    }
    let mut parts = Vec::new();
    let mut e = value;
    while let ExprKind::Binary { op: BinaryOp::Concat, lhs, rhs } = &e.kind {
        parts.push(&**rhs);
        e = lhs;
    }
    if parts.is_empty() || !matches!(e.kind, ExprKind::Local(l) if l == local) || parts.iter().any(|x| has_block(x)) {
        return None;
    }
    parts.reverse();
    Some(parts)
}

/// The list locals of `fd` (not parameters) that never leave it: assigned only `[]` or their own `push`, read only by
/// `len`, `is_empty`, `get`, `first`, and `last`. A backend may hold each as a flat array of its elements (a C array,
/// a Go slice, a Java array, a Rust `Vec`) instead of a list of its runtime: pushing writes in place, reading copies an
/// element.
pub fn flat_locals(p: &Program, fd: &crate::FnDef, caps: Caps) -> BTreeMap<crate::LocalId, El> {
    let FnBody::Block(body) = &fd.body else {
        return BTreeMap::new();
    };
    let params: BTreeSet<crate::LocalId> = fd.params.iter().map(|x| x.local).collect();
    let mut cands: BTreeMap<crate::LocalId, El> = BTreeMap::new();
    for l in &fd.locals {
        if let Some(Q::List(el @ (El::Int | El::Bool | El::Rec(_)))) = qty(p, &l.ty, caps) {
            if !params.contains(&l.id) && Some(l.id) != fd.result_local {
                cands.insert(l.id, el);
            }
        }
    }
    if cands.is_empty() {
        return cands;
    }
    let is_cand = |e: &Expr| match &e.kind {
        ExprKind::Local(l) if cands.contains_key(l) => Some(*l),
        _ => None,
    };
    // every read of a candidate, and the reads in an allowed position
    let mut total: BTreeMap<crate::LocalId, usize> = BTreeMap::new();
    let mut allowed: BTreeMap<crate::LocalId, usize> = BTreeMap::new();
    let mut note = |e: &Expr| {
        if let Some(l) = is_cand(e) {
            *total.entry(l).or_default() += 1;
        }
        if let ExprKind::Call { callee, args, .. } = &e.kind {
            if matches!(
                builtin(p, *callee),
                Some("list_len" | "list_is_empty" | "list_get" | "list_first" | "list_last")
            ) {
                if let Some(l) = args.first().and_then(is_cand) {
                    *allowed.entry(l).or_default() += 1;
                }
            }
        }
    };
    for c in fd.requires.iter().chain(&fd.ensures).chain(&fd.decreases) {
        c.expr.walk(&mut note);
    }
    body.walk(&mut note);
    // every write: `[]`, or the local's own push (whose receiver is an allowed read)
    let mut bad: BTreeSet<crate::LocalId> = BTreeSet::new();
    for top in &body.stmts {
        top.walk_stmts(&mut |st| {
            if let Stmt::Let { local, value } | Stmt::Assign { local, value } = st {
                if !cands.contains_key(local) {
                    return;
                }
                let ok = match &value.kind {
                    ExprKind::List(items) => items.is_empty(),
                    ExprKind::Call { callee, args, .. } => {
                        matches!(st, Stmt::Assign { .. })
                            && builtin(p, *callee) == Some("list_push")
                            && args.len() == 2
                            && matches!(args[0].kind, ExprKind::Local(l) if l == *local)
                    }
                    _ => false,
                };
                if ok {
                    if matches!(value.kind, ExprKind::Call { .. }) {
                        *allowed.entry(*local).or_default() += 1;
                    }
                } else {
                    bad.insert(*local);
                }
            }
        });
    }
    cands.retain(|l, _| !bad.contains(l) && total.get(l).copied().unwrap_or(0) == allowed.get(l).copied().unwrap_or(0));
    cands
}

/// A list local held flat while it is built, then as a list: `flat_locals`' rules hold for the top-level statements of
/// the body before `at` (the body's value counting as statement `stmts.len()`), its `let` among them, and from `at` on
/// it is never written. A backend builds it as a flat array and makes the list once, before statement `at`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Built {
    pub el: El,
    pub at: usize,
}

/// The list locals of `fd` built flat and then frozen (`Built`); none of them is in `flat_locals`.
pub fn built_locals(p: &Program, fd: &crate::FnDef, caps: Caps) -> BTreeMap<crate::LocalId, Built> {
    let mut out = BTreeMap::new();
    let FnBody::Block(body) = &fd.body else {
        return out;
    };
    let flat = flat_locals(p, fd, caps);
    let params: BTreeSet<crate::LocalId> = fd.params.iter().map(|x| x.local).collect();
    // the top-level statements, the body's value as the last
    let tail = body.tail.as_ref().map(|t| Stmt::Expr((**t).clone()));
    let stmts: Vec<&Stmt> = body.stmts.iter().chain(tail.as_ref()).collect();
    for l in &fd.locals {
        let el = match qty(p, &l.ty, caps) {
            Some(Q::List(el @ (El::Int | El::Bool | El::Rec(_)))) => el,
            _ => continue,
        };
        let id = l.id;
        if params.contains(&id) || Some(id) == fd.result_local || flat.contains_key(&id) {
            continue;
        }
        let mut in_contract = false;
        for c in fd.requires.iter().chain(&fd.ensures).chain(&fd.decreases) {
            c.expr.walk(&mut |e| in_contract |= matches!(e.kind, ExprKind::Local(x) if x == id));
        }
        let Some(decl) = body.stmts.iter().position(|st| matches!(st, Stmt::Let { local, .. } if *local == id)) else {
            continue;
        };
        if in_contract {
            continue;
        }
        let shapes: Vec<(bool, bool)> = stmts.iter().map(|st| shape(p, st, id)).collect();
        let Some(at) = shapes.iter().position(|(flat_ok, _)| !flat_ok) else {
            continue;
        };
        if decl < at && shapes[at..].iter().all(|(_, writes)| !writes) {
            out.insert(id, Built { el, at });
        }
    }
    out
}

/// For local `l` in the statement `st`: whether `flat_locals`' rules hold in it, and whether it writes `l`.
fn shape(p: &Program, st: &Stmt, l: crate::LocalId) -> (bool, bool) {
    let is_l = |e: &Expr| matches!(e.kind, ExprKind::Local(x) if x == l);
    let (mut total, mut allowed) = (0usize, 0usize);
    st.walk(&mut |e| {
        if is_l(e) {
            total += 1;
        }
        if let ExprKind::Call { callee, args, .. } = &e.kind {
            if matches!(
                builtin(p, *callee),
                Some("list_len" | "list_is_empty" | "list_get" | "list_first" | "list_last")
            ) && args.first().is_some_and(is_l)
            {
                allowed += 1;
            }
        }
    });
    let (mut writes, mut bad) = (false, false);
    st.walk_stmts(&mut |x| {
        if let Stmt::Let { local, value } | Stmt::Assign { local, value } = x {
            if *local != l {
                return;
            }
            writes = true;
            match &value.kind {
                ExprKind::List(items) if items.is_empty() => {}
                ExprKind::Call { callee, args, .. }
                    if matches!(x, Stmt::Assign { .. })
                        && builtin(p, *callee) == Some("list_push")
                        && args.len() == 2
                        && is_l(&args[0]) =>
                {
                    allowed += 1;
                }
                _ => bad = true,
            }
        }
    });
    (!bad && total == allowed, writes)
}

/// Whether evaluating `e` can neither fault nor bail nor allocate: literals, locals, and records, enums, and options
/// of them (so that evaluating it lazily, or not at all, is unobservable).
pub fn cannot_fault(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Lit(Lit::Int(n)) => small_literal(n).is_some(),
        ExprKind::Lit(Lit::Bool(_)) | ExprKind::Lit(Lit::Text(_)) | ExprKind::Local(_) => true,
        ExprKind::Widen { expr, .. } => cannot_fault(expr),
        ExprKind::Field { base, .. } => cannot_fault(base),
        ExprKind::Ctor { args, .. } => args.iter().all(cannot_fault),
        ExprKind::StructLit { spread, fields, .. } => {
            spread.as_ref().is_none_or(|b| cannot_fault(b)) && fields.iter().all(|(_, f)| cannot_fault(f))
        }
        _ => false,
    }
}

/// Whether evaluating `e` can assign a local (it holds a block: only `if` and `match` do).
pub fn has_block(e: &Expr) -> bool {
    let mut found = false;
    e.walk(&mut |x| {
        if matches!(x.kind, ExprKind::If { .. } | ExprKind::Match { .. }) {
            found = true;
        }
    });
    found
}

fn settle(fns: &mut BTreeMap<DefId, BTreeSet<DefId>>) {
    loop {
        let gone: Vec<DefId> = fns
            .iter()
            .filter(|(_, cs)| cs.iter().any(|c| !fns.contains_key(c)))
            .map(|(d, _)| *d)
            .collect();
        if gone.is_empty() {
            return;
        }
        for d in gone {
            fns.remove(&d);
        }
    }
}

pub fn add_record(p: &Program, q: Q, out: &mut BTreeSet<DefId>) {
    let r = match q {
        Q::Rec(r) | Q::List(El::Rec(r)) | Q::Opt(El::Rec(r)) => r,
        _ => return,
    };
    if out.insert(r) {
        for f in fields(p, r) {
            add_record(p, f, out);
        }
    }
}

/// The eligible functions (each with whether it carries a measure) and every record they use.
#[derive(Default, Clone)]
pub struct Plan {
    pub fns: BTreeMap<DefId, bool>,
    pub records: BTreeSet<DefId>,
    /// every enum a scalar form uses (`Caps::enums`)
    pub enums: BTreeSet<DefId>,
}

/// The eligible functions of `p`; lists and options only as `lists` has them.
pub fn plan(p: &Program, caps: Caps) -> Plan {
    // the locally eligible functions and the calls each makes
    let mut calls: BTreeMap<DefId, BTreeSet<DefId>> = BTreeMap::new();
    for d in p.defs() {
        if let Some(callees) = local_check(p, d.id, caps) {
            calls.insert(d.id, callees);
        }
    }
    // a function stays only while everything it calls stays
    settle(&mut calls);
    // a measured function must be its own recursive group (among all functions: a generic caller in between would
    // hold the activation its measure is compared with)
    let groups = sccs(&call_graph(p));
    calls.retain(|d, _| {
        let measured = p.def(*d).kind.as_fn().is_some_and(|f| f.decreases.is_some());
        !(measured && groups.get(d).is_some_and(|g| g.len() > 1))
    });
    settle(&mut calls);
    let mut plan = Plan::default();
    for d in calls.keys() {
        let fd = p.def(*d).kind.as_fn().expect("a function");
        plan.fns.insert(*d, fd.decreases.is_some());
        let mut types = Vec::new();
        types.push(fd.ret.clone());
        types.extend(fd.locals.iter().map(|l| l.ty.clone()));
        // a type may appear only in an expression (a record literal read at once, a constructor matched at once)
        let mut note = |e: &Expr| types.push(e.ty.clone());
        for c in fd.requires.iter().chain(&fd.ensures).chain(&fd.decreases) {
            c.expr.walk(&mut note);
        }
        if let FnBody::Block(b) = &fd.body {
            b.walk(&mut note);
        }
        for ty in &types {
            match qty(p, ty, caps) {
                Some(Q::Enum(d)) => {
                    plan.enums.insert(d);
                }
                Some(q) => add_record(p, q, &mut plan.records),
                None => {}
            }
        }
    }
    plan
}

/// Whether `d` is eligible but for its callees (which it returns).
fn local_check(p: &Program, d: DefId, caps: Caps) -> Option<BTreeSet<DefId>> {
    let fd = p.def(d).kind.as_fn()?;
    let FnBody::Block(body) = &fd.body else {
        return None;
    };
    if !fd.generics.is_empty() || !fd.using.is_empty() {
        return None;
    }
    // an option is never a parameter or a result (the wrapper neither unboxes nor boxes one); a word is a parameter
    // (the caller's value, borrowed), never a result
    let boundary = |ty: &Ty| matches!(qty(p, ty, caps), Some(Q::Int | Q::Bool | Q::Rec(_) | Q::List(_) | Q::Enum(_) | Q::Text));
    let param = |ty: &Ty| boundary(ty) || qty(p, ty, caps) == Some(Q::Word);
    if !boundary(&fd.ret) || !fd.params.iter().all(|x| param(&x.ty)) {
        return None;
    }
    for l in &fd.locals {
        qty(p, &l.ty, caps)?;
    }
    let mut c = Check {
        p,
        caps,
        ok: true,
        callees: BTreeSet::new(),
    };
    for clause in fd.requires.iter().chain(&fd.ensures).chain(&fd.decreases) {
        c.expr(&clause.expr);
    }
    c.block(body);
    c.ok.then_some(c.callees)
}

struct Check<'a> {
    p: &'a Program,
    caps: Caps,
    ok: bool,
    callees: BTreeSet<DefId>,
}

impl Check<'_> {
    fn block(&mut self, b: &Block) {
        for s in &b.stmts {
            self.stmt(s);
        }
        if let Some(e) = &b.tail {
            self.expr(e);
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let { value, .. } | Stmt::Assign { value, .. } => self.expr(value),
            Stmt::Discard(e) | Stmt::Expr(e) | Stmt::Return(Some(e)) => self.expr(e),
            Stmt::Assert { expr, .. } => self.expr(expr),
            Stmt::While {
                cond,
                invariants,
                decreases,
                body,
            } => {
                if !invariants.is_empty() || decreases.is_some() {
                    self.ok = false;
                }
                self.expr(cond);
                self.block(body);
            }
            Stmt::Return(None) | Stmt::Concurrent(_) => self.ok = false,
        }
    }

    fn q(&self, ty: &Ty) -> Option<Q> {
        qty(self.p, ty, self.caps)
    }

    fn pattern_ok(&self, pat: &Pattern, scrutinee: Option<Q>) -> bool {
        let leafy = |k: &PatternKind| match k {
            PatternKind::Wildcard | PatternKind::Bind(_) | PatternKind::Bool(_) => true,
            PatternKind::Int(n) => small_literal(n).is_some(),
            _ => false,
        };
        match scrutinee {
            Some(Q::Int | Q::Bool) => leafy(&pat.kind),
            Some(Q::Text) => matches!(pat.kind, PatternKind::Wildcard | PatternKind::Bind(_) | PatternKind::Text(_)),
            Some(Q::Enum(d)) => match &pat.kind {
                PatternKind::Wildcard | PatternKind::Bind(_) => true,
                PatternKind::Ctor { def, args, .. } => *def == d && args.iter().all(|a| leafy(&a.kind)),
                _ => false,
            },
            // a word: bound whole, or a variant tested with its payloads bound (each an Int, a Bool, or a word)
            Some(Q::Word) => match &pat.kind {
                PatternKind::Wildcard | PatternKind::Bind(_) => true,
                PatternKind::Ctor { args, .. } => args.iter().all(|a| match a.kind {
                    PatternKind::Wildcard => true,
                    PatternKind::Bind(_) => matches!(self.q(&a.ty), Some(Q::Int | Q::Bool | Q::Word)),
                    _ => false,
                }),
                _ => false,
            },
            Some(Q::Opt(el)) => match &pat.kind {
                PatternKind::Wildcard => true,
                PatternKind::Ctor { def, args, .. } if *def == self.p.std_defs.option => args.iter().all(|a| match el {
                    // a record payload is bound or ignored, never compared
                    El::Rec(_) => matches!(a.kind, PatternKind::Wildcard | PatternKind::Bind(_)),
                    _ => leafy(&a.kind),
                }),
                _ => false,
            },
            _ => false,
        }
    }

    fn expr(&mut self, e: &Expr) {
        if !self.ok {
            return;
        }
        let unit = matches!(&e.ty, Ty::Adt(d, args) if *d == self.p.std_defs.unit && args.is_empty());
        let q = self.q(&e.ty);
        if q.is_none() && !unit {
            self.ok = false;
            return;
        }
        // a word is only ever borrowed: a local, a field read, or an `if` or `match` of those (a builtin's option of a
        // list, say, would be a value the scalar form has no word for)
        if q == Some(Q::Word)
            && !matches!(
                e.kind,
                ExprKind::Local(_) | ExprKind::Field { .. } | ExprKind::Widen { .. } | ExprKind::If { .. } | ExprKind::Match { .. }
            )
        {
            self.ok = false;
            return;
        }
        match &e.kind {
            ExprKind::Lit(Lit::Int(n)) => {
                if small_literal(n).is_none() {
                    self.ok = false;
                }
            }
            ExprKind::Lit(Lit::Bool(_)) | ExprKind::Local(_) => {}
            ExprKind::Lit(Lit::Text(_)) if q == Some(Q::Text) => {}
            ExprKind::Ctor { def, args, .. } if *def == self.p.std_defs.unit && args.is_empty() => {}
            // (never a word's: a scalar form builds none)
            ExprKind::Ctor { def, args, .. }
                if q != Some(Q::Word) && (*def == self.p.std_defs.option || q == Some(Q::Enum(*def))) =>
            {
                for a in args {
                    self.expr(a);
                }
            }
            ExprKind::List(items) => {
                for a in items {
                    self.expr(a);
                }
            }
            ExprKind::Widen { expr, .. } => self.expr(expr),
            ExprKind::Field { base, .. } => {
                // a record's field, or an Int, a Bool, or a word read out of a struct held as a word
                let of_word = self.q(&base.ty) == Some(Q::Word)
                    && matches!(strip(self.p, &base.ty), Ty::Adt(d, _) if matches!(self.p.def(d).kind, DefKind::Struct(_)))
                    && matches!(q, Some(Q::Int | Q::Bool | Q::Word));
                if !matches!(self.q(&base.ty), Some(Q::Rec(_))) && !of_word {
                    self.ok = false;
                    return;
                }
                self.expr(base);
            }
            ExprKind::StructLit {
                def,
                spread,
                fields,
                ..
            } => {
                // no literal of a struct with an invariant (its value would need the invariant's check)
                let checked = matches!(&self.p.def(*def).kind, DefKind::Struct(s) if s.invariant.is_some());
                if q != Some(Q::Rec(*def)) || checked {
                    self.ok = false;
                    return;
                }
                if let Some(b) = spread {
                    self.expr(b);
                }
                for (_, f) in fields {
                    self.expr(f);
                }
            }
            ExprKind::Unary { op, operand } => {
                let want = match op {
                    UnaryOp::Not => Q::Bool,
                    UnaryOp::Neg => Q::Int,
                };
                if self.q(&operand.ty) != Some(want) {
                    self.ok = false;
                    return;
                }
                self.expr(operand);
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let (l, r) = (self.q(&lhs.ty), self.q(&rhs.ty));
                let fits = match op {
                    BinaryOp::Concat => matches!(l, Some(Q::List(_) | Q::Text)) && l == r,
                    BinaryOp::And | BinaryOp::Or => l == Some(Q::Bool) && r == Some(Q::Bool),
                    BinaryOp::Eq | BinaryOp::Ne => {
                        matches!(l, Some(Q::Int | Q::Bool | Q::Rec(_) | Q::Enum(_) | Q::Text)) && l == r
                    }
                    _ => l == Some(Q::Int) && r == Some(Q::Int),
                };
                if !fits {
                    self.ok = false;
                    return;
                }
                self.expr(lhs);
                self.expr(rhs);
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
                let sq = self.q(&scrutinee.ty);
                if !arms.iter().all(|a| self.pattern_ok(&a.pattern, sq)) {
                    self.ok = false;
                    return;
                }
                self.expr(scrutinee);
                for arm in arms {
                    self.block(&arm.body);
                }
            }
            ExprKind::Call {
                callee,
                generic_args,
                args,
                using,
            } => {
                if !using.is_empty() {
                    self.ok = false;
                    return;
                }
                if let Some(which) = builtin(self.p, *callee) {
                    let receiver = args.first().and_then(|a| self.q(&a.ty));
                    let fits = match which {
                        "list_len" | "list_is_empty" | "list_first" | "list_last" | "list_drop_last" => {
                            args.len() == 1 && matches!(receiver, Some(Q::List(_)))
                        }
                        "list_get" | "list_take" | "list_drop" => {
                            args.len() == 2
                                && matches!(receiver, Some(Q::List(_)))
                                && self.q(&args[1].ty) == Some(Q::Int)
                        }
                        "list_push" => {
                            args.len() == 2
                                && matches!(receiver, Some(Q::List(l)) if self.q(&args[1].ty) == Some(of_el(l)))
                        }
                        "text_len" | "text_is_empty" => args.len() == 1 && receiver == Some(Q::Text),
                        "int_to_text" => args.len() == 1 && receiver == Some(Q::Int) && q == Some(Q::Text),
                        "option_is_some" | "option_is_none" => {
                            args.len() == 1 && matches!(receiver, Some(Q::Opt(_) | Q::Word))
                        }
                        "option_unwrap_or" if args.len() == 2 && self.caps.lists == Lists::Full && nested_read(self.p, &args[0]) => {
                            // an element list of a list of lists, or the default: the read's own operands are
                            // checked, not its Option type (which has no scalar form)
                            let ExprKind::Call { args: inner, .. } = &args[0].kind else {
                                self.ok = false;
                                return;
                            };
                            let want = match self.q(&inner[0].ty) {
                                Some(Q::List(El::List(l))) => Some(Q::List(el_of_leaf(l))),
                                _ => None,
                            };
                            if want.is_none() || self.q(&args[1].ty) != want {
                                self.ok = false;
                                return;
                            }
                            for a in inner {
                                self.expr(a);
                            }
                            self.expr(&args[1]);
                            return;
                        }
                        "option_unwrap_or" => {
                            args.len() == 2
                                && matches!(receiver, Some(Q::Opt(l)) if self.q(&args[1].ty) == Some(of_el(l)))
                        }
                        _ => false,
                    };
                    if !fits {
                        self.ok = false;
                        return;
                    }
                    for a in args {
                        self.expr(a);
                    }
                    return;
                }
                if !generic_args.is_empty() {
                    self.ok = false;
                    return;
                }
                match self.p.defs.get(callee.0 as usize).and_then(|d| d.kind.as_fn()) {
                    Some(f) if matches!(f.body, FnBody::Block(_)) && f.params.len() == args.len() => {}
                    _ => {
                        self.ok = false;
                        return;
                    }
                }
                self.callees.insert(*callee);
                for a in args {
                    self.expr(a);
                }
            }
            _ => self.ok = false,
        }
    }
}

/// Every call edge between functions (bodies and contracts).
fn call_graph(p: &Program) -> BTreeMap<DefId, BTreeSet<DefId>> {
    let mut g = BTreeMap::new();
    for d in p.defs() {
        let mut out = BTreeSet::new();
        let mut note = |e: &Expr| {
            if let ExprKind::Call { callee, .. } = &e.kind {
                out.insert(*callee);
            }
        };
        if let Some(fd) = d.kind.as_fn() {
            for c in fd.requires.iter().chain(&fd.ensures).chain(&fd.decreases) {
                c.expr.walk(&mut note);
            }
            if let FnBody::Block(b) = &fd.body {
                b.walk(&mut note);
            }
        }
        g.insert(d.id, out);
    }
    g
}

/// The strongly connected component of every node, as its member set.
fn sccs(g: &BTreeMap<DefId, BTreeSet<DefId>>) -> BTreeMap<DefId, BTreeSet<DefId>> {
    let nodes: Vec<DefId> = g.keys().copied().collect();
    let index: BTreeMap<DefId, usize> = nodes.iter().enumerate().map(|(i, d)| (*d, i)).collect();
    let edges: Vec<Vec<usize>> = nodes
        .iter()
        .map(|d| g[d].iter().filter_map(|c| index.get(c).copied()).collect())
        .collect();
    const NONE: usize = usize::MAX;
    let n = nodes.len();
    let (mut idx, mut low, mut on) = (vec![NONE; n], vec![0; n], vec![false; n]);
    let (mut stack, mut next) = (Vec::new(), 0usize);
    let mut out = BTreeMap::new();
    for root in 0..n {
        if idx[root] != NONE {
            continue;
        }
        let mut work: Vec<(usize, usize)> = vec![(root, 0)];
        while let Some(&mut (u, ref mut at)) = work.last_mut() {
            if *at == 0 && idx[u] == NONE {
                idx[u] = next;
                low[u] = next;
                next += 1;
                stack.push(u);
                on[u] = true;
            }
            if let Some(&v) = edges[u].get(*at) {
                *at += 1;
                if idx[v] == NONE {
                    work.push((v, 0));
                } else if on[v] {
                    low[u] = low[u].min(idx[v]);
                }
                continue;
            }
            work.pop();
            if let Some(&(parent, _)) = work.last() {
                low[parent] = low[parent].min(low[u]);
            }
            if low[u] == idx[u] {
                let mut members = BTreeSet::new();
                while let Some(w) = stack.pop() {
                    on[w] = false;
                    members.insert(nodes[w]);
                    if w == u {
                        break;
                    }
                }
                for m in &members {
                    out.insert(*m, members.clone());
                }
            }
        }
    }
    out
}
