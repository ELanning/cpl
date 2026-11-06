//! The Rust host's scalar tier (`spec/hosts.md` H14): for every function of the scalar subset (`cpl_hir::scalar`, with
//! `Caps::NATIVE`), a form `<name>__q` on `i64`, `bool`, records (`Copy` unless they hold texts), `cpl::Text`, enums,
//! the generic form's own lists (`cpl::List<cpl::Int>`, of `bool`, of records, of lists; elements read in place), and
//! `Option`s of a non-list element, that returns
//! `None` ("bails") wherever the generic form could do anything but compute the same value (an `i64` overflow, a
//! division by zero, a contract, assertion, or measure that does not hold, the depth limit, the stack floor). The
//! generic `<name>__x` tries it first and, on `None`, runs itself, which then raises exactly what it always did: the
//! function is pure, so running it again computes the same thing.

use cpl_hir::scalar::{builtin, fields, of_el, small_literal, variant_fields, El, Plan, Q};
use cpl_hir::{Block, DefId, DefKind, Expr, ExprKind, FnBody, Lit, PatternKind, Program, Stmt, Ty};
use cpl_syntax::ast::{BinaryOp, UnaryOp};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// The functions with a speculative form, and the records and enums they use.
pub(crate) struct RustQ {
    plan: Plan,
    /// The generated item of each record and enum of the plan, set once paths exist.
    pub(crate) gen: BTreeMap<DefId, String>,
    /// The generated variant of each variant of an enum of the plan.
    pub(crate) variants: BTreeMap<(DefId, u32), String>,
}

const CAPS: cpl_hir::scalar::Caps = cpl_hir::scalar::Caps::HOSTS;

fn qty(p: &Program, ty: &Ty) -> Option<Q> {
    cpl_hir::scalar::qty(p, ty, CAPS)
}

/// Whether the record `d` holds texts, at any depth (it is then `Clone`, not `Copy`).
fn texty(p: &Program, d: DefId) -> bool {
    fields(p, d).iter().any(|f| match f {
        Q::Text => true,
        Q::Rec(n) => texty(p, *n),
        _ => false,
    })
}

/// Whether a value of `q` is `Copy` in its speculative form: else a copy of it is a `clone()`.
fn copyable(p: &Program, q: Q) -> bool {
    match q {
        Q::Int | Q::Bool | Q::Enum(_) => true,
        Q::Rec(d) => !texty(p, d),
        Q::Opt(el) => copyable(p, of_el(el)),
        Q::List(_) | Q::Text => false,
        Q::Word => unreachable!("Caps::HOSTS has no words"),
    }
}

impl RustQ {
    fn rty(&self, q: Q) -> String {
        match q {
            Q::Int => "i64".to_string(),
            Q::Bool => "bool".to_string(),
            Q::Text => "cpl::Text".to_string(),
            Q::Rec(d) => format!("crate::__q::R{}", d.0),
            Q::Enum(d) => format!("crate::__q::E{}", d.0),
            Q::List(El::Bool) => "cpl::List<bool>".to_string(),
            Q::List(El::Rec(d)) => format!("cpl::List<{}>", self.gen[&d]),
            Q::List(El::List(l)) => format!("cpl::List<{}>", self.rty(Q::List(cpl_hir::scalar::el_of_leaf(l)))),
            Q::List(El::Int) => "cpl::List<cpl::Int>".to_string(),
            Q::Opt(el) => format!("Option<{}>", self.rty(of_el(el))),
            Q::Word => unreachable!("Caps::HOSTS has no words"),
        }
    }
}

impl RustQ {
    /// The declaration of local `q<n>` of type `q`: a list is left unset (every path sets it before reading it), so
    /// that a call makes no empty list it does not use.
    fn decl(&self, n: u32, q: Q) -> String {
        match q {
            Q::List(_) => format!("let mut q{n}: {};", self.rty(q)),
            _ => format!("let mut q{n}: {} = {};", self.rty(q), self.zero(q)),
        }
    }

    /// An element of a list of `el` as the list holds it, from its speculative value `v`.
    fn word(&self, el: El, v: &str) -> String {
        match el {
            El::Bool | El::List(_) => v.to_string(),
            El::Rec(d) => format!("crate::__q::box_{}({v})", d.0),
            El::Int => format!("cpl::Int::from_i64({v})"),
        }
    }

    fn zero(&self, q: Q) -> String {
        match q {
            Q::Int => "0i64".to_string(),
            Q::Bool => "false".to_string(),
            Q::Text => "cpl::Text::default()".to_string(),
            Q::Rec(d) => format!("crate::__q::R{}::default()", d.0),
            Q::Enum(d) => format!("crate::__q::E{}::default()", d.0),
            Q::List(_) => "cpl::List::new()".to_string(),
            Q::Opt(_) => "None".to_string(),
            Q::Word => unreachable!("Caps::HOSTS has no words"),
        }
    }
}

impl RustQ {
    pub(crate) fn new(p: &Program) -> RustQ {
        RustQ {
            plan: cpl_hir::scalar::plan(p, CAPS),
            gen: BTreeMap::new(),
            variants: BTreeMap::new(),
        }
    }

    pub(crate) fn has(&self, d: DefId) -> bool {
        self.plan.fns.contains_key(&d)
    }

    /// The records and enums whose generated paths the tier needs.
    pub(crate) fn named(&self) -> Vec<DefId> {
        self.plan.records.iter().chain(&self.plan.enums).copied().collect()
    }

    /// The `__q` module: each record as a `Copy` struct, with its conversions from and to the generated struct
    /// (`path` names a definition's generated item).
    pub(crate) fn module(&self, p: &Program, path: &dyn Fn(DefId) -> String) -> String {
        let mut s = String::from("\n#[doc(hidden)]\npub mod __q {\n    use crate::cpl;\n");
        for r in &self.plan.records {
            let fs = fields(p, *r);
            let names: Vec<String> = match &p.def(*r).kind {
                DefKind::Struct(st) => st.fields.iter().map(|f| crate::rust::ident(&f.name)).collect(),
                _ => Vec::new(),
            };
            let members: String = fs.iter().enumerate().map(|(i, f)| format!(" pub f{i}: {},", self.rty(*f))).collect();
            let _ = writeln!(
                s,
                "    #[derive(Clone, {}PartialEq, Eq, Default, Debug)]\n    pub struct R{} {{{members} }}",
                if texty(p, *r) { "" } else { "Copy, " },
                r.0
            );
            let gen = path(*r);
            // from the generated struct: None when a field is a bigint
            let unbox: Vec<String> = fs
                .iter()
                .zip(&names)
                .enumerate()
                .map(|(i, (f, n))| match f {
                    Q::Int => format!("f{i}: v.{n}.to_i64()?"),
                    Q::Bool => format!("f{i}: v.{n}"),
                    Q::Rec(m) => format!("f{i}: unbox_{}(&v.{n})?", m.0),
                    Q::Text => format!("f{i}: v.{n}.clone()"),
                    _ => format!("f{i}: Default::default()"),
                })
                .collect();
            let _ = writeln!(
                s,
                "    #[inline]\n    pub fn unbox_{n}(v: &{gen}) -> Option<R{n}> {{\n        Some(R{n} {{ {u} }})\n    }}",
                n = r.0,
                u = unbox.join(", ")
            );
            let boxed: Vec<String> = fs
                .iter()
                .zip(&names)
                .enumerate()
                .map(|(i, (f, n))| match f {
                    Q::Int => format!("{n}: cpl::Int::from_i64(v.f{i})"),
                    Q::Bool => format!("{n}: v.f{i}"),
                    Q::Rec(m) => format!("{n}: box_{}(v.f{i})", m.0),
                    Q::Text => format!("{n}: v.f{i}"),
                    _ => format!("{n}: Default::default()"),
                })
                .collect();
            let _ = writeln!(
                s,
                "    #[inline]\n    pub fn box_{n}(v: R{n}) -> {gen} {{\n        {gen} {{ {b} }}\n    }}",
                n = r.0,
                b = boxed.join(", ")
            );
        }
        for d in &self.plan.enums {
            s.push_str(&self.enum_type(p, *d));
        }
        s.push_str("}\n");
        s
    }

    /// An enum of the speculative forms: a `Copy` Rust enum of the same variants with `i64` and `bool` payloads,
    /// and its conversions from and to the generated enum.
    fn enum_type(&self, p: &Program, d: DefId) -> String {
        let n = d.0;
        let nv = p.def(d).kind.as_enum().map(|e| e.variants.len()).unwrap_or(0) as u32;
        let gen = &self.gen[&d];
        let mut s = String::new();
        let mut vs = Vec::new();
        for v in 0..nv {
            let fs = variant_fields(p, d, v);
            if fs.is_empty() {
                vs.push(format!("V{v}"));
            } else {
                vs.push(format!("V{v}({})", fs.iter().map(|f| self.rty(*f)).collect::<Vec<_>>().join(", ")));
            }
        }
        let _ = writeln!(s, "    #[derive(Clone, Copy, PartialEq, Eq, Debug)]\n    pub enum E{n} {{ {} }}", vs.join(", "));
        let first: Vec<String> = variant_fields(p, d, 0).iter().map(|f| self.zero(*f)).collect();
        let _ = writeln!(
            s,
            "    impl Default for E{n} {{\n        fn default() -> E{n} {{\n            E{n}::V0{}\n        }}\n    }}",
            if first.is_empty() { String::new() } else { format!("({})", first.join(", ")) }
        );
        let mut unbox = String::new();
        let mut boxed = String::new();
        for v in 0..nv {
            let fs = variant_fields(p, d, v);
            let gv = &self.variants[&(d, v)];
            if fs.is_empty() {
                let _ = write!(unbox, " {gv} => E{n}::V{v},");
                let _ = write!(boxed, " E{n}::V{v} => {gv},");
                continue;
            }
            let xs: Vec<String> = (0..fs.len()).map(|i| format!("x{i}")).collect();
            let from: Vec<String> = fs
                .iter()
                .enumerate()
                .map(|(i, f)| if *f == Q::Int { format!("x{i}.to_i64()?") } else { format!("*x{i}") })
                .collect();
            let to: Vec<String> = fs
                .iter()
                .enumerate()
                .map(|(i, f)| if *f == Q::Int { format!("cpl::Int::from_i64(x{i})") } else { format!("x{i}") })
                .collect();
            let _ = write!(unbox, " {gv}({}) => E{n}::V{v}({}),", xs.join(", "), from.join(", "));
            let _ = write!(boxed, " E{n}::V{v}({}) => {gv}({}),", xs.join(", "), to.join(", "));
        }
        let _ = writeln!(s, "    #[inline]\n    pub fn unboxe_{n}(v: &{gen}) -> Option<E{n}> {{\n        Some(match v {{{unbox} }})\n    }}");
        let _ = writeln!(s, "    #[inline]\n    pub fn boxe_{n}(v: E{n}) -> {gen} {{\n        match v {{{boxed} }}\n    }}");
        s
    }

    /// The lines that open `<name>__x`: the speculative call, its result converted on success; on `None` the
    /// generic form follows. `params` are the Rust names of `__x`'s parameters, `id` the function's measure id.
    pub(crate) fn wrapper(&self, p: &Program, d: DefId, params: &[String], id: u32, path: &dyn Fn(DefId) -> String) -> String {
        let fd = p.def(d).kind.as_fn().expect("a function");
        let mut s = String::from("    if cx.q_ok() {\n        'q: {\n");
        let mut args = Vec::new();
        for (i, (param, v)) in fd.params.iter().zip(params).enumerate() {
            match qty(p, &param.ty) {
                Some(Q::Int) => {
                    let _ = writeln!(s, "            let Some(a{i}) = {v}.to_i64() else {{ break 'q }};");
                }
                Some(Q::Bool) => {
                    let _ = writeln!(s, "            let a{i} = {v};");
                }
                Some(Q::Rec(r)) if texty(p, r) => {
                    let _ = writeln!(s, "            let Some(a{i}) = crate::__q::unbox_{}(&{v}) else {{ break 'q }};", r.0);
                    let _ = writeln!(s, "            let a{i} = &a{i};");
                }
                Some(Q::Rec(r)) => {
                    let _ = writeln!(s, "            let Some(a{i}) = crate::__q::unbox_{}(&{v}) else {{ break 'q }};", r.0);
                }
                // borrowed: the speculative form takes a reference (no count taken)
                Some(Q::List(_) | Q::Text) => {
                    let _ = writeln!(s, "            let a{i} = &{v};");
                }
                Some(Q::Enum(r)) => {
                    let _ = writeln!(s, "            let Some(a{i}) = crate::__q::unboxe_{}(&{v}) else {{ break 'q }};", r.0);
                }
                _ => return String::new(),
            }
            args.push(format!("a{i}"));
        }
        if self.plan.fns[&d] {
            args.push(format!("cx.q_prev({id})"));
        }
        let call = format!("{}__q(cx, cx.depth(){})", path(d), args.iter().map(|a| format!(", {a}")).collect::<String>());
        let boxed = match qty(p, &fd.ret) {
            Some(Q::Int) => "cpl::Int::from_i64(r)".to_string(),
            Some(Q::Rec(r)) => format!("crate::__q::box_{}(r)", r.0),
            Some(Q::Enum(r)) => format!("crate::__q::boxe_{}(r)", r.0),
            _ => "r".to_string(),
        };
        let _ = writeln!(
            s,
            "            match {call} {{\n                Some(r) => return Ok({boxed}),\n                None => cx.q_bail(),\n            }}"
        );
        s.push_str("        }\n    }\n");
        s
    }

    /// `<name>__q` (contracts and measure) and `<name>__qb` (the body), placed beside the generic forms.
    pub(crate) fn functions(&self, p: &Program, d: DefId, path: &dyn Fn(DefId) -> String) -> String {
        let fd = p.def(d).kind.as_fn().expect("a function");
        let FnBody::Block(body) = &fd.body else {
            return String::new();
        };
        let name = crate::rust::ident(&p.def(d).name.display());
        let measured = self.plan.fns[&d];
        let rq = qty(p, &fd.ret).expect("a scalar result");
        let params: Vec<String> = fd
            .params
            .iter()
            .map(|x| {
                // a value that is not `Copy` is borrowed: the caller keeps its counts
                let q = qty(p, &x.ty).expect("a scalar parameter");
                let r = if copyable(p, q) { "" } else { "&" };
                format!("q{}: {r}{}", x.local.0, self.rty(q))
            })
            .collect();
        let refs: BTreeSet<cpl_hir::LocalId> = fd
            .params
            .iter()
            .filter(|x| !copyable(p, qty(p, &x.ty).unwrap_or(Q::Int)))
            .map(|x| x.local)
            .collect();
        let pnames: Vec<String> = fd.params.iter().map(|x| format!("q{}", x.local.0)).collect();
        let flat = cpl_hir::scalar::flat_locals(p, fd, CAPS);
        // the list locals built as a `Vec` and then made lists (cpl_hir::scalar::built_locals)
        let built = cpl_hir::scalar::built_locals(p, fd, CAPS);
        let mut e = Printer {
            p,
            rq: self,
            refs,
            flat: flat.clone(),
            built: built.clone(),
            frozen: BTreeSet::new(),
            def: d,
            measured,
            in_contract: true,
            unit: p.std_defs.unit,
            path,
        };
        let mut s = String::new();
        // the contracts and the measure
        let _ = writeln!(
            s,
            "#[doc(hidden)]\n#[inline]\npub(crate) fn {name}__q(cx: &mut cpl::Cx, qd: u32{}{}) -> Option<{}> {{\n    if !cx.q_enter(qd) {{\n        return None;\n    }}",
            params.iter().map(|x| format!(", mut {x}")).collect::<String>(),
            if measured { ", mprev: i64" } else { "" },
            self.rty(rq)
        );
        // a contract expression may bind locals of its own (a `match`)
        let pset: BTreeSet<_> = fd.params.iter().map(|x| x.local).collect();
        for l in &fd.locals {
            if !pset.contains(&l.id) && Some(l.id) != fd.result_local {
                if let Some(el) = flat.get(&l.id) {
                    let _ = writeln!(s, "    let mut q{}: Vec<{}> = Vec::new();", l.id.0, self.rty(of_el(*el)));
                } else if let Some(q) = qty(p, &l.ty) {
                    if let Some(b) = built.get(&l.id) {
                        // an `Int` list built as the generic form holds it (made a list in place)
                        let t = if b.el == El::Int { "cpl::Int".to_string() } else { self.rty(of_el(b.el)) };
                        let _ = writeln!(s, "    let mut qf{}: Vec<{t}> = Vec::new();", l.id.0);
                    }
                    let _ = writeln!(s, "    {}", self.decl(l.id.0, q));
                }
            }
        }
        for c in &fd.requires {
            let v = e.expr(&c.expr);
            let _ = writeln!(s, "    if !({v}) {{\n        return None;\n    }}");
        }
        let mut body_args = pnames.clone();
        if let Some(c) = &fd.decreases {
            let v = e.expr(&c.expr);
            let _ = writeln!(
                s,
                "    let mself: i64 = {v};\n    if mself < 0 || (mprev != i64::MIN && mself >= mprev) {{\n        return None;\n    }}"
            );
            body_args.push("mself".to_string());
        }
        if fd.ensures.is_empty() {
            // nothing runs after the body: it follows in place
            e.in_contract = false;
            let b = e.block_body(body);
            s.push_str(&b);
            s.push_str("}\n\n");
            return s;
        }
        let _ = writeln!(
            s,
            "    let r = {name}__qb(cx, qd{})?;",
            body_args.iter().map(|x| format!(", {x}")).collect::<String>()
        );
        if !fd.ensures.is_empty() {
            // the measure is recorded by now (S35): a call of this function compares with it
            e.in_contract = false;
            if let Some(res) = fd.result_local {
                let c = if copyable(p, rq) { "" } else { ".clone()" };
                let _ = writeln!(s, "    let q{} = r{c};", res.0);
            }
            for c in &fd.ensures {
                let v = e.expr(&c.expr);
                let _ = writeln!(s, "    if !({v}) {{\n        return None;\n    }}");
            }
        }
        s.push_str("    Some(r)\n}\n\n");
        // the body
        e.in_contract = false;
        let _ = writeln!(
            s,
            "#[doc(hidden)]\n#[inline]\npub(crate) fn {name}__qb(cx: &mut cpl::Cx, qd: u32{}{}) -> Option<{}> {{",
            params.iter().map(|x| format!(", mut {x}")).collect::<String>(),
            if measured { ", mself: i64" } else { "" },
            self.rty(rq)
        );
        for l in &fd.locals {
            if !pset.contains(&l.id) {
                if let Some(el) = flat.get(&l.id) {
                    let _ = writeln!(s, "    let mut q{}: Vec<{}> = Vec::new();", l.id.0, self.rty(of_el(*el)));
                } else if let Some(q) = qty(p, &l.ty) {
                    if let Some(b) = built.get(&l.id) {
                        // an `Int` list built as the generic form holds it (made a list in place)
                        let t = if b.el == El::Int { "cpl::Int".to_string() } else { self.rty(of_el(b.el)) };
                        let _ = writeln!(s, "    let mut qf{}: Vec<{t}> = Vec::new();", l.id.0);
                    }
                    let _ = writeln!(s, "    {}", self.decl(l.id.0, q));
                }
            }
        }
        let b = e.block_body(body);
        s.push_str(&b);
        s.push_str("}\n\n");
        s
    }
}

/// The Rust printer of one speculative form: HIR expressions as Rust expressions (`?` bails).
struct Printer<'a> {
    p: &'a Program,
    rq: &'a RustQ,
    /// The parameters taken by reference (their values are not `Copy`).
    refs: BTreeSet<cpl_hir::LocalId>,
    /// The list locals held as a `Vec` of their elements (cpl_hir::scalar::flat_locals).
    flat: BTreeMap<cpl_hir::LocalId, El>,
    /// The list locals built as a `Vec` `qf<n>`, then made the lists `q<n>` (those of `frozen` by now).
    built: BTreeMap<cpl_hir::LocalId, cpl_hir::scalar::Built>,
    frozen: BTreeSet<cpl_hir::LocalId>,
    def: DefId,
    measured: bool,
    in_contract: bool,
    unit: DefId,
    path: &'a dyn Fn(DefId) -> String,
}

impl Printer<'_> {
    fn q(&self, ty: &Ty) -> Option<Q> {
        qty(self.p, ty)
    }

    /// A function body: its statements, then `Some(tail)` (or nothing past a final `return`).
    /// The `Vec` holding list local `l`, its element, and whether it holds them as the generic form does (a built
    /// list of `Int`: `cpl::Int`s, made a list without a copy), while it is one.
    fn flat_var(&self, l: cpl_hir::LocalId) -> Option<(El, String, bool)> {
        if let Some(el) = self.flat.get(&l) {
            return Some((*el, format!("q{}", l.0), false));
        }
        let b = self.built.get(&l)?;
        (!self.frozen.contains(&l)).then(|| (b.el, format!("qf{}", l.0), b.el == El::Int))
    }

    /// The function's body: each built list made from its `Vec` before the statement that first reads it as a list.
    fn block_body(&mut self, b: &Block) -> String {
        let mut s = String::new();
        let built: Vec<(cpl_hir::LocalId, cpl_hir::scalar::Built)> = self.built.iter().map(|(l, x)| (*l, *x)).collect();
        for i in 0..=b.stmts.len() {
            for (l, x) in built.iter().filter(|(_, x)| x.at == i) {
                let n = l.0;
                let made = match x.el {
                    El::Rec(d) => format!(
                        "cpl::List::from_vec(std::mem::take(&mut qf{n}).into_iter().map(crate::__q::box_{}).collect())",
                        d.0
                    ),
                    _ => format!("cpl::List::from_vec(std::mem::take(&mut qf{n}))"),
                };
                let _ = writeln!(s, "    q{n} = {made};");
                self.frozen.insert(*l);
            }
            if let Some(st) = b.stmts.get(i) {
                s.push_str("    ");
                s.push_str(&self.stmt(st));
                s.push('\n');
            }
        }
        match &b.tail {
            Some(t) if self.q(&t.ty).is_some() => {
                // a local read last: moved out, not cloned (a parameter taken by reference is cloned)
                let v = match &t.kind {
                    ExprKind::Local(id) if !self.refs.contains(id) => format!("q{}", id.0),
                    _ => self.expr(t),
                };
                let _ = writeln!(s, "    Some({v})");
            }
            Some(t) => {
                let v = self.expr(t);
                let _ = writeln!(s, "    let _ = {v};\n    #[allow(unreachable_code)]\n    None");
            }
            None => s.push_str("    #[allow(unreachable_code)]\n    None\n"),
        }
        s
    }

    /// A block as a Rust block expression.
    fn block(&mut self, b: &Block) -> String {
        let mut s = String::from("{ ");
        for st in &b.stmts {
            s.push_str(&self.stmt(st));
            s.push(' ');
        }
        if let Some(t) = &b.tail {
            let v = self.expr(t);
            s.push_str(&v);
        }
        s.push_str(" }");
        s
    }

    fn stmt(&mut self, st: &Stmt) -> String {
        match st {
            // a `Vec`: `[]` empties it, its own push appends
            Stmt::Let { local, value } | Stmt::Assign { local, value } if self.flat_var(*local).is_some() => {
                let (_, q, generic) = self.flat_var(*local).expect("a Vec");
                match &value.kind {
                    ExprKind::Call { args, .. } if args.len() == 2 => {
                        let v = self.expr(&args[1]);
                        let w = if generic { "cpl::Int::from_i64(v_)" } else { "v_" };
                        format!("{{ let v_ = {v}; {q}.push({w}); }}")
                    }
                    _ => format!("{q}.clear();"),
                }
            }
            Stmt::Let { local, value } | Stmt::Assign { local, value } => {
                // `xs = xs.push(v)` and `xs = xs ++ ys` in place: the local's own list grows where nothing else holds it
                if let Some(s) = self.in_place(*local, value) {
                    return s;
                }
                let v = self.expr(value);
                format!("q{} = {v};", local.0)
            }
            Stmt::Discard(e) | Stmt::Expr(e) => {
                let v = self.expr(e);
                format!("let _ = {v};")
            }
            Stmt::Return(Some(e)) => {
                let v = match &e.kind {
                    ExprKind::Local(id) if !self.refs.contains(id) => format!("q{}", id.0),
                    _ => self.expr(e),
                };
                format!("return Some({v});")
            }
            Stmt::Assert { expr, .. } => {
                let v = self.expr(expr);
                format!("if !({v}) {{ return None; }}")
            }
            Stmt::While { cond, body, .. } => {
                let c = self.expr(cond);
                let b = self.block(body);
                format!("while {c} {{ let _ = {b}; }}")
            }
            Stmt::Return(None) | Stmt::Concurrent(_) => "return None;".to_string(),
        }
    }

    /// `local = local.push(v)` or `local = local ++ ys` as an update of the local's list.
    fn in_place(&mut self, local: cpl_hir::LocalId, value: &Expr) -> Option<String> {
        // `t = t ++ x` on a text local: appended in place where nothing else holds it
        if self.q(&value.ty) == Some(Q::Text) {
            // every part first (each sees the old text), then appended
            let parts = cpl_hir::scalar::text_append_chain(self.p, local, value)?;
            let xs: Vec<String> = parts.iter().map(|x| self.expr(x)).collect();
            let lets: String = xs.iter().enumerate().map(|(i, x)| format!("let x{i}_ = {x}; ")).collect();
            let appends: String = (0..xs.len()).map(|i| format!("q{}.append_mut(&x{i}_); ", local.0)).collect();
            return Some(format!("{{ {lets}{appends}}}"));
        }
        let el = match self.q(&value.ty) {
            Some(Q::List(el)) => el,
            _ => return None,
        };
        let is_local = |e: &Expr| matches!(e.kind, ExprKind::Local(l) if l == local);
        match &value.kind {
            ExprKind::Call { callee, args, .. }
                if builtin(self.p, *callee) == Some("list_push") && args.len() == 2 && is_local(&args[0]) =>
            {
                let v = self.expr(&args[1]);
                Some(format!("{{ let v_ = {v}; q{}.push_mut({}); }}", local.0, self.rq.word(el, "v_")))
            }
            ExprKind::Call { callee, args, .. }
                if matches!(builtin(self.p, *callee), Some("list_take" | "list_drop")) && args.len() == 2 && is_local(&args[0]) =>
            {
                let k = self.expr(&args[1]);
                let which = &builtin(self.p, *callee).unwrap_or("list_drop")[5..];
                Some(format!("{{ let k_ = {k}; cpl::q_{which}_mut(&mut q{}, k_)?; }}", local.0))
            }
            ExprKind::Call { callee, args, .. }
                if builtin(self.p, *callee) == Some("list_drop_last") && args.len() == 1 && is_local(&args[0]) =>
            {
                Some(format!("cpl::q_drop_last_mut(&mut q{});", local.0))
            }
            ExprKind::Binary { op: BinaryOp::Concat, lhs, rhs } if is_local(lhs) => {
                let b = self.borrowed(rhs);
                Some(format!("{{ let b_ = ({b}).clone(); q{}.append_mut(&b_); }}", local.0))
            }
            _ => None,
        }
    }

    /// A list operand only read: a `&cpl::List`.
    fn borrowed(&mut self, e: &Expr) -> String {
        match &e.kind {
            ExprKind::Local(id) => format!("&q{}", id.0),
            // a row of a list of lists or the empty list, read in place
            ExprKind::Call { callee, args, .. }
                if builtin(self.p, *callee) == Some("option_unwrap_or")
                    && args.len() == 2
                    && cpl_hir::scalar::nested_read(self.p, &args[0])
                    && matches!(&args[1].kind, ExprKind::List(items) if items.is_empty()) =>
            {
                let ExprKind::Call { callee: read, args: inner, .. } = &args[0].kind else {
                    unreachable!("a nested read is a call")
                };
                let xs = self.borrowed(&inner[0]);
                match builtin(self.p, *read) {
                    Some("list_get") => {
                        let i = self.expr(&inner[1]);
                        format!("{{ let k_ = {i}; cpl::q_row({xs}, k_) }}")
                    }
                    Some("list_first") => format!("cpl::q_row({xs}, 0)"),
                    _ => format!("{{ let l_ = {xs}; cpl::q_row(l_, l_.len() as i64 - 1) }}"),
                }
            }
            _ => {
                let v = self.expr(e);
                format!("&({v})")
            }
        }
    }

    /// A list or option builtin of the scalar subset.
    fn builtin(&mut self, which: &str, e: &Expr, args: &[Expr]) -> String {
        // a `Vec`: its length, or an element as an option (a copy)
        if let Some(ExprKind::Local(l)) = args.first().map(|a| &a.kind) {
            if let Some((el, q, generic)) = self.flat_var(*l) {
                let c = if generic {
                    ".to_i64()?"
                } else if copyable(self.p, of_el(el)) {
                    ""
                } else {
                    ".clone()"
                };
                let at = |i: &str| {
                    format!("{{ let k_ = {i}; if k_ >= 0 && (k_ as u64) < {q}.len() as u64 {{ Some({q}[k_ as usize]{c}) }} else {{ None }} }}")
                };
                return match which {
                    "list_len" => format!("({q}.len() as i64)"),
                    "list_is_empty" => format!("{q}.is_empty()"),
                    "list_get" => {
                        let i = self.expr(&args[1]);
                        at(&i)
                    }
                    "list_first" => at("0i64"),
                    _ => at(&format!("({q}.len() as i64 - 1)")),
                };
            }
        }
        let el = match args.first().and_then(|a| self.q(&a.ty)) {
            Some(Q::List(el) | Q::Opt(el)) => el,
            _ => El::Int,
        };
        let get = |xs: &str, i: &str| match el {
            El::Bool => format!("cpl::q_get_bool({xs}, {i})"),
            // a record element unboxed in place (None from unbox: a bail)
            El::Rec(d) => format!(
                "{{ let l_ = {xs}; let k_ = {i}; if k_ >= 0 && (k_ as u64) < l_.len() as u64 {{ Some(crate::__q::unbox_{}(&l_.as_slice()[k_ as usize])?) }} else {{ None }} }}",
                d.0
            ),
            _ => format!("cpl::q_get({xs}, {i})?"),
        };
        match which {
            "list_len" => format!("(({}).len() as i64)", self.borrowed(&args[0])),
            "list_is_empty" => format!("({}).is_empty()", self.borrowed(&args[0])),
            "list_get" => {
                let xs = self.borrowed(&args[0]);
                let i = self.expr(&args[1]);
                format!("{{ let i_ = {i}; {} }}", get(&xs, "i_"))
            }
            "list_first" => {
                let xs = self.borrowed(&args[0]);
                get(&xs, "0i64")
            }
            "list_last" => {
                let xs = self.borrowed(&args[0]);
                format!("{{ let l_ = {xs}; {} }}", get("l_", "(l_.len() as i64 - 1)"))
            }
            "list_push" => {
                let xs = self.expr(&args[0]);
                let v = self.expr(&args[1]);
                format!("{{ let mut l_ = {xs}; let v_ = {v}; l_.push_mut({}); l_ }}", self.rq.word(el, "v_"))
            }
            "list_take" | "list_drop" => {
                let xs = self.borrowed(&args[0]);
                let k = self.expr(&args[1]);
                format!("cpl::q_{}({xs}, {k})?", &which[5..])
            }
            "list_drop_last" => format!("cpl::q_drop_last({})", self.borrowed(&args[0])),
            // an element list of a list of lists (its own reference), or the default (the read's Option has no
            // scalar form); a default that cannot fault evaluated only when it is the value
            "option_unwrap_or" if cpl_hir::scalar::nested_read(self.p, &args[0]) => {
                let ExprKind::Call { callee, args: inner, .. } = &args[0].kind else {
                    unreachable!("a nested read is a call")
                };
                let xs = self.borrowed(&inner[0]);
                let i = match builtin(self.p, *callee) {
                    Some("list_get") => self.expr(&inner[1]),
                    Some("list_first") => "0i64".to_string(),
                    _ => "(l_.len() as i64 - 1)".to_string(),
                };
                let d = self.expr(&args[1]);
                // `[]` too: cpl::List::new() allocates, but nothing observable
                let lazy = cpl_hir::scalar::cannot_fault(&args[1])
                    || matches!(&args[1].kind, ExprKind::List(items) if items.is_empty());
                let (early, late) = if lazy {
                    (String::new(), d)
                } else {
                    (format!("let d_ = {d}; "), "d_".to_string())
                };
                format!(
                    "{{ let l_ = {xs}; let k_ = {i}; {early}if k_ >= 0 && (k_ as u64) < l_.len() as u64 {{ l_.as_slice()[k_ as usize].clone() }} else {{ {late} }} }}"
                )
            }
            // a default that cannot fault is evaluated only when it is the value
            "option_unwrap_or" if cpl_hir::scalar::cannot_fault(&args[1]) => {
                let o = self.expr(&args[0]);
                let d = self.expr(&args[1]);
                format!("(match {o} {{ Some(x_) => x_, None => {d} }})")
            }
            "option_unwrap_or" => {
                let o = self.expr(&args[0]);
                let d = self.expr(&args[1]);
                format!("({o}).unwrap_or({d})")
            }
            "text_len" => format!("(({}).chars().count() as i64)", self.borrowed(&args[0])),
            "text_is_empty" => format!("({}).is_empty()", self.borrowed(&args[0])),
            "int_to_text" => format!("cpl::Text::of_i64({})", self.expr(&args[0])),
            "option_is_some" => format!("({}).is_some()", self.expr(&args[0])),
            "option_is_none" => format!("({}).is_none()", self.expr(&args[0])),
            _ => {
                let _ = e;
                "(return None)".to_string()
            }
        }
    }

    fn expr(&mut self, e: &Expr) -> String {
        match &e.kind {
            ExprKind::Lit(Lit::Int(n)) => match small_literal(n) {
                Some(i64::MIN) => "i64::MIN".to_string(),
                Some(v) => format!("({v}i64)"),
                None => "(return None)".to_string(),
            },
            ExprKind::Lit(Lit::Bool(b)) => b.to_string(),
            ExprKind::Lit(Lit::Text(t)) => format!("cpl::Text::lit({t:?})"),
            // a list, text, or record of texts read as a value: its own reference
            ExprKind::Local(id) if !copyable(self.p, self.q(&e.ty).unwrap_or(Q::Int)) => format!("q{}.clone()", id.0),
            ExprKind::Local(id) => format!("q{}", id.0),
            ExprKind::Ctor { def, variant, args } if self.q(&e.ty) == Some(Q::Enum(*def)) => {
                if args.is_empty() {
                    return format!("crate::__q::E{}::V{variant}", def.0);
                }
                let vals: Vec<String> = args.iter().map(|a| self.expr(a)).collect();
                let lets: String = vals.iter().enumerate().map(|(i, v)| format!("let e{i} = {v}; ")).collect();
                let names: Vec<String> = (0..vals.len()).map(|i| format!("e{i}")).collect();
                format!("{{ {lets}crate::__q::E{}::V{variant}({}) }}", def.0, names.join(", "))
            }
            ExprKind::Ctor { def, variant, args } if *def == self.p.std_defs.option => match (variant, args.first()) {
                (0, Some(a)) => {
                    let v = self.expr(a);
                    format!("Some({v})")
                }
                _ => "None".to_string(),
            },
            ExprKind::List(items) => {
                let el = match self.q(&e.ty) {
                    Some(Q::List(el)) => el,
                    _ => El::Int,
                };
                if items.is_empty() {
                    return "cpl::List::new()".to_string();
                }
                let vals: Vec<String> = items.iter().map(|a| self.expr(a)).collect();
                let lets: String = vals.iter().enumerate().map(|(i, v)| format!("let e{i} = {v}; ")).collect();
                let words: Vec<String> = (0..vals.len()).map(|i| self.rq.word(el, &format!("e{i}"))).collect();
                format!("{{ {lets}cpl::List::from_vec(vec![{}]) }}", words.join(", "))
            }
            ExprKind::Widen { expr, .. } => self.expr(expr),
            ExprKind::Ctor { def, .. } if *def == self.unit => "()".to_string(),
            ExprKind::Field { base, index, .. } => {
                let c = if copyable(self.p, self.q(&e.ty).unwrap_or(Q::Int)) { "" } else { ".clone()" };
                match &base.kind {
                    // a field of a local: read in place
                    ExprKind::Local(id) => format!("q{}.f{index}{c}", id.0),
                    _ => {
                        let b = self.expr(base);
                        format!("({b}).f{index}{c}")
                    }
                }
            }
            ExprKind::StructLit { def, spread, fields: given, .. } => {
                let mut parts: Vec<String> = given.iter().map(|(i, f)| format!("f{i}: {}", self.expr(f))).collect();
                if let Some(b) = spread {
                    parts.push(format!("..{}", self.expr(b)));
                }
                format!("crate::__q::R{} {{ {} }}", def.0, parts.join(", "))
            }
            ExprKind::Unary { op, operand } => {
                let a = self.expr(operand);
                match op {
                    UnaryOp::Not => format!("(!{a})"),
                    UnaryOp::Neg => format!("({a}).checked_neg()?"),
                }
            }
            ExprKind::Binary { op: BinaryOp::Concat, lhs, rhs } => {
                let a = self.expr(lhs);
                let b = self.expr(rhs);
                let f = if self.q(&lhs.ty) == Some(Q::Text) { "concat_text" } else { "concat_list" };
                format!("{{ let a_ = {a}; let b_ = {b}; cpl::{f}(a_, b_) }}")
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let (a, b) = (self.expr(lhs), self.expr(rhs));
                match op {
                    BinaryOp::Add => format!("({a}).checked_add({b})?"),
                    BinaryOp::Sub => format!("({a}).checked_sub({b})?"),
                    BinaryOp::Mul => format!("({a}).checked_mul({b})?"),
                    // None on a zero divisor and on MIN / -1: the generic form raises or computes a bigint
                    BinaryOp::Div => format!("({a}).checked_div({b})?"),
                    BinaryOp::Rem => format!("({a}).checked_rem({b})?"),
                    BinaryOp::Eq => format!("({a} == {b})"),
                    BinaryOp::Ne => format!("({a} != {b})"),
                    BinaryOp::Lt => format!("({a} < {b})"),
                    BinaryOp::Le => format!("({a} <= {b})"),
                    BinaryOp::Gt => format!("({a} > {b})"),
                    BinaryOp::Ge => format!("({a} >= {b})"),
                    BinaryOp::And => format!("({a} && {b})"),
                    BinaryOp::Or => format!("({a} || {b})"),
                    _ => "(return None)".to_string(),
                }
            }
            ExprKind::If { cond, then_block, else_block } => {
                let c = self.expr(cond);
                let t = self.block(then_block);
                match else_block {
                    Some(b) => {
                        let f = self.block(b);
                        format!("(if {c} {t} else {f})")
                    }
                    None => format!("(if {c} {t})"),
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                let sq = self.q(&scrutinee.ty).unwrap_or(Q::Int);
                let sv = self.expr(scrutinee);
                // the scrutinee fixed in `s_`; a text is matched as a `&str`
                let view = if sq == Q::Text { "&*s_" } else { "s_" };
                let mut s = format!("{{ let s_ = {sv}; match {view} {{ ");
                let mut total = false;
                for arm in arms {
                    let body = self.block(&arm.body);
                    let lit = |k: &PatternKind| -> Option<String> {
                        match k {
                            PatternKind::Int(n) => Some(format!("{}i64", small_literal(n).unwrap_or(0))),
                            PatternKind::Bool(b) => Some(b.to_string()),
                            PatternKind::Text(t) => Some(format!("{t:?}")),
                            _ => None,
                        }
                    };
                    match (&arm.pattern.kind, sq) {
                        (PatternKind::Bind(l), Q::Text) => {
                            total = true;
                            let _ = write!(s, "_ => {{ q{} = s_.clone(); {body} }}, ", l.0);
                        }
                        (PatternKind::Bind(l), _) => {
                            total = true;
                            let _ = write!(s, "__s => {{ q{} = __s; {body} }}, ", l.0);
                        }
                        // an enum variant: its payload's literals, bindings, and `_`
                        (PatternKind::Ctor { variant, args, .. }, Q::Enum(d)) => {
                            let mut binds = String::new();
                            let subs: Vec<String> = args
                                .iter()
                                .enumerate()
                                .map(|(i, a)| match &a.kind {
                                    PatternKind::Bind(l) => {
                                        let _ = write!(binds, "q{} = __b{i}; ", l.0);
                                        format!("__b{i}")
                                    }
                                    k => lit(k).unwrap_or_else(|| "_".to_string()),
                                })
                                .collect();
                            let pat = if subs.is_empty() {
                                format!("crate::__q::E{}::V{variant}", d.0)
                            } else {
                                format!("crate::__q::E{}::V{variant}({})", d.0, subs.join(", "))
                            };
                            let _ = write!(s, "{pat} => {{ {binds}{body} }}, ");
                        }
                        // an option: `Some(...)` (a literal, a binding, `_`) or `None`
                        (PatternKind::Ctor { variant: 0, args, .. }, _) => match args.first().map(|a| &a.kind) {
                            Some(PatternKind::Bind(l)) => {
                                let _ = write!(s, "Some(__s) => {{ q{} = __s; {body} }}, ", l.0);
                            }
                            Some(k) => match lit(k) {
                                Some(v) => {
                                    let _ = write!(s, "Some({v}) => {body}, ");
                                }
                                None => {
                                    let _ = write!(s, "Some(_) => {body}, ");
                                }
                            },
                            None => {
                                let _ = write!(s, "Some(_) => {body}, ");
                            }
                        },
                        (PatternKind::Ctor { .. }, _) => {
                            let _ = write!(s, "None => {body}, ");
                        }
                        (k, _) => match lit(k) {
                            Some(v) => {
                                let _ = write!(s, "{v} => {body}, ");
                            }
                            None => {
                                total = true;
                                let _ = write!(s, "_ => {body}, ");
                            }
                        },
                    }
                }
                if !total {
                    s.push_str("_ => return None, ");
                }
                s.push_str("} }");
                s
            }
            ExprKind::Call { callee, args, .. } => {
                if let Some(which) = builtin(self.p, *callee) {
                    return self.builtin(which, e, args);
                }
                let mut vals: Vec<String> = args
                    .iter()
                    .map(|a| {
                        if copyable(self.p, self.q(&a.ty).unwrap_or(Q::Int)) {
                            self.expr(a)
                        } else if let ExprKind::Local(id) = &a.kind {
                            // borrowed as it is: no count taken
                            format!("&q{}", id.0)
                        } else {
                            let v = self.expr(a);
                            format!("&({v})")
                        }
                    })
                    .collect();
                if self.rq.plan.fns.get(callee) == Some(&true) {
                    vals.push(if *callee == self.def && self.measured && !self.in_contract {
                        "mself".to_string()
                    } else {
                        "i64::MIN".to_string()
                    });
                }
                // the arguments first, in order: an argument that calls another form borrows `cx` too
                let lets: String = vals.iter().enumerate().map(|(i, v)| format!("let a{i} = {v}; ")).collect();
                format!(
                    "{{ {lets}{}__q(cx, qd + 1{})? }}",
                    (self.path)(*callee),
                    (0..vals.len()).map(|i| format!(", a{i}")).collect::<String>()
                )
            }
            _ => "(return None)".to_string(),
        }
    }
}
