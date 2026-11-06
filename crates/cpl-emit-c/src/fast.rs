//! The scalar tier (`spec/cabi.md` 1.3a): a second, speculative C form `cpl_q<n>` of every pure function whose values
//! are all scalars, on `int64_t`, `int`, C structs, and list words instead of tagged slots: no slot discipline, no
//! frame, no `Option` or struct allocation. It bails (returns `ok` 0) wherever the generic form could do anything but
//! compute the same value: an overflow of `int64_t`, a division by zero, a contract, assertion, or measure that does
//! not hold, a negative count, an element that is a bigint, an allocation that fails, the depth limit, the stack floor,
//! a deadline, a cancellation. The generic form `cpl_f<n>` tries it first and, on a bail, runs itself, which then
//! raises exactly what it always did: the function is pure, so running it again computes the same thing.
//!
//! The scalar types: `Int` and `Bool` (refinements of them included); records, structs without type parameters or an
//! invariant whose fields are `Int`, `Bool`, or records; `List<Int>` and `List<Bool>`, as the runtime's list words;
//! `Option<Int>` and `Option<Bool>`, as `cplrt_qopt` (locals and expressions only, never a parameter or a result).
//!
//! Eligible: a function with a block body, no type parameters, no `using`, whose parameters, result, and locals have
//! scalar types, whose body and contracts use only literals, locals, arithmetic, comparisons, `not`/`and`/`or`, `if`,
//! `match` on an `Int`, a `Bool`, or an `Option` with literal, binding, `Some`, and `None` patterns, `while` without
//! clauses, `let`, assignment, `return`, `assert`, widening, struct literals and field reads of records, list literals,
//! `++` of lists, `Some`/`None`, the list builtins `len`, `is_empty`, `get`, `first`, `last`, `push`, `take`, `drop`,
//! `drop_last`, the option builtins `unwrap_or`, `is_some`, `is_none`, and calls of eligible functions; a function with
//! a `decreases` clause must also be its own recursive group (no other function reaches it and is reached by it), so
//! that the nearest activation of it a speculative call compares its measure with is its caller or none.
//!
//! Ownership: every list held by a local or a temporary of a scalar form is a reference of its own (released at its
//! return and at its bail), a parameter's is the caller's, and a result is the caller's to keep.

use crate::tables::Tables;
use cpl_hir::scalar::{builtin, fields, has_block, nested_read, small_literal, text_append_chain, variant_fields, El, Q};

/// The scalar type of `ty` (lists included: the C tier has them).
fn qty(p: &Program, ty: &Ty) -> Option<Q> {
    cpl_hir::scalar::qty(p, ty, cpl_hir::scalar::Caps::NATIVE)
}
use cpl_hir::{Block, DefId, Expr, ExprKind, FnBody, Lit, LocalId, Pattern, PatternKind, Program, Stmt, Ty};
use cpl_syntax::ast::{BinaryOp, UnaryOp};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

fn cty(q: Q) -> String {
    match q {
        Q::Int => "int64_t".to_string(),
        Q::Bool => "int".to_string(),
        Q::Rec(d) => format!("cpl_r{}", d.0),
        Q::List(_) => "cplrt_w".to_string(),
        Q::Opt(El::Rec(d)) => format!("cpl_qo{}", d.0),
        Q::Opt(_) => "cplrt_qopt".to_string(),
        Q::Enum(d) => format!("cpl_v{}", d.0),
        Q::Text => "cplrt_w".to_string(),
        // a borrowed word: `cplrt_w` by another name, so that no temporary of it is released (see `emit`)
        Q::Word => "uint64_t".to_string(),
    }
}

/// The C type of a flat array's elements of `el`.
fn flat_cty(el: El) -> String {
    match el {
        El::Bool => "int".to_string(),
        El::Rec(d) => format!("cpl_r{}", d.0),
        _ => "int64_t".to_string(),
    }
}

/// Whether values of the record `d` hold texts, at any depth: each record variable then owns references to them
/// (`cpl_ret<n>` takes them, `cpl_rel<n>` gives them back).
fn texty(p: &Program, d: DefId) -> bool {
    fields(p, d).iter().any(|f| match f {
        Q::Text => true,
        Q::Rec(n) => texty(p, *n),
        _ => false,
    })
}

/// Whether a C variable of type `q` owns a reference: a list or a text (released with `cplrt_release`).
fn word_owned(q: Q) -> bool {
    matches!(q, Q::List(_) | Q::Text)
}

/// The C type a scalar form returns.
fn ret_cty(q: Q) -> String {
    match q {
        Q::Rec(d) => format!("cpl_qr{}", d.0),
        Q::Enum(d) => format!("cpl_qv{}", d.0),
        Q::List(_) | Q::Text => "cplrt_qw".to_string(),
        _ => "cplrt_qi".to_string(),
    }
}

fn zero(q: Q) -> String {
    match q {
        Q::Rec(r) => format!("cpl_z{}", r.0),
        Q::Enum(r) => format!("cpl_zv{}", r.0),
        Q::Opt(El::Rec(r)) => format!("cpl_zo{}", r.0),
        Q::Opt(_) => "{ 0, 0 }".to_string(),
        _ => "0".to_string(),
    }
}

/// `a == b` for two C values of scalar type `q` (never a list or an option: see `Check`).
fn equal(p: &Program, q: Q, a: &str, b: &str) -> String {
    match q {
        Q::Rec(d) => {
            let parts: Vec<String> = fields(p, d)
                .into_iter()
                .enumerate()
                .map(|(i, f)| equal(p, f, &format!("{a}.f{i}"), &format!("{b}.f{i}")))
                .collect();
            format!("({})", parts.join(" && "))
        }
        Q::Text => format!("cplrt_text_eq({a}, {b})"),
        // every member: the ones of other variants are 0 in every value (constructed and unboxed from zero)
        Q::Enum(d) => {
            let mut parts = vec![format!("{a}.tag == {b}.tag")];
            for m in enum_members(p, d) {
                parts.push(format!("{a}.{m} == {b}.{m}"));
            }
            format!("({})", parts.join(" && "))
        }
        _ => format!("({a} == {b})"),
    }
}

/// The C members of the enum `d`'s struct: `v<variant>_<i>` for each payload field.
fn enum_members(p: &Program, d: DefId) -> Vec<String> {
    let n = p.def(d).kind.as_enum().map(|e| e.variants.len()).unwrap_or(0);
    let mut out = Vec::new();
    for v in 0..n as u32 {
        for i in 0..variant_fields(p, d, v).len() {
            out.push(format!("v{v}_{i}"));
        }
    }
    out
}

/// The C type of the enum `d` of the scalar tier, its zero values, and its box and unbox helpers.
fn enum_type(p: &Program, d: DefId) -> String {
    let n = d.0;
    let nv = p.def(d).kind.as_enum().map(|e| e.variants.len()).unwrap_or(0) as u32;
    let mut s = String::new();
    let _ = writeln!(s, "/* {}: an enum of the scalar tier */", crate::print::comment_text(&p.display_path(d)));
    let mut members = String::from(" uint32_t tag;");
    let mut most = 1usize;
    for v in 0..nv {
        let fs = variant_fields(p, d, v);
        most = most.max(fs.len());
        for (i, f) in fs.iter().enumerate() {
            let _ = write!(members, " {} v{v}_{i};", cty(*f));
        }
    }
    let _ = writeln!(s, "typedef struct {{{members} }} cpl_v{n};");
    let _ = writeln!(s, "typedef struct {{ cpl_v{n} v; int ok; }} cpl_qv{n};");
    let _ = writeln!(s, "static const cpl_v{n} cpl_zv{n};");
    let _ = writeln!(s, "static const cpl_qv{n} cpl_zqv{n};");
    // unbox: 1 when every Int of the payload is small; the members of other variants stay 0
    let _ = writeln!(s, "static int cpl_unboxv{n}(cplrt_w w, cpl_v{n} *o)\n{{\n    *o = cpl_zv{n};\n    o->tag = cplrt_variant(w);\n    switch (o->tag) {{");
    for v in 0..nv {
        let fs = variant_fields(p, d, v);
        if fs.is_empty() {
            continue;
        }
        let _ = writeln!(s, "    case {v}u:");
        for (i, f) in fs.iter().enumerate() {
            let w = format!("cplrt_payload(w, {i}u)");
            if *f == Q::Bool {
                let _ = writeln!(s, "        o->v{v}_{i} = cplrt_truth({w});");
            } else {
                let _ = writeln!(s, "        if (!cplrt_is_small({w}))\n            return 0;\n        o->v{v}_{i} = cplrt_small_value({w});");
            }
        }
        s.push_str("        break;\n");
    }
    s.push_str("    default:\n        break;\n    }\n    return 1;\n}\n");
    // box: CPLRT_OK, or CPLRT_NOMEM with nothing raised and *out unchanged
    let _ = writeln!(
        s,
        "static int cpl_boxv{n}(cplrt_cx *cx, const cpl_v{n} *v, cplrt_w *out)\n{{\n    cplrt_w f[{most}] = {{ 0 }};\n    uint32_t k = 0;\n    int st = CPLRT_OK;\n    switch (v->tag) {{"
    );
    for v in 0..nv {
        let fs = variant_fields(p, d, v);
        if fs.is_empty() {
            continue;
        }
        let _ = writeln!(s, "    case {v}u:");
        for (i, f) in fs.iter().enumerate() {
            if *f == Q::Bool {
                let _ = writeln!(s, "        f[{i}] = cplrt_bool(v->v{v}_{i} != 0);");
            } else {
                let _ = writeln!(s, "        if (st == CPLRT_OK)\n            st = cplrt_q_box(cx, v->v{v}_{i}, &f[{i}]);");
            }
        }
        let _ = writeln!(s, "        k = {}u;\n        break;", fs.len());
    }
    let _ = writeln!(
        s,
        "    default:\n        break;\n    }}\n    if (st == CPLRT_OK)\n        st = cplrt_q_enum(cx, {n}u, v->tag, f, k, out);\n    for (uint32_t i = 0; i < k; i++)\n        cplrt_release(f[i]);\n    return st;\n}}"
    );
    s
}

/// The functions with a scalar form, and the records they use.
#[derive(Default)]
pub(crate) struct Plan {
    /// Each eligible function, and whether it carries a measure (a `decreases` clause).
    fns: BTreeMap<DefId, bool>,
    /// Every record a scalar form uses, nested records included.
    records: BTreeSet<DefId>,
    /// Every enum a scalar form uses.
    enums: BTreeSet<DefId>,
    /// The program-unit constant (`CPL_K`) of each text literal a scalar form reads.
    pub(crate) texts: BTreeMap<String, u32>,
}

impl Plan {
    /// The eligible functions of `p` (none unless `enabled`).
    pub(crate) fn new(p: &Program, enabled: bool) -> Plan {
        if !enabled {
            return Plan::default();
        }
        let sp = cpl_hir::scalar::plan(p, cpl_hir::scalar::Caps::NATIVE);
        Plan {
            fns: sp.fns,
            records: sp.records,
            enums: sp.enums,
            texts: BTreeMap::new(),
        }
    }

    /// Every text literal of the scalar forms (expressions and patterns), for its program-unit constant.
    pub(crate) fn text_literals(&self, p: &Program) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut note = |e: &Expr| match &e.kind {
            ExprKind::Lit(Lit::Text(t)) => {
                out.insert(t.clone());
            }
            ExprKind::Match { arms, .. } => {
                for a in arms {
                    if let PatternKind::Text(t) = &a.pattern.kind {
                        out.insert(t.clone());
                    }
                }
            }
            _ => {}
        };
        for d in self.fns.keys() {
            let fd = p.def(*d).kind.as_fn().expect("a function");
            for c in fd.requires.iter().chain(&fd.ensures).chain(&fd.decreases) {
                c.expr.walk(&mut note);
            }
            if let FnBody::Block(b) = &fd.body {
                b.walk(&mut note);
            }
        }
        out
    }

    /// `CPL_K<k>` of a text literal registered by `text_literals`.
    fn text_k(&self, t: &str) -> String {
        format!("CPL_K{}", self.texts[t])
    }

    pub(crate) fn has(&self, d: DefId) -> bool {
        self.fns.contains_key(&d)
    }

    /// The C types of the records, nested ones first, with their zero values and their box and unbox helpers.
    pub(crate) fn types(&self, p: &Program) -> String {
        fn visit(p: &Program, r: DefId, seen: &mut BTreeSet<DefId>, order: &mut Vec<DefId>) {
            if !seen.insert(r) {
                return;
            }
            for f in fields(p, r) {
                if let Q::Rec(n) = f {
                    visit(p, n, seen, order);
                }
            }
            order.push(r);
        }
        let mut order: Vec<DefId> = Vec::new();
        let mut seen = BTreeSet::new();
        for r in &self.records {
            visit(p, *r, &mut seen, &mut order);
        }
        let mut s = String::new();
        for r in &order {
            let fs = fields(p, *r);
            let n = r.0;
            let _ = writeln!(
                s,
                "/* {}: a record of the scalar tier */",
                crate::print::comment_text(&p.display_path(*r))
            );
            let members: String = fs
                .iter()
                .enumerate()
                .map(|(i, f)| format!(" {} f{i};", cty(*f)))
                .collect();
            let _ = writeln!(s, "typedef struct {{{members} }} cpl_r{n};");
            let _ = writeln!(s, "typedef struct {{ cpl_r{n} v; int ok; }} cpl_qr{n};");
            let _ = writeln!(s, "static const cpl_r{n} cpl_z{n};");
            let _ = writeln!(s, "static const cpl_qr{n} cpl_zq{n};");
            let _ = writeln!(s, "typedef struct {{ cpl_r{n} v; int has; }} cpl_qo{n};");
            let _ = writeln!(s, "static const cpl_qo{n} cpl_zo{n};");
            if texty(p, *r) {
                // the references of a record variable to its texts: taken on a copy, given back on a release
                let _ = writeln!(s, "static inline void cpl_ret{n}(const cpl_r{n} *v)\n{{");
                for (i, f) in fs.iter().enumerate() {
                    match f {
                        Q::Text => {
                            let _ = writeln!(s, "    cplrt_retain(v->f{i});");
                        }
                        Q::Rec(m) if texty(p, *m) => {
                            let _ = writeln!(s, "    cpl_ret{}(&v->f{i});", m.0);
                        }
                        _ => {}
                    }
                }
                s.push_str("}\n");
                let _ = writeln!(s, "static inline void cpl_rel{n}(cpl_r{n} *v)\n{{");
                for (i, f) in fs.iter().enumerate() {
                    match f {
                        Q::Text => {
                            let _ = writeln!(s, "    cplrt_release(v->f{i});\n    v->f{i} = 0;");
                        }
                        Q::Rec(m) if texty(p, *m) => {
                            let _ = writeln!(s, "    cpl_rel{}(&v->f{i});", m.0);
                        }
                        _ => {}
                    }
                }
                s.push_str("}\n");
            }
            // unbox: 1 when every leaf is a small Int or a Bool; a text is borrowed from w
            let _ = writeln!(s, "static int cpl_unbox{n}(cplrt_w w, cpl_r{n} *o)\n{{");
            for (i, f) in fs.iter().enumerate() {
                let w = format!("cplrt_field(w, {i}u)");
                match f {
                    Q::Int => {
                        let _ = writeln!(
                            s,
                            "    if (!cplrt_is_small({w}))\n        return 0;\n    o->f{i} = cplrt_small_value({w});"
                        );
                    }
                    Q::Bool => {
                        let _ = writeln!(s, "    o->f{i} = cplrt_truth({w});");
                    }
                    Q::Rec(m) => {
                        let _ = writeln!(s, "    if (!cpl_unbox{}({w}, &o->f{i}))\n        return 0;", m.0);
                    }
                    Q::Text => {
                        let _ = writeln!(s, "    o->f{i} = {w};");
                    }
                    _ => {}
                }
            }
            s.push_str("    return 1;\n}\n");
            // box: CPLRT_OK, or CPLRT_NOMEM with nothing raised and *out unchanged
            let _ = writeln!(s, "static int cpl_box{n}(cplrt_cx *cx, const cpl_r{n} *v, cplrt_w *out)\n{{");
            if fs.iter().all(|f| matches!(f, Q::Int | Q::Bool)) {
                // flat and every Int small: the struct filled in place, no field boxed or retained
                let small: Vec<String> = fs
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| **f == Q::Int)
                    .map(|(i, _)| format!("cplrt_q_small(v->f{i})"))
                    .collect();
                let cond = if small.is_empty() { "1".to_string() } else { small.join(" && ") };
                let _ = writeln!(
                    s,
                    "    if ({cond}) {{\n        cplrt_node *s_ = cplrt_q_node(cx, {n}u, {}u);\n        if (s_ == NULL)\n            return 2; /* CPLRT_NOMEM */",
                    fs.len()
                );
                for (i, f) in fs.iter().enumerate() {
                    let w = if *f == Q::Int {
                        format!("cplrt_small(v->f{i})")
                    } else {
                        format!("cplrt_bool(v->f{i} != 0)")
                    };
                    let _ = writeln!(s, "        s_->item[{i}] = {w};");
                }
                s.push_str("        CPLRT_SET(*out, cplrt_word_of(&s_->hdr));\n        return CPLRT_OK;\n    }\n");
            }
            let _ = writeln!(s, "    cplrt_w f[{}] = {{ 0 }};\n    int st = CPLRT_OK;", fs.len());
            for (i, f) in fs.iter().enumerate() {
                match f {
                    Q::Int => {
                        let _ = writeln!(
                            s,
                            "    if (st == CPLRT_OK)\n        st = cplrt_q_box(cx, v->f{i}, &f[{i}]);"
                        );
                    }
                    Q::Bool => {
                        let _ = writeln!(s, "    f[{i}] = cplrt_bool(v->f{i} != 0);");
                    }
                    Q::Rec(m) => {
                        let _ = writeln!(
                            s,
                            "    if (st == CPLRT_OK)\n        st = cpl_box{}(cx, &v->f{i}, &f[{i}]);",
                            m.0
                        );
                    }
                    // a reference of f's own, given back below with the others
                    Q::Text => {
                        let _ = writeln!(s, "    f[{i}] = v->f{i};\n    cplrt_retain(f[{i}]);");
                    }
                    _ => {}
                }
            }
            let _ = writeln!(
                s,
                "    if (st == CPLRT_OK)\n        st = cplrt_q_struct(cx, {n}u, f, {}u, out);",
                fs.len()
            );
            let _ = writeln!(
                s,
                "    for (uint32_t i = 0; i < {}u; i++)\n        cplrt_release(f[i]);\n    return st;\n}}",
                fs.len()
            );
        }
        for d in &self.enums {
            s.push_str(&enum_type(p, *d));
        }
        s
    }

    /// `static <ret> cpl_q<n>(cplrt_cx *cx, uint32_t qd, <params>[, int64_t mprev])`.
    pub(crate) fn prototype(&self, p: &Program, d: DefId) -> String {
        let fd = p.def(d).kind.as_fn().expect("a function");
        let ret = ret_cty(qty(p, &fd.ret).expect("a scalar result"));
        let mut s = format!("static {ret} cpl_q{}(cplrt_cx *cx, uint32_t qd", d.0);
        for param in &fd.params {
            let ty = cty(qty(p, &param.ty).expect("a scalar parameter"));
            let _ = write!(s, ", {ty} q{}", param.local.0);
        }
        if self.fns[&d] {
            s.push_str(", int64_t mprev");
        }
        s.push(')');
        s
    }

    /// The lines that open `cpl_f<n>`: the speculative call, its result boxed on success; on a bail the generic body
    /// follows. `params` are the C names of the generic parameters.
    pub(crate) fn wrapper(&self, p: &Program, d: DefId, params: &[String], dn: &str) -> String {
        let fd = p.def(d).kind.as_fn().expect("a function");
        let mut decls = String::new();
        let mut guards = Vec::new();
        let mut args = Vec::new();
        for (i, (param, c)) in fd.params.iter().zip(params).enumerate() {
            match qty(p, &param.ty).expect("a scalar parameter") {
                Q::Bool => args.push(format!("cplrt_truth({c})")),
                Q::Int => {
                    guards.push(format!("cplrt_is_small({c})"));
                    args.push(format!("cplrt_small_value({c})"));
                }
                Q::Rec(r) => {
                    let _ = writeln!(decls, "        cpl_r{} qa{i}_;", r.0);
                    guards.push(format!("cpl_unbox{}({c}, &qa{i}_)", r.0));
                    args.push(format!("qa{i}_"));
                }
                Q::Enum(r) => {
                    let _ = writeln!(decls, "        cpl_v{} qa{i}_;", r.0);
                    guards.push(format!("cpl_unboxv{}({c}, &qa{i}_)", r.0));
                    args.push(format!("qa{i}_"));
                }
                // borrowed: the scalar form reads its elements as it goes (a word, its fields)
                Q::List(_) | Q::Opt(_) | Q::Text | Q::Word => args.push(c.clone()),
            }
        }
        if self.fns[&d] {
            args.push(format!("cplrt_q_prev(cx, {dn})"));
        }
        let rq = qty(p, &fd.ret).expect("a scalar result");
        let boxed = match rq {
            Q::Bool => "CPLRT_SET(*out, cplrt_bool(qr_.v != 0)); return CPLRT_OK;".to_string(),
            Q::Int => "if (cplrt_q_box(cx, qr_.v, out) == CPLRT_OK) return CPLRT_OK;".to_string(),
            // the record's texts were the scalar form's to give: given back once the struct holds its own
            Q::Rec(r) if texty(p, r) => format!(
                "int bst_ = cpl_box{n}(cx, &qr_.v, out); cpl_rel{n}(&qr_.v); if (bst_ == CPLRT_OK) return CPLRT_OK;",
                n = r.0
            ),
            Q::Rec(r) => format!(
                "if (cpl_box{}(cx, &qr_.v, out) == CPLRT_OK) return CPLRT_OK;",
                r.0
            ),
            Q::Enum(r) => format!(
                "if (cpl_boxv{}(cx, &qr_.v, out) == CPLRT_OK) return CPLRT_OK;",
                r.0
            ),
            Q::List(_) | Q::Opt(_) | Q::Text => "CPLRT_SET(*out, qr_.v); return CPLRT_OK;".to_string(),
            Q::Word => unreachable!("a word is never a result (local_check)"),
        };
        let mut call = format!("cpl_q{}(cx, cx->calls", d.0);
        for a in &args {
            let _ = write!(call, ", {a}");
        }
        call.push(')');
        let guard = if guards.is_empty() {
            "1".to_string()
        } else {
            guards.join(" && ")
        };
        let mut s = String::new();
        s.push_str("    if (cx->q_ok) { /* the scalar tier */\n");
        s.push_str(&decls);
        let _ = writeln!(s, "        if ({guard}) {{");
        let _ = writeln!(s, "            {} qr_ = {call};", ret_cty(rq));
        let _ = writeln!(s, "            if (qr_.ok) {{ {boxed} }}");
        let _ = writeln!(
            s,
            "            else if (++cx->q_bails >= CPLRT_Q_MAX_BAILS) cx->q_ok = 0;"
        );
        s.push_str("        }\n    }\n");
        s
    }

    /// The definition of `cpl_q<n>`.
    pub(crate) fn emit(&self, t: &Tables<'_>, d: DefId) -> String {
        let p = t.p;
        let fd = p.def(d).kind.as_fn().expect("a function");
        let FnBody::Block(body) = &fd.body else {
            return String::new();
        };
        let rq = qty(p, &fd.ret).expect("a scalar result");
        let params: BTreeSet<LocalId> = fd.params.iter().map(|x| x.local).collect();
        let mut e = Emitter {
            p,
            plan: self,
            def: d,
            out: String::new(),
            decls: Vec::new(),
            indent: 1,
            next: 0,
            measured: self.fns[&d],
            in_contract: false,
            unit: p.std_defs.unit,
            option: p.std_defs.option,
            locals: fd
                .locals
                .iter()
                .filter_map(|l| qty(p, &l.ty).map(|q| (l.id, q)))
                .collect(),
            empty: false,
            flat: cpl_hir::scalar::flat_locals(p, fd, cpl_hir::scalar::Caps::NATIVE),
        };
        // the body, then everything it declared, at the top
        e.line("if (!cplrt_q_enter(cx, qd))");
        e.line("    goto bail;");
        // requires, then the measure (S35: none is recorded while either is being evaluated)
        e.in_contract = true;
        if !fd.requires.is_empty() {
            e.line("if (cx->check_contracts) {");
            e.indent += 1;
            for c in &fd.requires {
                let v = e.expr(&c.expr);
                e.line(&format!("if (!({v})) goto bail;"));
            }
            e.indent -= 1;
            e.line("}");
        }
        if let Some(c) = &fd.decreases {
            e.line("if (cx->check_contracts) {");
            e.indent += 1;
            let v = e.expr(&c.expr);
            e.line(&format!("mself = {v};"));
            e.line("if (mself < 0 || (mprev != CPLRT_Q_NONE && mself >= mprev)) goto bail;");
            e.indent -= 1;
            e.line("}");
        }
        e.in_contract = false;
        if let Some(v) = e.block(body) {
            e.assign("qret", rq, &v);
        }
        e.line("goto ret;");
        // the exit: ensures with `result` bound (the measure is recorded: a call of this function compares with it)
        e.out.push_str("ret:\n");
        if !fd.ensures.is_empty() {
            e.line("if (cx->check_contracts) {");
            e.indent += 1;
            if let Some(r) = fd.result_local {
                e.assign(&format!("q{}", r.0), rq, "qret");
            }
            for c in &fd.ensures {
                let v = e.expr(&c.expr);
                e.line(&format!("if (!({v})) goto bail;"));
            }
            e.indent -= 1;
            e.line("}");
        }
        // every list or text a local or a temporary holds is released, and every text a record variable holds (the
        // result is the caller's)
        let mut release = String::new();
        let mut give_back = |q: Q, n: &str| match q {
            Q::List(_) | Q::Text => {
                let _ = write!(release, "cplrt_release({n}); ");
            }
            Q::Rec(d) if texty(p, d) => {
                let _ = write!(release, "cpl_rel{}(&{n}); ", d.0);
            }
            Q::Opt(El::Rec(d)) if texty(p, d) => {
                let _ = write!(release, "cpl_rel{}(&{n}.v); ", d.0);
            }
            _ => {}
        };
        let mut flat_release = String::new();
        for l in &fd.locals {
            if let Some(el) = e.flat.get(&l.id) {
                // a flat array: its records' texts, then its block
                if let El::Rec(d) = el {
                    if texty(p, *d) {
                        let _ = write!(
                            flat_release,
                            "for (uint32_t i_ = 0; i_ < q{n}.n; i_++) cpl_rel{r}(&((cpl_r{r} *)q{n}.a)[i_]); ",
                            n = l.id.0,
                            r = d.0
                        );
                    }
                }
                let _ = write!(flat_release, "cplrt_q_buf_free(&q{}); ", l.id.0);
            } else if !params.contains(&l.id) {
                if let Some(q) = qty(p, &l.ty) {
                    give_back(q, &format!("q{}", l.id.0));
                }
            }
        }
        for (ty, n, _) in &e.decls {
            if ty == "cplrt_w" {
                give_back(Q::Text, n);
            } else if let Some(d) = ty.strip_prefix("cpl_r").and_then(|x| x.parse::<u32>().ok()) {
                give_back(Q::Rec(DefId(d)), n);
            } else if let Some(d) = ty.strip_prefix("cpl_qo").and_then(|x| x.parse::<u32>().ok()) {
                give_back(Q::Opt(El::Rec(DefId(d))), n);
            }
        }
        release.push_str(&flat_release);
        if !release.is_empty() {
            e.line(release.trim_end());
        }
        match rq {
            Q::Rec(r) => {
                e.line(&format!(
                    "{{ cpl_qr{} qo_; qo_.v = qret; qo_.ok = 1; return qo_; }}",
                    r.0
                ));
            }
            Q::Enum(r) => {
                e.line(&format!(
                    "{{ cpl_qv{} qo_; qo_.v = qret; qo_.ok = 1; return qo_; }}",
                    r.0
                ));
            }
            Q::List(_) | Q::Opt(_) | Q::Text => e.line("return (cplrt_qw){ qret, 1 };"),
            _ => e.line("return (cplrt_qi){ (int64_t)qret, 1 };"),
        }
        e.out.push_str("bail:\n");
        let mut bail = release;
        match rq {
            Q::List(_) | Q::Text => bail.push_str("cplrt_release(qret); "),
            Q::Rec(d) if texty(p, d) => {
                let _ = write!(bail, "cpl_rel{}(&qret); ", d.0);
            }
            _ => {}
        }
        if !bail.is_empty() {
            e.line(bail.trim_end());
        }
        match rq {
            Q::Rec(r) => e.line(&format!("return cpl_zq{};", r.0)),
            Q::Enum(r) => e.line(&format!("return cpl_zqv{};", r.0)),
            Q::List(_) | Q::Opt(_) | Q::Text => e.line("return (cplrt_qw){ 0, 0 };"),
            _ => e.line("return (cplrt_qi){ 0, 0 };"),
        }
        // the function: declarations first
        let mut s = String::new();
        let _ = writeln!(
            s,
            "/* {}: the scalar tier of the function below */",
            crate::print::comment_text(&p.display_path(d))
        );
        s.push_str(&self.prototype(p, d));
        s.push_str("\n{\n");
        for l in &fd.locals {
            if e.flat.contains_key(&l.id) {
                let _ = writeln!(s, "    cplrt_qbuf q{} = {{ NULL, 0u, 0u }};", l.id.0);
            } else if !params.contains(&l.id) {
                let q = qty(p, &l.ty).expect("a scalar local");
                let _ = writeln!(s, "    {} q{} = {};", cty(q), l.id.0, zero(q));
            }
        }
        let _ = writeln!(s, "    {} qret = {};", cty(rq), zero(rq));
        // whether a loop's tick polls, read once (cplrt_q_tick_i)
        if e.out.contains("cplrt_q_tick_i(cx, qi_)") {
            s.push_str("    const int qi_ = cplrt_q_interruptible(cx);\n");
        }
        if self.fns[&d] {
            s.push_str("    int64_t mself = CPLRT_Q_NONE;\n    (void)mself;\n");
        }
        for (ty, n, init) in &e.decls {
            let _ = writeln!(s, "    {ty} {n} = {init};");
        }
        // a variable the body only writes is no warning (-Wunused-but-set-variable)
        for l in &fd.locals {
            if !params.contains(&l.id) {
                let _ = writeln!(s, "    (void)q{};", l.id.0);
            }
        }
        for (_, n, _) in &e.decls {
            let _ = writeln!(s, "    (void){n};");
        }
        if e.empty {
            // immortal: nothing to release
            s.push_str("    cplrt_w qempty = 0;
    if (cplrt_q_list(cx, NULL, 0u, &qempty) != CPLRT_OK)
        goto bail;
");
        }
        s.push_str(&e.out);
        s.push_str("}\n");
        s
    }
}

/// The C printer of one scalar form. Expressions print as C expressions after the statements they need (a list
/// expression is a word some variable holds, borrowed); a block's value is `None` when it has type `Unit`.
struct Emitter<'a> {
    p: &'a Program,
    plan: &'a Plan,
    def: DefId,
    out: String,
    /// The temporaries: (C type, name, initializer), declared at the top.
    decls: Vec<(String, String, String)>,
    indent: usize,
    next: u32,
    measured: bool,
    /// In a contract expression: a recursive call there compares with no measure (S35).
    in_contract: bool,
    unit: DefId,
    option: DefId,
    locals: BTreeMap<LocalId, Q>,
    /// Some `[]` reads `qempty`.
    empty: bool,
    /// The list locals held as flat arrays (`flat_locals`).
    flat: BTreeMap<LocalId, El>,
}

impl Emitter<'_> {
    fn line(&mut self, s: &str) {
        for _ in 0..self.indent {
            self.out.push_str("    ");
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    /// The index of `Some` among `Option`'s variants.
    fn some_variant(&self) -> usize {
        self.p
            .def(self.option)
            .kind
            .as_enum()
            .and_then(|e| e.variants.iter().position(|v| v.name.as_str() == "Some"))
            .unwrap_or(0)
    }

    /// A fresh C variable of scalar type `q`, declared at the top.
    fn temp(&mut self, q: Q) -> String {
        self.next += 1;
        let t = format!("qt{}", self.next);
        self.decls.push((cty(q), t.clone(), zero(q)));
        t
    }

    fn q(&self, ty: &Ty) -> Q {
        qty(self.p, ty).unwrap_or(Q::Int)
    }

    /// `dst = v` for a value of type `q`. A list: each variable holds its own reference, so one a temporary holds
    /// moves (a temporary's value is read once: the expression it stands for), and any other is retained.
    fn assign(&mut self, dst: &str, q: Q, v: &str) {
        // a record holding texts: a temporary's references moved (it is read once: the expression it stands for);
        // another value's taken, then the old one's given back
        let temp = v.strip_prefix("qt").is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()));
        match q {
            Q::Rec(d) if temp && v != dst && texty(self.p, d) => {
                let n = d.0;
                self.line(&format!("cpl_rel{n}(&{dst}); {dst} = {v}; {v} = cpl_z{n};"));
                return;
            }
            Q::Opt(El::Rec(d)) if temp && v != dst && texty(self.p, d) => {
                let n = d.0;
                self.line(&format!("cpl_rel{n}(&{dst}.v); {dst} = {v}; {v} = cpl_zo{n};"));
                return;
            }
            Q::Rec(d) if v != dst && texty(self.p, d) => {
                let n = d.0;
                self.line(&format!("{{ cpl_r{n} qc_ = {v}; cpl_ret{n}(&qc_); cpl_rel{n}(&{dst}); {dst} = qc_; }}"));
                return;
            }
            Q::Opt(El::Rec(d)) if v != dst && texty(self.p, d) => {
                let n = d.0;
                self.line(&format!("{{ cpl_qo{n} qc_ = {v}; cpl_ret{n}(&qc_.v); cpl_rel{n}(&{dst}.v); {dst} = qc_; }}"));
                return;
            }
            _ => {}
        }
        if word_owned(q) {
            let temp = v.strip_prefix("qt").is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()));
            if temp && v != dst {
                self.line(&format!("CPLRT_MOVE({dst}, {v});"));
            } else if v != dst {
                self.line(&format!("CPLRT_COPY({dst}, {v});"));
            }
        } else {
            self.line(&format!("{dst} = {v};"));
        }
    }

    /// Statements, then the tail's value.
    fn block(&mut self, b: &Block) -> Option<String> {
        for s in &b.stmts {
            self.stmt(s);
        }
        match &b.tail {
            Some(e) => {
                let v = self.expr(e);
                qty(self.p, &e.ty).is_some().then_some(v)
            }
            None => None,
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let { local, value } | Stmt::Assign { local, value } if self.flat.contains_key(local) => {
                self.flat_write(*local, value)
            }
            Stmt::Assign { local, value } if self.in_place(*local, value) => {}
            // t = t ++ x on the local's own text: appended in place where it is unique with room
            Stmt::Assign { local, value } if text_append_chain(self.p, *local, value).is_some() => {
                // every part first (each sees the old text), then appended in place where it is unique with room
                let parts = text_append_chain(self.p, *local, value).unwrap_or_default();
                let xs: Vec<String> = parts.iter().map(|x| self.fixed(x)).collect();
                for x in xs {
                    self.line(&format!("if (cplrt_q_text_append(cx, &q{}, {x}) != CPLRT_OK) goto bail;", local.0));
                }
            }
            Stmt::Let { local, value } | Stmt::Assign { local, value } => {
                let v = self.expr(value);
                let q = self.locals.get(local).copied().unwrap_or(Q::Int);
                self.assign(&format!("q{}", local.0), q, &v);
            }
            Stmt::Discard(e) | Stmt::Expr(e) => {
                let v = self.expr(e);
                self.line(&format!("(void)({v});"));
            }
            Stmt::Return(Some(e)) => {
                let v = self.expr(e);
                let q = self.q(&e.ty);
                self.assign("qret", q, &v);
                self.line("goto ret;");
            }
            Stmt::Assert { expr, .. } => {
                let v = self.expr(expr);
                self.line(&format!("if (!({v})) goto bail;"));
            }
            Stmt::While { cond, body, .. } => {
                self.line("for (;;) {");
                self.indent += 1;
                self.line("if (!cplrt_q_tick_i(cx, qi_)) goto bail;");
                let c = self.expr(cond);
                self.line(&format!("if (!({c})) break;"));
                let _ = self.block(body);
                self.indent -= 1;
                self.line("}");
            }
            // excluded by local_check
            Stmt::Return(None) | Stmt::Concurrent(_) => self.line("goto bail;"),
        }
    }

    /// `xs = xs.push(x)` and `xs = xs.drop(k)` on the local's own list, in place (the runtime copies a shared one),
    /// when the argument cannot assign `xs` before the list is read.
    /// A write of a flat array (`flat_locals`): `[]` empties it, its own `push` appends in place.
    fn flat_write(&mut self, local: LocalId, value: &Expr) {
        let el = self.flat[&local];
        let n = local.0;
        let ty = flat_cty(el);
        let texts = matches!(el, El::Rec(d) if texty(self.p, d));
        match &value.kind {
            ExprKind::Call { args, .. } if args.len() == 2 => {
                let v = self.fixed(&args[1]);
                self.line(&format!(
                    "if (q{n}.n == q{n}.cap && cplrt_q_buf_grow(cx, &q{n}, sizeof({ty})) != CPLRT_OK) goto bail;"
                ));
                match el {
                    // the temporary's references moved into the array
                    El::Rec(d) if texts => {
                        self.line(&format!("(({ty} *)q{n}.a)[q{n}.n++] = {v}; {v} = cpl_z{};", d.0))
                    }
                    _ => self.line(&format!("(({ty} *)q{n}.a)[q{n}.n++] = {v};")),
                }
            }
            _ => {
                if let (true, El::Rec(d)) = (texts, el) {
                    self.line(&format!(
                        "for (uint32_t i_ = 0; i_ < q{n}.n; i_++) cpl_rel{r}(&(({ty} *)q{n}.a)[i_]);",
                        r = d.0
                    ));
                }
                self.line(&format!("q{n}.n = 0u;"));
            }
        }
    }

    /// A read of a flat array: `len`, `is_empty`, or an element as an option (a copy: a record's texts taken).
    fn flat_read(&mut self, which: &str, args: &[Expr]) -> Option<String> {
        let ExprKind::Local(l) = args.first()?.kind else {
            return None;
        };
        let el = *self.flat.get(&l)?;
        let n = l.0;
        let ty = flat_cty(el);
        let index = match which {
            "list_len" => return Some(format!("((int64_t)q{n}.n)")),
            "list_is_empty" => return Some(format!("(q{n}.n == 0u)")),
            "list_get" => self.fixed(&args[1]),
            "list_first" => "INT64_C(0)".to_string(),
            "list_last" => format!("((int64_t)q{n}.n - 1)"),
            _ => return None,
        };
        let t = self.temp(Q::Opt(el));
        match el {
            El::Rec(d) if texty(self.p, d) => self.line(&format!("cpl_rel{r}(&{t}.v); {t} = cpl_zo{r};", r = d.0)),
            El::Rec(d) => self.line(&format!("{t} = cpl_zo{};", d.0)),
            _ => self.line(&format!("{t}.has = 0; {t}.v = 0;")),
        }
        let retain = match el {
            El::Rec(d) if texty(self.p, d) => format!(" cpl_ret{}(&{t}.v);", d.0),
            _ => String::new(),
        };
        self.line(&format!(
            "{{ int64_t k_ = {index}; if (k_ >= 0 && (uint64_t)k_ < q{n}.n) {{ {t}.v = (({ty} *)q{n}.a)[k_];{retain} {t}.has = 1; }} }}"
        ));
        Some(t)
    }

    fn in_place(&mut self, local: LocalId, value: &Expr) -> bool {
        let ExprKind::Call { callee, args, .. } = &value.kind else {
            return false;
        };
        let which = builtin(self.p, *callee);
        if !matches!(which, Some("list_push" | "list_drop"))
            || args.len() != 2
            || !matches!(args[0].kind, ExprKind::Local(id) if id == local)
            || has_block(&args[1])
        {
            return false;
        }
        let slot = format!("q{}", local.0);
        let a = self.fixed(&args[1]);
        if which == Some("list_push") {
            let el = match self.q(&args[0].ty) {
                Q::List(e) => e,
                _ => El::Int,
            };
            let w = self.word(el, &a);
            self.line(&format!("if (cplrt_q_push_fast(cx, &{slot}, {w}) != CPLRT_OK) goto bail;"));
        } else {
            self.line(&format!("if ({a} < 0) goto bail;"));
            self.line(&format!(
                "if (cplrt_q_drop_fast(cx, &{slot}, {a} > (int64_t)cplrt_len({slot}) ? cplrt_len({slot}) : (uint32_t){a}) != CPLRT_OK) goto bail;"
            ));
        }
        true
    }

    /// The word of a value `v` (a variable) of element type `el`, for a list: a small Int (else a bail: a bigint
    /// element would need an allocation), a Bool, a record boxed into a list-word temporary (which keeps its own
    /// reference, released like any), or a list word as it is.
    fn word(&mut self, el: El, v: &str) -> String {
        match el {
            El::Bool => format!("cplrt_bool({v} != 0)"),
            El::Int => {
                self.line(&format!(
                    "if ({v} < CPLRT_SMALL_MIN || {v} > CPLRT_SMALL_MAX) goto bail;"
                ));
                format!("cplrt_small({v})")
            }
            El::Rec(d) => {
                let t = self.temp(Q::List(El::Int));
                self.line(&format!("if (cpl_box{}(cx, &{v}, &{t}) != CPLRT_OK) goto bail;", d.0));
                t
            }
            El::List(_) => v.to_string(),
        }
    }

    /// `e` fixed in a fresh variable (its value stays while later code, which may assign locals, runs).
    fn fixed(&mut self, e: &Expr) -> String {
        let v = self.expr(e);
        let q = self.q(&e.ty);
        let t = self.temp(q);
        self.assign(&t, q, &v);
        t
    }

    /// A list operand that `rest` is evaluated after: the local itself (borrowed) when nothing in `rest` can assign
    /// it, else fixed.
    fn list_operand(&mut self, e: &Expr, rest: &[&Expr]) -> String {
        if let ExprKind::Local(id) = &e.kind {
            if !rest.iter().any(|r| has_block(r)) {
                return format!("q{}", id.0);
            }
        }
        if let Some(b) = self.borrowed_element(e, rest) {
            return b;
        }
        self.fixed(e)
    }

    /// `ys.get(i).unwrap_or(d)` (or `first`, `last`) on a local list of lists `ys`, with `d` a local or `[]`, read by
    /// an operation that only borrows it: the element list borrowed from `ys` (or `d`), with no reference taken.
    /// Nothing before the read's consumer can assign `ys` or `d` (no block in the read or in `rest`).
    fn borrowed_element(&mut self, e: &Expr, rest: &[&Expr]) -> Option<String> {
        let ExprKind::Call { callee, args, using, .. } = &e.kind else {
            return None;
        };
        if !using.is_empty() || builtin(self.p, *callee) != Some("option_unwrap_or") || !nested_read(self.p, &args[0]) {
            return None;
        }
        let ExprKind::Call { callee: read, args: rargs, .. } = &args[0].kind else {
            return None;
        };
        let ExprKind::Local(ys) = &rargs[0].kind else {
            return None;
        };
        let d = &args[1];
        if has_block(e) || rest.iter().any(|r| has_block(r)) {
            return None;
        }
        let dv = match &d.kind {
            ExprKind::Local(l) => format!("q{}", l.0),
            ExprKind::List(items) if items.is_empty() => {
                self.empty = true;
                "qempty".to_string()
            }
            _ => return None,
        };
        let xs = format!("q{}", ys.0);
        let index = match builtin(self.p, *read).unwrap_or("") {
            "list_get" => self.fixed(&rargs[1]),
            "list_first" => "INT64_C(0)".to_string(),
            _ => format!("((int64_t)cplrt_len({xs}) - 1)"),
        };
        self.next += 1;
        let b = format!("qb{}", self.next);
        // not "cplrt_w": a borrowed word is never released
        self.decls.push(("cplrt_w /* borrowed */".to_string(), b.clone(), "0".to_string()));
        self.line(&format!(
            "{{ int64_t k_ = {index}; {b} = k_ >= 0 && (uint64_t)k_ < cplrt_len({xs}) ? cplrt_items({xs})[k_] : {dv}; }}"
        ));
        Some(b)
    }

    /// The element at `index` (an int64_t C expression) of the list `xs`, as an option (never of a list).
    fn element(&mut self, xs: &str, index: &str, el: El) -> String {
        let t = self.temp(Q::Opt(el));
        match el {
            El::Rec(d) if texty(self.p, d) => self.line(&format!("cpl_rel{n}(&{t}.v); {t} = cpl_zo{n};", n = d.0)),
            El::Rec(d) => self.line(&format!("{t} = cpl_zo{};", d.0)),
            _ => self.line(&format!("{t}.has = 0; {t}.v = 0;")),
        }
        self.line(&format!(
            "{{ int64_t k_ = {index}; if (k_ >= 0 && (uint64_t)k_ < cplrt_len({xs})) {{ cplrt_w w_ = cplrt_items({xs})[k_];"
        ));
        match el {
            El::Int => self.line(&format!(
                "    if (!cplrt_is_small(w_)) {{ goto bail; }} {t}.v = cplrt_small_value(w_); {t}.has = 1; }} }}"
            )),
            El::Bool => self.line(&format!("    {t}.v = cplrt_truth(w_); {t}.has = 1; }} }}")),
            // its texts borrowed from the list into a scratch, then references of the temporary's own
            El::Rec(d) if texty(self.p, d) => self.line(&format!(
                "    cpl_r{n} u_; if (!cpl_unbox{n}(w_, &u_)) {{ goto bail; }} cpl_ret{n}(&u_); {t}.v = u_; {t}.has = 1; }} }}",
                n = d.0
            )),
            El::Rec(d) => self.line(&format!(
                "    if (!cpl_unbox{}(w_, &{t}.v)) {{ goto bail; }} {t}.has = 1; }} }}",
                d.0
            )),
            El::List(_) => self.line("    goto bail; } }"),
        }
        t
    }

    /// `xs.get(i).unwrap_or(d)`, `xs.first().unwrap_or(d)`, `xs.last().unwrap_or(d)` on a list of lists: the element
    /// list, retained into a temporary of its own, or `d`.
    fn list_element_or(&mut self, read: &Expr, d: &Expr) -> String {
        let ExprKind::Call { callee, args, .. } = &read.kind else {
            return "0".to_string();
        };
        let which = builtin(self.p, *callee).unwrap_or("");
        let rest: Vec<&Expr> = args.iter().skip(1).chain(std::iter::once(d)).collect();
        let xs = self.list_operand(&args[0], &rest);
        let index = match which {
            "list_get" => self.fixed(&args[1]),
            "list_first" => "INT64_C(0)".to_string(),
            _ => format!("((int64_t)cplrt_len({xs}) - 1)"),
        };
        let dv = self.fixed(d);
        let t = self.temp(self.q(&d.ty));
        self.line(&format!(
            "{{ int64_t k_ = {index}; if (k_ >= 0 && (uint64_t)k_ < cplrt_len({xs})) {{ CPLRT_COPY({t}, cplrt_items({xs})[k_]); }} else {{ CPLRT_COPY({t}, {dv}); }} }}"
        ));
        t
    }

    fn list_builtin(&mut self, which: &str, e: &Expr, args: &[Expr]) -> String {
        let rest: Vec<&Expr> = args.iter().skip(1).collect();
        let xs = self.list_operand(&args[0], &rest);
        let el = match qty(self.p, &args[0].ty) {
            Some(Q::List(e)) => e,
            _ => El::Int,
        };
        match which {
            "list_len" => format!("((int64_t)cplrt_len({xs}))"),
            "list_is_empty" => format!("(cplrt_len({xs}) == 0u)"),
            "list_first" => self.element(&xs, "INT64_C(0)", el),
            "list_last" => self.element(&xs, &format!("((int64_t)cplrt_len({xs}) - 1)"), el),
            "list_get" => {
                let i = self.fixed(&args[1]);
                self.element(&xs, &i, el)
            }
            "list_push" => {
                let a = self.fixed(&args[1]);
                let w = self.word(el, &a);
                let t = self.temp(self.q(&e.ty));
                self.line(&format!("if (cplrt_q_push(cx, {xs}, {w}, &{t}) != CPLRT_OK) goto bail;"));
                t
            }
            "list_take" | "list_drop" => {
                let k = self.fixed(&args[1]);
                self.line(&format!("if ({k} < 0) goto bail;"));
                let t = self.temp(self.q(&e.ty));
                let n = format!("({k} > (int64_t)cplrt_len({xs}) ? cplrt_len({xs}) : (uint32_t){k})");
                let (from, to) = if which == "list_take" {
                    ("0u".to_string(), n)
                } else {
                    (n, format!("cplrt_len({xs})"))
                };
                self.line(&format!(
                    "if (cplrt_q_slice(cx, {xs}, {from}, {to}, &{t}) != CPLRT_OK) goto bail;"
                ));
                t
            }
            _ => {
                // list_drop_last
                let t = self.temp(self.q(&e.ty));
                self.line(&format!(
                    "if (cplrt_q_slice(cx, {xs}, 0u, cplrt_len({xs}) == 0u ? 0u : cplrt_len({xs}) - 1u, &{t}) != CPLRT_OK) goto bail;"
                ));
                t
            }
        }
    }

    fn expr(&mut self, e: &Expr) -> String {
        match &e.kind {
            ExprKind::Lit(Lit::Int(n)) => match small_literal(n) {
                Some(v) if v == i64::MIN => "INT64_MIN".to_string(),
                Some(v) => format!("INT64_C({v})"),
                None => {
                    self.line("goto bail;");
                    "0".to_string()
                }
            },
            ExprKind::Lit(Lit::Bool(b)) => if *b { "1" } else { "0" }.to_string(),
            // the program's immortal constant: borrowed
            ExprKind::Lit(Lit::Text(t)) => self.plan.text_k(t),
            ExprKind::Local(id) => format!("q{}", id.0),
            ExprKind::Widen { expr, .. } => self.expr(expr),
            ExprKind::Ctor { def, .. } if *def == self.unit => "0".to_string(),
            ExprKind::Ctor { def, variant, args } if self.q(&e.ty) == Q::Enum(*def) => {
                let vals: Vec<String> = args.iter().map(|a| self.fixed(a)).collect();
                let t = self.temp(Q::Enum(*def));
                self.line(&format!("{t} = cpl_zv{};", def.0));
                self.line(&format!("{t}.tag = {variant}u;"));
                for (i, v) in vals.iter().enumerate() {
                    self.line(&format!("{t}.v{variant}_{i} = {v};"));
                }
                t
            }
            ExprKind::Ctor { def, variant, args } if *def == self.option => {
                let q = self.q(&e.ty);
                let v = args.first().map(|a| self.fixed(a));
                let t = self.temp(q);
                if let Q::Opt(El::Rec(d)) = q {
                    let n = d.0;
                    let texts = texty(self.p, d);
                    if texts {
                        self.line(&format!("cpl_rel{n}(&{t}.v);"));
                    }
                    match (variant, v) {
                        (0, Some(v)) if texts => {
                            self.line(&format!("{{ cpl_r{n} qs_ = {v}; cpl_ret{n}(&qs_); {t}.v = qs_; }} {t}.has = 1;"))
                        }
                        (0, Some(v)) => self.line(&format!("{t}.v = {v}; {t}.has = 1;")),
                        _ => self.line(&format!("{t}.v = cpl_z{n}; {t}.has = 0;")),
                    }
                    return t;
                }
                match (variant, v) {
                    (0, Some(v)) => self.line(&format!("{t}.v = {v}; {t}.has = 1;")),
                    _ => self.line(&format!("{t}.v = 0; {t}.has = 0;")),
                }
                t
            }
            ExprKind::List(items) => {
                let vals: Vec<String> = items.iter().map(|a| self.fixed(a)).collect();
                let el = match self.q(&e.ty) {
                    Q::List(x) => x,
                    _ => El::Int,
                };
                let words: Vec<String> = vals.iter().map(|v| self.word(el, v)).collect();
                if words.is_empty() {
                    // `[]`: the runtime's immortal empty list, fetched once per call (at the top)
                    self.empty = true;
                    return "qempty".to_string();
                }
                let t = self.temp(self.q(&e.ty));
                {
                    self.line(&format!(
                        "{{ const cplrt_w qa_[{}] = {{ {} }}; if (cplrt_q_list(cx, qa_, {}u, &{t}) != CPLRT_OK) goto bail; }}",
                        words.len(),
                        words.join(", "),
                        words.len()
                    ));
                }
                t
            }
            // a field of a struct held as a word: an Int (a bail on a bigint), a Bool, or a word, borrowed
            ExprKind::Field { base, index, .. } if self.q(&base.ty) == Q::Word => {
                let b = self.fixed(base);
                let q = self.q(&e.ty);
                let t = self.temp(q);
                let f = format!("cplrt_field({b}, {index}u)");
                match q {
                    Q::Int => self.line(&format!(
                        "{{ cplrt_w f_ = {f}; if (!cplrt_is_small(f_)) goto bail; {t} = cplrt_small_value(f_); }}"
                    )),
                    Q::Bool => self.line(&format!("{t} = cplrt_truth({f});")),
                    _ => self.line(&format!("{t} = {f};")),
                }
                t
            }
            // a field of a local: the field alone fixed (the record is not copied, nor its texts taken)
            ExprKind::Field { base, index, .. } if matches!(base.kind, ExprKind::Local(_)) => {
                let ExprKind::Local(id) = &base.kind else {
                    return "0".to_string();
                };
                let q = self.q(&e.ty);
                let t = self.temp(q);
                self.assign(&t, q, &format!("q{}.f{index}", id.0));
                t
            }
            ExprKind::Field { base, index, .. } => {
                let b = self.fixed(base);
                format!("{b}.f{index}")
            }
            ExprKind::StructLit {
                def,
                spread,
                fields: given,
                ..
            } => {
                // every field value first, in source order, then the record
                let base = spread.as_ref().map(|b| self.fixed(b));
                let vals: Vec<(u32, String)> = given.iter().map(|(i, f)| (*i, self.fixed(f))).collect();
                let t = self.temp(Q::Rec(*def));
                if texty(self.p, *def) {
                    // built in a scratch sharing the parts' references, then references of the temporary's own
                    let n = def.0;
                    let mut s = format!("{{ cpl_r{n} qs_ = {};", base.unwrap_or_else(|| format!("cpl_z{n}")));
                    for (i, v) in vals {
                        let _ = write!(s, " qs_.f{i} = {v};");
                    }
                    let _ = write!(s, " cpl_ret{n}(&qs_); cpl_rel{n}(&{t}); {t} = qs_; }}");
                    self.line(&s);
                    return t;
                }
                match base {
                    Some(b) => self.line(&format!("{t} = {b};")),
                    None => self.line(&format!("{t} = cpl_z{};", def.0)),
                }
                for (i, v) in vals {
                    self.line(&format!("{t}.f{i} = {v};"));
                }
                t
            }
            ExprKind::Unary { op, operand } => {
                let a = self.fixed(operand);
                match op {
                    UnaryOp::Not => format!("(!{a})"),
                    UnaryOp::Neg => {
                        let t = self.temp(Q::Int);
                        self.line(&format!("if ({a} == INT64_MIN) goto bail;"));
                        self.line(&format!("{t} = -{a};"));
                        t
                    }
                }
            }
            ExprKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs, e),
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => {
                let c = self.fixed(cond);
                let q = qty(self.p, &e.ty);
                let t = q.map(|q| self.temp(q));
                self.line(&format!("if ({c}) {{"));
                self.indent += 1;
                let v = self.block(then_block);
                if let (Some(t), Some(v), Some(q)) = (&t, v, q) {
                    self.assign(t, q, &v);
                }
                self.indent -= 1;
                if let Some(b) = else_block {
                    self.line("} else {");
                    self.indent += 1;
                    let v = self.block(b);
                    if let (Some(t), Some(v), Some(q)) = (&t, v, q) {
                        self.assign(t, q, &v);
                    }
                    self.indent -= 1;
                }
                self.line("}");
                t.unwrap_or_else(|| "0".to_string())
            }
            ExprKind::Match { scrutinee, arms } => {
                let sq = self.q(&scrutinee.ty);
                let st = self.fixed(scrutinee);
                let q = qty(self.p, &e.ty);
                let t = q.map(|q| self.temp(q));
                let mut first = true;
                for arm in arms {
                    let (cond, binds) = self.arm_test(&arm.pattern, &st, sq);
                    self.line(&format!("{}if ({cond}) {{", if first { "" } else { "else " }));
                    first = false;
                    self.indent += 1;
                    for (local, v) in binds {
                        let lq = self.locals.get(&local).copied().unwrap_or(Q::Int);
                        let dst = format!("q{}", local.0);
                        match (sq, lq) {
                            // a word's payload: an Int (a bail on a bigint), a Bool, or a word, borrowed
                            (Q::Word, Q::Int) => {
                                self.line(&format!("if (!cplrt_is_small({v})) goto bail;"));
                                self.assign(&dst, lq, &format!("cplrt_small_value({v})"));
                            }
                            (Q::Word, Q::Bool) => self.assign(&dst, lq, &format!("cplrt_truth({v})")),
                            _ => self.assign(&dst, lq, &v),
                        }
                    }
                    let v = self.block(&arm.body);
                    if let (Some(t), Some(v), Some(q)) = (&t, v, q) {
                        self.assign(t, q, &v);
                    }
                    self.indent -= 1;
                    self.line("}");
                }
                // (no arm at all: a match of an enum without variants, a value no caller can have)
                self.line(if first { "goto bail;" } else { "else goto bail;" });
                t.unwrap_or_else(|| "0".to_string())
            }
            ExprKind::Call { callee, args, .. } => {
                if let Some(which) = builtin(self.p, *callee) {
                    if let Some(r) = self.flat_read(which, args) {
                        return r;
                    }
                    return match which {
                        "option_unwrap_or" if nested_read(self.p, &args[0]) => {
                            self.list_element_or(&args[0], &args[1])
                        }
                        // a default that cannot fault is evaluated only when it is the value (the generic form, which
                        // evaluates it always, computes the same thing)
                        "option_unwrap_or" if cpl_hir::scalar::cannot_fault(&args[1]) => {
                            let o = self.fixed(&args[0]);
                            let q = self.q(&e.ty);
                            let t = self.temp(q);
                            self.line(&format!("if ({o}.has) {{"));
                            self.indent += 1;
                            match q {
                                // the option temporary's record moved out of it
                                Q::Rec(r) if texty(self.p, r) => self.line(&format!(
                                    "cpl_rel{n}(&{t}); {t} = {o}.v; {o}.v = cpl_z{n};",
                                    n = r.0
                                )),
                                _ => self.assign(&t, q, &format!("{o}.v")),
                            }
                            self.indent -= 1;
                            self.line("} else {");
                            self.indent += 1;
                            let d = self.expr(&args[1]);
                            self.assign(&t, q, &d);
                            self.indent -= 1;
                            self.line("}");
                            t
                        }
                        "option_unwrap_or" => {
                            let o = self.fixed(&args[0]);
                            let d = self.fixed(&args[1]);
                            format!("({o}.has ? {o}.v : {d})")
                        }
                        "option_is_some" | "option_is_none" if self.q(&args[0].ty) == Q::Word => {
                            let o = self.fixed(&args[0]);
                            let some = self.some_variant();
                            let op = if which == "option_is_some" { "==" } else { "!=" };
                            format!("(cplrt_variant({o}) {op} {some}u)")
                        }
                        "option_is_some" => {
                            let o = self.fixed(&args[0]);
                            format!("({o}.has != 0)")
                        }
                        "option_is_none" => {
                            let o = self.fixed(&args[0]);
                            format!("({o}.has == 0)")
                        }
                        "text_len" => {
                            let x = self.list_operand(&args[0], &[]);
                            format!("((int64_t)cplrt_text_of({x})->scalars)")
                        }
                        "text_is_empty" => {
                            let x = self.list_operand(&args[0], &[]);
                            format!("(cplrt_text_of({x})->bytes == 0u)")
                        }
                        "int_to_text" => {
                            let v = self.fixed(&args[0]);
                            let t = self.temp(Q::Text);
                            self.line(&format!("if (cplrt_q_int_text(cx, {v}, &{t}) != CPLRT_OK) goto bail;"));
                            t
                        }
                        which => self.list_builtin(which, e, args),
                    };
                }
                let refs: Vec<&Expr> = args.iter().collect();
                let mut vals = Vec::new();
                for (i, a) in args.iter().enumerate() {
                    // a local the callee borrows, as it is (no reference taken for a list, a text, a record's texts)
                    let v = match self.q(&a.ty) {
                        Q::List(_) | Q::Text | Q::Rec(_) => self.list_operand(a, &refs[i + 1..]),
                        _ => self.fixed(a),
                    };
                    vals.push(v);
                }
                if self.plan.fns.get(callee) == Some(&true) {
                    let m = if *callee == self.def && self.measured && !self.in_contract {
                        "mself"
                    } else {
                        "CPLRT_Q_NONE"
                    };
                    vals.push(m.to_string());
                }
                let rq = self.q(&e.ty);
                self.next += 1;
                let r = format!("qr{}", self.next);
                self.decls.push((ret_cty(rq), r.clone(), "{ 0 }".to_string()));
                self.line(&format!(
                    "{r} = cpl_q{}(cx, qd + 1u{});",
                    callee.0,
                    vals.iter().map(|v| format!(", {v}")).collect::<String>()
                ));
                self.line(&format!("if (!{r}.ok) goto bail;"));
                match rq {
                    Q::Bool => format!("((int){r}.v)"),
                    Q::List(_) | Q::Text => {
                        // the callee's reference is this temporary's to release
                        let t = self.temp(rq);
                        self.line(&format!("CPLRT_SET({t}, {r}.v); {r}.v = 0;"));
                        t
                    }
                    // likewise the references of a record's texts: moved, not taken again
                    Q::Rec(d) if texty(self.p, d) => {
                        let t = self.temp(rq);
                        self.line(&format!("cpl_rel{n}(&{t}); {t} = {r}.v; {r}.v = cpl_z{n};", n = d.0));
                        t
                    }
                    _ => format!("{r}.v"),
                }
            }
            // excluded by local_check
            _ => {
                self.line("goto bail;");
                "0".to_string()
            }
        }
    }

    /// The test of a match arm against the scrutinee `st` of type `sq`, and the bindings it makes.
    fn arm_test(&mut self, pat: &Pattern, st: &str, sq: Q) -> (String, Vec<(LocalId, String)>) {
        // a word: its variant, its payloads bound (`Check` allows nothing else)
        if let Q::Word = sq {
            return match &pat.kind {
                PatternKind::Ctor { variant, args, .. } => {
                    let binds = args
                        .iter()
                        .enumerate()
                        .filter_map(|(i, a)| match a.kind {
                            PatternKind::Bind(l) => Some((l, format!("cplrt_payload({st}, {i}u)"))),
                            _ => None,
                        })
                        .collect();
                    (format!("cplrt_variant({st}) == {variant}u"), binds)
                }
                PatternKind::Bind(l) => ("1".to_string(), vec![(*l, st.to_string())]),
                _ => ("1".to_string(), Vec::new()),
            };
        }
        if let Q::Enum(_) = sq {
            return match &pat.kind {
                PatternKind::Ctor { variant, args, .. } => {
                    let mut conds = vec![format!("{st}.tag == {variant}u")];
                    let mut binds = Vec::new();
                    for (i, a) in args.iter().enumerate() {
                        let part = format!("{st}.v{variant}_{i}");
                        match &a.kind {
                            PatternKind::Bind(l) => binds.push((*l, part)),
                            PatternKind::Int(n) => {
                                if let Some(v) = small_literal(n) {
                                    conds.push(format!("{part} == INT64_C({v})"));
                                }
                            }
                            PatternKind::Bool(b) => conds.push(format!("{part} == {}", i32::from(*b))),
                            _ => {}
                        }
                    }
                    (conds.join(" && "), binds)
                }
                PatternKind::Bind(l) => ("1".to_string(), vec![(*l, st.to_string())]),
                _ => ("1".to_string(), Vec::new()),
            };
        }
        let (cond, bind) = self.arm_test0(pat, st, sq);
        (cond, bind.into_iter().collect())
    }

    /// `arm_test` for a scrutinee that is not an enum: at most one binding.
    fn arm_test0(&mut self, pat: &Pattern, st: &str, sq: Q) -> (String, Option<(LocalId, String)>) {
        let plan = self.plan;
        let lit = |k: &PatternKind, part: &str| -> Option<String> {
            match k {
                PatternKind::Int(n) => small_literal(n).map(|v| format!("{part} == INT64_C({v})")),
                PatternKind::Bool(b) => Some(format!("{part} == {}", i32::from(*b))),
                PatternKind::Text(t) => Some(format!("cplrt_text_eq({part}, {})", plan.text_k(t))),
                _ => None,
            }
        };
        if let Q::Opt(_) = sq {
            return match &pat.kind {
                PatternKind::Ctor { variant: 0, args, .. } => {
                    let inner = args.first().map(|a| &a.kind);
                    match inner {
                        Some(PatternKind::Bind(l)) => (format!("{st}.has"), Some((*l, format!("{st}.v")))),
                        Some(k) => match lit(k, &format!("{st}.v")) {
                            Some(c) => (format!("{st}.has && {c}"), None),
                            None => (format!("{st}.has"), None),
                        },
                        None => (format!("{st}.has"), None),
                    }
                }
                PatternKind::Ctor { .. } => (format!("!{st}.has"), None),
                _ => ("1".to_string(), None),
            };
        }
        match &pat.kind {
            PatternKind::Bind(l) => ("1".to_string(), Some((*l, st.to_string()))),
            k => (lit(k, st).unwrap_or_else(|| "1".to_string()), None),
        }
    }

    fn binary(&mut self, op: BinaryOp, lhs: &Expr, rhs: &Expr, e: &Expr) -> String {
        if matches!(op, BinaryOp::And | BinaryOp::Or) {
            let a = self.expr(lhs);
            let t = self.temp(Q::Bool);
            self.line(&format!("{t} = {a};"));
            self.line(&if op == BinaryOp::And {
                format!("if ({t}) {{")
            } else {
                format!("if (!{t}) {{")
            });
            self.indent += 1;
            let b = self.expr(rhs);
            self.line(&format!("{t} = {b};"));
            self.indent -= 1;
            self.line("}");
            return t;
        }
        if op == BinaryOp::Concat {
            let a = self.list_operand(lhs, &[rhs]);
            let b = self.list_operand(rhs, &[]);
            let q = self.q(&e.ty);
            let t = self.temp(q);
            let f = if q == Q::Text { "cplrt_q_text_concat" } else { "cplrt_q_concat" };
            self.line(&format!("if ({f}(cx, {a}, {b}, &{t}) != CPLRT_OK) goto bail;"));
            return t;
        }
        let at = self.fixed(lhs);
        let bt = self.fixed(rhs);
        let rel = |r: &str| format!("({at} {r} {bt})");
        match op {
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul => {
                let f = match op {
                    BinaryOp::Add => "cplrt_q_add",
                    BinaryOp::Sub => "cplrt_q_sub",
                    _ => "cplrt_q_mul",
                };
                let t = self.temp(Q::Int);
                self.line(&format!("if ({f}({at}, {bt}, &{t})) goto bail;"));
                t
            }
            BinaryOp::Div | BinaryOp::Rem => {
                let t = self.temp(Q::Int);
                self.line(&format!(
                    "if ({bt} == 0 || ({at} == INT64_MIN && {bt} == -1)) goto bail;"
                ));
                let o = if op == BinaryOp::Div { "/" } else { "%" };
                self.line(&format!("{t} = {at} {o} {bt};"));
                t
            }
            BinaryOp::Eq => equal(self.p, self.q(&lhs.ty), &at, &bt),
            BinaryOp::Ne => format!("(!{})", equal(self.p, self.q(&lhs.ty), &at, &bt)),
            BinaryOp::Lt => rel("<"),
            BinaryOp::Le => rel("<="),
            BinaryOp::Gt => rel(">"),
            BinaryOp::Ge => rel(">="),
            // excluded by local_check
            _ => {
                self.line("goto bail;");
                "0".to_string()
            }
        }
    }
}

