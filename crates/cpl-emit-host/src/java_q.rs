//! The Java host's scalar tier (`spec/hosts.md` H14): for every function of the scalar subset (`cpl_hir::scalar` with
//! `Caps::HOSTS`: lists of `Int`, `Bool`, records, and lists, options, texts, and enums), a static method `q_<name>` on
//! `long`, `boolean`, `String`, records of them (`Support.QR<n>`), enums (`Support.QE<n>`: a tag and every variant's
//! payload), the generic form's own lists (elements read in place), and options as a pair of locals (`<x>_v`,
//! `<x>_h`: an option never allocates). It throws `cpl.Q.BAIL` (or an `ArithmeticException` of `Math.addExact` and
//! the others) wherever the generic form could do anything but compute the same value. The generic `x_<name>` tries
//! it first and, on a bail, runs itself, which then raises exactly what it always did. The method bodies are
//! statements, with every local declared first; nothing is printed past a `return` or `throw` (Java rejects an
//! unreachable statement).

use cpl_hir::scalar::{builtin, fields, of_el, small_literal, variant_fields, Caps, El, Plan, Q};
use cpl_hir::{Block, DefId, Expr, ExprKind, FnBody, Lit, LocalId, Pattern, PatternKind, Program, Stmt, Ty};
use cpl_syntax::ast::{BinaryOp, UnaryOp};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

pub(crate) struct JavaQ {
    plan: Plan,
    /// The base package (`Support` holds the records), and the runtime package.
    base: String,
    rt: String,
    /// The generated type of each record and enum of the plan, set once names exist.
    pub(crate) gen: BTreeMap<DefId, String>,
    /// The generated record of each variant of an enum of the plan.
    pub(crate) variants: BTreeMap<(DefId, u32), String>,
    /// The accessor of each field of a record of the plan.
    pub(crate) accessors: BTreeMap<(DefId, usize), String>,
}

fn qty(p: &Program, ty: &Ty) -> Option<Q> {
    cpl_hir::scalar::qty(p, ty, Caps::HOSTS)
}

impl JavaQ {
    pub(crate) fn new(p: &Program, base: &str, rt: &str) -> JavaQ {
        JavaQ {
            plan: cpl_hir::scalar::plan(p, Caps::HOSTS),
            base: base.to_string(),
            rt: rt.to_string(),
            gen: BTreeMap::new(),
            variants: BTreeMap::new(),
            accessors: BTreeMap::new(),
        }
    }

    pub(crate) fn has(&self, d: DefId) -> bool {
        self.plan.fns.contains_key(&d)
    }

    /// The records and enums whose generated names the tier needs.
    pub(crate) fn named(&self) -> Vec<DefId> {
        self.plan.records.iter().chain(&self.plan.enums).copied().collect()
    }

    /// The Java type of a value of `q` (an option has none: it is a pair of locals).
    fn jty(&self, q: Q) -> String {
        match q {
            Q::Int => "long".to_string(),
            Q::Bool => "boolean".to_string(),
            Q::Text => "java.lang.String".to_string(),
            Q::Rec(d) => format!("{}.Support.QR{}", self.base, d.0),
            Q::Enum(d) => format!("{}.Support.QE{}", self.base, d.0),
            Q::List(el) => format!("{}.List<{}>", self.rt, self.elem(el)),
            Q::Opt(_) => "java.lang.Object".to_string(),
            Q::Word => unreachable!("Caps::HOSTS has no words"),
        }
    }

    /// The type of the generic form's list elements of `el`.
    fn elem(&self, el: El) -> String {
        match el {
            El::Int => format!("{}.Int", self.rt),
            El::Bool => "java.lang.Boolean".to_string(),
            El::Rec(d) => self.gen.get(&d).cloned().unwrap_or_default(),
            El::List(l) => format!("{}.List<{}>", self.rt, self.elem(cpl_hir::scalar::el_of_leaf(l))),
        }
    }

    fn zero(&self, q: Q) -> String {
        match q {
            Q::Int => "0L".to_string(),
            Q::Bool => "false".to_string(),
            Q::Text => "\"\"".to_string(),
            _ => "null".to_string(),
        }
    }

    /// An element of a list of `el` as the list holds it, from its speculative value `v`.
    fn word(&self, el: El, v: &str) -> String {
        match el {
            El::Int => format!("{}.Int.of({v})", self.rt),
            El::Rec(d) => format!("{}.Support.qBox{}({v})", self.base, d.0),
            _ => v.to_string(),
        }
    }

    /// The records and enums, as members of `Support`, with their conversions from and to the generated types.
    pub(crate) fn records(&self, p: &Program) -> String {
        let rt = &self.rt;
        let mut s = String::new();
        for r in &self.plan.records {
            let n = r.0;
            let fs = fields(p, *r);
            let comps: Vec<String> = fs.iter().enumerate().map(|(i, f)| format!("{} f{i}", self.jty(*f))).collect();
            let _ = writeln!(s, "    /** A record of the scalar tier: internal. */\n    public record QR{n}({}) {{\n    }}\n", comps.join(", "));
            let gen = &self.gen[r];
            // from the generated record: null when an Int does not fit in a long
            let mut unbox = String::new();
            let mut args = Vec::new();
            let mut boxed = Vec::new();
            for (i, f) in fs.iter().enumerate() {
                let get = format!("v.{}()", self.accessors[&(*r, i)]);
                match f {
                    Q::Int => {
                        let _ = writeln!(unbox, "        if (!{get}.fitsLong()) {{\n            return null;\n        }}");
                        args.push(format!("{get}.toLong()"));
                        boxed.push(format!("{rt}.Int.of(v.f{i}())"));
                    }
                    Q::Rec(m) => {
                        let _ = writeln!(unbox, "        QR{} u{i} = qUnbox{}({get});\n        if (u{i} == null) {{\n            return null;\n        }}", m.0, m.0);
                        args.push(format!("u{i}"));
                        boxed.push(format!("qBox{}(v.f{i}())", m.0));
                    }
                    _ => {
                        args.push(get);
                        boxed.push(format!("v.f{i}()"));
                    }
                }
            }
            let _ = writeln!(
                s,
                "    public static QR{n} qUnbox{n}({gen} v) {{\n{unbox}        return new QR{n}({});\n    }}\n",
                args.join(", ")
            );
            let _ = writeln!(s, "    public static {gen} qBox{n}(QR{n} v) {{\n        return new {gen}({});\n    }}\n", boxed.join(", "));
        }
        for d in &self.plan.enums {
            s.push_str(&self.enum_type(p, *d));
        }
        s
    }

    /// The members of an enum's record after its tag: `v<variant>_<i>` for each payload field.
    fn enum_members(&self, p: &Program, d: DefId) -> Vec<(u32, usize, Q)> {
        let nv = p.def(d).kind.as_enum().map(|e| e.variants.len()).unwrap_or(0) as u32;
        let mut out = Vec::new();
        for v in 0..nv {
            for (i, f) in variant_fields(p, d, v).into_iter().enumerate() {
                out.push((v, i, f));
            }
        }
        out
    }

    fn enum_type(&self, p: &Program, d: DefId) -> String {
        let rt = &self.rt;
        let n = d.0;
        let nv = p.def(d).kind.as_enum().map(|e| e.variants.len()).unwrap_or(0) as u32;
        let gen = &self.gen[&d];
        let members = self.enum_members(p, d);
        let mut comps = vec!["int tag".to_string()];
        for (v, i, f) in &members {
            comps.push(format!("{} v{v}_{i}", self.jty(*f)));
        }
        let mut s = String::new();
        let _ = writeln!(s, "    /** An enum of the scalar tier: internal. */\n    public record QE{n}({}) {{\n    }}\n", comps.join(", "));
        // unbox: null for a null value and a payload Int that does not fit in a long
        let _ = writeln!(s, "    public static QE{n} qUnboxE{n}({gen} v) {{");
        for v in 0..nv {
            let vt = &self.variants[&(d, v)];
            let _ = writeln!(s, "        if (v instanceof {vt} x) {{");
            let mut args = vec![v.to_string()];
            for (mv, mi, mf) in &members {
                if *mv == v {
                    if *mf == Q::Int {
                        let _ = writeln!(s, "            if (!x.f{mi}().fitsLong()) {{\n                return null;\n            }}");
                        args.push(format!("x.f{mi}().toLong()"));
                    } else {
                        args.push(format!("x.f{mi}()"));
                    }
                } else {
                    args.push(self.zero(*mf));
                }
            }
            let _ = writeln!(s, "            return new QE{n}({});\n        }}", args.join(", "));
        }
        s.push_str("        return null;\n    }\n\n");
        let _ = writeln!(s, "    public static {gen} qBoxE{n}(QE{n} v) {{\n        switch (v.tag()) {{");
        for v in 0..nv {
            let vt = &self.variants[&(d, v)];
            let parts: Vec<String> = members
                .iter()
                .filter(|(mv, _, _)| *mv == v)
                .map(|(_, mi, mf)| if *mf == Q::Int { format!("{rt}.Int.of(v.v{v}_{mi}())") } else { format!("v.v{v}_{mi}()") })
                .collect();
            let _ = writeln!(s, "            case {v}:\n                return new {vt}({});", parts.join(", "));
        }
        s.push_str("            default:\n                throw new java.lang.IllegalStateException(\"no such variant\");\n        }\n    }\n\n");
        s
    }

    /// The lines that open `x_<name>`: the speculative call; on a bail the generic form follows. `params` are the
    /// Java names of the parameters, `id` the function's measure id, `class` the module class of `d`.
    pub(crate) fn wrapper(&self, p: &Program, d: DefId, params: &[String], id: u32, class: &str, name: &str) -> String {
        let rt = &self.rt;
        let fd = p.def(d).kind.as_fn().expect("a function");
        let mut conds = Vec::new();
        let mut lets = String::new();
        let mut args = Vec::new();
        for (i, (param, v)) in fd.params.iter().zip(params).enumerate() {
            match qty(p, &param.ty) {
                Some(Q::Int) => {
                    conds.push(format!("{v}.fitsLong()"));
                    args.push(format!("{v}.toLong()"));
                }
                Some(Q::Bool | Q::Text | Q::List(_)) => args.push(v.clone()),
                Some(Q::Rec(r)) => {
                    let _ = writeln!(lets, "            final {} a{i} = {}.Support.qUnbox{}({v});", self.jty(Q::Rec(r)), self.base, r.0);
                    conds.push(format!("a{i} != null"));
                    args.push(format!("a{i}"));
                }
                Some(Q::Enum(r)) => {
                    let _ = writeln!(lets, "            final {} a{i} = {}.Support.qUnboxE{}({v});", self.jty(Q::Enum(r)), self.base, r.0);
                    conds.push(format!("a{i} != null"));
                    args.push(format!("a{i}"));
                }
                _ => return String::new(),
            }
        }
        if self.plan.fns[&d] {
            args.push(format!("cx.qPrev({id})"));
        }
        let call = format!("{class}.q_{name}(cx, cx.depth(){})", args.iter().map(|a| format!(", {a}")).collect::<String>());
        let boxed = match qty(p, &fd.ret) {
            Some(Q::Int) => format!("{rt}.Int.of({call})"),
            Some(Q::Rec(r)) => format!("{}.Support.qBox{}({call})", self.base, r.0),
            Some(Q::Enum(r)) => format!("{}.Support.qBoxE{}({call})", self.base, r.0),
            _ => call,
        };
        let cond = if conds.is_empty() { "true".to_string() } else { conds.join(" && ") };
        format!(
            "        if (cx.qOk()) {{\n{lets}            if ({cond}) {{\n                try {{\n                    return {boxed};\n                }} catch ({rt}.Q.Bail | java.lang.ArithmeticException | java.lang.StackOverflowError $q) {{\n                    cx.qBail();\n                }}\n            }}\n        }}\n"
        )
    }

    /// `q_<name>` (contracts and measure, and the body in place when there is no `ensures`) and `qb_<name>`.
    pub(crate) fn functions(&self, p: &Program, d: DefId, class: &dyn Fn(DefId) -> String) -> String {
        let rt = &self.rt;
        let fd = p.def(d).kind.as_fn().expect("a function");
        let FnBody::Block(body) = &fd.body else {
            return String::new();
        };
        let name = crate::java::ident(&p.def(d).name.display());
        let measured = self.plan.fns[&d];
        let rq = qty(p, &fd.ret).expect("a scalar result");
        let rty = self.jty(rq);
        let params: Vec<String> = fd
            .params
            .iter()
            .map(|x| format!("{} q{}", self.jty(qty(p, &x.ty).expect("a scalar parameter")), x.local.0))
            .collect();
        let pset: BTreeSet<LocalId> = fd.params.iter().map(|x| x.local).collect();
        let locals: Vec<(LocalId, Q)> = fd
            .locals
            .iter()
            .filter(|l| !pset.contains(&l.id))
            .filter_map(|l| qty(p, &l.ty).map(|q| (l.id, q)))
            .collect();
        // the texts a statement appends to: a `TextBuf` each, whose `String` is made when it is read
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
        // the list locals that never leave the method: arrays with a count (cpl_hir::scalar::flat_locals)
        let flat = cpl_hir::scalar::flat_locals(p, fd, Caps::HOSTS);
        // the list locals built as arrays and then made lists (cpl_hir::scalar::built_locals)
        let built = cpl_hir::scalar::built_locals(p, fd, Caps::HOSTS);
        let decl_locals = |s: &mut String| {
            for (l, q) in &locals {
                if let Some(el) = flat.get(l) {
                    let t = self.jty(of_el(*el));
                    let _ = writeln!(s, "        {t}[] q{n} = new {t}[0];\n        int q{n}_n = 0;", n = l.0);
                    continue;
                }
                if let Some(b) = built.get(l) {
                    let t = self.jty(of_el(b.el));
                    let _ = writeln!(s, "        {t}[] qf{n} = new {t}[0];\n        int qf{n}_n = 0;", n = l.0);
                }
                match q {
                    Q::Opt(el) => {
                        let vq = of_el(*el);
                        let _ = writeln!(s, "        {} q{n}_v = {};\n        boolean q{n}_h = false;", self.jty(vq), self.zero(vq), n = l.0);
                    }
                    Q::Text if appended.contains(l) => {
                        let _ = writeln!(s, "        final {rt}.TextBuf q{} = new {rt}.TextBuf(\"\");", l.0);
                    }
                    _ => {
                        let _ = writeln!(s, "        {} q{} = {};", self.jty(*q), l.0, self.zero(*q));
                    }
                }
            }
        };
        let mut s = String::new();
        // the contracts and the measure
        let mut e = Emitter::new(p, self, d, measured, true, class, &appended, &flat, &built);
        e.line("if (!cx.qEnter(qd)) {");
        e.line(&format!("    throw {rt}.Q.BAIL;"));
        e.line("}");
        for c in &fd.requires {
            let v = e.expr(&c.expr);
            e.bail_unless(&v);
        }
        let mut body_args: Vec<String> = fd.params.iter().map(|x| format!("q{}", x.local.0)).collect();
        if let Some(c) = &fd.decreases {
            let v = e.expr(&c.expr);
            e.line(&format!("final long mself = {v};"));
            e.bail_unless(&format!("mself >= 0L && (mprev == {rt}.Q.NONE || mself < mprev)"));
            body_args.push("mself".to_string());
        }
        let head = format!(
            "    /** The speculative form of {{@code {name}}}: internal. */\n    public static {rty} q_{name}({}.Cx cx, int qd{}{}) {{\n",
            rt,
            params.iter().map(|x| format!(", {x}")).collect::<String>(),
            if measured { ", long mprev" } else { "" }
        );
        if fd.ensures.is_empty() {
            // nothing runs after the body: it follows in place
            e.in_contract = false;
            if let Some(v) = e.body(body) {
                e.line(&format!("return {v};"));
            } else {
                e.fail();
            }
            s.push_str(&head);
            decl_locals(&mut s);
            e.finish(&mut s);
            s.push_str("    }\n\n");
            return s;
        }
        e.line(&format!(
            "final {rty} r = {}.qb_{name}(cx, qd{});",
            class(d),
            body_args.iter().map(|a| format!(", {a}")).collect::<String>()
        ));
        // the measure is recorded by now (S35): a call of this function compares with it
        e.in_contract = false;
        if let Some(res) = fd.result_local {
            if locals.iter().any(|(l, _)| *l == res) {
                e.line(&format!("q{} = r;", res.0));
            } else {
                e.line(&format!("final {rty} q{} = r;", res.0));
            }
        }
        for c in &fd.ensures {
            let v = e.expr(&c.expr);
            e.bail_unless(&v);
        }
        e.line("return r;");
        s.push_str(&head);
        decl_locals(&mut s);
        e.finish(&mut s);
        s.push_str("    }\n\n");
        // the body
        let mut e = Emitter::new(p, self, d, measured, false, class, &appended, &flat, &built);
        if let Some(v) = e.body(body) {
            e.line(&format!("return {v};"));
        } else {
            e.fail();
        }
        let _ = writeln!(
            s,
            "    /** The body of the speculative form of {{@code {name}}}: internal. */\n    public static {rty} qb_{name}({}.Cx cx, int qd{}{}) {{",
            rt,
            params.iter().map(|x| format!(", {x}")).collect::<String>(),
            if measured { ", long mself" } else { "" }
        );
        decl_locals(&mut s);
        e.finish(&mut s);
        s.push_str("    }\n\n");
        s
    }
}

/// The Java printer of one speculative method.
struct Emitter<'a> {
    p: &'a Program,
    q: &'a JavaQ,
    def: DefId,
    measured: bool,
    in_contract: bool,
    class: &'a dyn Fn(DefId) -> String,
    /// The text locals that are `TextBuf`s.
    appended: &'a BTreeSet<LocalId>,
    /// The list locals held as arrays with a count (`q<n>`, `q<n>_n`).
    flat: &'a BTreeMap<LocalId, El>,
    /// The list locals built as arrays `qf<n>`, `qf<n>_n`, then made the lists `q<n>` (those of `frozen` by now).
    built: &'a BTreeMap<LocalId, cpl_hir::scalar::Built>,
    frozen: BTreeSet<LocalId>,
    out: String,
    decls: Vec<(String, String, String)>,
    indent: usize,
    next: u32,
    /// Past a `return` or `throw` in the current block: nothing more is printed there.
    dead: bool,
}

impl<'a> Emitter<'a> {
    fn new(
        p: &'a Program,
        q: &'a JavaQ,
        def: DefId,
        measured: bool,
        in_contract: bool,
        class: &'a dyn Fn(DefId) -> String,
        appended: &'a BTreeSet<LocalId>,
        flat: &'a BTreeMap<LocalId, El>,
        built: &'a BTreeMap<LocalId, cpl_hir::scalar::Built>,
    ) -> Emitter<'a> {
        Emitter {
            p,
            q,
            def,
            measured,
            in_contract,
            class,
            appended,
            flat,
            built,
            frozen: BTreeSet::new(),
            out: String::new(),
            decls: Vec::new(),
            indent: 2,
            next: 0,
            dead: false,
        }
    }

    fn finish(&self, s: &mut String) {
        for (t, n, z) in &self.decls {
            let _ = writeln!(s, "        {t} {n} = {z};");
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
            self.out.push_str("    ");
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    /// A fresh local of type `q` (an option: the pair `<t>_v`, `<t>_h`).
    fn temp(&mut self, q: Q) -> String {
        self.next += 1;
        let t = format!("qt{}", self.next);
        match q {
            Q::Opt(el) => {
                let vq = of_el(el);
                self.decls.push((self.q.jty(vq), format!("{t}_v"), self.q.zero(vq)));
                self.decls.push(("boolean".to_string(), format!("{t}_h"), "false".to_string()));
            }
            _ => self.decls.push((self.q.jty(q), t.clone(), self.q.zero(q))),
        }
        t
    }

    fn qt(&self, ty: &Ty) -> Q {
        qty(self.p, ty).unwrap_or(Q::Int)
    }

    fn fail(&mut self) {
        let b = format!("throw {}.Q.BAIL;", self.q.rt);
        self.line(&b);
        self.dead = true;
    }

    fn bail_unless(&mut self, cond: &str) {
        if self.dead {
            return;
        }
        self.line(&format!("if (!({cond})) {{"));
        self.indent += 1;
        self.fail();
        self.dead = false;
        self.indent -= 1;
        self.line("}");
    }

    /// `dst = v` for a value of type `q`: an option, both of its locals.
    fn assign(&mut self, dst: &str, q: Q, v: &str) {
        match q {
            Q::Opt(_) => self.line(&format!("{dst}_v = {v}_v; {dst}_h = {v}_h;")),
            _ => self.line(&format!("{dst} = {v};")),
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

    /// The array holding list local `l` (with its count `<name>_n`) and its element, while it is one.
    fn flat_var(&self, l: LocalId) -> Option<(El, String)> {
        if let Some(el) = self.flat.get(&l) {
            return Some((*el, format!("q{}", l.0)));
        }
        let b = self.built.get(&l)?;
        (!self.frozen.contains(&l)).then(|| (b.el, format!("qf{}", l.0)))
    }

    /// The method's body: each built list made from its array before the statement that first reads it as a list.
    fn body(&mut self, b: &Block) -> Option<String> {
        let built: Vec<(LocalId, cpl_hir::scalar::Built)> = self.built.iter().map(|(l, x)| (*l, *x)).collect();
        let rt = self.q.rt.clone();
        for i in 0..=b.stmts.len() {
            if self.dead {
                return None;
            }
            for (l, x) in built.iter().filter(|(_, x)| x.at == i) {
                let n = l.0;
                let made = match x.el {
                    El::Int => format!("{rt}.Q.freezeI(qf{n}, qf{n}_n)"),
                    El::Bool => format!("{rt}.Q.freezeB(qf{n}, qf{n}_n)"),
                    El::Rec(d) => format!("{rt}.Q.freeze(qf{n}, qf{n}_n, {}.Support::qBox{})", self.q.base, d.0),
                    El::List(_) => unreachable!("a built list holds no lists"),
                };
                self.line(&format!("q{n} = {made};"));
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

    /// An expression evaluated for its bails alone.
    fn discard(&mut self, e: &Expr) {
        let v = self.expr(e);
        if let Some(q) = qty(self.p, &e.ty) {
            let t = self.temp(q);
            self.assign(&t, q, &v);
        }
    }

    fn stmt(&mut self, st: &Stmt) {
        match st {
            // an array: `[]` empties it, its own push appends (grown by doubling)
            Stmt::Let { local, value } | Stmt::Assign { local, value } if self.flat_var(*local).is_some() => {
                let (_, q) = self.flat_var(*local).expect("an array");
                match &value.kind {
                    ExprKind::Call { args, .. } if args.len() == 2 => {
                        let v = self.fixed(&args[1]);
                        let rt = self.q.rt.clone();
                        self.line(&format!("if ({q}_n == {q}.length) {{"));
                        self.line(&format!("    {q} = {rt}.Q.grow({q});"));
                        self.line("}");
                        self.line(&format!("{q}[{q}_n++] = {v};"));
                    }
                    _ => self.line(&format!("{q}_n = 0;")),
                }
            }
            Stmt::Let { local, value } | Stmt::Assign { local, value } => {
                if self.appended.contains(local) {
                    // a TextBuf: appended to (every part first: each sees the old text), or set
                    if let Some(parts) = cpl_hir::scalar::text_append_chain(self.p, *local, value) {
                        let xs: Vec<String> = parts.iter().map(|x| self.fixed(x)).collect();
                        for x in xs {
                            self.line(&format!("q{}.append({x});", local.0));
                        }
                        return;
                    }
                    let v = self.expr(value);
                    self.line(&format!("q{}.set({v});", local.0));
                    return;
                }
                let v = self.expr(value);
                let q = self.qt(&value.ty);
                self.assign(&format!("q{}", local.0), q, &v);
            }
            Stmt::Discard(e) | Stmt::Expr(e) => self.discard(e),
            Stmt::Return(Some(e)) => {
                let v = self.expr(e);
                self.line(&format!("return {v};"));
                self.dead = true;
            }
            Stmt::Assert { expr, .. } => {
                let v = self.expr(expr);
                self.bail_unless(&v);
            }
            Stmt::While { cond, body, .. } => {
                // the condition may print statements of its own: they run on every iteration
                self.line("for (;;) {");
                self.indent += 1;
                let c = self.expr(cond);
                // a condition that always bails: the loop never ends normally
                let stuck = self.dead;
                if !stuck {
                    self.line(&format!("if (!({c})) {{"));
                    self.line("    break;");
                    self.line("}");
                    let _ = self.block(body);
                }
                self.dead = stuck;
                self.indent -= 1;
                self.brace("}");
            }
            Stmt::Return(None) | Stmt::Concurrent(_) | Stmt::Context { .. } => self.fail(),
        }
    }

    fn fixed(&mut self, e: &Expr) -> String {
        let v = self.expr(e);
        let q = self.qt(&e.ty);
        let t = self.temp(q);
        self.assign(&t, q, &v);
        t
    }

    /// A stand-in for an expression past a bail: never printed, but of the right type.
    fn dummy(&mut self, ty: &Ty) -> String {
        match qty(self.p, ty) {
            Some(q @ Q::Opt(_)) => self.temp(q),
            Some(q) => self.q.zero(q),
            None => "0L".to_string(),
        }
    }

    fn expr(&mut self, e: &Expr) -> String {
        if self.dead {
            return self.dummy(&e.ty);
        }
        let rt = self.q.rt.clone();
        match &e.kind {
            ExprKind::Lit(Lit::Int(n)) => match small_literal(n) {
                Some(i64::MIN) => "java.lang.Long.MIN_VALUE".to_string(),
                Some(v) => format!("{v}L"),
                None => {
                    self.fail();
                    "0L".to_string()
                }
            },
            ExprKind::Lit(Lit::Bool(b)) => b.to_string(),
            ExprKind::Lit(Lit::Text(t)) => crate::java::java_str(t),
            ExprKind::Local(id) if self.appended.contains(id) => format!("q{}.get()", id.0),
            ExprKind::Local(id) => format!("q{}", id.0),
            ExprKind::Widen { expr, .. } => self.expr(expr),
            ExprKind::Ctor { def, variant, args } if *def == self.p.std_defs.option => {
                let q = self.qt(&e.ty);
                let v = args.first().map(|a| self.fixed(a));
                let t = self.temp(q);
                match (variant, v) {
                    (0, Some(v)) => self.line(&format!("{t}_v = {v}; {t}_h = true;")),
                    _ => {
                        let z = match q {
                            Q::Opt(el) => self.q.zero(of_el(el)),
                            _ => "0L".to_string(),
                        };
                        self.line(&format!("{t}_v = {z}; {t}_h = false;"));
                    }
                }
                t
            }
            ExprKind::Ctor { def, variant, args } if self.qt(&e.ty) == Q::Enum(*def) => {
                let vals: Vec<String> = args.iter().map(|a| self.fixed(a)).collect();
                let mut parts = vec![variant.to_string()];
                for (v, i, f) in self.q.enum_members(self.p, *def) {
                    parts.push(if v == *variant { vals[i].clone() } else { self.q.zero(f) });
                }
                format!("new {}({})", self.q.jty(Q::Enum(*def)), parts.join(", "))
            }
            ExprKind::List(items) => {
                let el = match self.qt(&e.ty) {
                    Q::List(el) => el,
                    _ => El::Int,
                };
                let vals: Vec<String> = items.iter().map(|a| self.fixed(a)).collect();
                let elem = self.q.elem(el);
                if vals.is_empty() {
                    return format!("{rt}.List.<{elem}>empty()");
                }
                let words: Vec<String> = vals.iter().map(|v| self.q.word(el, v)).collect();
                format!("{rt}.List.<{elem}>lit({})", words.join(", "))
            }
            ExprKind::Field { base, index, .. } => {
                let b = match &base.kind {
                    ExprKind::Local(id) if !self.appended.contains(id) => format!("q{}", id.0),
                    _ => self.fixed(base),
                };
                format!("{b}.f{index}()")
            }
            ExprKind::StructLit { def, spread, fields: given, .. } => {
                let base = spread.as_ref().map(|b| self.fixed(b));
                let mut vals: Vec<Option<String>> = vec![None; fields(self.p, *def).len()];
                for (i, f) in given {
                    let v = self.fixed(f);
                    if let Some(slot) = vals.get_mut(*i as usize) {
                        *slot = Some(v);
                    }
                }
                let args: Vec<String> = vals
                    .into_iter()
                    .enumerate()
                    .map(|(i, v)| match (v, &base) {
                        (Some(v), _) => v,
                        (None, Some(b)) => format!("{b}.f{i}()"),
                        (None, None) => "0L".to_string(),
                    })
                    .collect();
                format!("new {}({})", self.q.jty(Q::Rec(*def)), args.join(", "))
            }
            ExprKind::Unary { op, operand } => {
                let a = self.fixed(operand);
                match op {
                    UnaryOp::Not => format!("(!{a})"),
                    UnaryOp::Neg => format!("java.lang.Math.negateExact({a})"),
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
                self.line(&format!("if ({c}) {{"));
                self.indent += 1;
                let v = self.block(then_block);
                if let (Some(t), Some(v), Some(q)) = (&t, v, q) {
                    self.assign(t, q, &v);
                }
                let then_dead = std::mem::replace(&mut self.dead, false);
                self.indent -= 1;
                let mut else_dead = false;
                if let Some(b) = else_block {
                    self.brace("} else {");
                    self.indent += 1;
                    let v = self.block(b);
                    if let (Some(t), Some(v), Some(q)) = (&t, v, q) {
                        self.assign(t, q, &v);
                    }
                    else_dead = std::mem::replace(&mut self.dead, false);
                    self.indent -= 1;
                }
                self.brace("}");
                // both branches leave: the `if` completes only abruptly
                self.dead = then_dead && else_dead;
                match t {
                    Some(t) => t,
                    None => self.dummy(&e.ty),
                }
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
                let mut all_dead = true;
                for arm in arms {
                    let (cond, binds) = self.arm_test(&arm.pattern, &st, sq);
                    let head = match cond {
                        None => {
                            total = true;
                            String::from("{")
                        }
                        Some(c) => format!("if ({c}) {{"),
                    };
                    match (first, total) {
                        (true, _) => self.brace(&head),
                        (false, true) => self.brace("} else {"),
                        (false, false) => self.brace(&format!("}} else {head}")),
                    }
                    first = false;
                    self.indent += 1;
                    for (l, v) in binds {
                        self.line(&format!("q{} = {v};", l.0));
                    }
                    let v = self.block(&arm.body);
                    if let (Some(t), Some(v), Some(q)) = (&t, v, q) {
                        self.assign(t, q, &v);
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
                match t {
                    Some(t) => t,
                    None => self.dummy(&e.ty),
                }
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
                        format!("{rt}.Q.NONE")
                    });
                }
                let name = crate::java::ident(&self.p.def(*callee).name.display());
                let call = format!(
                    "{}.q_{name}(cx, qd + 1{})",
                    (self.class)(*callee),
                    vals.iter().map(|v| format!(", {v}")).collect::<String>()
                );
                // the call made here, in order with the statements around it
                let t = self.temp(self.qt(&e.ty));
                self.line(&format!("{t} = {call};"));
                t
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
                PatternKind::Int(n) => Some(format!("{part} == {}L", small_literal(n).unwrap_or(0))),
                PatternKind::Bool(b) => Some(format!("{part} == {b}")),
                PatternKind::Text(t) => Some(format!("{part}.equals({})", crate::java::java_str(t))),
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
                    let mut conds = vec![format!("{st}_h")];
                    let mut binds = Vec::new();
                    sub(args, &|_| format!("{st}_v"), &mut conds, &mut binds);
                    (Some(conds.join(" && ")), binds)
                } else {
                    (Some(format!("!{st}_h")), Vec::new())
                }
            }
            (PatternKind::Ctor { variant, args, .. }, Q::Enum(_)) => {
                let mut conds = vec![format!("{st}.tag() == {variant}")];
                let mut binds = Vec::new();
                sub(args, &|i| format!("{st}.v{variant}_{i}()"), &mut conds, &mut binds);
                (Some(conds.join(" && ")), binds)
            }
            (k, _) => (Some(lit(k, st).unwrap_or_else(|| "true".to_string())), Vec::new()),
        }
    }

    /// A read of an array: `len`, `is_empty`, or an element as an option (a copy).
    fn flat_read(&mut self, which: &str, args: &[Expr]) -> Option<String> {
        let ExprKind::Local(l) = args.first()?.kind else {
            return None;
        };
        let (el, q) = self.flat_var(l)?;
        let index = match which {
            "list_len" => return Some(format!("((long) {q}_n)")),
            "list_is_empty" => return Some(format!("({q}_n == 0)")),
            "list_get" => self.fixed(&args[1]),
            "list_first" => "0L".to_string(),
            "list_last" => format!("((long) {q}_n - 1L)"),
            _ => return None,
        };
        let t = self.temp(Q::Opt(el));
        let z = self.q.zero(of_el(el));
        self.line(&format!("{t}_v = {z}; {t}_h = false;"));
        self.line(&format!("if ({index} >= 0L && {index} < (long) {q}_n) {{"));
        self.line(&format!("    {t}_v = {q}[(int) {index}]; {t}_h = true;"));
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
                format!("((long) {xs}.size())")
            }
            "list_is_empty" => {
                let xs = self.fixed(&args[0]);
                format!("{xs}.isEmpty()")
            }
            "list_get" | "list_first" | "list_last" => {
                let xs = self.fixed(&args[0]);
                let i = match which {
                    "list_get" => self.fixed(&args[1]),
                    "list_first" => "0L".to_string(),
                    _ => format!("((long) {xs}.size() - 1L)"),
                };
                self.element(&xs, &i, el)
            }
            "list_push" => {
                let xs = self.fixed(&args[0]);
                let v = self.fixed(&args[1]);
                format!("{xs}.push({})", self.q.word(el, &v))
            }
            "list_take" | "list_drop" => {
                let xs = self.fixed(&args[0]);
                let k = self.fixed(&args[1]);
                self.bail_unless(&format!("{k} >= 0L"));
                let n = format!("(int) java.lang.Math.min({k}, (long) {xs}.size())");
                if which == "list_take" {
                    format!("{xs}.slice(0, {n})")
                } else {
                    format!("{xs}.slice({n}, {xs}.size())")
                }
            }
            "list_drop_last" => {
                let xs = self.fixed(&args[0]);
                format!("{xs}.slice(0, java.lang.Math.max({xs}.size() - 1, 0))")
            }
            // an element list of a list of lists, or the default (the read's Option has no scalar form)
            "option_unwrap_or" if cpl_hir::scalar::nested_read(self.p, &args[0]) => {
                let ExprKind::Call { callee, args: inner, .. } = &args[0].kind else {
                    unreachable!("a nested read is a call")
                };
                let xs = self.fixed(&inner[0]);
                let i = match builtin(self.p, *callee) {
                    Some("list_get") => self.fixed(&inner[1]),
                    Some("list_first") => "0L".to_string(),
                    _ => format!("((long) {xs}.size() - 1L)"),
                };
                let d = self.fixed(&args[1]);
                format!("(({i}) >= 0L && ({i}) < (long) {xs}.size() ? {xs}.get((int) ({i})) : {d})")
            }
            "option_unwrap_or" => {
                let o = self.fixed(&args[0]);
                let d = self.fixed(&args[1]);
                format!("({o}_h ? {o}_v : {d})")
            }
            "option_is_some" => {
                let o = self.fixed(&args[0]);
                format!("{o}_h")
            }
            "option_is_none" => {
                let o = self.fixed(&args[0]);
                format!("(!{o}_h)")
            }
            "text_len" => {
                let x = self.fixed(&args[0]);
                format!("((long) {x}.codePointCount(0, {x}.length()))")
            }
            "text_is_empty" => {
                let x = self.fixed(&args[0]);
                format!("{x}.isEmpty()")
            }
            "int_to_text" => {
                let v = self.fixed(&args[0]);
                format!("java.lang.Long.toString({v})")
            }
            _ => {
                self.fail();
                self.dummy(&e.ty)
            }
        }
    }

    /// Element `i` (a long expression) of the list `xs` of `el`, as an option (bails on a bigint).
    fn element(&mut self, xs: &str, i: &str, el: El) -> String {
        let t = self.temp(Q::Opt(el));
        if self.dead {
            return t;
        }
        let rt = self.q.rt.clone();
        let z = self.q.zero(of_el(el));
        self.line(&format!("{t}_v = {z}; {t}_h = false;"));
        self.line(&format!("if ({i} >= 0L && {i} < (long) {xs}.size()) {{"));
        self.indent += 1;
        let w = format!("{xs}.get((int) {i})");
        match el {
            El::Int => {
                self.next += 1;
                let wn = format!("qw{}", self.next);
                self.decls.push((format!("{rt}.Int"), wn.clone(), "null".to_string()));
                self.line(&format!("{wn} = {w};"));
                self.bail_unless(&format!("{wn}.fitsLong()"));
                self.line(&format!("{t}_v = {wn}.toLong();"));
            }
            El::Bool => self.line(&format!("{t}_v = {w};")),
            El::Rec(d) => {
                self.line(&format!("{t}_v = {}.Support.qUnbox{}({w});", self.q.base, d.0));
                self.bail_unless(&format!("{t}_v != null"));
            }
            El::List(_) => self.fail(),
        }
        self.line(&format!("{t}_h = true;"));
        self.dead = false;
        self.indent -= 1;
        self.line("}");
        t
    }

    fn binary(&mut self, op: BinaryOp, lhs: &Expr, rhs: &Expr) -> String {
        if matches!(op, BinaryOp::And | BinaryOp::Or) {
            let a = self.expr(lhs);
            let t = self.temp(Q::Bool);
            self.line(&format!("{t} = {a};"));
            if self.dead {
                return t;
            }
            self.line(&if op == BinaryOp::And { format!("if ({t}) {{") } else { format!("if (!{t}) {{") });
            self.indent += 1;
            let b = self.expr(rhs);
            self.line(&format!("{t} = {b};"));
            // a right operand that always bails: the `if` still completes when it is not taken
            self.dead = false;
            self.indent -= 1;
            self.brace("}");
            return t;
        }
        let a = self.fixed(lhs);
        let b = self.fixed(rhs);
        let rt = self.q.rt.clone();
        let lq = self.qt(&lhs.ty);
        let objects = matches!(lq, Q::Rec(_) | Q::Enum(_) | Q::Text);
        match op {
            BinaryOp::Concat if lq == Q::Text => format!("({a} + {b})"),
            BinaryOp::Concat => format!("{rt}.List.concat({a}, {b})"),
            BinaryOp::Add => format!("java.lang.Math.addExact({a}, {b})"),
            BinaryOp::Sub => format!("java.lang.Math.subtractExact({a}, {b})"),
            BinaryOp::Mul => format!("java.lang.Math.multiplyExact({a}, {b})"),
            BinaryOp::Div => format!("{rt}.Q.div({a}, {b})"),
            BinaryOp::Rem => format!("({a} % {b})"),
            BinaryOp::Eq if objects => format!("{a}.equals({b})"),
            BinaryOp::Ne if objects => format!("!{a}.equals({b})"),
            BinaryOp::Eq => format!("({a} == {b})"),
            BinaryOp::Ne => format!("({a} != {b})"),
            BinaryOp::Lt => format!("({a} < {b})"),
            BinaryOp::Le => format!("({a} <= {b})"),
            BinaryOp::Gt => format!("({a} > {b})"),
            BinaryOp::Ge => format!("({a} >= {b})"),
            _ => {
                self.fail();
                "0L".to_string()
            }
        }
    }
}
