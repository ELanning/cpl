//! The `clause` of a fault raised while a property's `for` range is
//! evaluated (semantics 5.1, 7.4, S34, S71): the clause text of the range.
//!
//! A `for` clause's range is a contract expression (5.1), and the `clause`
//! of every other contract expression is the clause text of the expression
//! alone, without its keyword (`n >= 0` for `requires n >= 0`, `self > 0`
//! for `where self > 0`; toolchain 3.3 `Clause::text`, 5.2). So a fault in
//! `for x in gen.range(1 / 0, 5)` reports the clause `gen.range(1 / 0, 5)`
//! (S71). The HIR keeps no text for a `for` clause (toolchain 3.3
//! `Binder`), so the text is derived from the formatter's canonical
//! rendering of the clause's range in the property's AST (toolchain 5.2,
//! `format_fragment`), which every checked program carries
//! (`Program::asts`). That rendering is the range's *facet text*, which
//! keeps the multi-line layout of the blocks of an `if` or `match`; the
//! `clause` is its *clause text*, the facet text joined onto one line by
//! replacing every line break and the indentation after it with one space
//! (toolchain 5.2, T56 [D-246]; [`one_line`]), exactly as the checker
//! derives `Clause::text`. A program built without ASTs (by hand, for
//! tests) gets [`Render`]'s rendering of the HIR in the same syntax, joined
//! the same way; it differs from the canonical text only where the HIR no
//! longer knows the source: grouping parentheses (re-inserted by
//! precedence, grammar 3), qualified spellings of names (written bare), and
//! `let` type annotations (omitted).

use crate::types::bare_name;
use cpl_diag::{quote_text, DeclName};
use cpl_hir::{
    Binder, Block, DefId, Expr, ExprKind, GenExpr, Lit, LocalDef, LocalId, Pattern, PatternKind,
    Program, Stmt,
};
use cpl_syntax::ast::{self, BinaryOp, UnaryOp};
use cpl_syntax::{format_fragment, Fragment, FragmentMode};

/// The clause text of the range of `binder`, the `index`th (0-based) `for`
/// clause of the property `prop`: the canonical rendering of the range on
/// one line (T56).
pub(crate) fn range_text(p: &Program, prop: DefId, index: usize, binder: &Binder) -> String {
    if let Some(range) = range_ast(p, prop, index, binder) {
        return one_line(&format_fragment(
            Fragment::Expr(range),
            FragmentMode::Canonical,
        ));
    }
    let locals = p
        .defs
        .get(prop.0 as usize)
        .and_then(|d| d.kind.as_property())
        .map_or(&[][..], |pd| pd.locals.as_slice());
    let mut r = Render {
        p,
        locals,
        out: String::new(),
        indent: 0,
    };
    r.gen(&binder.gen);
    one_line(&r.out)
}

/// The clause text of a facet text (toolchain 5.2, T56 [D-246]): every line
/// break, with the indentation after it, replaced by one space
/// (`if n > 0 {\n    1\n} else {\n    2\n}` is `if n > 0 { 1 } else { 2 }`).
/// Canonical text has no comments and escapes every line break inside a
/// text literal, so every line break it has is layout.
fn one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            out.push(' ');
            out.push_str(line.trim_start());
        } else {
            out.push_str(line);
        }
    }
    out
}

/// The range expression of the property's `for` clause in its AST: the
/// clause with the binder's span, else the `index`th clause of the same
/// declaration; `None` when the program has no AST for the property.
fn range_ast<'a>(
    p: &'a Program,
    prop: DefId,
    index: usize,
    binder: &Binder,
) -> Option<&'a ast::Expr> {
    let def = p.defs.get(prop.0 as usize)?;
    let decl = p.file_ast(def.file)?.declarations.get(def.decl_index)?;
    let ast::DeclKind::Property(pd) = &decl.kind else {
        return None;
    };
    if !matches!(&def.name, DeclName::Text(name) if *name == pd.name.value) {
        return None;
    }
    pd.for_clauses
        .iter()
        .find(|fc| fc.span == binder.span)
        .or_else(|| pd.for_clauses.get(index))
        .map(|fc| &fc.range)
}

/// How tightly an expression's outermost form binds (the precedence table
/// of grammar 3, highest first): 7 primaries and postfix forms, 6 prefix
/// forms, 5 `*` `/` `%`, 4 `+` `-` `++`, 3 comparisons, 2 `and`, 1 `or`,
/// 0 `forall`.
fn strength(e: &Expr) -> u8 {
    match &e.kind {
        ExprKind::Binary { op, .. } => binary_strength(*op),
        ExprKind::Unary { .. } | ExprKind::Hole { .. } => 6,
        ExprKind::Forall { .. } | ExprKind::Parallel { .. } => 0,
        ExprKind::Widen { expr, .. } | ExprKind::Refine { expr, .. } => strength(expr),
        ExprKind::Lit(Lit::Int(n)) if n.sign() == num_bigint::Sign::Minus => 6,
        _ => 7,
    }
}

fn binary_strength(op: BinaryOp) -> u8 {
    match op {
        BinaryOp::Or => 1,
        BinaryOp::And => 2,
        BinaryOp::Add | BinaryOp::Sub | BinaryOp::Concat => 4,
        BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => 5,
        _ => 3,
    }
}

/// True when the std stub `def` carries `@property` (`xs.len`, written
/// without parentheses), read from its AST.
fn is_property(p: &Program, def: DefId) -> bool {
    p.defs
        .get(def.0 as usize)
        .and_then(|d| p.file_ast(d.file)?.declarations.get(d.decl_index))
        .is_some_and(|decl| decl.annotations.iter().any(|a| a.name.text == "property"))
}

/// A rendering of HIR in CPL surface syntax, as close to the canonical
/// text as the HIR allows (module docs).
struct Render<'a> {
    p: &'a Program,
    /// The property's local table: the names of the locals a range binds
    /// (`forall` binders, `match` bindings, `let`s in blocks).
    locals: &'a [LocalDef],
    out: String,
    /// The block nesting level (4 spaces each).
    indent: usize,
}

impl Render<'_> {
    fn push(&mut self, s: &str) {
        self.out.push_str(s);
    }

    fn gen(&mut self, g: &GenExpr) {
        match g {
            GenExpr::Int => self.push("gen.int"),
            GenExpr::PositiveInt => self.push("gen.positive_int"),
            GenExpr::Bool => self.push("gen.bool"),
            GenExpr::Text => self.push("gen.text"),
            GenExpr::List(inner) => {
                self.push("gen.list(");
                self.gen(inner);
                self.push(")");
            }
            GenExpr::Option(inner) => {
                self.push("gen.option(");
                self.gen(inner);
                self.push(")");
            }
            GenExpr::Range(lo, hi) => {
                self.push("gen.range(");
                self.expr(lo);
                self.push(", ");
                self.expr(hi);
                self.push(")");
            }
        }
    }

    fn local(&self, id: LocalId) -> String {
        self.locals
            .get(id.0 as usize)
            .map_or_else(|| format!("${}", id.0), |l| l.name.clone())
    }

    fn def_name(&self, def: DefId) -> String {
        bare_name(self.p, def)
    }

    /// `e`, parenthesized when it binds less tightly than `min`.
    fn operand(&mut self, e: &Expr, min: u8) {
        if strength(e) < min {
            self.push("(");
            self.expr(e);
            self.push(")");
        } else {
            self.expr(e);
        }
    }

    fn args(&mut self, args: &[Expr]) {
        for (i, a) in args.iter().enumerate() {
            if i > 0 {
                self.push(", ");
            }
            self.expr(a);
        }
    }

    fn expr(&mut self, e: &Expr) {
        let p = self.p;
        match &e.kind {
            ExprKind::Lit(Lit::Int(n)) => self.push(&n.to_string()),
            ExprKind::Lit(Lit::Text(t)) => self.push(&quote_text(t)),
            ExprKind::Lit(Lit::Bool(b)) => self.push(if *b { "true" } else { "false" }),
            ExprKind::Local(id) => {
                let name = self.local(*id);
                self.push(&name);
            }
            ExprKind::Ctor { def, variant, args } => {
                let name = p
                    .defs
                    .get(def.0 as usize)
                    .and_then(|d| d.kind.as_enum())
                    .and_then(|en| en.variants.get(*variant as usize))
                    .map_or_else(|| format!("<variant {variant}>"), |v| v.name.clone());
                self.push(&name);
                if !args.is_empty() {
                    self.push("(");
                    self.args(args);
                    self.push(")");
                }
            }
            ExprKind::Wrap { def, arg } => {
                let name = self.def_name(*def);
                self.push(&name);
                self.push("(");
                self.expr(arg);
                self.push(")");
            }
            ExprKind::Field { base, def, index } => {
                self.operand(base, 7);
                let field = p
                    .defs
                    .get(def.0 as usize)
                    .and_then(|d| d.kind.as_struct())
                    .and_then(|s| s.fields.get(*index as usize))
                    .map_or_else(|| format!("<field {index}>"), |f| f.name.clone());
                self.push(".");
                self.push(&field);
            }
            ExprKind::Call { callee, args, .. } => {
                match p.defs.get(callee.0 as usize).map(|d| &d.name) {
                    Some(DeclName::Member { name, .. }) if !args.is_empty() => {
                        self.operand(&args[0], 7);
                        self.push(".");
                        self.push(name);
                        if !is_property(p, *callee) {
                            self.push("(");
                            self.args(&args[1..]);
                            self.push(")");
                        }
                    }
                    name => {
                        let name =
                            name.map_or_else(|| format!("{{def {}}}", callee.0), |n| n.display());
                        self.push(&name);
                        self.push("(");
                        self.args(args);
                        self.push(")");
                    }
                }
            }
            ExprKind::CapCall {
                handle,
                capability,
                op,
                args,
            } => {
                self.operand(handle, 7);
                let name = p
                    .defs
                    .get(capability.0 as usize)
                    .and_then(|d| d.kind.as_capability())
                    .and_then(|c| c.ops.get(*op as usize))
                    .map_or_else(|| format!("<op {op}>"), |o| o.name.clone());
                self.push(".");
                self.push(&name);
                self.push("(");
                self.args(args);
                self.push(")");
            }
            ExprKind::Check { refined, arg } => {
                let name = self.def_name(*refined);
                self.push("check<");
                self.push(&name);
                self.push(">(");
                self.expr(arg);
                self.push(")");
            }
            ExprKind::Propagate(inner) => {
                self.operand(inner, 7);
                self.push("?");
            }
            ExprKind::Unary { op, operand } => {
                self.push(match op {
                    UnaryOp::Not => "not ",
                    UnaryOp::Neg => "-",
                });
                let start = self.out.len();
                self.operand(operand, 6);
                // `- -a`, not `--a`.
                if *op == UnaryOp::Neg && self.out[start..].starts_with('-') {
                    self.out.insert(start, ' ');
                }
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let s = binary_strength(*op);
                // Comparisons do not associate (one per operand chain); the
                // others are left-associative.
                self.operand(lhs, if s == 3 { s + 1 } else { s });
                self.push(" ");
                self.push(op.symbol());
                self.push(" ");
                self.operand(rhs, s + 1);
            }
            ExprKind::Forall {
                binder,
                collection,
                predicate,
            }
            | ExprKind::Parallel {
                binder,
                collection,
                predicate,
            } => {
                let name = self.local(*binder);
                self.push(if matches!(e.kind, ExprKind::Parallel { .. }) {
                    "parallel for "
                } else {
                    "forall "
                });
                self.push(&name);
                self.push(" in ");
                self.operand(collection, 1);
                self.push(": ");
                self.expr(predicate);
            }
            ExprKind::Hole { name, .. } => {
                self.push("?");
                self.push(name);
            }
            ExprKind::List(items) => {
                self.push("[");
                self.args(items);
                self.push("]");
            }
            ExprKind::StructLit {
                def,
                spread,
                fields,
                ..
            } => {
                let name = self.def_name(*def);
                self.push(&name);
                if spread.is_none() && fields.is_empty() {
                    self.push(" {}");
                    return;
                }
                self.push(" { ");
                let mut first = true;
                if let Some(base) = spread {
                    self.push("..");
                    self.expr(base);
                    first = false;
                }
                for (index, value) in fields {
                    if !first {
                        self.push(", ");
                    }
                    first = false;
                    let field = p
                        .defs
                        .get(def.0 as usize)
                        .and_then(|d| d.kind.as_struct())
                        .and_then(|s| s.fields.get(*index as usize))
                        .map_or_else(|| format!("<field {index}>"), |f| f.name.clone());
                    self.push(&field);
                    self.push(": ");
                    self.expr(value);
                }
                self.push(" }");
            }
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => {
                self.push("if ");
                self.expr(cond);
                self.push(" ");
                self.block(then_block);
                if let Some(else_block) = else_block {
                    self.push(" else ");
                    match (else_block.stmts.as_slice(), else_block.tail.as_deref()) {
                        (
                            [],
                            Some(
                                tail @ Expr {
                                    kind: ExprKind::If { .. },
                                    ..
                                },
                            ),
                        ) => self.expr(tail),
                        _ => self.block(else_block),
                    }
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                self.push("match ");
                self.expr(scrutinee);
                self.push(" {");
                self.indent += 1;
                for arm in arms {
                    self.newline();
                    self.pattern(&arm.pattern);
                    self.push(" => ");
                    match (arm.body.stmts.as_slice(), arm.body.tail.as_deref()) {
                        ([], Some(tail)) => self.expr(tail),
                        _ => self.block(&arm.body),
                    }
                }
                self.indent -= 1;
                self.newline();
                self.push("}");
            }
            ExprKind::Widen { expr, .. } | ExprKind::Refine { expr, .. } => self.expr(expr),
            ExprKind::Gen(g) => self.gen(g),
            ExprKind::Error => self.push("<error>"),
        }
    }

    fn newline(&mut self) {
        self.out.push('\n');
        for _ in 0..self.indent {
            self.out.push_str("    ");
        }
    }

    /// A block as the formatter lays it out (2.4 rule 8): `{`, one
    /// statement per line one level deeper, `}`.
    fn block(&mut self, b: &Block) {
        if b.stmts.is_empty() && b.tail.is_none() {
            self.push("{}");
            return;
        }
        self.push("{");
        self.indent += 1;
        for s in &b.stmts {
            self.newline();
            self.stmt(s);
        }
        if let Some(tail) = &b.tail {
            self.newline();
            self.expr(tail);
        }
        self.indent -= 1;
        self.newline();
        self.push("}");
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let { local, value } => {
                let name = self.local(*local);
                self.push("let ");
                self.push(&name);
                self.push(" = ");
                self.expr(value);
            }
            Stmt::Discard(e) => {
                self.push("let _ = ");
                self.expr(e);
            }
            Stmt::Assign { local, value } => {
                let name = self.local(*local);
                self.push(&name);
                self.push(" = ");
                self.expr(value);
            }
            Stmt::Return(e) => {
                self.push("return");
                if let Some(e) = e {
                    self.push(" ");
                    self.expr(e);
                }
            }
            Stmt::Assert { text, .. } => {
                self.push("assert ");
                self.push(text);
            }
            Stmt::While { cond, body, .. } => {
                self.push("while ");
                self.expr(cond);
                self.push(" ");
                self.block(body);
            }
            Stmt::Concurrent(tasks) => {
                self.push("concurrent {");
                self.indent += 1;
                for t in tasks {
                    self.newline();
                    self.stmt(&t.stmt);
                }
                self.indent -= 1;
                self.newline();
                self.push("}");
            }
            Stmt::Expr(e) => self.expr(e),
        }
    }

    fn pattern(&mut self, pat: &Pattern) {
        match &pat.kind {
            PatternKind::Wildcard => self.push("_"),
            PatternKind::Int(n) => self.push(&n.to_string()),
            PatternKind::Text(t) => self.push(&quote_text(t)),
            PatternKind::Bool(b) => self.push(if *b { "true" } else { "false" }),
            PatternKind::Bind(id) => {
                let name = self.local(*id);
                self.push(&name);
            }
            PatternKind::Ctor { def, variant, args } => {
                let name = self
                    .p
                    .defs
                    .get(def.0 as usize)
                    .and_then(|d| d.kind.as_enum())
                    .and_then(|en| en.variants.get(*variant as usize))
                    .map_or_else(|| format!("<variant {variant}>"), |v| v.name.clone());
                self.push(&name);
                if !args.is_empty() {
                    self.push("(");
                    for (i, a) in args.iter().enumerate() {
                        if i > 0 {
                            self.push(", ");
                        }
                        self.pattern(a);
                    }
                    self.push(")");
                }
            }
            PatternKind::Unwrap { def, arg } => {
                let name = self.def_name(*def);
                self.push(&name);
                self.push("(");
                self.pattern(arg);
                self.push(")");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Hir, ProgramBuilder};
    use cpl_hir::Ty;
    use cpl_syntax::ast::BinaryOp::*;

    fn render(p: &Program, e: &Expr) -> String {
        let mut r = Render {
            p,
            locals: &[],
            out: String::new(),
            indent: 0,
        };
        r.expr(e);
        r.out
    }

    fn fixture() -> (ProgramBuilder, Hir) {
        let mut b = ProgramBuilder::new();
        let m = b.module("m");
        let h = b.hir(m);
        (b, h)
    }

    #[test]
    fn operands_are_parenthesized_by_the_precedence_table() {
        let (b, h) = fixture();
        let p = b.build();
        let n = |v: i64| h.int(v);
        let t = || h.bool(true);
        let cases = [
            (
                h.binary(Mul, h.binary(Add, n(1), n(2)), n(3)),
                "(1 + 2) * 3",
            ),
            (h.binary(Add, n(1), h.binary(Mul, n(2), n(3))), "1 + 2 * 3"),
            (
                h.binary(Sub, n(1), h.binary(Sub, n(2), n(3))),
                "1 - (2 - 3)",
            ),
            (h.binary(Sub, h.binary(Sub, n(1), n(2)), n(3)), "1 - 2 - 3"),
            (
                h.binary(Eq, h.binary(Lt, n(1), n(2)), t()),
                "(1 < 2) == true",
            ),
            (
                h.binary(Or, h.binary(And, t(), t()), t()),
                "true and true or true",
            ),
            (
                h.binary(And, h.binary(Or, t(), t()), t()),
                "(true or true) and true",
            ),
            (h.unary(UnaryOp::Neg, h.binary(Add, n(1), n(2))), "-(1 + 2)"),
            (h.unary(UnaryOp::Neg, h.unary(UnaryOp::Neg, n(1))), "- -1"),
            (h.unary(UnaryOp::Neg, n(-1)), "- -1"),
            (
                h.unary(UnaryOp::Not, h.binary(And, t(), t())),
                "not (true and true)",
            ),
            (h.binary(Div, n(1), n(-2)), "1 / -2"),
        ];
        for (e, text) in cases {
            assert_eq!(render(&p, &e), text);
        }
    }

    #[test]
    fn members_calls_and_generators_use_their_surface_forms() {
        let (b, h) = fixture();
        let take = b.builtin("list_take");
        let len = b.builtin("list_len");
        let abs = b.builtin("int_abs");
        let trim = b.builtin("text_trim");
        let positive = b.std_def("std.gen.Positive");
        let li = Ty::list(Ty::Int);
        let p = b.build();
        let taken = h.call_generic(
            take,
            vec![Ty::Int],
            vec![h.list(vec![h.int(1), h.int(2)], Ty::Int), h.int(1)],
            li.clone(),
        );
        let e = h.call_generic(len, vec![Ty::Int], vec![taken], Ty::Int);
        assert_eq!(render(&p, &e), "[1, 2].take(1).len");
        let e = h.call(abs, vec![h.unary(UnaryOp::Neg, h.int(1))], Ty::Int);
        assert_eq!(render(&p, &e), "(-1).abs()");
        let e = h.call(trim, vec![h.text(" \"x\"\n")], Ty::Text);
        assert_eq!(render(&p, &e), r#"trim(" \"x\"\n")"#);
        let e = h.check(positive, h.int(5), Ty::Bool);
        assert_eq!(render(&p, &e), "check<Positive>(5)");
        let e = h.some(h.int(1));
        assert_eq!(render(&p, &e), "Some(1)");
        let e = h.none(Ty::Int);
        assert_eq!(render(&p, &e), "None");
        let g = GenExpr::List(Box::new(GenExpr::Option(Box::new(GenExpr::Range(
            Box::new(h.int(0)),
            Box::new(h.binary(Mul, h.int(2), h.int(3))),
        )))));
        assert_eq!(
            render(&p, &h.gen(g, Ty::Int)),
            "gen.list(gen.option(gen.range(0, 2 * 3)))"
        );
    }

    #[test]
    fn a_range_with_blocks_is_one_line_of_clause_text_t56() {
        // toolchain 5.2, T56 [D-246]: the clause text of a range is its
        // facet text with every line break and the indentation after it
        // replaced by one space, on the HIR fallback path as on the AST one.
        let (mut b, h) = fixture();
        let m = b.module("tests.lines");
        let bound = h.if_(
            h.bool(true),
            h.value(h.binary(Div, h.int(1), h.int(0))),
            Some(h.value(h.int(0))),
        );
        let mut pb = crate::testing::PropertyBuilder::new("joined");
        pb.binder(
            "x",
            GenExpr::List(Box::new(GenExpr::Range(
                Box::new(bound),
                Box::new(h.int(5)),
            ))),
            Ty::list(Ty::Int),
        );
        pb.body(h.block(vec![], None));
        let prop = b.add_property(m, pb);
        let p = b.build();
        let binder = &p.def(prop).kind.as_property().unwrap().binders[0];
        assert_eq!(
            range_text(&p, prop, 0, binder),
            "gen.list(gen.range(if true { 1 / 0 } else { 0 }, 5))"
        );
        assert_eq!(one_line("a {\n    b\n        c\n}"), "a { b c }");
        assert_eq!(one_line("x > 0"), "x > 0");
    }

    #[test]
    fn blocks_are_laid_out_one_line_per_statement() {
        let (b, h) = fixture();
        let p = b.build();
        let e = h.if_(
            h.bool(true),
            h.value(h.int(1)),
            Some(h.value(h.if_(h.bool(false), h.value(h.int(2)), Some(h.value(h.int(3)))))),
        );
        assert_eq!(
            render(&p, &e),
            "if true {\n    1\n} else if false {\n    2\n} else {\n    3\n}"
        );
    }
}
