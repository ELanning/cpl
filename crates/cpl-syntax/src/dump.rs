//! Deterministic S-expression rendering of the AST, without spans.
//!
//! The dump is used by the golden tests (`tests/golden/*.ast`) and by later
//! crates for debugging. Every node renders as `(head child*)`; a node's
//! trivia, when present, is its last child: `(trivia blank (lead "// c")
//! (lead-blank "// d") (trail "// e"))` where `blank` records a blank line
//! before the node, `lead-blank` a comment preceded by a blank line, and
//! `trail` a trailing comment. Doc comments render as `(docs "line" ...)`.
//! An annotation renders as `(annotation name arg*)`, and one written with an
//! empty argument list, `@name()`, as `(annotation name ())`.
//! Lists that fit in 100 columns are printed on one line; longer ones put
//! each child on its own line, indented by two spaces.

use crate::ast::*;

const WIDTH: usize = 100;

enum S {
    A(String),
    L(Vec<S>),
}

fn a(text: impl Into<String>) -> S {
    S::A(text.into())
}

fn q(text: &str) -> S {
    S::A(format!("{text:?}"))
}

fn l(items: Vec<S>) -> S {
    S::L(items)
}

impl S {
    fn flat(&self) -> String {
        match self {
            S::A(s) => s.clone(),
            S::L(items) => {
                let parts: Vec<String> = items.iter().map(S::flat).collect();
                format!("({})", parts.join(" "))
            }
        }
    }

    fn has_nested_list(&self) -> bool {
        match self {
            S::A(_) => false,
            S::L(items) => items.iter().any(|i| matches!(i, S::L(_))),
        }
    }

    fn render(&self, out: &mut String, indent: usize) {
        let flat = self.flat();
        if indent + flat.len() <= WIDTH || !self.has_nested_list() {
            out.push_str(&flat);
            return;
        }
        let S::L(items) = self else {
            out.push_str(&flat);
            return;
        };
        // Leading atoms (the head and its name-like arguments) stay on the
        // first line; every following child gets its own line.
        out.push('(');
        let mut index = 0;
        while let Some(S::A(atom)) = items.get(index) {
            if index > 0 {
                out.push(' ');
            }
            out.push_str(atom);
            index += 1;
        }
        for item in &items[index..] {
            out.push('\n');
            out.push_str(&" ".repeat(indent + 2));
            item.render(out, indent + 2);
        }
        out.push(')');
    }
}

fn finish(s: S) -> String {
    let mut out = String::new();
    s.render(&mut out, 0);
    out.push('\n');
    out
}

/// Render a `.cpl` module.
pub fn dump_module(module: &Module) -> String {
    let mut items = vec![a("module"), a(module.header.path.dotted())];
    if let Some(d) = &module.docs {
        items.push(docs(d));
    }
    for u in &module.uses {
        items.push(use_decl(u));
    }
    for d in &module.declarations {
        items.push(declaration(d));
    }
    push_trailing_comments(&mut items, &module.trailing_comments);
    push_trivia(&mut items, &module.trivia);
    finish(l(items))
}

/// Render a `.work` file.
pub fn dump_work(work: &WorkFile) -> String {
    let mut items = vec![a("work")];
    for u in &work.uses {
        items.push(use_decl(u));
    }
    for c in &work.changes {
        items.push(change(c));
    }
    push_trailing_comments(&mut items, &work.trailing_comments);
    finish(l(items))
}

/// Render a `.patch` file.
pub fn dump_patch(patch: &PatchFile) -> String {
    let mut items = vec![a("patch-file")];
    for f in &patch.forms {
        items.push(patch_form(f));
    }
    push_trailing_comments(&mut items, &patch.trailing_comments);
    finish(l(items))
}

fn push_trivia(items: &mut Vec<S>, t: &Trivia) {
    if t.is_empty() {
        return;
    }
    let mut parts = vec![a("trivia")];
    if t.blank_line_before {
        parts.push(a("blank"));
    }
    for c in &t.leading {
        let head = if c.blank_line_before {
            "lead-blank"
        } else {
            "lead"
        };
        parts.push(l(vec![a(head), q(&c.text)]));
    }
    for c in &t.trailing {
        parts.push(l(vec![a("trail"), q(&c.text)]));
    }
    items.push(l(parts));
}

fn push_trailing_comments(items: &mut Vec<S>, comments: &[Comment]) {
    if comments.is_empty() {
        return;
    }
    let mut parts = vec![a("trailing-comments")];
    for c in comments {
        parts.push(q(&c.text));
    }
    items.push(l(parts));
}

fn docs(d: &Docs) -> S {
    let mut parts = vec![a("docs")];
    for line in &d.lines {
        parts.push(q(&line.text));
    }
    l(parts)
}

fn use_decl(u: &UseDecl) -> S {
    let mut items = vec![a("use"), a(u.path.dotted())];
    if u.braced {
        let mut list = vec![a("items")];
        for item in &u.items {
            if item.trivia.is_empty() {
                list.push(a(&item.name.text));
            } else {
                let mut parts = vec![a("item"), a(&item.name.text)];
                push_trivia(&mut parts, &item.trivia);
                list.push(l(parts));
            }
        }
        push_trailing_comments(&mut list, &u.trailing_comments);
        items.push(l(list));
    } else if let Some(item) = u.items.first() {
        items.push(a(&item.name.text));
    }
    push_trivia(&mut items, &u.trivia);
    l(items)
}

fn declaration(d: &Declaration) -> S {
    let mut items: Vec<S> = Vec::new();
    match &d.kind {
        DeclKind::Type(t) => {
            items.push(a("type"));
            items.push(a(&t.name.text));
            if t.is_pub {
                items.push(a("pub"));
            }
            push_prefix(&mut items, d);
            match &t.kind {
                TypeDeclKind::Wrapper(w) => items.push(l(vec![a("wrapper"), ty(&w.payload)])),
                TypeDeclKind::Refinement(r) => items.push(l(vec![
                    a("refinement"),
                    ty(&r.base),
                    l(vec![a("where"), expr(&r.predicate)]),
                ])),
                TypeDeclKind::Opaque(o) => {
                    let mut parts = vec![a("opaque")];
                    if let Some(g) = &o.generic_params {
                        parts.push(generic_params(g));
                    }
                    items.push(l(parts));
                }
            }
        }
        DeclKind::Struct(s) => {
            items.push(a(if s.is_program { "program" } else { "struct" }));
            items.push(a(&s.name.text));
            if s.is_pub {
                items.push(a("pub"));
            }
            push_prefix(&mut items, d);
            if let Some(g) = &s.generic_params {
                items.push(generic_params(g));
            }
            for f in &s.fields {
                let mut parts = vec![a("field"), a(&f.name.text), ty(&f.ty)];
                if let Some(doc) = &f.docs {
                    parts.push(docs(doc));
                }
                push_trivia(&mut parts, &f.trivia);
                items.push(l(parts));
            }
            if let Some(inv) = &s.invariant {
                items.push(l(vec![a("where"), expr(&inv.expr)]));
            }
            push_trailing_comments(&mut items, &s.trailing_comments);
        }
        DeclKind::Enum(e) => {
            items.push(a("enum"));
            items.push(a(&e.name.text));
            if e.is_pub {
                items.push(a("pub"));
            }
            push_prefix(&mut items, d);
            if let Some(g) = &e.generic_params {
                items.push(generic_params(g));
            }
            for v in &e.variants {
                let mut parts = vec![a("variant"), a(&v.name.text)];
                for t in &v.payload {
                    parts.push(ty(t));
                }
                if let Some(doc) = &v.docs {
                    parts.push(docs(doc));
                }
                push_trivia(&mut parts, &v.trivia);
                items.push(l(parts));
            }
            push_trailing_comments(&mut items, &e.trailing_comments);
        }
        DeclKind::Capability(c) => {
            items.push(a(if c.is_interface {
                "interface"
            } else {
                "capability"
            }));
            items.push(a(&c.name.text));
            if c.is_pub {
                items.push(a("pub"));
            }
            push_prefix(&mut items, d);
            for op in &c.operations {
                let mut parts = vec![a("op"), a(&op.name.text)];
                parts.push(params(&op.params));
                parts.push(l(vec![a("ret"), ty(&op.ret)]));
                for c in &op.clauses {
                    parts.push(contract_clause(c));
                }
                if let Some(doc) = &op.docs {
                    parts.push(docs(doc));
                }
                push_trivia(&mut parts, &op.trivia);
                items.push(l(parts));
            }
            push_trailing_comments(&mut items, &c.trailing_comments);
        }
        DeclKind::Fn(f) => {
            items.push(a(f.mode.prefix()));
            items.push(a(&f.name.text));
            if f.is_pub {
                items.push(a("pub"));
            }
            push_prefix(&mut items, d);
            if let Some(g) = &f.generic_params {
                items.push(generic_params(g));
            }
            items.push(params(&f.params));
            items.push(l(vec![a("ret"), ty(&f.ret)]));
            for c in &f.clauses {
                items.push(match c {
                    FnClause::Effects(e) => {
                        let mut parts = vec![a("effects")];
                        for r in &e.effects {
                            if r.trivia.is_empty() {
                                parts.push(a(format!("{}.{}", r.param.text, r.operation.text)));
                            } else {
                                let mut ep = vec![
                                    a("effect"),
                                    a(format!("{}.{}", r.param.text, r.operation.text)),
                                ];
                                push_trivia(&mut ep, &r.trivia);
                                parts.push(l(ep));
                            }
                        }
                        push_trailing_comments(&mut parts, &e.trailing_comments);
                        push_trivia(&mut parts, &e.trivia);
                        l(parts)
                    }
                    FnClause::Contract(c) => contract_clause(c),
                    FnClause::Decreases(dc) => {
                        let mut parts = vec![a("decreases"), expr(&dc.expr)];
                        push_trivia(&mut parts, &dc.trivia);
                        l(parts)
                    }
                });
            }
            match &f.body {
                Some(b) => items.push(block(b)),
                None => items.push(a("bodiless")),
            }
        }
        DeclKind::Test(t) => {
            items.push(a("test"));
            items.push(q(&t.name.value));
            push_prefix(&mut items, d);
            items.push(block(&t.body));
        }
        DeclKind::Property(p) => {
            items.push(a("property"));
            items.push(q(&p.name.value));
            push_prefix(&mut items, d);
            for fc in &p.for_clauses {
                let mut parts = vec![a("for"), a(&fc.binder.text), expr(&fc.range)];
                push_trivia(&mut parts, &fc.trivia);
                items.push(l(parts));
            }
            items.push(block(&p.body));
        }
    }
    push_trivia(&mut items, &d.trivia);
    l(items)
}

/// Docs and annotations of a declaration.
fn push_prefix(items: &mut Vec<S>, d: &Declaration) {
    if let Some(doc) = &d.docs {
        items.push(docs(doc));
    }
    for ann in &d.annotations {
        let mut parts = vec![a("annotation"), a(&ann.name.text)];
        for arg in &ann.args {
            parts.push(literal(arg));
        }
        // `@name()`: the empty argument list is part of the source (T14);
        // with arguments the parentheses are implied.
        if ann.parens && ann.args.is_empty() {
            parts.push(l(Vec::new()));
        }
        push_trivia(&mut parts, &ann.trivia);
        items.push(l(parts));
    }
}

fn contract_clause(c: &ContractClause) -> S {
    let mut parts = vec![a(c.kind.keyword()), expr(&c.expr)];
    push_trivia(&mut parts, &c.trivia);
    l(parts)
}

fn generic_params(g: &GenericParams) -> S {
    let mut parts = vec![a("generics")];
    for p in &g.params {
        parts.push(a(&p.text));
    }
    l(parts)
}

fn params(p: &Params) -> S {
    let mut parts = vec![a("params")];
    for x in &p.positional {
        parts.push(param(x));
    }
    for u in &p.using {
        let mut up = vec![a("using"), a(&u.name.text), ty(&u.ty)];
        push_trivia(&mut up, &u.trivia);
        parts.push(l(up));
    }
    push_trailing_comments(&mut parts, &p.trailing_comments);
    l(parts)
}

fn param(p: &Param) -> S {
    let mut parts = vec![a("param"), a(&p.name.text), ty(&p.ty)];
    push_trivia(&mut parts, &p.trivia);
    l(parts)
}

fn ty(t: &Type) -> S {
    let mut parts = vec![a("ty"), a(t.path.dotted())];
    if let Some(g) = &t.generic_args {
        for arg in &g.args {
            parts.push(ty(arg));
        }
    }
    l(parts)
}

fn literal(lit: &Literal) -> S {
    match &lit.kind {
        LiteralKind::Int(d) => l(vec![a("int"), a(d)]),
        LiteralKind::Text(t) => l(vec![a("text"), q(t)]),
        LiteralKind::Bool(b) => l(vec![a("bool"), a(b.to_string())]),
    }
}

fn block(b: &Block) -> S {
    let mut parts = vec![a("block")];
    for s in &b.statements {
        parts.push(statement(s));
    }
    if let Some(t) = &b.tail {
        let mut tp = vec![a("tail"), expr(&t.expr)];
        push_trivia(&mut tp, &t.trivia);
        parts.push(l(tp));
    }
    push_trailing_comments(&mut parts, &b.trailing_comments);
    l(parts)
}

fn statement(s: &Statement) -> S {
    let mut parts = match s {
        Statement::Let(st) => {
            let mut p = vec![a("let")];
            match &st.binding {
                LetBinding::Name(n) => p.push(a(&n.text)),
                LetBinding::Wildcard(_) => p.push(a("_")),
            }
            if let Some(t) = &st.ty {
                p.push(ty(t));
            }
            p.push(expr(&st.value));
            p
        }
        Statement::Var(st) => {
            let mut p = vec![a("var"), a(&st.name.text)];
            if let Some(t) = &st.ty {
                p.push(ty(t));
            }
            p.push(expr(&st.value));
            p
        }
        Statement::Assign(st) => vec![a("assign"), a(&st.target.text), expr(&st.value)],
        Statement::Return(st) => {
            let mut p = vec![a("return")];
            if let Some(v) = &st.value {
                p.push(expr(v));
            }
            p
        }
        Statement::Assert(st) => vec![a("assert"), expr(&st.condition)],
        Statement::While(st) => {
            let mut p = vec![a("while"), expr(&st.condition)];
            for c in &st.clauses {
                p.push(match c {
                    WhileClause::Invariant(i) => {
                        let mut ip = vec![a("invariant"), expr(&i.expr)];
                        push_trivia(&mut ip, &i.trivia);
                        l(ip)
                    }
                    WhileClause::Decreases(d) => {
                        let mut dp = vec![a("decreases"), expr(&d.expr)];
                        push_trivia(&mut dp, &d.trivia);
                        l(dp)
                    }
                });
            }
            p.push(block(&st.body));
            p
        }
        Statement::Expr(st) => vec![a("expr"), expr(&st.expr)],
    };
    push_trivia(&mut parts, s.trivia());
    l(parts)
}

fn expr(e: &Expr) -> S {
    match &e.kind {
        ExprKind::Literal(lit) => literal(lit),
        ExprKind::Name(n) => l(vec![a("name"), a(&n.text)]),
        ExprKind::Hole(n) => l(vec![a("hole"), a(&n.text)]),
        ExprKind::Forall(f) | ExprKind::Parallel(f) => l(vec![
            a(if matches!(e.kind, ExprKind::Parallel(_)) {
                "parallel"
            } else {
                "forall"
            }),
            a(&f.binder.text),
            expr(&f.collection),
            expr(&f.predicate),
        ]),
        ExprKind::Binary(b) => l(vec![a(b.op.symbol()), expr(&b.lhs), expr(&b.rhs)]),
        ExprKind::Unary(u) => {
            let head = match u.op {
                UnaryOp::Not => "not",
                UnaryOp::Neg => "neg",
            };
            l(vec![a(head), expr(&u.operand)])
        }
        ExprKind::Member(m) => l(vec![a("member"), expr(&m.target), a(&m.member.text)]),
        ExprKind::Call(c) => {
            let mut parts = vec![a("call"), expr(&c.callee)];
            if let Some(g) = &c.generic_args {
                let mut gp = vec![a("generics")];
                for t in &g.args {
                    gp.push(ty(t));
                }
                parts.push(l(gp));
            }
            for arg in &c.args.positional {
                if arg.trivia.is_empty() {
                    parts.push(expr(&arg.expr));
                } else {
                    let mut ap = vec![a("arg"), expr(&arg.expr)];
                    push_trivia(&mut ap, &arg.trivia);
                    parts.push(l(ap));
                }
            }
            for u in &c.args.using {
                let mut up = vec![a("using"), expr(&u.expr)];
                push_trivia(&mut up, &u.trivia);
                parts.push(l(up));
            }
            push_trailing_comments(&mut parts, &c.args.trailing_comments);
            l(parts)
        }
        ExprKind::Propagate(inner) => l(vec![a("propagate"), expr(inner)]),
        ExprKind::Paren(inner) => l(vec![a("paren"), expr(inner)]),
        ExprKind::List(list) => {
            let mut parts = vec![a("list")];
            for el in &list.elements {
                if el.trivia.is_empty() {
                    parts.push(expr(&el.expr));
                } else {
                    let mut ep = vec![a("elem"), expr(&el.expr)];
                    push_trivia(&mut ep, &el.trivia);
                    parts.push(l(ep));
                }
            }
            push_trailing_comments(&mut parts, &list.trailing_comments);
            l(parts)
        }
        ExprKind::Struct(s) => {
            let mut parts = vec![a("struct-lit"), a(s.path.dotted())];
            if let Some(sp) = &s.entries.spread {
                let mut spp = vec![a("spread"), expr(sp)];
                push_trivia(&mut spp, &s.entries.spread_trivia);
                parts.push(l(spp));
            }
            for f in &s.entries.fields {
                let mut fp = vec![a("field"), a(&f.name.text), expr(&f.value)];
                push_trivia(&mut fp, &f.trivia);
                parts.push(l(fp));
            }
            push_trailing_comments(&mut parts, &s.entries.trailing_comments);
            l(parts)
        }
        ExprKind::If(i) => if_expr(i),
        ExprKind::Match(m) => {
            let mut parts = vec![a("match"), expr(&m.scrutinee)];
            for arm in &m.arms {
                let body = match &arm.body {
                    ArmBody::Block(b) => block(b),
                    ArmBody::Expr(e) => expr(e),
                };
                let mut ap = vec![a("arm"), pattern(&arm.pattern), body];
                push_trivia(&mut ap, &arm.trivia);
                parts.push(l(ap));
            }
            push_trailing_comments(&mut parts, &m.trailing_comments);
            l(parts)
        }
    }
}

fn if_expr(i: &IfExpr) -> S {
    let mut parts = vec![a("if"), expr(&i.condition), block(&i.then_block)];
    if let Some(e) = &i.else_branch {
        parts.push(match e {
            ElseBranch::Block(b) => l(vec![a("else"), block(b)]),
            ElseBranch::If(nested) => l(vec![a("else"), if_expr(nested)]),
        });
    }
    l(parts)
}

fn pattern(p: &Pattern) -> S {
    match &p.kind {
        PatternKind::Wildcard => a("_"),
        PatternKind::Int { negative, digits } => l(vec![
            a("pat-int"),
            a(format!("{}{}", if *negative { "-" } else { "" }, digits)),
        ]),
        PatternKind::Text(t) => l(vec![a("pat-text"), q(t)]),
        PatternKind::Bool(b) => l(vec![a("pat-bool"), a(b.to_string())]),
        PatternKind::Binding(n) => l(vec![a("bind"), a(&n.text)]),
        PatternKind::Ctor(c) => {
            let mut parts = vec![a("ctor"), a(c.path.dotted())];
            if let Some(args) = &c.args {
                for arg in args {
                    parts.push(pattern(arg));
                }
            }
            l(parts)
        }
    }
}

fn selector(s: &Selector) -> S {
    let path = s.path.dotted();
    let text = match &s.kind {
        SelectorKind::Namespace => format!("{path}.*"),
        SelectorKind::Facet(f) => format!("{path}.{}", f.kind.as_str()),
        SelectorKind::Declaration => path,
    };
    if s.trivia.is_empty() {
        a(text)
    } else {
        let mut parts = vec![a("sel"), a(text)];
        push_trivia(&mut parts, &s.trivia);
        l(parts)
    }
}

fn selector_set(head: &str, set: &SelectorSet) -> S {
    let mut parts = vec![a(head)];
    for s in &set.selectors {
        parts.push(selector(s));
    }
    push_trailing_comments(&mut parts, &set.trailing_comments);
    l(parts)
}

fn change(c: &ChangeDecl) -> S {
    let mut items = vec![a("change"), a(&c.name.text)];
    if let Some(d) = &c.docs {
        items.push(docs(d));
    }
    for item in &c.items {
        let mut parts = match &item.kind {
            ChangeItemKind::Base(g) => vec![a("base"), q(&g.reference.value)],
            ChangeItemKind::Target(g) => vec![a("target"), q(&g.reference.value)],
            ChangeItemKind::Workers(w) => vec![a("workers"), a(&w.digits)],
            ChangeItemKind::Task(t) => {
                let mut tp = vec![a("task"), a(&t.name.text)];
                if let Some(d) = &t.docs {
                    tp.push(docs(d));
                }
                if let Some(after) = &t.after {
                    let mut ap = vec![a("after")];
                    for n in &after.tasks {
                        ap.push(a(&n.text));
                    }
                    tp.push(l(ap));
                }
                for ti in &t.items {
                    let mut tip = match &ti.kind {
                        TaskItemKind::Write(s) => vec![selector_set("write", s)],
                        TaskItemKind::Add(s) => vec![selector_set("add", s)],
                        TaskItemKind::Depend(s) => vec![selector_set("depend", s)],
                        TaskItemKind::Preserve(s) => vec![selector_set("preserve", s)],
                        TaskItemKind::Goal(g) => vec![l(vec![a("goal"), q(&g.value)])],
                    };
                    if ti.trivia.is_empty() {
                        tp.push(tip.remove(0));
                    } else {
                        let mut wrapped = vec![a("item"), tip.remove(0)];
                        push_trivia(&mut wrapped, &ti.trivia);
                        tp.push(l(wrapped));
                    }
                }
                push_trailing_comments(&mut tp, &t.trailing_comments);
                tp
            }
            ChangeItemKind::Integrate(i) => {
                let mut ip = vec![a("integrate")];
                for r in &i.requirements {
                    let req = match &r.requirement.kind {
                        RequirementKind::Typecheck => a("typecheck"),
                        RequirementKind::NoHoles => a("no_holes"),
                        RequirementKind::NoUnresolvedAssumptions => a("no_unresolved_assumptions"),
                        RequirementKind::Prove(s) => l(vec![a("prove"), selector(s)]),
                        RequirementKind::Test(s) => l(vec![a("test"), selector(s)]),
                        RequirementKind::Review(n) => l(vec![a("review"), a(&n.text)]),
                    };
                    let mut rp = vec![a("require"), req];
                    push_trivia(&mut rp, &r.trivia);
                    ip.push(l(rp));
                }
                push_trailing_comments(&mut ip, &i.trailing_comments);
                ip
            }
        };
        push_trivia(&mut parts, &item.trivia);
        items.push(l(parts));
    }
    push_trailing_comments(&mut items, &c.trailing_comments);
    push_trivia(&mut items, &c.trivia);
    l(items)
}

fn patch_form(f: &PatchForm) -> S {
    let mut items = vec![a("patch"), selector(&f.target)];
    if let Some(d) = &f.docs {
        items.push(docs(d));
    }
    let mut ep = vec![a("expect"), selector(&f.expect.base)];
    push_trivia(&mut ep, &f.expect.trivia);
    items.push(l(ep));
    items.push(block(&f.body));
    push_trivia(&mut items, &f.trivia);
    l(items)
}
