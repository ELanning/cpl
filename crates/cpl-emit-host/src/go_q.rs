//! The Go host's scalar tier (`spec/hosts.md` H14): for every function of the scalar subset (`cpl_hir::scalar` with
//! `Caps::HOSTS`: lists of `Int`, `Bool`, records, and lists, options, texts, and enums), a form `q_<name>` on `int64`,
//! `bool`, `string`, Go structs for records (`qR<n>`) and enums (`qE<n>`: a tag and every variant's payload), the
//! generic form's own lists (elements read in place), and `cpl.QOpt` options, that returns `ok == false` ("bails")
//! wherever the generic form could do anything but compute the same value (an `int64` overflow, a division by zero, a
//! contract, assertion, or measure that does not hold, an element that is a bigint, a negative count, the depth
//! limit). The generic `x_<name>` tries it first and, on a bail, runs itself, which then raises exactly what it
//! always did. Go strings are values: a text costs nothing to copy.

use cpl_hir::scalar::{builtin, fields, of_el, small_literal, variant_fields, Caps, El, Plan, Q};
use cpl_hir::{Block, DefId, Expr, ExprKind, FnBody, Lit, LocalId, Pattern, PatternKind, Program, Stmt, Ty};
use cpl_syntax::ast::{BinaryOp, UnaryOp};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

pub(crate) struct GoQ {
    plan: Plan,
    /// The generated Go type of each record and enum of the plan (an enum's interface), set once names exist.
    pub(crate) gen: BTreeMap<DefId, String>,
    /// The generated Go type of each variant of an enum of the plan.
    pub(crate) variants: BTreeMap<(DefId, u32), String>,
}

fn qty(p: &Program, ty: &Ty) -> Option<Q> {
    cpl_hir::scalar::qty(p, ty, Caps::HOSTS)
}

impl GoQ {
    pub(crate) fn new(p: &Program) -> GoQ {
        GoQ {
            plan: cpl_hir::scalar::plan(p, Caps::HOSTS),
            gen: BTreeMap::new(),
            variants: BTreeMap::new(),
        }
    }

    pub(crate) fn has(&self, d: DefId) -> bool {
        self.plan.fns.contains_key(&d)
    }

    /// The records and enums whose generated names the tier needs.
    pub(crate) fn named(&self) -> Vec<DefId> {
        self.plan.records.iter().chain(&self.plan.enums).copied().collect()
    }

    fn gty(&self, q: Q) -> String {
        match q {
            Q::Int => "int64".to_string(),
            Q::Bool => "bool".to_string(),
            Q::Text => "string".to_string(),
            Q::Rec(d) => format!("qR{}", d.0),
            Q::Enum(d) => format!("qE{}", d.0),
            Q::List(el) => format!("cpl.List[{}]", self.elem(el)),
            Q::Opt(el) => format!("cpl.QOpt[{}]", self.gty(of_el(el))),
            Q::Word => unreachable!("Caps::HOSTS has no words"),
        }
    }

    /// The type of the generic form's list elements of `el`.
    fn elem(&self, el: El) -> String {
        match el {
            El::Int => "cpl.Int".to_string(),
            El::Bool => "bool".to_string(),
            El::Rec(d) => self.gen.get(&d).cloned().unwrap_or_default(),
            El::List(l) => format!("cpl.List[{}]", self.elem(cpl_hir::scalar::el_of_leaf(l))),
        }
    }

    fn zero(&self, q: Q) -> String {
        match q {
            Q::Int => "0".to_string(),
            Q::Bool => "false".to_string(),
            Q::Text => "\"\"".to_string(),
            _ => format!("{}{{}}", self.gty(q)),
        }
    }

    /// An element of a list of `el` as the list holds it, from its speculative value `v`.
    fn word(&self, el: El, v: &str) -> String {
        match el {
            El::Int => format!("cpl.I({v})"),
            El::Rec(d) => format!("qBox{}({v})", d.0),
            _ => v.to_string(),
        }
    }

    /// The records and enums as Go structs, with their conversions from and to the generated types (`field` names a
    /// struct's field).
    pub(crate) fn records(&self, p: &Program, field: &dyn Fn(DefId, usize) -> String) -> String {
        let mut s = String::new();
        for r in &self.plan.records {
            let n = r.0;
            let fs = fields(p, *r);
            let members: String = fs.iter().enumerate().map(|(i, f)| format!("\tf{i} {}\n", self.gty(*f))).collect();
            let _ = writeln!(s, "type qR{n} struct {{\n{members}}}\n");
            let gen = &self.gen[r];
            let mut unbox = String::new();
            let mut boxed = Vec::new();
            for (i, f) in fs.iter().enumerate() {
                let name = field(*r, i);
                match f {
                    Q::Int => {
                        let _ = writeln!(unbox, "\tif x, ok := v.{name}.Int64(); ok {{\n\t\to.f{i} = x\n\t}} else {{\n\t\treturn o, false\n\t}}");
                        boxed.push(format!("{name}: cpl.I(v.f{i})"));
                    }
                    Q::Rec(m) => {
                        let _ = writeln!(unbox, "\tif x, ok := qUnbox{}(&v.{name}); ok {{\n\t\to.f{i} = x\n\t}} else {{\n\t\treturn o, false\n\t}}", m.0);
                        boxed.push(format!("{name}: qBox{}(v.f{i})", m.0));
                    }
                    _ => {
                        let _ = writeln!(unbox, "\to.f{i} = v.{name}");
                        boxed.push(format!("{name}: v.f{i}"));
                    }
                }
            }
            let _ = writeln!(s, "func qUnbox{n}(v *{gen}) (qR{n}, bool) {{\n\tvar o qR{n}\n{unbox}\treturn o, true\n}}\n");
            let _ = writeln!(s, "func qBox{n}(v qR{n}) {gen} {{\n\treturn {gen}{{{}}}\n}}\n", boxed.join(", "));
        }
        for d in &self.plan.enums {
            s.push_str(&self.enum_type(p, *d));
        }
        s
    }

    /// An enum's struct (its tag, then `v<variant>_<i>` for each payload field) and its conversions.
    fn enum_type(&self, p: &Program, d: DefId) -> String {
        let n = d.0;
        let nv = p.def(d).kind.as_enum().map(|e| e.variants.len()).unwrap_or(0) as u32;
        let gen = &self.gen[&d];
        let mut s = String::new();
        let mut members = String::from("\ttag uint32\n");
        let mut payloads = false;
        for v in 0..nv {
            for (i, f) in variant_fields(p, d, v).iter().enumerate() {
                payloads = true;
                let _ = writeln!(members, "\tv{v}_{i} {}", self.gty(*f));
            }
        }
        let _ = writeln!(s, "type qE{n} struct {{\n{members}}}\n");
        // unbox: false for a nil value and a payload Int that does not fit
        let _ = writeln!(s, "func qUnboxE{n}(v {gen}) (qE{n}, bool) {{\n\tvar o qE{n}");
        let _ = writeln!(s, "\tswitch {}v.(type) {{", if payloads { "x := " } else { "" });
        for v in 0..nv {
            let _ = writeln!(s, "\tcase {}:\n\t\to.tag = {v}", self.variants[&(d, v)]);
            for (i, f) in variant_fields(p, d, v).iter().enumerate() {
                if *f == Q::Int {
                    let _ = writeln!(s, "\t\tif y, ok := x.F{i}.Int64(); ok {{\n\t\t\to.v{v}_{i} = y\n\t\t}} else {{\n\t\t\treturn o, false\n\t\t}}");
                } else {
                    let _ = writeln!(s, "\t\to.v{v}_{i} = x.F{i}");
                }
            }
        }
        s.push_str("\tdefault:\n\t\treturn o, false\n\t}\n\treturn o, true\n}\n\n");
        let _ = writeln!(s, "func qBoxE{n}(v qE{n}) {gen} {{\n\tswitch v.tag {{");
        for v in 0..nv {
            let fs = variant_fields(p, d, v);
            let parts: Vec<String> = fs
                .iter()
                .enumerate()
                .map(|(i, f)| if *f == Q::Int { format!("F{i}: cpl.I(v.v{v}_{i})") } else { format!("F{i}: v.v{v}_{i}") })
                .collect();
            let _ = writeln!(s, "\tcase {v}:\n\t\treturn {}{{{}}}", self.variants[&(d, v)], parts.join(", "));
        }
        s.push_str("\t}\n\tpanic(\"cpl: no such variant\")\n}\n\n");
        s
    }

    /// The lines that open `x_<name>`: the speculative call; on a bail the generic form follows. `params` are the
    /// Go names of the parameters, `id` the function's measure id.
    pub(crate) fn wrapper(&self, p: &Program, d: DefId, params: &[String], id: u32, short: &dyn Fn(DefId) -> String) -> String {
        let fd = p.def(d).kind.as_fn().expect("a function");
        let mut opens = String::new();
        let mut closes = String::new();
        let mut args = Vec::new();
        let mut depth = 2;
        for (i, (param, v)) in fd.params.iter().zip(params).enumerate() {
            let pad = "\t".repeat(depth);
            match qty(p, &param.ty) {
                Some(Q::Int) => {
                    let _ = writeln!(opens, "{pad}if a{i}, ok := {v}.Int64(); ok {{");
                    closes = format!("{pad}}}\n{closes}");
                    depth += 1;
                }
                Some(Q::Bool | Q::Text | Q::List(_)) => {
                    let _ = writeln!(opens, "{pad}a{i} := {v}");
                }
                Some(Q::Rec(r)) => {
                    let _ = writeln!(opens, "{pad}if a{i}, ok := qUnbox{}(&{v}); ok {{", r.0);
                    closes = format!("{pad}}}\n{closes}");
                    depth += 1;
                }
                Some(Q::Enum(r)) => {
                    let _ = writeln!(opens, "{pad}if a{i}, ok := qUnboxE{}({v}); ok {{", r.0);
                    closes = format!("{pad}}}\n{closes}");
                    depth += 1;
                }
                _ => return String::new(),
            }
            args.push(format!("a{i}"));
        }
        if self.plan.fns[&d] {
            args.push(format!("cx.QPrev({id})"));
        }
        let boxed = match qty(p, &fd.ret) {
            Some(Q::Int) => "cpl.I(r)".to_string(),
            Some(Q::Rec(r)) => format!("qBox{}(r)", r.0),
            Some(Q::Enum(r)) => format!("qBoxE{}(r)", r.0),
            _ => "r".to_string(),
        };
        let pad = "\t".repeat(depth);
        let mut s = String::from("\tif cx.QOk() {\n");
        s.push_str(&opens);
        let _ = writeln!(
            s,
            "{pad}if r, ok := q_{}(cx, cx.Depth(){}); ok {{\n{pad}\treturn {boxed}\n{pad}}}\n{pad}cx.QBail()",
            short(d),
            args.iter().map(|a| format!(", {a}")).collect::<String>()
        );
        s.push_str(&closes);
        s.push_str("\t}\n");
        s
    }

    /// `q_<name>` (contracts and measure, and the body in place when there is no `ensures`) and `qb_<name>`.
    pub(crate) fn functions(&self, p: &Program, d: DefId, short: &dyn Fn(DefId) -> String) -> String {
        let fd = p.def(d).kind.as_fn().expect("a function");
        let FnBody::Block(body) = &fd.body else {
            return String::new();
        };
        let name = short(d);
        let measured = self.plan.fns[&d];
        let rq = qty(p, &fd.ret).expect("a scalar result");
        let rt = self.gty(rq);
        let params: Vec<String> = fd
            .params
            .iter()
            .map(|x| format!("q{} {}", x.local.0, self.gty(qty(p, &x.ty).expect("a scalar parameter"))))
            .collect();
        let pset: BTreeSet<LocalId> = fd.params.iter().map(|x| x.local).collect();
        let locals: Vec<(LocalId, Q)> = fd
            .locals
            .iter()
            .filter(|l| !pset.contains(&l.id))
            .filter_map(|l| qty(p, &l.ty).map(|q| (l.id, q)))
            .collect();
        // the texts a statement appends to: through a buffer, so that a loop of appends is linear
        let mut appended = BTreeSet::new();
        for top in &body.stmts {
            top.walk_stmts(&mut |st| {
                if let Stmt::Assign { local, value } = st {
                    if cpl_hir::scalar::text_append_chain(p, *local, value).is_some() {
                        appended.insert(*local);
                    }
                }
            });
        }
        // the list locals that never leave the function: Go slices (cpl_hir::scalar::flat_locals)
        let flat = cpl_hir::scalar::flat_locals(p, fd, Caps::HOSTS);
        // the list locals built as Go slices and then made lists (cpl_hir::scalar::built_locals)
        let built = cpl_hir::scalar::built_locals(p, fd, Caps::HOSTS);
        let decl_locals = |s: &mut String| {
            for (l, q) in &locals {
                if let Some(el) = flat.get(l) {
                    let _ = writeln!(s, "\tvar q{} []{}\n\t_ = q{}", l.0, self.gty(of_el(*el)), l.0);
                    continue;
                }
                if let Some(b) = built.get(l) {
                    // of `int64` for an `Int` list (no pointers: no write barrier per append, nothing for the GC
                    // to scan), made a list of cpl.Int once
                    let _ = writeln!(s, "\tvar qf{} []{}\n\t_ = qf{}", l.0, self.gty(of_el(b.el)), l.0);
                }
                let _ = writeln!(s, "\tvar q{} {} = {}\n\t_ = q{}", l.0, self.gty(*q), self.zero(*q), l.0);
            }
            for l in &appended {
                let _ = writeln!(s, "\tvar tb{} cpl.TextBuf", l.0);
            }
        };
        let mut s = String::new();
        // the contracts and the measure
        let mut e = Emitter::new(p, self, d, measured, true, short, &appended, &flat, &built);
        e.line("if !cx.QEnter(qd) {");
        e.line(&format!("\treturn {}, false", self.zero(rq)));
        e.line("}");
        for c in &fd.requires {
            let v = e.expr(&c.expr);
            e.bail_unless(&v);
        }
        let mut body_args: Vec<String> = fd.params.iter().map(|x| format!("q{}", x.local.0)).collect();
        if let Some(c) = &fd.decreases {
            let v = e.expr(&c.expr);
            e.line(&format!("mself := {v}"));
            e.bail_unless("mself >= 0 && (mprev == cpl.QNone || mself < mprev)");
            body_args.push("mself".to_string());
        }
        if fd.ensures.is_empty() {
            // nothing runs after the body: it follows in place (Go inlines no recursive call)
            e.in_contract = false;
            match e.body(body) {
                Some(v) => e.line(&format!("return {v}, true")),
                None => e.fail(),
            }
            let _ = writeln!(
                s,
                "func q_{name}(cx *cpl.Cx, qd int{}{}) ({rt}, bool) {{",
                params.iter().map(|x| format!(", {x}")).collect::<String>(),
                if measured { ", mprev int64" } else { "" }
            );
            decl_locals(&mut s);
            e.finish(&mut s);
            s.push_str("}\n\n");
            return s;
        }
        e.line(&format!(
            "r, ok := qb_{name}(cx, qd{})",
            body_args.iter().map(|a| format!(", {a}")).collect::<String>()
        ));
        e.bail_unless("ok");
        // the measure is recorded by now (S35): a call of this function compares with it
        e.in_contract = false;
        if let Some(res) = fd.result_local {
            e.line(&format!("q{} = r", res.0));
        }
        for c in &fd.ensures {
            let v = e.expr(&c.expr);
            e.bail_unless(&v);
        }
        e.line("return r, true");
        let _ = writeln!(
            s,
            "func q_{name}(cx *cpl.Cx, qd int{}{}) ({rt}, bool) {{",
            params.iter().map(|x| format!(", {x}")).collect::<String>(),
            if measured { ", mprev int64" } else { "" }
        );
        decl_locals(&mut s);
        e.finish(&mut s);
        s.push_str("}\n\n");
        // the body
        let mut e = Emitter::new(p, self, d, measured, false, short, &appended, &flat, &built);
        match e.body(body) {
            Some(v) => e.line(&format!("return {v}, true")),
            None => e.fail(),
        }
        let _ = writeln!(
            s,
            "func qb_{name}(cx *cpl.Cx, qd int{}{}) ({rt}, bool) {{",
            params.iter().map(|x| format!(", {x}")).collect::<String>(),
            if measured { ", mself int64" } else { "" }
        );
        decl_locals(&mut s);
        e.finish(&mut s);
        s.push_str("}\n\n");
        s
    }
}

/// The Go printer of one speculative function: statements, with temporaries declared at the top (a `return` bails).
struct Emitter<'a> {
    p: &'a Program,
    q: &'a GoQ,
    def: DefId,
    measured: bool,
    in_contract: bool,
    short: &'a dyn Fn(DefId) -> String,
    /// The text locals appended to through `tb<n>`.
    appended: &'a BTreeSet<LocalId>,
    /// The list locals held as Go slices of their elements.
    flat: &'a BTreeMap<LocalId, El>,
    /// The list locals built as slices `qf<n>`, then made the lists `q<n>` (those of `frozen` by now).
    built: &'a BTreeMap<LocalId, cpl_hir::scalar::Built>,
    frozen: BTreeSet<LocalId>,
    out: String,
    decls: Vec<(String, String)>,
    indent: usize,
    next: u32,
    unit: DefId,
    /// The bail: `return` with the zero of the function's result.
    bail: String,
    /// Past a `return` in the current block: nothing more is printed there (`go vet -unreachable`).
    dead: bool,
}

impl<'a> Emitter<'a> {
    fn new(
        p: &'a Program,
        q: &'a GoQ,
        def: DefId,
        measured: bool,
        in_contract: bool,
        short: &'a dyn Fn(DefId) -> String,
        appended: &'a BTreeSet<LocalId>,
        flat: &'a BTreeMap<LocalId, El>,
        built: &'a BTreeMap<LocalId, cpl_hir::scalar::Built>,
    ) -> Emitter<'a> {
        let rq = qty(p, &p.def(def).kind.as_fn().expect("a function").ret).unwrap_or(Q::Int);
        Emitter {
            p,
            q,
            def,
            measured,
            in_contract,
            short,
            appended,
            flat,
            built,
            frozen: BTreeSet::new(),
            out: String::new(),
            decls: Vec::new(),
            indent: 1,
            next: 0,
            unit: p.std_defs.unit,
            bail: format!("return {}, false", q.zero(rq)),
            dead: false,
        }
    }

    fn finish(&self, s: &mut String) {
        for (t, n) in &self.decls {
            let _ = writeln!(s, "\tvar {n} {t}\n\t_ = {n}");
        }
        s.push_str(&self.out);
    }

    fn line(&mut self, s: &str) {
        if self.dead {
            return;
        }
        self.brace(s);
    }

    /// A line printed even past a `return`: the braces of a structure opened before it.
    fn brace(&mut self, s: &str) {
        for _ in 0..self.indent {
            self.out.push('\t');
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn temp(&mut self, q: Q) -> String {
        self.next += 1;
        let t = format!("qt{}", self.next);
        self.decls.push((self.q.gty(q), t.clone()));
        t
    }

    fn flag(&mut self) -> String {
        self.next += 1;
        let ok = format!("qok{}", self.next);
        self.decls.push(("bool".to_string(), ok.clone()));
        ok
    }

    fn qt(&self, ty: &Ty) -> Q {
        qty(self.p, ty).unwrap_or(Q::Int)
    }

    fn fail(&mut self) {
        let b = self.bail.clone();
        self.line(&b);
        self.dead = true;
    }

    /// A bail unless `cond` holds (nothing when past a `return`).
    fn bail_unless(&mut self, cond: &str) {
        if self.dead {
            return;
        }
        self.line(&format!("if !({cond}) {{"));
        self.indent += 1;
        self.fail();
        self.dead = false;
        self.indent -= 1;
        self.line("}");
    }

    /// The slice holding list local `l` and its element, while it is one.
    fn flat_var(&self, l: LocalId) -> Option<(El, String)> {
        if let Some(el) = self.flat.get(&l) {
            return Some((*el, format!("q{}", l.0)));
        }
        let b = self.built.get(&l)?;
        (!self.frozen.contains(&l)).then(|| (b.el, format!("qf{}", l.0)))
    }

    /// The function's body: each built list made from its slice before the statement that first reads it as a list.
    fn body(&mut self, b: &Block) -> Option<String> {
        let built: Vec<(LocalId, cpl_hir::scalar::Built)> = self.built.iter().map(|(l, x)| (*l, *x)).collect();
        for i in 0..=b.stmts.len() {
            if self.dead {
                return None;
            }
            for (l, x) in built.iter().filter(|(_, x)| x.at == i) {
                let n = l.0;
                let made = match x.el {
                    El::Int => format!("cpl.QFreezeI(qf{n})"),
                    El::Rec(d) => format!("cpl.QFreezeF(qf{n}, qBox{})", d.0),
                    _ => format!("cpl.QFreeze(qf{n})"),
                };
                self.line(&format!("q{n} = {made}"));
                self.frozen.insert(*l);
            }
            if let Some(st) = b.stmts.get(i) {
                self.stmt(st);
            }
        }
        match &b.tail {
            Some(_) if self.dead => None,
            Some(e) => {
                let v = self.expr(e);
                qty(self.p, &e.ty).is_some().then_some(v)
            }
            None => None,
        }
    }

    fn block(&mut self, b: &Block) -> Option<String> {
        for st in &b.stmts {
            if self.dead {
                return None;
            }
            self.stmt(st);
        }
        match &b.tail {
            Some(_) if self.dead => None,
            Some(e) => {
                let v = self.expr(e);
                qty(self.p, &e.ty).is_some().then_some(v)
            }
            None => None,
        }
    }

    fn stmt(&mut self, st: &Stmt) {
        match st {
            // a slice: `[]` empties it, its own push appends
            Stmt::Let { local, value } | Stmt::Assign { local, value } if self.flat_var(*local).is_some() => {
                let (el, q) = self.flat_var(*local).expect("a slice");
                match &value.kind {
                    ExprKind::Call { args, .. } if args.len() == 2 => {
                        let v = self.fixed(&args[1]);
                        self.line(&format!("{q} = append({q}, {v})"));
                    }
                    // room for 8 at first (a short list grows no more), the same array when emptied again
                    _ => self.line(&format!(
                        "if {q} == nil {{ {q} = make([]{}, 0, 8) }} else {{ {q} = {q}[:0] }}",
                        self.q.gty(of_el(el))
                    )),
                }
            }
            // t = t ++ x: through the local's buffer
            Stmt::Assign { local, value } if self.appended.contains(local) && self.append(*local, value) => {}
            Stmt::Let { local, value } | Stmt::Assign { local, value } => {
                let v = self.expr(value);
                self.line(&format!("q{} = {v}", local.0));
            }
            Stmt::Discard(e) | Stmt::Expr(e) => {
                let v = self.expr(e);
                if qty(self.p, &e.ty).is_some() {
                    self.line(&format!("_ = {v}"));
                }
            }
            Stmt::Return(Some(e)) => {
                let v = self.expr(e);
                self.line(&format!("return {v}, true"));
                self.dead = true;
            }
            Stmt::Assert { expr, .. } => {
                let v = self.expr(expr);
                self.bail_unless(&v);
            }
            Stmt::While { cond, body, .. } => {
                // the condition may print statements of its own: they run on every iteration
                self.line("for {");
                self.indent += 1;
                let c = self.expr(cond);
                // a condition that always bails: the loop never ends (Go: a `for` with no `break` terminates)
                let stuck = self.dead;
                if !stuck {
                    self.line(&format!("if !({c}) {{\n{}\tbreak\n{}}}", "\t".repeat(self.indent), "\t".repeat(self.indent)));
                    let _ = self.block(body);
                }
                // a loop's body ends where its brace does; the loop itself may end (its condition fails)
                self.dead = stuck;
                self.indent -= 1;
                self.brace("}");
            }
            Stmt::Return(None) | Stmt::Concurrent(_) => self.fail(),
        }
    }

    /// `t = t ++ a ++ b ...` (the shape `appended` found) through `tb<t>`: every part first, then appended.
    fn append(&mut self, local: LocalId, value: &Expr) -> bool {
        let Some(parts) = cpl_hir::scalar::text_append_chain(self.p, local, value) else {
            return false;
        };
        let xs: Vec<String> = parts.iter().map(|x| self.fixed(x)).collect();
        for x in xs {
            self.line(&format!("q{n} = tb{n}.Append(q{n}, {x})", n = local.0));
        }
        true
    }

    fn fixed(&mut self, e: &Expr) -> String {
        let v = self.expr(e);
        let t = self.temp(self.qt(&e.ty));
        self.line(&format!("{t} = {v}"));
        t
    }

    fn checked(&mut self, call: &str, q: Q) -> String {
        let t = self.temp(q);
        if self.dead {
            return t;
        }
        let ok = self.flag();
        self.line(&format!("{t}, {ok} = {call}"));
        self.bail_unless(&ok);
        t
    }

    /// A stand-in for an expression past a bail: never printed.
    fn dummy(&self, ty: &Ty) -> String {
        match qty(self.p, ty) {
            Some(q) => self.q.zero(q),
            None => "struct{}{}".to_string(),
        }
    }

    fn expr(&mut self, e: &Expr) -> String {
        if self.dead {
            // never printed: every line that would use it is past the `return`
            return self.dummy(&e.ty);
        }
        match &e.kind {
            ExprKind::Lit(Lit::Int(n)) => match small_literal(n) {
                Some(i64::MIN) => "int64(-9223372036854775807 - 1)".to_string(),
                Some(v) => format!("int64({v})"),
                None => {
                    self.fail();
                    "0".to_string()
                }
            },
            ExprKind::Lit(Lit::Bool(b)) => b.to_string(),
            ExprKind::Lit(Lit::Text(t)) => crate::go::go_str(t),
            ExprKind::Local(id) => format!("q{}", id.0),
            ExprKind::Widen { expr, .. } => self.expr(expr),
            ExprKind::Ctor { def, .. } if *def == self.unit => "struct{}{}".to_string(),
            ExprKind::Ctor { def, variant, args } if *def == self.p.std_defs.option => {
                let ty = self.q.gty(self.qt(&e.ty));
                match (variant, args.first()) {
                    (0, Some(a)) => {
                        let v = self.fixed(a);
                        format!("{ty}{{V: {v}, Has: true}}")
                    }
                    _ => format!("{ty}{{}}"),
                }
            }
            ExprKind::Ctor { def, variant, args } if self.qt(&e.ty) == Q::Enum(*def) => {
                let vals: Vec<String> = args.iter().map(|a| self.fixed(a)).collect();
                let mut parts = vec![format!("tag: {variant}")];
                for (i, v) in vals.iter().enumerate() {
                    parts.push(format!("v{variant}_{i}: {v}"));
                }
                format!("qE{}{{{}}}", def.0, parts.join(", "))
            }
            ExprKind::List(items) => {
                let el = match self.qt(&e.ty) {
                    Q::List(el) => el,
                    _ => El::Int,
                };
                let vals: Vec<String> = items.iter().map(|a| self.fixed(a)).collect();
                let elem = self.q.elem(el);
                if vals.is_empty() {
                    return format!("cpl.List[{elem}]{{}}");
                }
                let words: Vec<String> = vals.iter().map(|v| self.q.word(el, v)).collect();
                format!("cpl.ListLit[{elem}]({})", words.join(", "))
            }
            ExprKind::Field { base, index, .. } => {
                let b = match &base.kind {
                    ExprKind::Local(id) => format!("q{}", id.0),
                    _ => self.fixed(base),
                };
                let t = self.temp(self.qt(&e.ty));
                self.line(&format!("{t} = {b}.f{index}"));
                t
            }
            ExprKind::StructLit { def, spread, fields: given, .. } => {
                let base = spread.as_ref().map(|b| self.fixed(b));
                let vals: Vec<(u32, String)> = given.iter().map(|(i, f)| (*i, self.fixed(f))).collect();
                let t = self.temp(Q::Rec(*def));
                match base {
                    Some(b) => self.line(&format!("{t} = {b}")),
                    None => self.line(&format!("{t} = qR{}{{}}", def.0)),
                }
                for (i, v) in vals {
                    self.line(&format!("{t}.f{i} = {v}"));
                }
                t
            }
            ExprKind::Unary { op, operand } => {
                let a = self.fixed(operand);
                match op {
                    UnaryOp::Not => format!("(!{a})"),
                    UnaryOp::Neg => self.checked(&format!("cpl.QSub(0, {a})"), Q::Int),
                }
            }
            ExprKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs),
            ExprKind::If { cond, then_block, else_block } => {
                let c = self.fixed(cond);
                if self.dead {
                    return self.dummy(&e.ty);
                }
                let q = qty(self.p, &e.ty);
                let t = q.map(|q| self.temp(q));
                self.line(&format!("if {c} {{"));
                self.indent += 1;
                let v = self.block(then_block);
                if let (Some(t), Some(v)) = (&t, v) {
                    self.line(&format!("{t} = {v}"));
                }
                let then_dead = std::mem::replace(&mut self.dead, false);
                self.indent -= 1;
                let mut else_dead = false;
                if let Some(b) = else_block {
                    self.brace("} else {");
                    self.indent += 1;
                    let v = self.block(b);
                    if let (Some(t), Some(v)) = (&t, v) {
                        self.line(&format!("{t} = {v}"));
                    }
                    else_dead = std::mem::replace(&mut self.dead, false);
                    self.indent -= 1;
                }
                self.brace("}");
                // both branches return: the `if` is a terminating statement
                self.dead = then_dead && else_dead;
                t.unwrap_or_else(|| "struct{}{}".to_string())
            }
            ExprKind::Match { scrutinee, arms } => {
                let sq = self.qt(&scrutinee.ty);
                let st = self.fixed(scrutinee);
                if self.dead {
                    return self.dummy(&e.ty);
                }
                let q = qty(self.p, &e.ty);
                let t = q.map(|q| self.temp(q));
                let mut first = true;
                let mut total = false;
                // every arm returns (and the fallback bails): the chain is a terminating statement
                let mut all_dead = true;
                for arm in arms {
                    let (cond, binds) = self.arm_test(&arm.pattern, &st, sq);
                    let head = match cond {
                        None => {
                            total = true;
                            String::from("{")
                        }
                        Some(c) => format!("if {c} {{"),
                    };
                    match (first, total) {
                        (true, _) => self.brace(&head),
                        (false, true) => self.brace("} else {"),
                        (false, false) => self.brace(&format!("}} else {head}")),
                    }
                    first = false;
                    self.indent += 1;
                    for (l, v) in binds {
                        self.line(&format!("q{} = {v}", l.0));
                    }
                    let v = self.block(&arm.body);
                    if let (Some(t), Some(v)) = (&t, v) {
                        self.line(&format!("{t} = {v}"));
                    }
                    all_dead &= std::mem::replace(&mut self.dead, false);
                    self.indent -= 1;
                    if total {
                        break;
                    }
                }
                if !total {
                    self.brace("} else {");
                    self.indent += 1;
                    self.fail();
                    self.dead = false;
                    self.indent -= 1;
                }
                self.brace("}");
                self.dead = all_dead;
                t.unwrap_or_else(|| "struct{}{}".to_string())
            }
            ExprKind::Call { callee, args, .. } => {
                if let Some(which) = builtin(self.p, *callee) {
                    return self.builtin(which, e, args);
                }
                let mut vals: Vec<String> = args.iter().map(|a| self.fixed(a)).collect();
                if self.q.plan.fns.get(callee) == Some(&true) {
                    vals.push(if *callee == self.def && self.measured && !self.in_contract {
                        "mself".to_string()
                    } else {
                        "cpl.QNone".to_string()
                    });
                }
                let q = self.qt(&e.ty);
                self.checked(
                    &format!(
                        "q_{}(cx, qd+1{})",
                        (self.short)(*callee),
                        vals.iter().map(|v| format!(", {v}")).collect::<String>()
                    ),
                    q,
                )
            }
            _ => {
                self.fail();
                self.dummy(&e.ty)
            }
        }
    }

    /// A match arm's condition (`None`: it always matches) and the bindings it makes, against `st` of type `sq`.
    fn arm_test(&mut self, pat: &Pattern, st: &str, sq: Q) -> (Option<String>, Vec<(LocalId, String)>) {
        let lit = |k: &PatternKind, part: &str| -> Option<String> {
            match k {
                PatternKind::Int(n) => Some(format!("{part} == int64({})", small_literal(n).unwrap_or(0))),
                PatternKind::Bool(b) => Some(format!("{part} == {b}")),
                PatternKind::Text(t) => Some(format!("{part} == {}", crate::go::go_str(t))),
                _ => None,
            }
        };
        let sub = |args: &[Pattern], part: &dyn Fn(usize) -> String, conds: &mut Vec<String>, binds: &mut Vec<(LocalId, String)>| {
            for (i, a) in args.iter().enumerate() {
                match &a.kind {
                    PatternKind::Bind(l) => binds.push((*l, part(i))),
                    k => {
                        if let Some(c) = lit(k, &part(i)) {
                            conds.push(c);
                        }
                    }
                }
            }
        };
        match (&pat.kind, sq) {
            (PatternKind::Wildcard, _) => (None, Vec::new()),
            (PatternKind::Bind(l), _) => (None, vec![(*l, st.to_string())]),
            (PatternKind::Ctor { variant, args, .. }, Q::Opt(_)) => {
                if *variant == 0 {
                    let mut conds = vec![format!("{st}.Has")];
                    let mut binds = Vec::new();
                    sub(args, &|_| format!("{st}.V"), &mut conds, &mut binds);
                    (Some(conds.join(" && ")), binds)
                } else {
                    (Some(format!("!{st}.Has")), Vec::new())
                }
            }
            (PatternKind::Ctor { variant, args, .. }, Q::Enum(_)) => {
                let mut conds = vec![format!("{st}.tag == {variant}")];
                let mut binds = Vec::new();
                sub(args, &|i| format!("{st}.v{variant}_{i}"), &mut conds, &mut binds);
                (Some(conds.join(" && ")), binds)
            }
            (k, _) => (Some(lit(k, st).unwrap_or_else(|| "true".to_string())), Vec::new()),
        }
    }

    /// A read of a slice: `len`, `is_empty`, or an element as an option (a copy).
    fn flat_read(&mut self, which: &str, args: &[Expr]) -> Option<String> {
        let ExprKind::Local(l) = args.first()?.kind else {
            return None;
        };
        let (el, q) = self.flat_var(l)?;
        let index = match which {
            "list_len" => return Some(format!("int64(len({q}))")),
            "list_is_empty" => return Some(format!("(len({q}) == 0)")),
            "list_get" => self.fixed(&args[1]),
            "list_first" => "int64(0)".to_string(),
            "list_last" => format!("(int64(len({q})) - 1)"),
            _ => return None,
        };
        let t = self.temp(Q::Opt(el));
        self.line(&format!("{t} = {}{{}}", self.q.gty(Q::Opt(el))));
        self.line(&format!("if k := {index}; k >= 0 && k < int64(len({q})) {{"));
        self.line(&format!("\t{t}.V, {t}.Has = {q}[k], true"));
        self.line("}");
        Some(t)
    }

    /// A list, option, or text builtin of the scalar subset.
    fn builtin(&mut self, which: &str, e: &Expr, args: &[Expr]) -> String {
        if let Some(r) = self.flat_read(which, args) {
            return r;
        }
        let el = match args.first().map(|a| self.qt(&a.ty)) {
            Some(Q::List(el) | Q::Opt(el)) => el,
            _ => El::Int,
        };
        match which {
            "list_len" => {
                let xs = self.fixed(&args[0]);
                format!("int64({xs}.Len())")
            }
            "list_is_empty" => {
                let xs = self.fixed(&args[0]);
                format!("{xs}.IsEmpty()")
            }
            "list_get" | "list_first" | "list_last" => {
                let xs = self.fixed(&args[0]);
                let i = match which {
                    "list_get" => self.fixed(&args[1]),
                    "list_first" => "int64(0)".to_string(),
                    _ => format!("(int64({xs}.Len()) - 1)"),
                };
                self.element(&xs, &i, el)
            }
            "list_push" => {
                let xs = self.fixed(&args[0]);
                let v = self.fixed(&args[1]);
                format!("{xs}.Push({})", self.q.word(el, &v))
            }
            "list_take" | "list_drop" => {
                let xs = self.fixed(&args[0]);
                let k = self.fixed(&args[1]);
                let f = if which == "list_take" { "cpl.QTake" } else { "cpl.QDrop" };
                self.checked(&format!("{f}({xs}, {k})"), self.qt(&e.ty))
            }
            "list_drop_last" => {
                let xs = self.fixed(&args[0]);
                format!("cpl.ListDropLast({xs})")
            }
            // an element list of a list of lists, or the default (the read's Option has no scalar form)
            "option_unwrap_or" if cpl_hir::scalar::nested_read(self.p, &args[0]) => {
                let ExprKind::Call { callee, args: inner, .. } = &args[0].kind else {
                    unreachable!("a nested read is a call")
                };
                let xs = self.fixed(&inner[0]);
                let i = match builtin(self.p, *callee) {
                    Some("list_get") => self.fixed(&inner[1]),
                    Some("list_first") => "int64(0)".to_string(),
                    _ => format!("(int64({xs}.Len()) - 1)"),
                };
                let d = self.fixed(&args[1]);
                let t = self.temp(self.qt(&e.ty));
                self.line(&format!("{t} = cpl.QGetOr({xs}, {i}, {d})"));
                t
            }
            "option_unwrap_or" => {
                let o = self.fixed(&args[0]);
                let d = self.fixed(&args[1]);
                let t = self.temp(self.qt(&e.ty));
                self.line(&format!("{t} = {d}"));
                self.line(&format!("if {o}.Has {{"));
                self.line(&format!("\t{t} = {o}.V"));
                self.line("}");
                t
            }
            "option_is_some" => {
                let o = self.fixed(&args[0]);
                format!("{o}.Has")
            }
            "option_is_none" => {
                let o = self.fixed(&args[0]);
                format!("(!{o}.Has)")
            }
            "text_len" => {
                let x = self.fixed(&args[0]);
                format!("cpl.QTextLen({x})")
            }
            "text_is_empty" => {
                let x = self.fixed(&args[0]);
                format!("({x} == \"\")")
            }
            "int_to_text" => {
                let v = self.fixed(&args[0]);
                format!("cpl.QIntText({v})")
            }
            _ => {
                self.fail();
                self.dummy(&e.ty)
            }
        }
    }

    /// Element `i` (an int64 expression) of the list `xs` of `el`, as an option (bails on a bigint).
    fn element(&mut self, xs: &str, i: &str, el: El) -> String {
        let t = self.temp(Q::Opt(el));
        if self.dead {
            return t;
        }
        match el {
            El::Int => {
                let ok = self.flag();
                self.line(&format!("{t}.V, {t}.Has, {ok} = cpl.QGetI({xs}, {i})"));
                self.bail_unless(&ok);
            }
            El::Bool => self.line(&format!("{t}.V, {t}.Has = cpl.QGetB({xs}, {i})")),
            El::Rec(d) => {
                self.line(&format!("{t} = cpl.QOpt[qR{}]{{}}", d.0));
                self.line(&format!("if p := cpl.QGetP({xs}, {i}); p != nil {{"));
                self.indent += 1;
                let ok = self.flag();
                self.line(&format!("{t}.V, {ok} = qUnbox{}(p)", d.0));
                self.bail_unless(&ok);
                self.line(&format!("{t}.Has = true"));
                self.indent -= 1;
                self.line("}");
            }
            El::List(_) => self.fail(),
        }
        t
    }

    fn binary(&mut self, op: BinaryOp, lhs: &Expr, rhs: &Expr) -> String {
        if matches!(op, BinaryOp::And | BinaryOp::Or) {
            let a = self.expr(lhs);
            let t = self.temp(Q::Bool);
            self.line(&format!("{t} = {a}"));
            if self.dead {
                return t;
            }
            self.line(&if op == BinaryOp::And { format!("if {t} {{") } else { format!("if !{t} {{") });
            self.indent += 1;
            let b = self.expr(rhs);
            self.line(&format!("{t} = {b}"));
            self.dead = false;
            self.indent -= 1;
            self.brace("}");
            return t;
        }
        let a = self.fixed(lhs);
        let b = self.fixed(rhs);
        if op == BinaryOp::Concat {
            return match self.qt(&lhs.ty) {
                Q::Text => format!("({a} + {b})"),
                _ => format!("cpl.ListConcat({a}, {b})"),
            };
        }
        match op {
            BinaryOp::Add => self.checked(&format!("cpl.QAdd({a}, {b})"), Q::Int),
            BinaryOp::Sub => self.checked(&format!("cpl.QSub({a}, {b})"), Q::Int),
            BinaryOp::Mul => self.checked(&format!("cpl.QMul({a}, {b})"), Q::Int),
            BinaryOp::Div => self.checked(&format!("cpl.QDiv({a}, {b})"), Q::Int),
            BinaryOp::Rem => self.checked(&format!("cpl.QRem({a}, {b})"), Q::Int),
            BinaryOp::Eq => format!("({a} == {b})"),
            BinaryOp::Ne => format!("({a} != {b})"),
            BinaryOp::Lt => format!("({a} < {b})"),
            BinaryOp::Le => format!("({a} <= {b})"),
            BinaryOp::Gt => format!("({a} > {b})"),
            BinaryOp::Ge => format!("({a} >= {b})"),
            _ => {
                self.fail();
                "0".to_string()
            }
        }
    }
}
