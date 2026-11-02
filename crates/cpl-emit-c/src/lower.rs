//! The lowering of `spec/cabi.md` 1.1-1.4: every body in A-normal form, each subexpression in its own temporary in the
//! evaluation order of semantics 6.2 (crates/cpl-eval/src/interp/eval.rs), printed under the calling convention of
//! 1.2 and the slot discipline of 1.3. One `Lowerer` emits one C function.

use crate::print::{c_bytes, comment_text};
use crate::tables::{
    has_block_body, ConstKey, HoleRow, Tables, CK_DECREASES, CK_ENSURES, CK_INVARIANT, CK_REQUIRES,
};
use cpl_diag::Span;
use cpl_hir::{
    Arm, Block, Clause, DefId, DefKind, Expr, ExprKind, FnBody, GenExpr, LocalId, Pattern,
    PatternKind, Program, Stmt, Ty,
};
use cpl_rtspec::messages::{self, render};
use cpl_syntax::ast::{BinaryOp, UnaryOp};
use num_bigint::{BigInt, Sign};
use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;

/// The macros (`CPL_D`, `CPL_S`, ...) the program unit uses, defined at its top.
#[derive(Default)]
pub(crate) struct Names {
    pub used: BTreeSet<(u8, u32)>,
}

pub(crate) const N_D: u8 = 0;
pub(crate) const N_S: u8 = 1;
pub(crate) const N_CL: u8 = 2;
pub(crate) const N_H: u8 = 3;
pub(crate) const N_A: u8 = 4;
pub(crate) const N_K: u8 = 5;
pub(crate) const N_R: u8 = 6;

impl Names {
    fn d(&mut self, n: u32) -> String {
        self.used.insert((N_D, n));
        format!("CPL_D{n}")
    }
    fn s(&mut self, n: u32) -> String {
        self.used.insert((N_S, n));
        format!("CPL_S{n}")
    }
    fn cl(&mut self, n: u32) -> String {
        self.used.insert((N_CL, n));
        format!("CPL_CL{n}")
    }
    fn h(&mut self, n: u32) -> String {
        self.used.insert((N_H, n));
        format!("CPL_H{n}")
    }
    fn a(&mut self, n: u32) -> String {
        self.used.insert((N_A, n));
        format!("CPL_A{n}")
    }
    fn k(&mut self, n: u32) -> String {
        self.used.insert((N_K, n));
        format!("CPL_K{n}")
    }
    fn r(&mut self, n: u32) -> String {
        self.used.insert((N_R, n));
        format!("CPL_R{n}")
    }
}

/// An operand of A-normal form.
#[derive(Clone, Debug)]
enum Op {
    /// a constant expression with no ownership: a small `Int`, a `Bool`, an immortal constant
    Imm(String),
    /// a word borrowed from a slot, a parameter, or a child of either: valid while its owner is unchanged
    Borrowed(String),
    /// a temporary slot that owns its content
    Slot(String),
}

impl Op {
    fn expr(&self) -> &str {
        match self {
            Op::Imm(s) | Op::Borrowed(s) | Op::Slot(s) => s,
        }
    }
}

/// What kind of function is emitted: it decides what `return` and `?` do, and how the function ends.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Ctx {
    Fn,
    Pred,
    Op,
    Test,
    Bound,
}

/// How a local is reached in C.
#[derive(Clone)]
enum LocalC {
    /// a positional parameter (borrowed)
    Param(String),
    /// a using parameter: a `const cplrt_capref *`
    Using(String),
    /// an owned slot
    Slot(String, bool),
    /// the result of an ensures clause: `rv`
    Rv,
}

pub(crate) struct Lowerer<'a, 'p> {
    t: &'a mut Tables<'p>,
    names: &'a mut Names,
    p: &'p Program,
    def: DefId,
    ctx: Ctx,
    body: String,
    helpers: String,
    indent: usize,
    temps: u32,
    slots: Vec<String>,
    /// Slots already released by an inner block on every path that continues.
    released_temps: BTreeSet<usize>,
    locals: HashMap<LocalId, LocalC>,
    generics: Vec<String>,
    /// the C of a fault's jump: `goto fail;`, or a claim before it inside a contract region
    fail: String,
    /// the span of the contract expression being lowered (a `return` or `?` there is control_flow_in_contract)
    region: Option<Span>,
    exit_used: bool,
    loops: u32,
    iter_vars: Vec<String>,
    /// binding sites of the body, by span: the locals in scope there, outermost first (`scope_at`)
    scopes: HashMap<(u32, u32, u32), Vec<LocalId>>,
}

/// Every HIR variant the coverage counters name (cabi.md 1.4), as `<enum>::<variant>`: what the coverage gate of
/// toolchain 12.6 requires to occur.
pub const COVERAGE_KINDS: [&str; 36] = [
    "ExprKind::Lit",
    "ExprKind::Local",
    "ExprKind::Ctor",
    "ExprKind::Wrap",
    "ExprKind::Field",
    "ExprKind::Call",
    "ExprKind::CapCall",
    "ExprKind::Check",
    "ExprKind::Propagate",
    "ExprKind::Unary",
    "ExprKind::Binary",
    "ExprKind::Forall",
    "ExprKind::Parallel",
    "ExprKind::Hole",
    "ExprKind::List",
    "ExprKind::StructLit",
    "ExprKind::If",
    "ExprKind::Match",
    "ExprKind::Widen",
    "ExprKind::Refine",
    "ExprKind::Gen",
    "ExprKind::Error",
    "Stmt::Let",
    "Stmt::Discard",
    "Stmt::Assign",
    "Stmt::Return",
    "Stmt::Assert",
    "Stmt::While",
    "Stmt::Expr",
    "PatternKind::Wildcard",
    "PatternKind::Int",
    "PatternKind::Text",
    "PatternKind::Bool",
    "PatternKind::Bind",
    "PatternKind::Ctor",
    "PatternKind::Unwrap",
];

fn expr_kind(k: &ExprKind) -> &'static str {
    match k {
        ExprKind::Lit(_) => "ExprKind::Lit",
        ExprKind::Local(_) => "ExprKind::Local",
        ExprKind::Ctor { .. } => "ExprKind::Ctor",
        ExprKind::Wrap { .. } => "ExprKind::Wrap",
        ExprKind::Field { .. } => "ExprKind::Field",
        ExprKind::Call { .. } => "ExprKind::Call",
        ExprKind::CapCall { .. } => "ExprKind::CapCall",
        ExprKind::Check { .. } => "ExprKind::Check",
        ExprKind::Propagate { .. } => "ExprKind::Propagate",
        ExprKind::Unary { .. } => "ExprKind::Unary",
        ExprKind::Binary { .. } => "ExprKind::Binary",
        ExprKind::Forall { .. } => "ExprKind::Forall",
        ExprKind::Parallel { .. } => "ExprKind::Parallel",
        ExprKind::Hole { .. } => "ExprKind::Hole",
        ExprKind::List(_) => "ExprKind::List",
        ExprKind::StructLit { .. } => "ExprKind::StructLit",
        ExprKind::If { .. } => "ExprKind::If",
        ExprKind::Match { .. } => "ExprKind::Match",
        ExprKind::Widen { .. } => "ExprKind::Widen",
        ExprKind::Refine { .. } => "ExprKind::Refine",
        ExprKind::Gen { .. } => "ExprKind::Gen",
        ExprKind::Error => "ExprKind::Error",
    }
}

fn stmt_kind(s: &Stmt) -> &'static str {
    match s {
        Stmt::Let { .. } => "Stmt::Let",
        Stmt::Discard(..) => "Stmt::Discard",
        Stmt::Assign { .. } => "Stmt::Assign",
        Stmt::Return(..) => "Stmt::Return",
        Stmt::Assert { .. } => "Stmt::Assert",
        Stmt::While { .. } => "Stmt::While",
        Stmt::Expr(..) => "Stmt::Expr",
    }
}

fn pattern_kind(k: &PatternKind) -> &'static str {
    match k {
        PatternKind::Wildcard => "PatternKind::Wildcard",
        PatternKind::Int(_) => "PatternKind::Int",
        PatternKind::Text(_) => "PatternKind::Text",
        PatternKind::Bool(_) => "PatternKind::Bool",
        PatternKind::Bind(_) => "PatternKind::Bind",
        PatternKind::Ctor { .. } => "PatternKind::Ctor",
        PatternKind::Unwrap { .. } => "PatternKind::Unwrap",
    }
}

fn span_key(s: Span) -> (u32, u32, u32) {
    (s.file.0, s.start, s.end)
}

/// The C identifier part of a CPL name (ASCII identifiers; anything else becomes `_`).
fn ident(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

impl<'a, 'p> Lowerer<'a, 'p> {
    fn new(t: &'a mut Tables<'p>, names: &'a mut Names, def: DefId, ctx: Ctx) -> Self {
        let p = t.p;
        let d = p.def(def);
        let scopes = cpl_eval::interp::scope_at(p, def)
            .into_iter()
            .map(|(span, locals)| (span_key(span), locals))
            .collect();
        let mut l = Lowerer {
            t,
            names,
            p,
            def,
            ctx,
            body: String::new(),
            helpers: String::new(),
            indent: 1,
            temps: 0,
            slots: Vec::new(),
            released_temps: BTreeSet::new(),
            locals: HashMap::new(),
            generics: d.kind.generics().to_vec(),
            fail: "goto fail;".to_string(),
            region: None,
            exit_used: false,
            loops: 0,
            iter_vars: Vec::new(),
            scopes,
        };
        // every local of the definition's table is a slot unless it is a parameter
        for ld in d.kind.locals() {
            let name = format!("l{}_{}", ld.id.0, ident(&ld.name));
            l.locals.insert(ld.id, LocalC::Slot(name, ld.mutable));
        }
        l
    }

    /// With coverage (cabi.md 1.4): a new counter `k` for one HIR node of variant `kind`, its index.
    fn cov_counter(&mut self, kind: &'static str) -> Option<usize> {
        let v = self.t.cov.as_mut()?;
        v.push(kind);
        Some(v.len() - 1)
    }

    /// With coverage: `CPLRT_COV(k);` at the start of a node's lowering.
    fn cov(&mut self, kind: &'static str) {
        if let Some(k) = self.cov_counter(kind) {
            self.line(&format!("CPLRT_COV({k}u);"));
        }
    }

    fn line(&mut self, s: &str) {
        for _ in 0..self.indent {
            self.body.push_str("    ");
        }
        self.body.push_str(s);
        self.body.push('\n');
    }

    fn temp(&mut self) -> String {
        let name = format!("t{}", self.temps);
        self.temps += 1;
        self.slots.push(name.clone());
        name
    }

    /// `if (<call> != CPLRT_OK) <fail>`
    fn checked(&mut self, call: &str) {
        let f = self.fail.clone();
        self.line(&format!("if ({call} != CPLRT_OK) {f}"));
    }

    fn site(&mut self, span: Span) -> String {
        let s = self.t.site(span);
        self.names.s(s)
    }

    fn def_name(&mut self, def: DefId) -> String {
        self.names.d(def.0)
    }

    /// Stores an operand into an owned slot (1.3 rule 2).
    fn store(&mut self, slot: &str, op: Op) {
        match op {
            Op::Imm(e) => self.line(&format!("CPLRT_SET({slot}, {e});")),
            Op::Borrowed(e) => self.line(&format!("CPLRT_COPY({slot}, {e});")),
            Op::Slot(t) => self.line(&format!("CPLRT_MOVE({slot}, {t});")),
        }
    }

    /// An operand in a temporary of its own (a borrowed word copied, an owned one moved).
    fn temp_of(&mut self, op: Op) -> String {
        if let Op::Slot(t) = &op {
            return t.clone();
        }
        let t = self.temp();
        self.store(&t, op);
        t
    }

    fn unit(&mut self, slot: &str) {
        let unit = self.p.std_defs.unit;
        let d = self.def_name(unit);
        self.checked(&format!("cplrt_enum_new(cx, {d}, 0u, NULL, 0u, &{slot})"));
    }

    /// The `visible` array of a binding site: the C of each local in scope, innermost first.
    fn visible(&self, scope: &[LocalId]) -> Vec<String> {
        scope
            .iter()
            .rev()
            .map(|id| match self.locals.get(id) {
                Some(LocalC::Param(n)) => n.clone(),
                Some(LocalC::Using(n)) => format!("(cplrt_w)(uintptr_t){n}"),
                Some(LocalC::Slot(n, _)) => n.clone(),
                Some(LocalC::Rv) => "rv".to_string(),
                None => "0".to_string(),
            })
            .collect()
    }

    fn scope_of(&self, span: Span) -> Vec<LocalId> {
        self.scopes
            .get(&span_key(span))
            .cloned()
            .unwrap_or_default()
    }

    /// `{ const cplrt_w visible[n] = {...}; <call with visible, n> }`, or NULL, 0 for none.
    fn with_visible(&mut self, scope: &[LocalId], call: impl FnOnce(&str, usize) -> String) {
        let v = self.visible(scope);
        if v.is_empty() {
            let c = call("NULL", 0);
            self.line(&c);
        } else {
            let c = call("visible", v.len());
            self.line(&format!(
                "{{ const cplrt_w visible[{}] = {{ {} }}; {c} }}",
                v.len(),
                v.join(", ")
            ));
        }
    }

    // ---- expressions ----------------------------------------------------------------------------------------------

    fn lower_all(&mut self, es: &'p [Expr]) -> Vec<Op> {
        let mut out = Vec::with_capacity(es.len());
        for e in es {
            let op = self.expr(e);
            out.push(op);
        }
        out
    }

    /// An array of borrowed words for a call: `{ const cplrt_w v[n] = {...}; <stmt> }`, or NULL for none.
    fn array_call(&mut self, ops: &[Op], stmt: impl FnOnce(&str, usize) -> String) {
        if ops.is_empty() {
            let s = stmt("NULL", 0);
            self.line(&s);
        } else {
            let items: Vec<&str> = ops.iter().map(Op::expr).collect();
            let s = stmt("v", ops.len());
            self.line(&format!(
                "{{ const cplrt_w v[{}] = {{ {} }}; {s} }}",
                ops.len(),
                items.join(", ")
            ));
        }
    }

    fn int_lit(&mut self, n: &BigInt) -> Op {
        let min = BigInt::from(-(1i64 << 62));
        let max = BigInt::from((1i64 << 62) - 1);
        if *n >= min && *n <= max {
            let v: i64 = n.try_into().expect("in the small range");
            if v < 0 {
                Op::Imm(format!("cplrt_small(-INT64_C({}))", -(v as i128)))
            } else {
                Op::Imm(format!("cplrt_small(INT64_C({v}))"))
            }
        } else {
            let k = self.t.constant(ConstKey::Int(n.clone()));
            Op::Imm(self.names.k(k))
        }
    }

    fn text_lit(&mut self, s: &str) -> Op {
        let k = self.t.constant(ConstKey::Text(s.to_string()));
        Op::Imm(self.names.k(k))
    }

    fn local(&mut self, id: LocalId) -> Op {
        match self.locals.get(&id).cloned() {
            Some(LocalC::Param(n)) => Op::Borrowed(n),
            Some(LocalC::Using(n)) => Op::Imm(format!("(cplrt_w)(uintptr_t){n}")),
            Some(LocalC::Rv) => Op::Borrowed("rv".to_string()),
            Some(LocalC::Slot(n, mutable)) => {
                if mutable {
                    // a var may be reassigned by a later sibling: take a reference of its own
                    let t = self.temp();
                    self.line(&format!("CPLRT_COPY({t}, {n});"));
                    Op::Slot(t)
                } else {
                    Op::Borrowed(n)
                }
            }
            None => Op::Imm("0".to_string()),
        }
    }

    /// The C of a using argument: the capability reference of a using parameter.
    fn using_arg(&mut self, e: &'p Expr) -> String {
        if let ExprKind::Local(id) = &e.kind {
            if let Some(LocalC::Using(n)) = self.locals.get(id) {
                return n.clone();
            }
        }
        "NULL".to_string()
    }

    fn internal_here(&mut self, span: Option<Span>, kind: &str, detail: &str) {
        let site = match span {
            Some(s) => self.site(s),
            None => "CPLRT_NO_SITE".to_string(),
        };
        let f = self.fail.clone();
        self.line(&format!(
            "cplrt_internal(cx, {site}, {}, {});",
            c_bytes(kind.as_bytes()),
            c_bytes(detail.as_bytes())
        ));
        self.line(&f);
    }

    fn parallel(&mut self, binder: LocalId, collection: &'p Expr, body: &'p Expr) -> Op {
        let input = self.expr(collection);
        let input = self.temp_of(input);
        let mut reads = BTreeSet::new();
        body.walk(&mut |e| {
            if let ExprKind::Local(id) = e.kind {
                reads.insert(id);
            }
        });
        reads.remove(&binder);
        fn bindings(block: &Block, bound: &mut BTreeSet<LocalId>) {
            for stmt in &block.stmts {
                match stmt {
                    Stmt::Let { local, .. } => {
                        bound.insert(*local);
                    }
                    Stmt::While { body, .. } => bindings(body, bound),
                    _ => {}
                }
            }
        }
        fn pattern_bindings(pattern: &Pattern, bound: &mut BTreeSet<LocalId>) {
            match &pattern.kind {
                PatternKind::Bind(id) => {
                    bound.insert(*id);
                }
                PatternKind::Ctor { args, .. } => {
                    for arg in args {
                        pattern_bindings(arg, bound);
                    }
                }
                PatternKind::Unwrap { arg, .. } => pattern_bindings(arg, bound),
                _ => {}
            }
        }
        let mut bound = BTreeSet::from([binder]);
        body.walk(&mut |e| match &e.kind {
            ExprKind::Forall { binder, .. } | ExprKind::Parallel { binder, .. } => {
                bound.insert(*binder);
            }
            ExprKind::If {
                then_block,
                else_block,
                ..
            } => {
                bindings(then_block, &mut bound);
                if let Some(b) = else_block {
                    bindings(b, &mut bound);
                }
            }
            ExprKind::Match { arms, .. } => {
                for arm in arms {
                    pattern_bindings(&arm.pattern, &mut bound);
                    bindings(&arm.body, &mut bound);
                }
            }
            _ => {}
        });
        // Fault payloads include visible locals even when the task's normal path never reads them.
        for ((file, start, end), scope) in &self.scopes {
            if *file == body.span.file.0 && *start >= body.span.start && *end <= body.span.end {
                reads.extend(scope);
            }
        }
        reads.retain(|id| !bound.contains(id));
        let captures: Vec<_> = reads
            .into_iter()
            .filter(|id| self.locals.contains_key(id))
            .collect();
        let name = format!("cpl_parallel_{}_{}", self.def.0, body.span.start);
        let mut child = Lowerer::new(self.t, self.names, self.def, Ctx::Bound);
        child.locals = self.locals.clone();
        child.locals.insert(binder, LocalC::Param("item".into()));
        for (i, id) in captures.iter().enumerate() {
            child
                .locals
                .insert(*id, LocalC::Param(format!("captures[{i}]")));
        }
        let value = child.expr(body);
        child.store("rv", value);
        let mut helper = std::mem::take(&mut child.helpers);
        let _ = writeln!(helper, "static int {name}(cplrt_cx *cx, cplrt_w item, const cplrt_w *captures, cplrt_w *out)\n{{");
        helper.push_str(&child.declarations(&[]));
        helper.push_str(&child.body);
        helper.push_str(
            "    CPLRT_MOVE(*out, rv);\n    goto done;\nfail:\n    st = CPLRT_FAULT;\ndone:\n",
        );
        helper.push_str(&child.releases());
        helper.push_str("    return st;\n}\n");
        self.helpers.push_str(&helper);
        let ops: Vec<_> = captures.iter().map(|id| self.local(*id)).collect();
        let out = self.temp();
        let site = self.site(body.span);
        let fail = self.fail.clone();
        self.array_call(&ops, |v, _| format!("if (cplrt_parallel_map(cx, {site}, {input}, {name}, {v}, &{out}) != CPLRT_OK) {fail}"));
        Op::Slot(out)
    }

    fn expr(&mut self, e: &'p Expr) -> Op {
        self.cov(expr_kind(&e.kind));
        if self.t.optimize {
            if !matches!(e.kind, ExprKind::Lit(_)) {
                if let Some(lit) = crate::optimize::literal(e, &mut 256) {
                    return match lit {
                        cpl_hir::Lit::Int(n) => self.int_lit(&n),
                        cpl_hir::Lit::Text(s) => self.text_lit(&s),
                        cpl_hir::Lit::Bool(b) => {
                            Op::Imm(if b { "CPLRT_TRUE" } else { "CPLRT_FALSE" }.into())
                        }
                    };
                }
            }
            if let Some(identity) = crate::optimize::identity(e) {
                return self.expr(identity);
            }
            if let ExprKind::If {
                cond,
                then_block,
                else_block,
            } = &e.kind
            {
                if let Some(cpl_hir::Lit::Bool(b)) = crate::optimize::literal(cond, &mut 256) {
                    if b {
                        return self.block(then_block);
                    }
                    if let Some(block) = else_block {
                        return self.block(block);
                    }
                    let slot = self.temp();
                    self.unit(&slot);
                    return Op::Slot(slot);
                }
            }
        }
        match &e.kind {
            ExprKind::Lit(cpl_hir::Lit::Int(n)) => self.int_lit(n),
            ExprKind::Lit(cpl_hir::Lit::Bool(b)) => {
                Op::Imm(if *b { "CPLRT_TRUE" } else { "CPLRT_FALSE" }.to_string())
            }
            ExprKind::Lit(cpl_hir::Lit::Text(s)) => self.text_lit(s),
            ExprKind::Local(id) => self.local(*id),
            ExprKind::Ctor { def, variant, args } => {
                let ops = self.lower_all(args);
                let t = self.temp();
                let d = self.def_name(*def);
                let f = self.fail.clone();
                let variant = *variant;
                self.array_call(&ops, |v, n| {
                    format!(
                        "if (cplrt_enum_new(cx, {d}, {variant}u, {v}, {n}u, &{t}) != CPLRT_OK) {f}"
                    )
                });
                Op::Slot(t)
            }
            ExprKind::Wrap { def, arg } => {
                let inner = self.expr(arg);
                let t = self.temp();
                let d = self.def_name(*def);
                self.checked(&format!("cplrt_wrap(cx, {d}, {}, &{t})", inner.expr()));
                Op::Slot(t)
            }
            ExprKind::Field { base, index, .. } => {
                let b = self.expr(base);
                let b = match b {
                    Op::Imm(x) => Op::Borrowed(x),
                    other => other,
                };
                Op::Borrowed(format!("cplrt_field({}, {}u)", b.expr(), index))
            }
            ExprKind::Call {
                callee,
                args,
                using,
                ..
            } => self.call(*callee, args, using, e.span),
            ExprKind::CapCall {
                handle,
                capability,
                op,
                args,
            } => {
                let h = self.using_arg(handle);
                let ops = self.lower_all(args);
                let t = self.temp();
                let site = self.site(e.span);
                let mut call = format!("cpl_o{}_{}(cx, {site}, {h}", capability.0, op);
                for o in &ops {
                    let _ = write!(call, ", {}", o.expr());
                }
                let _ = write!(call, ", &{t})");
                self.checked(&call);
                Op::Slot(t)
            }
            ExprKind::Check { refined, arg } => {
                let v = self.expr(arg);
                let v = Op::Slot(self.temp_of(v));
                let t = self.temp();
                let chain: Vec<DefId> = cpl_eval::builtins::check_order(self.p, *refined);
                let site = self.site(e.span);
                let depth = chain.len();
                for r in &chain {
                    let ri = self.t.ref_ix.get(r).copied().unwrap_or(0);
                    let rn = self.names.r(ri);
                    self.checked(&format!("cpl_p{}(cx, {site}, {}, &holds)", r.0, v.expr()));
                    let f = self.fail.clone();
                    self.line(&format!(
                        "if (!holds) {{ if (cplrt_check_failure(cx, {rn}, {}, &{t}) != CPLRT_OK) {f} }} else {{",
                        v.expr()
                    ));
                    self.indent += 1;
                }
                self.checked(&format!("cplrt_result_ok(cx, {}, &{t})", v.expr()));
                for _ in 0..depth {
                    self.indent -= 1;
                    self.line("}");
                }
                Op::Slot(t)
            }
            ExprKind::Propagate(inner) => {
                let v = self.expr(inner);
                let v = match v {
                    Op::Imm(x) => Op::Borrowed(x),
                    other => other,
                };
                self.line(&format!("if (!cplrt_is_ok({})) {{", v.expr()));
                self.indent += 1;
                self.early_exit(v.clone(), e.span);
                self.indent -= 1;
                self.line("}");
                Op::Borrowed(format!("cplrt_payload({}, 0u)", v.expr()))
            }
            ExprKind::Unary { op, operand } => {
                let a = self.expr(operand);
                let t = self.temp();
                match op {
                    UnaryOp::Not => self.line(&format!(
                        "CPLRT_SET({t}, cplrt_bool(!cplrt_truth({})));",
                        a.expr()
                    )),
                    UnaryOp::Neg => {
                        let site = self.site(e.span);
                        self.checked(&format!("cplrt_int_neg(cx, {site}, {}, &{t})", a.expr()));
                    }
                }
                Op::Slot(t)
            }
            ExprKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs, e.span),
            ExprKind::Parallel {
                binder,
                collection,
                predicate,
            } => self.parallel(*binder, collection, predicate),
            ExprKind::Forall {
                binder,
                collection,
                predicate,
            } => {
                let c = self.expr(collection);
                let c = self.temp_of(c);
                let t = self.temp();
                let i = format!("i{}", self.loops);
                self.loops += 1;
                let slot = match self.locals.get(binder) {
                    Some(LocalC::Slot(n, _)) => n.clone(),
                    _ => self.temp(),
                };
                self.line(&format!("CPLRT_SET({t}, CPLRT_TRUE);"));
                self.line(&format!(
                    "for (uint32_t {i} = 0; {i} < cplrt_len({c}); {i}++) {{"
                ));
                self.indent += 1;
                self.line(&format!("CPLRT_COPY({slot}, cplrt_item({c}, {i}));"));
                let q = self.expr(predicate);
                self.line(&format!(
                    "if (!cplrt_truth({})) {{ CPLRT_SET({t}, CPLRT_FALSE); break; }}",
                    q.expr()
                ));
                self.indent -= 1;
                self.line("}");
                Op::Slot(t)
            }
            ExprKind::Hole { id, name } => {
                let scope = self.scope_of(e.span);
                let site = self.t.site(e.span);
                let bsite = self.t.bsite(self.def, &scope, None, None);
                let expected = self.p.display_ty_in(&e.ty, &self.generics);
                self.t.holes.insert(
                    id.0,
                    HoleRow {
                        name: name.clone(),
                        expected_type: expected,
                        site,
                        bsite,
                    },
                );
                let h = self.names.h(id.0);
                let f = self.fail.clone();
                self.with_visible(&scope, |v, n| format!("cplrt_hole(cx, {h}, {v}, {n}u);"));
                self.line(&f);
                let t = self.temp();
                Op::Slot(t)
            }
            ExprKind::List(items) => {
                let ops = self.lower_all(items);
                let t = self.temp();
                let f = self.fail.clone();
                self.array_call(&ops, |v, n| {
                    format!("if (cplrt_list_new(cx, {v}, {n}u, &{t}) != CPLRT_OK) {f}")
                });
                Op::Slot(t)
            }
            ExprKind::StructLit {
                def,
                invariant_proven,
                spread,
                fields,
            } => {
                let count = self
                    .p
                    .def(*def)
                    .kind
                    .as_struct()
                    .map_or(0, |s| s.fields.len());
                let base = match spread {
                    Some(b) => {
                        let o = self.expr(b);
                        Some(match o {
                            Op::Imm(x) => Op::Borrowed(x),
                            other => other,
                        })
                    }
                    None => None,
                };
                let mut given: Vec<Option<Op>> = vec![None; count];
                for (index, fe) in fields {
                    let o = self.expr(fe);
                    if let Some(slot) = given.get_mut(*index as usize) {
                        *slot = Some(o);
                    }
                }
                let mut ops = Vec::with_capacity(count);
                for (i, g) in given.into_iter().enumerate() {
                    match (g, &base) {
                        (Some(o), _) => ops.push(o),
                        (None, Some(b)) => {
                            ops.push(Op::Borrowed(format!("cplrt_field({}, {}u)", b.expr(), i)))
                        }
                        (None, None) => ops.push(Op::Imm("0".to_string())),
                    }
                }
                let t = self.temp();
                let d = self.def_name(*def);
                let f = self.fail.clone();
                self.array_call(&ops, |v, n| {
                    format!("if (cplrt_struct_new(cx, {d}, {v}, {n}u, &{t}) != CPLRT_OK) {f}")
                });
                if !invariant_proven && self.p.def(*def).kind.checked_predicate().is_some() {
                    let site = self.t.site(e.span);
                    let site = self.names.s(site);
                    let ri = self.t.ref_ix[def];
                    let clause = self.names.cl(self.t.refs[ri as usize].clause);
                    self.line("{");
                    self.indent += 1;
                    self.line("int holds = 0;");
                    self.line(&format!(
                        "if (cpl_p{}(cx, {site}, {t}, &holds) != CPLRT_OK) {f}",
                        def.0
                    ));
                    self.line(&format!(
                        "if (!holds) {{ cplrt_constructor_failed(cx, {clause}, {t}); {f} }}"
                    ));
                    self.indent -= 1;
                    self.line("}");
                }
                Op::Slot(t)
            }
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => {
                let c = self.expr(cond);
                let t = self.temp();
                self.line(&format!("if (cplrt_truth({})) {{", c.expr()));
                self.indent += 1;
                let v = self.block(then_block);
                self.store(&t, v);
                self.indent -= 1;
                self.line("} else {");
                self.indent += 1;
                match else_block {
                    Some(b) => {
                        let v = self.block(b);
                        self.store(&t, v);
                    }
                    None => self.unit(&t),
                }
                self.indent -= 1;
                self.line("}");
                Op::Slot(t)
            }
            ExprKind::Match { scrutinee, arms } => self.lower_match(scrutinee, arms, e.span),
            ExprKind::Widen { expr, .. } => self.expr(expr),
            ExprKind::Refine { expr, refined, .. } => {
                let v = self.expr(expr);
                let v = Op::Slot(self.temp_of(v));
                let chain: Vec<DefId> = cpl_eval::builtins::check_order(self.p, *refined);
                if !chain.is_empty() {
                    let site = self.site(e.span);
                    self.line("if (cx->check_contracts) { /* obl refine */");
                    self.indent += 1;
                    for r in &chain {
                        let ri = self.t.ref_ix.get(r).copied().unwrap_or(0);
                        let rn = self.names.r(ri);
                        self.checked(&format!("cpl_p{}(cx, {site}, {}, &holds)", r.0, v.expr()));
                        let f = self.fail.clone();
                        self.line(&format!(
                            "if (!holds) {{ cplrt_refine_unsound(cx, {site}, {rn}, {}); {f} }}",
                            v.expr()
                        ));
                    }
                    self.indent -= 1;
                    self.line("}");
                }
                v
            }
            ExprKind::Gen(_) => {
                let detail = render(&messages::DETAIL_GENERATOR_NOT_A_VALUE_EXPR, &[]);
                self.internal_here(Some(e.span), "generator_not_a_value", &detail);
                let t = self.temp();
                Op::Slot(t)
            }
            ExprKind::Error => {
                let detail = render(&messages::DETAIL_ERROR_NODE, &[]);
                self.internal_here(Some(e.span), "error_node", &detail);
                let t = self.temp();
                Op::Slot(t)
            }
        }
    }

    /// The exit of a `return` or a propagated `Err`: into `rv` and to `exit:` in a function; in a test body or a
    /// contract expression the evaluator's internal errors.
    fn early_exit(&mut self, v: Op, span: Span) {
        if let Some(region) = self.region {
            let detail = render(&messages::DETAIL_CONTROL_FLOW_IN_CONTRACT, &[]);
            self.internal_here(Some(region), "control_flow_in_contract", &detail);
            let _ = span;
            return;
        }
        match self.ctx {
            Ctx::Fn => {
                self.store("rv", v);
                self.exit_used = true;
                self.line("goto exit;");
            }
            Ctx::Test => {
                let detail = render(&messages::DETAIL_RETURN_OUTSIDE_BODY, &[]);
                self.internal_here(None, "return_outside_function", &detail);
            }
            _ => {
                let detail = render(&messages::DETAIL_RETURN_OUTSIDE_FUNCTION, &[]);
                self.internal_here(None, "return_outside_function", &detail);
            }
        }
    }

    fn call(&mut self, callee: DefId, args: &'p [Expr], using: &'p [Expr], span: Span) -> Op {
        let p = self.p;
        let ops = self.lower_all(args);
        let handles: Vec<String> = using.iter().map(|u| self.using_arg(u)).collect();
        let t = self.temp();
        let site = self.site(span);
        let Some(fd) = p.defs.get(callee.0 as usize).and_then(|d| d.kind.as_fn()) else {
            let detail = render(
                &messages::DETAIL_NOT_A_FUNCTION,
                &[("def", &callee.0.to_string())],
            );
            self.internal_here(Some(span), "not_a_function", &detail);
            return Op::Slot(t);
        };
        match &fd.body {
            FnBody::Builtin(id) => {
                let stub = self.def_name(callee);
                match cpl_rtspec::builtins::spec(id) {
                    Some(spec) if spec.callable => {
                        let mut call = format!("cplrt_bi({id})(cx, {site}, {stub}");
                        for o in &ops {
                            let _ = write!(call, ", {}", o.expr());
                        }
                        let _ = write!(call, ", &{t})");
                        self.checked(&call);
                    }
                    _ => {
                        let f = self.fail.clone();
                        let idl = c_bytes(id.as_bytes());
                        self.array_call(&ops, |v, n| {
                            format!("(void)cplrt_bi_uncallable(cx, {site}, {stub}, {idl}, {v}, {n}u); {f}")
                        });
                    }
                }
            }
            FnBody::Block(_) => {
                if args.len() != fd.params.len() || using.len() != fd.using.len() {
                    let detail = render(
                        &messages::DETAIL_CALL_MISMATCH,
                        &[
                            ("function", &p.display_path(callee)),
                            ("params", &fd.params.len().to_string()),
                            ("using", &fd.using.len().to_string()),
                            ("args", &args.len().to_string()),
                            ("handles", &using.len().to_string()),
                        ],
                    );
                    self.internal_here(Some(span), "call_mismatch", &detail);
                    return Op::Slot(t);
                }
                let mut call = format!("cpl_f{}(cx, {site}", callee.0);
                for o in &ops {
                    let _ = write!(call, ", {}", o.expr());
                }
                for h in &handles {
                    let _ = write!(call, ", {h}");
                }
                let _ = write!(call, ", &{t})");
                self.checked(&call);
            }
        }
        Op::Slot(t)
    }

    fn binary(&mut self, op: BinaryOp, lhs: &'p Expr, rhs: &'p Expr, span: Span) -> Op {
        match op {
            BinaryOp::And | BinaryOp::Or => {
                let a = self.expr(lhs);
                let t = self.temp();
                if op == BinaryOp::And {
                    self.line(&format!("if (cplrt_truth({})) {{", a.expr()));
                } else {
                    self.line(&format!("if (!cplrt_truth({})) {{", a.expr()));
                }
                self.indent += 1;
                let b = self.expr(rhs);
                self.line(&format!(
                    "CPLRT_SET({t}, cplrt_bool(cplrt_truth({})));",
                    b.expr()
                ));
                self.indent -= 1;
                self.line("} else {");
                self.line(&format!(
                    "    CPLRT_SET({t}, {});",
                    if op == BinaryOp::And {
                        "CPLRT_FALSE"
                    } else {
                        "CPLRT_TRUE"
                    }
                ));
                self.line("}");
                return Op::Slot(t);
            }
            _ => {}
        }
        let a = self.expr(lhs);
        let a = match a {
            Op::Borrowed(x) if self.later_may_reassign(rhs) => {
                Op::Slot(self.temp_of(Op::Borrowed(x)))
            }
            other => other,
        };
        let b = self.expr(rhs);
        let t = self.temp();
        let (ae, be) = (a.expr().to_string(), b.expr().to_string());
        match op {
            BinaryOp::Eq | BinaryOp::Ne => {
                self.checked(&format!("cplrt_equal(cx, {ae}, {be}, &eq)"));
                let neg = if op == BinaryOp::Ne { "!" } else { "" };
                self.line(&format!("CPLRT_SET({t}, cplrt_bool({neg}eq));"));
            }
            BinaryOp::Concat => self.checked(&format!("cplrt_concat(cx, {ae}, {be}, &{t})")),
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => {
                let site = self.site(span);
                let f = match op {
                    BinaryOp::Add => "cplrt_int_add",
                    BinaryOp::Sub => "cplrt_int_sub",
                    BinaryOp::Mul => "cplrt_int_mul",
                    BinaryOp::Div => "cplrt_int_div",
                    _ => "cplrt_int_rem",
                };
                let f = if self.t.optimize {
                    match op {
                        BinaryOp::Add => "cplrt_fast_add",
                        BinaryOp::Sub => "cplrt_fast_sub",
                        BinaryOp::Mul => "cplrt_fast_mul",
                        BinaryOp::Div => "cplrt_fast_div",
                        BinaryOp::Rem => "cplrt_fast_rem",
                        _ => f,
                    }
                } else {
                    f
                };
                self.checked(&format!("{f}(cx, {site}, {ae}, {be}, &{t})"));
            }
            BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
                let rel = match op {
                    BinaryOp::Lt => "<",
                    BinaryOp::Le => "<=",
                    BinaryOp::Gt => ">",
                    _ => ">=",
                };
                let cmp = if self.t.optimize {
                    format!("(cplrt_is_small({ae}) && cplrt_is_small({be}) ? cplrt_small_value({ae}) {rel} cplrt_small_value({be}) : cplrt_int_cmp({ae}, {be}) {rel} 0)")
                } else {
                    format!("cplrt_int_cmp({ae}, {be}) {rel} 0")
                };
                self.line(&format!("CPLRT_SET({t}, cplrt_bool({cmp}));"));
            }
            BinaryOp::And | BinaryOp::Or => unreachable!(),
        }
        Op::Slot(t)
    }

    /// Whether evaluating `e` may reassign a `var` (it holds a block, whose statements can): a borrowed operand that
    /// must survive it is then taken into a reference of its own. Reads of vars are already copied (`local`), so only
    /// children of borrowed vars matter; this keeps the rule conservative and simple.
    fn later_may_reassign(&self, e: &Expr) -> bool {
        fn walk(e: &Expr) -> bool {
            match &e.kind {
                ExprKind::If { .. } | ExprKind::Match { .. } => true,
                ExprKind::Ctor { args, .. } | ExprKind::List(args) => args.iter().any(walk),
                ExprKind::Call { args, .. } | ExprKind::CapCall { args, .. } => {
                    args.iter().any(walk)
                }
                ExprKind::Wrap { arg, .. } | ExprKind::Check { arg, .. } => walk(arg),
                ExprKind::Field { base, .. } => walk(base),
                ExprKind::Propagate(x)
                | ExprKind::Widen { expr: x, .. }
                | ExprKind::Refine { expr: x, .. } => walk(x),
                ExprKind::Unary { operand, .. } => walk(operand),
                ExprKind::Binary { lhs, rhs, .. } => walk(lhs) || walk(rhs),
                ExprKind::Forall {
                    collection,
                    predicate,
                    ..
                }
                | ExprKind::Parallel {
                    collection,
                    predicate,
                    ..
                } => walk(collection) || walk(predicate),
                ExprKind::StructLit { spread, fields, .. } => {
                    spread.as_deref().is_some_and(walk) || fields.iter().any(|(_, f)| walk(f))
                }
                _ => false,
            }
        }
        walk(e)
    }

    fn lower_match(&mut self, scrutinee: &'p Expr, arms: &'p [Arm], span: Span) -> Op {
        let v = self.expr(scrutinee);
        let v = match v {
            Op::Imm(x) => Op::Borrowed(x),
            Op::Borrowed(x) => Op::Slot(self.temp_of(Op::Borrowed(x))),
            other => other,
        };
        let t = self.temp();
        let ve = v.expr().to_string();
        let mut first = true;
        for arm in arms {
            let mut tests = Vec::new();
            let mut binds = Vec::new();
            self.pattern(&arm.pattern, &ve, &mut tests, &mut binds);
            let cond = if tests.is_empty() {
                "1".to_string()
            } else {
                tests.join(" && ")
            };
            if first {
                self.line(&format!("if ({cond}) {{"));
                first = false;
            } else {
                self.line(&format!("}} else if ({cond}) {{"));
            }
            self.indent += 1;
            for (local, part) in binds {
                if let Some(LocalC::Slot(n, _)) = self.locals.get(&local).cloned() {
                    self.line(&format!("CPLRT_COPY({n}, {part});"));
                }
            }
            let r = self.block(&arm.body);
            self.store(&t, r);
            self.indent -= 1;
        }
        let site = self.site(span);
        let f = self.fail.clone();
        if first {
            self.line(&format!("cplrt_no_match(cx, {site}, {ve}); {f}"));
        } else {
            self.line("} else {");
            self.line(&format!("    cplrt_no_match(cx, {site}, {ve}); {f}"));
            self.line("}");
        }
        Op::Slot(t)
    }

    /// The tests and bindings of `pat` against the word `part` (crates/cpl-eval/src/interp/eval.rs, `matches`).
    fn pattern(
        &mut self,
        pat: &Pattern,
        part: &str,
        tests: &mut Vec<String>,
        binds: &mut Vec<(LocalId, String)>,
    ) {
        // a pattern test is counted when the tests before it held (they are joined by &&)
        if let Some(k) = self.cov_counter(pattern_kind(&pat.kind)) {
            tests.push(format!("(CPLRT_COV({k}u), 1)"));
        }
        match &pat.kind {
            PatternKind::Wildcard => {}
            PatternKind::Bind(local) => binds.push((*local, part.to_string())),
            PatternKind::Int(n) => match self.int_lit(n) {
                Op::Imm(e) if e.starts_with("cplrt_small") => tests.push(format!("{part} == {e}")),
                other => tests.push(format!("cplrt_int_cmp({part}, {}) == 0", other.expr())),
            },
            PatternKind::Text(s) => {
                let k = self.text_lit(s);
                tests.push(format!("cplrt_text_eq({part}, {})", k.expr()));
            }
            PatternKind::Bool(b) => {
                tests.push(format!(
                    "{part} == {}",
                    if *b { "CPLRT_TRUE" } else { "CPLRT_FALSE" }
                ));
            }
            PatternKind::Ctor { variant, args, .. } => {
                tests.push(format!("cplrt_variant({part}) == {variant}u"));
                for (i, a) in args.iter().enumerate() {
                    let sub = format!("cplrt_payload({part}, {i}u)");
                    self.pattern(a, &sub, tests, binds);
                }
            }
            PatternKind::Unwrap { arg, .. } => {
                let sub = format!("cplrt_inner({part})");
                self.pattern(arg, &sub, tests, binds);
            }
        }
    }

    // ---- blocks and statements --------------------------------------------------------------------------------------

    fn block(&mut self, b: &'p Block) -> Op {
        self.block_result(b, false)
    }

    /// Discarded loop bodies still evaluate their tail, but need no Unit value
    /// and keep none of the block's temporaries alive into the next iteration.
    fn block_result(&mut self, b: &'p Block, discard: bool) -> Op {
        let first_temp = self.slots.len();
        for s in &b.stmts {
            self.stmt(s);
        }
        let value = match &b.tail {
            Some(e) => self.expr(e),
            None if discard => Op::Imm("CPLRT_EMPTY".into()),
            None => {
                let t = self.temp();
                self.unit(&t);
                Op::Slot(t)
            }
        };
        if !self.t.optimize {
            return value;
        }
        // The tail may borrow a local or a child of a temporary about to leave
        // scope. Own it before releasing the block's locals and intermediates.
        let value = if discard {
            Op::Imm("CPLRT_EMPTY".into())
        } else {
            match value {
                Op::Borrowed(_) => Op::Slot(self.temp_of(value)),
                other => other,
            }
        };
        for stmt in &b.stmts {
            if let Stmt::Let { local, .. } = stmt {
                let slot = self.slot_of(*local);
                self.line(&format!("CPLRT_SET({slot}, CPLRT_EMPTY);"));
            }
        }
        for i in first_temp..self.slots.len() {
            let slot = self.slots[i].clone();
            if !matches!(&value, Op::Slot(result) if result == &slot)
                && self.released_temps.insert(i)
            {
                self.line(&format!("CPLRT_SET({slot}, CPLRT_EMPTY);"));
            }
        }
        value
    }

    fn slot_of(&mut self, local: LocalId) -> String {
        match self.locals.get(&local) {
            Some(LocalC::Slot(n, _)) => n.clone(),
            _ => self.temp(),
        }
    }

    fn stmt(&mut self, s: &'p Stmt) {
        self.cov(stmt_kind(s));
        if self.t.optimize {
            if let Stmt::Assign { local, value } = s {
                if let ExprKind::Binary {
                    op: BinaryOp::Concat,
                    lhs,
                    rhs,
                } = &value.kind
                {
                    if matches!(lhs.kind, ExprKind::Local(id) if id == *local)
                        && matches!(value.ty, Ty::List(_) | Ty::Text | Ty::Bytes)
                        && !self.later_may_reassign(rhs)
                    {
                        let other = self.expr(rhs);
                        let slot = self.slot_of(*local);
                        let helper = match value.ty {
                            Ty::Text => "cplrt_text_concat_owned",
                            Ty::Bytes => "cplrt_bytes_concat_owned",
                            _ => "cplrt_list_concat_owned",
                        };
                        self.checked(&format!("{helper}(cx, &{slot}, {})", other.expr()));
                        return;
                    }
                }
                if let ExprKind::Call {
                    callee,
                    args,
                    using,
                    ..
                } = &value.kind
                {
                    let push = self.p.def(*callee).kind.as_fn().is_some_and(
                        |f| matches!(&f.body, FnBody::Builtin(id) if id == "list_push"),
                    );
                    if push
                        && using.is_empty()
                        && args.len() == 2
                        && matches!(args[0].kind, ExprKind::Local(id) if id == *local)
                        && !self.later_may_reassign(&args[1])
                    {
                        let item = self.expr(&args[1]);
                        let slot = self.slot_of(*local);
                        let site = self.site(value.span);
                        let stub = self.def_name(*callee);
                        self.checked(&format!(
                            "cplrt_list_push_owned(cx, {site}, {stub}, &{slot}, {})",
                            item.expr()
                        ));
                        return;
                    }
                }
            }
        }
        match s {
            Stmt::Let { local, value } | Stmt::Assign { local, value } => {
                let v = self.expr(value);
                let slot = self.slot_of(*local);
                self.store(&slot, v);
            }
            Stmt::Discard(e) | Stmt::Expr(e) => {
                let _ = self.expr(e);
            }
            Stmt::Return(e) => {
                let v = match e {
                    Some(e) => self.expr(e),
                    None => {
                        let t = self.temp();
                        self.unit(&t);
                        Op::Slot(t)
                    }
                };
                let span = e
                    .as_ref()
                    .map_or(Span::new(cpl_diag::FileId(0), 0, 0), |e| e.span);
                self.early_exit(v, span);
            }
            Stmt::Assert { expr, text } => {
                let v = self.expr(expr);
                let scope = self.scope_of(expr.span);
                let bsite = self.t.bsite(self.def, &scope, Some(text), Some(expr.span));
                let a = self.names.a(bsite);
                let f = self.fail.clone();
                self.line(&format!("if (!cplrt_truth({})) {{", v.expr()));
                self.indent += 1;
                self.with_visible(&scope, |vis, n| {
                    format!("cplrt_assert_failed(cx, {a}, {vis}, {n}u);")
                });
                self.line(&f);
                self.indent -= 1;
                self.line("}");
            }
            Stmt::While {
                cond,
                invariants,
                decreases,
                body,
            } => self.lower_while(cond, invariants, decreases.as_ref(), body),
        }
    }

    /// One contract region: the clause evaluated with its faults claimed (S34), then `on_false` when it is false.
    fn region(
        &mut self,
        clause: &'p Clause,
        cl: u32,
        tag: &str,
        on_false: impl FnOnce(&mut Self, &str),
    ) {
        let cln = self.names.cl(cl);
        self.line(&format!(
            "if (cx->check_contracts) {{ /* obl {tag}: {} */",
            comment_text(&clause.text)
        ));
        self.indent += 1;
        let saved_fail = std::mem::replace(
            &mut self.fail,
            format!("{{ cplrt_claim(cx, {cln}); goto fail; }}"),
        );
        let saved_region = self.region.replace(clause.expr.span);
        let v = self.expr(&clause.expr);
        self.fail = saved_fail;
        self.region = saved_region;
        let ve = v.expr().to_string();
        on_false(self, &ve);
        self.indent -= 1;
        self.line("}");
    }

    fn lower_while(
        &mut self,
        cond: &'p Expr,
        invariants: &'p [Clause],
        decreases: Option<&'p Clause>,
        body: &'p Block,
    ) {
        let n = self.loops;
        self.loops += 1;
        let iter = format!("it{n}");
        self.iter_vars.push(iter.clone());
        let scope = self.scope_of(cond.span);
        let bsite = self.t.bsite(self.def, &scope, None, None);
        let inv: Vec<u32> = invariants
            .iter()
            .map(|c| {
                self.t
                    .clause(&c.text, CK_INVARIANT, c.span, self.def, None, Some(bsite))
            })
            .collect();
        let dec = decreases.map(|c| {
            self.t
                .clause(&c.text, CK_DECREASES, c.span, self.def, None, Some(bsite))
        });
        let m = format!("m{n}");
        self.slots.push(m.clone());
        self.line(&format!("{iter} = 0;"));
        self.line(&format!("CPLRT_SET({m}, 0);"));
        for (i, c) in invariants.iter().enumerate() {
            let cl = inv[i];
            let sc = scope.clone();
            let iter2 = iter.clone();
            self.region(
                c,
                cl,
                &format!("loop/{}/invariant/{}/init", n + 1, i + 1),
                move |l, ve| {
                    let cln = l.names.cl(cl);
                    l.line(&format!("if (!cplrt_truth({ve})) {{"));
                    l.indent += 1;
                    l.with_visible(&sc, |v, k| {
                        format!("cplrt_invariant_failed(cx, {cln}, {v}, {k}u, {iter2});")
                    });
                    l.line("goto fail;");
                    l.indent -= 1;
                    l.line("}");
                },
            );
        }
        self.line("for (;;) {");
        self.indent += 1;
        let site = self.site(cond.span);
        let f = self.fail.clone();
        let tick = if self.t.optimize {
            "cplrt_fast_tick"
        } else {
            "cplrt_tick"
        };
        self.line(&format!("if ({tick}(cx, {site}) != CPLRT_OK) {f}"));
        let c = self.expr(cond);
        self.line(&format!("if (!cplrt_truth({})) break;", c.expr()));
        if let (Some(clause), Some(cl)) = (decreases, dec) {
            let sc = scope.clone();
            let iter2 = iter.clone();
            let m2 = m.clone();
            self.region(clause, cl, &format!("loop/{}/decreases", n + 1), move |l, ve| {
                let cln = l.names.cl(cl);
                l.with_visible(&sc, |v, k| {
                    format!("if (cplrt_loop_measure(cx, {cln}, {v}, {k}u, {iter2}, {ve}, &{m2}) != CPLRT_OK) goto fail;")
                });
            });
        }
        let _ = self.block_result(body, self.t.optimize);
        self.line(&format!("{iter} += 1;"));
        for (i, c) in invariants.iter().enumerate() {
            let cl = inv[i];
            let sc = scope.clone();
            let iter2 = iter.clone();
            self.region(
                c,
                cl,
                &format!("loop/{}/invariant/{}/preserve", n + 1, i + 1),
                move |l, ve| {
                    let cln = l.names.cl(cl);
                    l.line(&format!("if (!cplrt_truth({ve})) {{"));
                    l.indent += 1;
                    l.with_visible(&sc, |v, k| {
                        format!("cplrt_invariant_failed(cx, {cln}, {v}, {k}u, {iter2});")
                    });
                    l.line("goto fail;");
                    l.indent -= 1;
                    l.line("}");
                },
            );
        }
        self.indent -= 1;
        self.line("}");
    }

    // ---- the frame of a function -----------------------------------------------------------------------------------

    fn declarations(&self, extra_ints: &[&str]) -> String {
        let mut out = String::new();
        let mut ints = vec![
            "st = CPLRT_OK".to_string(),
            "eq = 0".to_string(),
            "holds = 0".to_string(),
        ];
        ints.extend(extra_ints.iter().map(|s| s.to_string()));
        let _ = writeln!(out, "    int {};", ints.join(", "));
        if !self.iter_vars.is_empty() {
            let v: Vec<String> = self.iter_vars.iter().map(|i| format!("{i} = 0")).collect();
            let _ = writeln!(out, "    uint64_t {};", v.join(", "));
        }
        let mut slots: Vec<String> = Vec::new();
        let mut seen = BTreeSet::new();
        // the locals of the table first (l<k>_<name>), then the temporaries, measures, and rv
        let mut local_slots: Vec<(u32, String)> = self
            .locals
            .iter()
            .filter_map(|(id, c)| match c {
                LocalC::Slot(n, _) => Some((id.0, n.clone())),
                _ => None,
            })
            .collect();
        local_slots.sort();
        for (_, n) in local_slots {
            if seen.insert(n.clone()) {
                slots.push(n);
            }
        }
        for s in &self.slots {
            if seen.insert(s.clone()) {
                slots.push(s.clone());
            }
        }
        if seen.insert("rv".to_string()) {
            slots.push("rv".to_string());
        }
        for chunk in slots.chunks(12) {
            let items: Vec<String> = chunk.iter().map(|s| format!("{s} = 0")).collect();
            let _ = writeln!(out, "    cplrt_w {};", items.join(", "));
        }
        out
    }

    fn releases(&self) -> String {
        let mut out = String::new();
        let mut seen = BTreeSet::new();
        let mut names: Vec<String> = Vec::new();
        let mut local_slots: Vec<(u32, String)> = self
            .locals
            .iter()
            .filter_map(|(id, c)| match c {
                LocalC::Slot(n, _) => Some((id.0, n.clone())),
                _ => None,
            })
            .collect();
        local_slots.sort();
        for (_, n) in local_slots {
            if seen.insert(n.clone()) {
                names.push(n);
            }
        }
        for s in &self.slots {
            if seen.insert(s.clone()) {
                names.push(s.clone());
            }
        }
        if seen.insert("rv".to_string()) {
            names.push("rv".to_string());
        }
        for chunk in names.chunks(6) {
            let items: Vec<String> = chunk
                .iter()
                .map(|s| format!("cplrt_release({s});"))
                .collect();
            let _ = writeln!(out, "    {}", items.join(" "));
        }
        out
    }
}

/// The C parameter name of a positional parameter.
fn param_name(local: LocalId, name: &str) -> String {
    format!("p{}_{}", local.0, ident(name))
}

fn using_name(local: LocalId, name: &str) -> String {
    format!("u{}_{}", local.0, ident(name))
}

/// The prototype of `cpl_f<n>`.
pub(crate) fn fn_prototype(p: &Program, def: DefId) -> String {
    let fd = p.def(def).kind.as_fn().expect("a function");
    let mut s = format!("static int cpl_f{}(cplrt_cx *cx, uint32_t site", def.0);
    for param in &fd.params {
        let _ = write!(s, ", cplrt_w {}", param_name(param.local, &param.name));
    }
    for u in &fd.using {
        let _ = write!(s, ", const cplrt_capref *{}", using_name(u.local, &u.name));
    }
    s.push_str(", cplrt_w *out)");
    s
}

pub(crate) fn op_prototype(p: &Program, cap: DefId, op: usize) -> String {
    let od = &p.def(cap).kind.as_capability().expect("a capability").ops[op];
    let mut s = format!(
        "static int cpl_o{}_{}(cplrt_cx *cx, uint32_t site, const cplrt_capref *h",
        cap.0, op
    );
    for param in &od.params {
        let _ = write!(s, ", cplrt_w {}", param_name(param.local, &param.name));
    }
    s.push_str(", cplrt_w *out)");
    s
}

pub(crate) fn pred_prototype(def: DefId, self_local: LocalId) -> String {
    format!(
        "static int cpl_p{}(cplrt_cx *cx, uint32_t site, cplrt_w {}, int *out_holds)",
        def.0,
        param_name(self_local, "self")
    )
}

/// The banner comment of an emitted function: its qualified name, its signature text, and its DefId.
fn banner(p: &Program, def: DefId) -> String {
    let d = p.def(def);
    format!(
        "/* {}  [identity {}, DefId {}] */\n",
        comment_text(&p.display_path(def)),
        comment_text(d.stable_id.as_str()),
        def.0
    )
}

/// `cpl_f<n>`: a function with a block body (1.4).
pub(crate) fn emit_fn(t: &mut Tables<'_>, names: &mut Names, def: DefId) -> String {
    let p = t.p;
    let fd = p.def(def).kind.as_fn().expect("a function");
    let FnBody::Block(body) = &fd.body else {
        return String::new();
    };
    let mut l = Lowerer::new(t, names, def, Ctx::Fn);
    for param in &fd.params {
        l.locals.insert(
            param.local,
            LocalC::Param(param_name(param.local, &param.name)),
        );
    }
    for u in &fd.using {
        l.locals
            .insert(u.local, LocalC::Using(using_name(u.local, &u.name)));
    }
    if let Some(r) = fd.result_local {
        l.locals.insert(r, LocalC::Rv);
    }
    let dn = l.def_name(def);
    // requires regions (in every one, `input` is the frame's parameters)
    let req: Vec<u32> = l.t.fn_requires.get(&def).cloned().unwrap_or_default();
    for (i, c) in fd.requires.iter().enumerate() {
        let cl = req.get(i).copied().unwrap_or(0);
        l.region(c, cl, &format!("requires/{}", i + 1), move |l, ve| {
            let cln = l.names.cl(cl);
            l.line(&format!(
                "if (!cplrt_truth({ve})) {{ cplrt_requires_failed(cx, {cln}); goto fail; }}"
            ));
        });
    }
    if let Some(c) = &fd.decreases {
        let cl = l.t.clause(&c.text, CK_DECREASES, c.span, def, None, None);
        l.region(c, cl, "decreases", move |l, ve| {
            let cln = l.names.cl(cl);
            l.line(&format!(
                "if (cplrt_header_measure(cx, &fr, {cln}, {ve}) != CPLRT_OK) goto fail;"
            ));
        });
    }
    let v = l.block(body);
    l.store("rv", v);
    let body_text = std::mem::take(&mut l.body);
    // ensures regions at exit:
    let ens: Vec<u32> = fd
        .ensures
        .iter()
        .map(|c| l.t.clause(&c.text, CK_ENSURES, c.span, def, None, None))
        .collect();
    for (i, c) in fd.ensures.iter().enumerate() {
        let cl = ens[i];
        l.region(c, cl, &format!("ensures/{}", i + 1), move |l, ve| {
            let cln = l.names.cl(cl);
            l.line(&format!(
                "if (!cplrt_truth({ve})) {{ cplrt_ensures_failed(cx, {cln}, rv); goto fail; }}"
            ));
        });
    }
    let ensures_text = std::mem::take(&mut l.body);
    let mut out = std::mem::take(&mut l.helpers);
    out.push_str(&banner(p, def));
    out.push_str(&fn_prototype(p, def));
    out.push_str("\n{\n");
    out.push_str(&l.declarations(&[]));
    out.push_str("    cplrt_frame fr;\n");
    let np = fd.params.len();
    if np == 0 {
        out.push_str("    const cplrt_w *params = NULL;\n");
    } else {
        let names: Vec<String> = fd
            .params
            .iter()
            .map(|x| param_name(x.local, &x.name))
            .collect();
        let _ = writeln!(
            out,
            "    const cplrt_w params[{np}] = {{ {} }};",
            names.join(", ")
        );
    }
    let _ = writeln!(
        out,
        "    if (cplrt_enter_fn(cx, &fr, {dn}, site, params, {np}u) != CPLRT_OK)"
    );
    out.push_str("        return CPLRT_FAULT;\n");
    out.push_str(&body_text);
    if l.exit_used || !fd.ensures.is_empty() {
        out.push_str("exit:\n");
    }
    out.push_str(&ensures_text);
    out.push_str(
        "    CPLRT_MOVE(*out, rv);\n    goto done;\nfail:\n    st = CPLRT_FAULT;\ndone:\n",
    );
    out.push_str(&l.releases());
    out.push_str("    return cplrt_leave(cx, &fr, st);\n}\n");
    format!("{}{out}", l.helpers)
}

/// `cpl_p<n>`: the `where` predicate of a refinement (1.4).
pub(crate) fn emit_pred(t: &mut Tables<'_>, names: &mut Names, def: DefId) -> String {
    let p = t.p;
    let rd = p
        .def(def)
        .kind
        .checked_predicate()
        .expect("a checked predicate")
        .clone();
    let ri = t.ref_ix.get(&def).copied().unwrap_or(0);
    let cl = t.refs[ri as usize].clause;
    let mut l = Lowerer::new(t, names, def, Ctx::Pred);
    l.locals.insert(
        rd.self_local,
        LocalC::Param(param_name(rd.self_local, "self")),
    );
    let dn = l.def_name(def);
    let cln = l.names.cl(cl);
    l.fail = format!("{{ cplrt_claim(cx, {cln}); goto fail; }}");
    l.region = Some(rd.predicate.expr.span);
    // the predicate is lowered from a clause the HIR owns; borrow it through the program for 'p
    let clause: &Clause = &p
        .def(def)
        .kind
        .checked_predicate()
        .expect("a checked predicate")
        .predicate;
    let v = l.expr(&clause.expr);
    l.line(&format!("*out_holds = cplrt_truth({});", v.expr()));
    let body = std::mem::take(&mut l.body);
    let mut out = std::mem::take(&mut l.helpers);
    out.push_str(&banner(p, def));
    out.push_str(&pred_prototype(def, rd.self_local));
    out.push_str("\n{\n");
    out.push_str(&l.declarations(&[]));
    out.push_str("    cplrt_frame fr;\n");
    let _ = writeln!(
        out,
        "    if (cplrt_enter_act(cx, &fr, {dn}, site) != CPLRT_OK)"
    );
    out.push_str("        return CPLRT_FAULT;\n");
    out.push_str(&body);
    out.push_str("    goto done;\nfail:\n    st = CPLRT_FAULT;\ndone:\n");
    out.push_str(&l.releases());
    out.push_str("    return cplrt_leave(cx, &fr, st);\n}\n");
    format!("{}{out}", l.helpers)
}

/// `cpl_o<cap>_<op>`: a capability operation (1.4, 2.6).
pub(crate) fn emit_op(t: &mut Tables<'_>, names: &mut Names, cap: DefId, op: usize) -> String {
    let p = t.p;
    let od = &p.def(cap).kind.as_capability().expect("a capability").ops[op];
    let mut l = Lowerer::new(t, names, cap, Ctx::Op);
    l.locals.clear();
    for ld in &od.locals {
        let name = format!("l{}_{}", ld.id.0, ident(&ld.name));
        l.locals.insert(ld.id, LocalC::Slot(name, ld.mutable));
    }
    for param in &od.params {
        l.locals.insert(
            param.local,
            LocalC::Param(param_name(param.local, &param.name)),
        );
    }
    if let Some(r) = od.result_local {
        l.locals.insert(r, LocalC::Rv);
    }
    l.scopes.clear();
    let n = od.params.len();
    let args = if n == 0 {
        "NULL".to_string()
    } else {
        "args".to_string()
    };
    for (i, c) in od.requires.iter().enumerate() {
        let cl =
            l.t.clause(&c.text, CK_REQUIRES, c.span, cap, Some(op as u32), None);
        let args2 = args.clone();
        l.region(c, cl, &format!("op requires/{}", i + 1), move |l, ve| {
            let cln = l.names.cl(cl);
            l.line(&format!(
                "if (!cplrt_truth({ve})) {{ cplrt_op_requires_failed(cx, {cln}, {args2}, {n}u); goto fail; }}"
            ));
        });
    }
    l.line(&format!(
        "if (cplrt_cap_invoke(cx, site, h, {op}u, {args}, {n}u, &rv) != CPLRT_OK)"
    ));
    l.line("    goto fail;");
    for (i, c) in od.ensures.iter().enumerate() {
        let cl =
            l.t.clause(&c.text, CK_ENSURES, c.span, cap, Some(op as u32), None);
        let args2 = args.clone();
        l.region(c, cl, &format!("op ensures/{}", i + 1), move |l, ve| {
            let cln = l.names.cl(cl);
            l.line(&format!(
                "if (!cplrt_truth({ve})) {{ cplrt_op_ensures_failed(cx, {cln}, {args2}, {n}u, rv); goto fail; }}"
            ));
        });
    }
    let body = std::mem::take(&mut l.body);
    let mut out = format!(
        "/* {}  [capability DefId {}, operation {op}] */\n",
        comment_text(cpl_eval::types::operation_identity(p, cap, &od.name).as_str()),
        cap.0
    );
    out.push_str(&op_prototype(p, cap, op));
    out.push_str("\n{\n");
    out.push_str(&l.declarations(&[]));
    if n > 0 {
        let names: Vec<String> = od
            .params
            .iter()
            .map(|x| param_name(x.local, &x.name))
            .collect();
        let _ = writeln!(
            out,
            "    const cplrt_w args[{n}] = {{ {} }};",
            names.join(", ")
        );
    }
    out.push_str(&body);
    out.push_str(
        "    CPLRT_MOVE(*out, rv);\n    goto done;\nfail:\n    st = CPLRT_FAULT;\ndone:\n",
    );
    out.push_str(&l.releases());
    out.push_str("    return st;\n}\n");
    format!("{}{out}", l.helpers)
}

/// `cpl_t<n>`: a test or property body (drivers only).
pub(crate) fn emit_test(t: &mut Tables<'_>, names: &mut Names, def: DefId) -> String {
    let p = t.p;
    let (body, binders): (&Block, Vec<LocalId>) = match &p.def(def).kind {
        DefKind::Test(td) => (&td.body, Vec::new()),
        DefKind::Property(pd) => (&pd.body, pd.binders.iter().map(|b| b.local).collect()),
        _ => return String::new(),
    };
    let mut l = Lowerer::new(t, names, def, Ctx::Test);
    let dn = l.def_name(def);
    for (i, b) in binders.iter().enumerate() {
        if let Some(LocalC::Slot(n, _)) = l.locals.get(b).cloned() {
            l.line(&format!("CPLRT_COPY({n}, binders[{i}]);"));
        }
    }
    let _ = l.block(body);
    let text = std::mem::take(&mut l.body);
    let mut out = std::mem::take(&mut l.helpers);
    out.push_str(&banner(p, def));
    let _ = writeln!(
        out,
        "static int cpl_t{}(cplrt_cx *cx, const cplrt_w *binders)\n{{",
        def.0
    );
    out.push_str(&l.declarations(&[]));
    out.push_str("    cplrt_frame fr;\n");
    let _ = writeln!(
        out,
        "    if (cplrt_enter_act(cx, &fr, {dn}, CPLRT_NO_SITE) != CPLRT_OK)"
    );
    out.push_str("        return CPLRT_FAULT;\n");
    out.push_str(&text);
    out.push_str("    goto done;\nfail:\n    st = CPLRT_FAULT;\ndone:\n");
    out.push_str(&l.releases());
    out.push_str("    return cplrt_leave(cx, &fr, st);\n}\n");
    format!("{}{out}", l.helpers)
}

/// `cpl_b<n>_<binder>_lo` and `_hi`: the bounds of a property's `gen.range` (drivers only).
pub(crate) fn emit_bounds(
    t: &mut Tables<'_>,
    names: &mut Names,
    def: DefId,
) -> (String, Vec<(u32, bool, String)>) {
    let p = t.p;
    let Some(pd) = p.def(def).kind.as_property() else {
        return (String::new(), Vec::new());
    };
    let mut out = String::new();
    let mut entries = Vec::new();
    for (i, b) in pd.binders.iter().enumerate() {
        let mut g = &b.gen;
        let range = loop {
            match g {
                GenExpr::Range(lo, hi) => break Some((lo, hi)),
                GenExpr::List(inner) | GenExpr::Option(inner) => g = inner,
                _ => break None,
            }
        };
        let Some((lo, hi)) = range else { continue };
        for (side, e) in [(false, lo), (true, hi)] {
            let name = format!("cpl_b{}_{}_{}", def.0, i, if side { "hi" } else { "lo" });
            // the property's locals stay slots: a block of the range expression binds its own (`let m = 1`)
            let mut l = Lowerer::new(t, names, def, Ctx::Bound);
            let dn = l.def_name(def);
            l.fail = format!("{{ cplrt_claim_range(cx, {dn}); goto fail; }}");
            l.region = Some(e.span);
            let v = l.expr(e);
            l.store("rv", v);
            let text = std::mem::take(&mut l.body);
            out.push_str(&l.helpers);
            let _ = writeln!(out, "static int {name}(cplrt_cx *cx, cplrt_w *out)\n{{");
            out.push_str(&l.declarations(&[]));
            out.push_str("    cplrt_frame fr;\n");
            let _ = writeln!(
                out,
                "    if (cplrt_enter_act(cx, &fr, {dn}, CPLRT_NO_SITE) != CPLRT_OK)"
            );
            out.push_str("        return CPLRT_FAULT;\n");
            out.push_str(&text);
            out.push_str(
                "    CPLRT_MOVE(*out, rv);\n    goto done;\nfail:\n    st = CPLRT_FAULT;\ndone:\n",
            );
            out.push_str(&l.releases());
            out.push_str("    return cplrt_leave(cx, &fr, st);\n}\n");
            entries.push((i as u32, side, name));
        }
    }
    (out, entries)
}

/// `cpl_e<n>`: the entry trampoline of a function with a block body, or of a callable builtin stub.
pub(crate) fn emit_entry(p: &Program, names: &mut Names, def: DefId) -> Option<String> {
    let fd = p.def(def).kind.as_fn()?;
    let mut call = match &fd.body {
        FnBody::Block(_) => format!("cpl_f{}(cx, CPLRT_NO_SITE", def.0),
        FnBody::Builtin(id) => {
            if !cpl_rtspec::builtins::spec(id).is_some_and(|s| s.callable) {
                return None;
            }
            let d = names.d(def.0);
            format!("cplrt_bi({id})(cx, CPLRT_NO_SITE, {d}")
        }
    };
    for i in 0..fd.params.len() {
        let _ = write!(call, ", args[{i}]");
    }
    if matches!(fd.body, FnBody::Block(_)) {
        for i in 0..fd.using.len() {
            let _ = write!(call, ", using[{i}]");
        }
    }
    call.push_str(", out)");
    Some(format!(
        "static int cpl_e{}(cplrt_cx *cx, const cplrt_w *args, const cplrt_capref *const *using, cplrt_w *out)\n{{\n    return {call};\n}}\n",
        def.0
    ))
}

/// The C definition of a program-unit constant.
pub(crate) fn emit_const(k: u32, key: &ConstKey) -> String {
    match key {
        ConstKey::Text(s) => {
            let bytes = s.len();
            let scalars = s.chars().count();
            format!(
                "static struct {{ cplrt_obj hdr; size_t bytes, scalars; char utf8[{}]; }} cpl_k{k} = {{ {{ CPLRT_TEXT, 0u, 0u, 0u, {{ .live = {{ CPLRT_RC_IMMORTAL, 0u }} }} }}, {bytes}u, {scalars}u, {} }};\n",
                bytes + 1,
                c_bytes(s.as_bytes())
            )
        }
        ConstKey::Int(n) => {
            let (sign, limbs) = n.to_u32_digits();
            let flags = if sign == Sign::Minus {
                "CPLRT_F_NEGATIVE"
            } else {
                "0u"
            };
            let items: Vec<String> = limbs.iter().map(|l| format!("{l}u")).collect();
            format!(
                "static struct {{ cplrt_obj hdr; uint32_t limb[{}]; }} cpl_k{k} = {{ {{ CPLRT_BIGINT, {flags}, 0u, {}u, {{ .live = {{ CPLRT_RC_IMMORTAL, 0u }} }} }}, {{ {} }} }};\n",
                limbs.len(),
                limbs.len(),
                items.join(", ")
            )
        }
    }
}

/// Whether `def` is a function whose `cpl_f` is emitted.
pub(crate) fn emits_fn(p: &Program, def: DefId) -> bool {
    has_block_body(p, def)
}

/// The capability type of a `using` parameter.
#[allow(dead_code)]
pub(crate) fn capability_of(ty: &Ty) -> Option<DefId> {
    match ty {
        Ty::Capability(c) => Some(*c),
        _ => None,
    }
}
