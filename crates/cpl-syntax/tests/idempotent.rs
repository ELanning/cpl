//! Formatter idempotency over every corpus of the workspace
//! (`spec/toolchain.md` Section 12, row "formatter idempotency").
//!
//! For every `.cpl`, `.work`, and `.patch` input `x` of every corpus the
//! test asserts exactly the two properties of the table:
//!
//! 1. `fmt(fmt(x)) == fmt(x)`;
//! 2. `strip_layout(parse(fmt(x))) == strip_layout(parse(x))`, where
//!    [`strip_layout`] undoes every change the formatter may legitimately
//!    make and nothing else: it clears spans, `Group::multiline`,
//!    `Trivia::blank_line_before`, `Comment::blank_line_before`,
//!    `BinaryExpr::break_before_op`, and every comment
//!    (`Trivia::leading`/`trailing` and every `trailing_comments` list),
//!    removes the trailing spaces and tabs of every `DocLine::text` (2.4
//!    rule 1: the formatter prints no trailing whitespace, while the lexer
//!    keeps it in a doc line's text; other Unicode white space, U+00A0
//!    included, is text and stays), and puts header clauses and `.work`
//!    items into the canonical orders of T13 (2.4 rule 7: `effects`,
//!    `requires`, `ensures`, `decreases`; `invariant`s then `decreases`;
//!    an operation's `requires` then `ensures`; rule 12 for `.work` items;
//!    a stable sort, so `requires` clauses keep their source order, D66).
//!
//! The corpora are the syntax crate's own (`tests/corpus/**`, the syntax
//! corpus of the golden tests, and the design-document examples of
//! `tests/golden/`), the
//! checker's (`crates/cpl-sema/tests/corpus/**`), the interpreter's
//! (`crates/cpl-eval/tests/corpus/**`), the semantic index's diff corpus
//! (`crates/cpl-index/tests/corpus/**`), the acceptance corpus
//! (`examples/**`), and the standard library's stub files (`std/**`). An
//! input that fails to parse (a negative case written to fail, grammar
//! D59) is skipped and counted; the acceptance corpus, the stubs, the
//! syntax corpus, and the golden inputs must all parse (grammar,
//! introduction and Section 6; `tests/golden.rs` holds an `.ast` golden for
//! each). The test prints how many inputs it covered.

use cpl_diag::{FileId, FileOrigin, SourceMap, Span};
use cpl_syntax::ast::*;
use cpl_syntax::{format_source, parse_module, parse_patch, parse_work, FileKind};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// strip_layout
// ---------------------------------------------------------------------------

/// Every span becomes this one, so that spans never take part in a comparison.
fn no_span() -> Span {
    Span::point(FileId(0), 0)
}

/// Clears exactly the layout fields toolchain 12 lists and applies the
/// reorderings of T13 (see the module documentation). Every struct is
/// destructured without `..`, so a field added to the AST is a compile
/// error here until the walk visits it.
trait StripLayout {
    fn strip(&mut self);
}

impl<T: StripLayout> StripLayout for Vec<T> {
    fn strip(&mut self) {
        for x in self {
            x.strip();
        }
    }
}

impl<T: StripLayout> StripLayout for Option<T> {
    fn strip(&mut self) {
        if let Some(x) = self {
            x.strip();
        }
    }
}

impl<T: StripLayout> StripLayout for Box<T> {
    fn strip(&mut self) {
        (**self).strip();
    }
}

impl StripLayout for Span {
    fn strip(&mut self) {
        *self = no_span();
    }
}

impl StripLayout for Group {
    fn strip(&mut self) {
        let Group { multiline } = self;
        *multiline = false;
    }
}

/// A list of comments (`Trivia::leading`, `Trivia::trailing`, or a
/// `trailing_comments` list): every comment is removed. Clearing a
/// comment's own span and `Comment::blank_line_before` first keeps the walk
/// literal; the removal subsumes both.
fn strip_comments(comments: &mut Vec<Comment>) {
    for c in comments.iter_mut() {
        let Comment {
            span,
            text: _,
            blank_line_before,
        } = c;
        span.strip();
        *blank_line_before = false;
    }
    comments.clear();
}

impl StripLayout for Trivia {
    fn strip(&mut self) {
        let Trivia {
            leading,
            trailing,
            blank_line_before,
        } = self;
        strip_comments(leading);
        strip_comments(trailing);
        *blank_line_before = false;
    }
}

impl StripLayout for Name {
    fn strip(&mut self) {
        let Name { text: _, span } = self;
        span.strip();
    }
}

impl StripLayout for TextLiteral {
    fn strip(&mut self) {
        let TextLiteral { span, value: _ } = self;
        span.strip();
    }
}

impl StripLayout for IntLiteral {
    fn strip(&mut self) {
        let IntLiteral { span, digits: _ } = self;
        span.strip();
    }
}

/// A doc line's trailing spaces and tabs are layout (toolchain 12, 2.4
/// rule 1); any other trailing white space is text.
impl StripLayout for DocLine {
    fn strip(&mut self) {
        let DocLine { span, text } = self;
        span.strip();
        let kept = text.trim_end_matches([' ', '\t']).len();
        text.truncate(kept);
    }
}

impl StripLayout for Docs {
    fn strip(&mut self) {
        let Docs { span, lines } = self;
        span.strip();
        lines.strip();
    }
}

// ---- files ----------------------------------------------------------------

impl StripLayout for Module {
    fn strip(&mut self) {
        let Module {
            span,
            trivia,
            docs,
            header,
            uses,
            declarations,
            trailing_comments,
        } = self;
        span.strip();
        trivia.strip();
        docs.strip();
        header.strip();
        uses.strip();
        declarations.strip();
        strip_comments(trailing_comments);
    }
}

impl StripLayout for ModuleHeader {
    fn strip(&mut self) {
        let ModuleHeader { span, path } = self;
        span.strip();
        path.strip();
    }
}

impl StripLayout for ModulePath {
    fn strip(&mut self) {
        let ModulePath { span, segments } = self;
        span.strip();
        segments.strip();
    }
}

impl StripLayout for UseDecl {
    fn strip(&mut self) {
        let UseDecl {
            span,
            trivia,
            path,
            braced: _,
            items,
            group,
            trailing_comments,
        } = self;
        span.strip();
        trivia.strip();
        path.strip();
        items.strip();
        group.strip();
        strip_comments(trailing_comments);
    }
}

impl StripLayout for UsePath {
    fn strip(&mut self) {
        let UsePath { span, segments } = self;
        span.strip();
        segments.strip();
    }
}

impl StripLayout for UseItem {
    fn strip(&mut self) {
        let UseItem { span, trivia, name } = self;
        span.strip();
        trivia.strip();
        name.strip();
    }
}

// ---- declarations -----------------------------------------------------------

impl StripLayout for Declaration {
    fn strip(&mut self) {
        let Declaration {
            span,
            trivia,
            docs,
            annotations,
            kind,
        } = self;
        span.strip();
        trivia.strip();
        docs.strip();
        annotations.strip();
        kind.strip();
    }
}

impl StripLayout for DeclKind {
    fn strip(&mut self) {
        match self {
            DeclKind::Type(d) => d.strip(),
            DeclKind::Struct(d) => d.strip(),
            DeclKind::Enum(d) => d.strip(),
            DeclKind::Capability(d) => d.strip(),
            DeclKind::Fn(d) => d.strip(),
            DeclKind::Test(d) => d.strip(),
            DeclKind::Property(d) => d.strip(),
        }
    }
}

impl StripLayout for Annotation {
    fn strip(&mut self) {
        let Annotation {
            span,
            trivia,
            name,
            args,
            group,
            parens: _,
        } = self;
        span.strip();
        trivia.strip();
        name.strip();
        args.strip();
        group.strip();
    }
}

impl StripLayout for TypeDecl {
    fn strip(&mut self) {
        let TypeDecl {
            span,
            is_pub: _,
            name,
            kind,
        } = self;
        span.strip();
        name.strip();
        match kind {
            TypeDeclKind::Wrapper(WrapperDecl { span, payload }) => {
                span.strip();
                payload.strip();
            }
            TypeDeclKind::Refinement(RefinementDecl {
                span,
                base,
                predicate,
            }) => {
                span.strip();
                base.strip();
                predicate.strip();
            }
            TypeDeclKind::Opaque(OpaqueDecl {
                span,
                generic_params,
            }) => {
                span.strip();
                generic_params.strip();
            }
        }
    }
}

impl StripLayout for StructDecl {
    fn strip(&mut self) {
        let StructDecl {
            span,
            is_pub: _,
            is_program: _,
            name,
            generic_params,
            fields,
            invariant,
            trailing_comments,
        } = self;
        span.strip();
        name.strip();
        generic_params.strip();
        fields.strip();
        invariant.strip();
        strip_comments(trailing_comments);
    }
}

impl StripLayout for InvariantClause {
    fn strip(&mut self) {
        self.span.strip();
        self.trivia.strip();
        self.expr.strip();
    }
}

impl StripLayout for FieldDecl {
    fn strip(&mut self) {
        let FieldDecl {
            span,
            trivia,
            docs,
            name,
            ty,
        } = self;
        span.strip();
        trivia.strip();
        docs.strip();
        name.strip();
        ty.strip();
    }
}

impl StripLayout for EnumDecl {
    fn strip(&mut self) {
        let EnumDecl {
            span,
            is_pub: _,
            name,
            generic_params,
            variants,
            trailing_comments,
        } = self;
        span.strip();
        name.strip();
        generic_params.strip();
        variants.strip();
        strip_comments(trailing_comments);
    }
}

impl StripLayout for VariantDecl {
    fn strip(&mut self) {
        let VariantDecl {
            span,
            trivia,
            docs,
            name,
            payload,
            group,
        } = self;
        span.strip();
        trivia.strip();
        docs.strip();
        name.strip();
        payload.strip();
        group.strip();
    }
}

impl StripLayout for CapabilityDecl {
    fn strip(&mut self) {
        let CapabilityDecl {
            span,
            is_pub: _,
            is_interface: _,
            name,
            operations,
            trailing_comments,
        } = self;
        span.strip();
        name.strip();
        operations.strip();
        strip_comments(trailing_comments);
    }
}

impl StripLayout for OpDecl {
    fn strip(&mut self) {
        let OpDecl {
            span,
            trivia,
            docs,
            name,
            params,
            ret,
            clauses,
        } = self;
        span.strip();
        trivia.strip();
        docs.strip();
        name.strip();
        params.strip();
        ret.strip();
        clauses.strip();
        // T13 / rule 7: an operation's `requires` clauses, then its `ensures`.
        clauses.sort_by_key(|c| c.kind == ContractKind::Ensures);
    }
}

impl StripLayout for FnDecl {
    fn strip(&mut self) {
        let FnDecl {
            span,
            is_pub: _,
            mode: _,
            name,
            generic_params,
            params,
            ret,
            clauses,
            body,
        } = self;
        span.strip();
        name.strip();
        generic_params.strip();
        params.strip();
        ret.strip();
        clauses.strip();
        // T13 / rule 7 (see the module documentation).
        clauses.sort_by_key(|c| match c {
            FnClause::Effects(_) => 0,
            FnClause::Contract(ContractClause {
                kind: ContractKind::Requires,
                ..
            }) => 1,
            FnClause::Contract(ContractClause {
                kind: ContractKind::Ensures,
                ..
            }) => 2,
            FnClause::Decreases(_) => 3,
        });
        body.strip();
    }
}

impl StripLayout for Params {
    fn strip(&mut self) {
        let Params {
            span,
            positional,
            using,
            group,
            trailing_comments,
        } = self;
        span.strip();
        positional.strip();
        using.strip();
        group.strip();
        strip_comments(trailing_comments);
    }
}

impl StripLayout for Param {
    fn strip(&mut self) {
        let Param {
            span,
            trivia,
            name,
            ty,
        } = self;
        span.strip();
        trivia.strip();
        name.strip();
        ty.strip();
    }
}

impl StripLayout for UsingParam {
    fn strip(&mut self) {
        let UsingParam {
            span,
            trivia,
            name,
            ty,
        } = self;
        span.strip();
        trivia.strip();
        name.strip();
        ty.strip();
    }
}

impl StripLayout for FnClause {
    fn strip(&mut self) {
        match self {
            FnClause::Effects(c) => c.strip(),
            FnClause::Contract(c) => c.strip(),
            FnClause::Decreases(c) => c.strip(),
        }
    }
}

impl StripLayout for EffectsClause {
    fn strip(&mut self) {
        let EffectsClause {
            span,
            trivia,
            effects,
            group,
            trailing_comments,
        } = self;
        span.strip();
        trivia.strip();
        effects.strip();
        group.strip();
        strip_comments(trailing_comments);
    }
}

impl StripLayout for EffectRef {
    fn strip(&mut self) {
        let EffectRef {
            span,
            trivia,
            param,
            operation,
        } = self;
        span.strip();
        trivia.strip();
        param.strip();
        operation.strip();
    }
}

impl StripLayout for ContractClause {
    fn strip(&mut self) {
        let ContractClause {
            span,
            trivia,
            kind: _,
            expr,
        } = self;
        span.strip();
        trivia.strip();
        expr.strip();
    }
}

impl StripLayout for DecreasesClause {
    fn strip(&mut self) {
        let DecreasesClause { span, trivia, expr } = self;
        span.strip();
        trivia.strip();
        expr.strip();
    }
}

impl StripLayout for TestDecl {
    fn strip(&mut self) {
        let TestDecl { span, name, body } = self;
        span.strip();
        name.strip();
        body.strip();
    }
}

impl StripLayout for PropertyDecl {
    fn strip(&mut self) {
        let PropertyDecl {
            span,
            name,
            for_clauses,
            body,
        } = self;
        span.strip();
        name.strip();
        for_clauses.strip();
        body.strip();
    }
}

impl StripLayout for ForClause {
    fn strip(&mut self) {
        let ForClause {
            span,
            trivia,
            binder,
            range,
        } = self;
        span.strip();
        trivia.strip();
        binder.strip();
        range.strip();
    }
}

// ---- types -----------------------------------------------------------------

impl StripLayout for Type {
    fn strip(&mut self) {
        let Type {
            span,
            path,
            generic_args,
        } = self;
        span.strip();
        path.strip();
        generic_args.strip();
    }
}

impl StripLayout for TypePath {
    fn strip(&mut self) {
        let TypePath {
            span,
            modules,
            name,
        } = self;
        span.strip();
        modules.strip();
        name.strip();
    }
}

impl StripLayout for GenericArgs {
    fn strip(&mut self) {
        let GenericArgs { span, args } = self;
        span.strip();
        args.strip();
    }
}

impl StripLayout for GenericParams {
    fn strip(&mut self) {
        let GenericParams { span, params } = self;
        span.strip();
        params.strip();
    }
}

// ---- statements ------------------------------------------------------------

impl StripLayout for Block {
    fn strip(&mut self) {
        let Block {
            span,
            statements,
            tail,
            trailing_comments,
        } = self;
        span.strip();
        statements.strip();
        tail.strip();
        strip_comments(trailing_comments);
    }
}

impl StripLayout for Statement {
    fn strip(&mut self) {
        match self {
            Statement::Let(LetStmt {
                span,
                trivia,
                binding,
                ty,
                value,
            }) => {
                span.strip();
                trivia.strip();
                match binding {
                    LetBinding::Name(n) => n.strip(),
                    LetBinding::Wildcard(s) => s.strip(),
                }
                ty.strip();
                value.strip();
            }
            Statement::Var(VarStmt {
                span,
                trivia,
                name,
                ty,
                value,
            }) => {
                span.strip();
                trivia.strip();
                name.strip();
                ty.strip();
                value.strip();
            }
            Statement::Assign(AssignStmt {
                span,
                trivia,
                target,
                value,
            }) => {
                span.strip();
                trivia.strip();
                target.strip();
                value.strip();
            }
            Statement::Return(ReturnStmt {
                span,
                trivia,
                value,
            }) => {
                span.strip();
                trivia.strip();
                value.strip();
            }
            Statement::Assert(AssertStmt {
                span,
                trivia,
                condition,
            }) => {
                span.strip();
                trivia.strip();
                condition.strip();
            }
            Statement::While(WhileStmt {
                span,
                trivia,
                condition,
                clauses,
                body,
            }) => {
                span.strip();
                trivia.strip();
                condition.strip();
                for c in clauses.iter_mut() {
                    match c {
                        WhileClause::Invariant(InvariantClause { span, trivia, expr }) => {
                            span.strip();
                            trivia.strip();
                            expr.strip();
                        }
                        WhileClause::Decreases(d) => d.strip(),
                    }
                }
                // T13 / rule 7: `invariant`s, then `decreases`.
                clauses.sort_by_key(|c| matches!(c, WhileClause::Decreases(_)));
                body.strip();
            }
            Statement::Expr(e) => e.strip(),
        }
    }
}

impl StripLayout for ExprStmt {
    fn strip(&mut self) {
        let ExprStmt { span, trivia, expr } = self;
        span.strip();
        trivia.strip();
        expr.strip();
    }
}

// ---- expressions -------------------------------------------------------------

impl StripLayout for Literal {
    fn strip(&mut self) {
        let Literal { span, kind: _ } = self;
        span.strip();
    }
}

impl StripLayout for Expr {
    fn strip(&mut self) {
        let Expr { span, kind } = self;
        span.strip();
        match kind {
            ExprKind::Literal(l) => l.strip(),
            ExprKind::Name(n) | ExprKind::Hole(n) => n.strip(),
            ExprKind::Forall(f) | ExprKind::Parallel(f) => {
                let ForallExpr {
                    span,
                    binder,
                    collection,
                    predicate,
                } = &mut **f;
                span.strip();
                binder.strip();
                collection.strip();
                predicate.strip();
            }
            ExprKind::Binary(b) => {
                let BinaryExpr {
                    span,
                    op: _,
                    op_span,
                    break_before_op,
                    lhs,
                    rhs,
                } = &mut **b;
                span.strip();
                op_span.strip();
                *break_before_op = false;
                lhs.strip();
                rhs.strip();
            }
            ExprKind::Unary(u) => {
                let UnaryExpr {
                    span,
                    op: _,
                    op_span,
                    operand,
                } = &mut **u;
                span.strip();
                op_span.strip();
                operand.strip();
            }
            ExprKind::Member(m) => {
                let MemberExpr {
                    span,
                    target,
                    member,
                } = &mut **m;
                span.strip();
                target.strip();
                member.strip();
            }
            ExprKind::Call(c) => {
                let CallExpr {
                    span,
                    callee,
                    generic_args,
                    args,
                } = &mut **c;
                span.strip();
                callee.strip();
                generic_args.strip();
                args.strip();
            }
            ExprKind::Propagate(inner) | ExprKind::Paren(inner) => inner.strip(),
            ExprKind::List(list) => {
                let ListLiteral {
                    span,
                    elements,
                    group,
                    trailing_comments,
                } = &mut **list;
                span.strip();
                for ListElem { span, trivia, expr } in elements {
                    span.strip();
                    trivia.strip();
                    expr.strip();
                }
                group.strip();
                strip_comments(trailing_comments);
            }
            ExprKind::Struct(s) => {
                let StructLiteral {
                    span,
                    path,
                    entries,
                } = &mut **s;
                span.strip();
                path.strip();
                let StructEntries {
                    span,
                    spread,
                    spread_trivia,
                    fields,
                    group,
                    trailing_comments,
                } = entries;
                span.strip();
                spread.strip();
                spread_trivia.strip();
                for FieldInit {
                    span,
                    trivia,
                    name,
                    value,
                } in fields
                {
                    span.strip();
                    trivia.strip();
                    name.strip();
                    value.strip();
                }
                group.strip();
                strip_comments(trailing_comments);
            }
            ExprKind::If(i) => i.strip(),
            ExprKind::Match(m) => {
                let MatchExpr {
                    span,
                    scrutinee,
                    arms,
                    trailing_comments,
                } = &mut **m;
                span.strip();
                scrutinee.strip();
                for MatchArm {
                    span,
                    trivia,
                    pattern,
                    body,
                } in arms
                {
                    span.strip();
                    trivia.strip();
                    pattern.strip();
                    match body {
                        ArmBody::Block(b) => b.strip(),
                        ArmBody::Expr(e) => e.strip(),
                    }
                }
                strip_comments(trailing_comments);
            }
        }
    }
}

impl StripLayout for Args {
    fn strip(&mut self) {
        let Args {
            span,
            positional,
            using,
            group,
            trailing_comments,
        } = self;
        span.strip();
        for Arg { span, trivia, expr } in positional {
            span.strip();
            trivia.strip();
            expr.strip();
        }
        for UsingArg { span, trivia, expr } in using {
            span.strip();
            trivia.strip();
            expr.strip();
        }
        group.strip();
        strip_comments(trailing_comments);
    }
}

impl StripLayout for IfExpr {
    fn strip(&mut self) {
        let IfExpr {
            span,
            condition,
            then_block,
            else_branch,
        } = self;
        span.strip();
        condition.strip();
        then_block.strip();
        match else_branch {
            Some(ElseBranch::Block(b)) => b.strip(),
            Some(ElseBranch::If(nested)) => nested.strip(),
            None => {}
        }
    }
}

// ---- patterns ----------------------------------------------------------------

impl StripLayout for Pattern {
    fn strip(&mut self) {
        let Pattern { span, kind } = self;
        span.strip();
        match kind {
            PatternKind::Wildcard
            | PatternKind::Int { .. }
            | PatternKind::Text(_)
            | PatternKind::Bool(_) => {}
            PatternKind::Binding(n) => n.strip(),
            PatternKind::Ctor(CtorPattern { path, args, group }) => {
                let CtorPath {
                    span,
                    qualifier,
                    name,
                } = path;
                span.strip();
                qualifier.strip();
                name.strip();
                args.strip();
                group.strip();
            }
        }
    }
}

// ---- .work ---------------------------------------------------------------------

impl StripLayout for WorkFile {
    fn strip(&mut self) {
        let WorkFile {
            span,
            uses,
            changes,
            trailing_comments,
        } = self;
        span.strip();
        uses.strip();
        changes.strip();
        strip_comments(trailing_comments);
    }
}

impl StripLayout for ChangeDecl {
    fn strip(&mut self) {
        let ChangeDecl {
            span,
            trivia,
            docs,
            name,
            items,
            trailing_comments,
        } = self;
        span.strip();
        trivia.strip();
        docs.strip();
        name.strip();
        items.strip();
        // T13 / rule 12: `base`, `target`, `workers`, tasks, `integrate`.
        items.sort_by_key(|item| match item.kind {
            ChangeItemKind::Base(_) => 0,
            ChangeItemKind::Target(_) => 1,
            ChangeItemKind::Workers(_) => 2,
            ChangeItemKind::Task(_) => 3,
            ChangeItemKind::Integrate(_) => 4,
        });
        strip_comments(trailing_comments);
    }
}

impl StripLayout for ChangeItem {
    fn strip(&mut self) {
        let ChangeItem { span, trivia, kind } = self;
        span.strip();
        trivia.strip();
        match kind {
            ChangeItemKind::Base(g) | ChangeItemKind::Target(g) => {
                let GitRef { span, reference } = g;
                span.strip();
                reference.strip();
            }
            ChangeItemKind::Workers(w) => w.strip(),
            ChangeItemKind::Task(t) => t.strip(),
            ChangeItemKind::Integrate(i) => {
                let IntegrateDecl {
                    span,
                    requirements,
                    trailing_comments,
                } = i;
                span.strip();
                for RequireItem {
                    span,
                    trivia,
                    requirement,
                } in requirements
                {
                    span.strip();
                    trivia.strip();
                    let Requirement { span, kind } = requirement;
                    span.strip();
                    match kind {
                        RequirementKind::Typecheck
                        | RequirementKind::NoHoles
                        | RequirementKind::NoUnresolvedAssumptions => {}
                        RequirementKind::Prove(s) | RequirementKind::Test(s) => s.strip(),
                        RequirementKind::Review(n) => n.strip(),
                    }
                }
                strip_comments(trailing_comments);
            }
        }
    }
}

impl StripLayout for TaskDecl {
    fn strip(&mut self) {
        let TaskDecl {
            span,
            docs,
            name,
            after,
            items,
            trailing_comments,
        } = self;
        span.strip();
        docs.strip();
        name.strip();
        if let Some(AfterClause { span, tasks }) = after {
            span.strip();
            tasks.strip();
        }
        for TaskItem { span, trivia, kind } in items.iter_mut() {
            span.strip();
            trivia.strip();
            match kind {
                TaskItemKind::Write(s)
                | TaskItemKind::Add(s)
                | TaskItemKind::Depend(s)
                | TaskItemKind::Preserve(s) => s.strip(),
                TaskItemKind::Goal(g) => g.strip(),
            }
        }
        // T13 / rule 12: `write`, `add`, `depend`, `preserve`, `goal`.
        items.sort_by_key(|item| match item.kind {
            TaskItemKind::Write(_) => 0,
            TaskItemKind::Add(_) => 1,
            TaskItemKind::Depend(_) => 2,
            TaskItemKind::Preserve(_) => 3,
            TaskItemKind::Goal(_) => 4,
        });
        strip_comments(trailing_comments);
    }
}

impl StripLayout for SelectorSet {
    fn strip(&mut self) {
        let SelectorSet {
            span,
            selectors,
            group,
            trailing_comments,
        } = self;
        span.strip();
        selectors.strip();
        group.strip();
        strip_comments(trailing_comments);
    }
}

impl StripLayout for Selector {
    fn strip(&mut self) {
        let Selector {
            span,
            trivia,
            path,
            kind,
        } = self;
        span.strip();
        trivia.strip();
        let SelectorPath { span, segments } = path;
        span.strip();
        segments.strip();
        match kind {
            SelectorKind::Namespace | SelectorKind::Declaration => {}
            SelectorKind::Facet(Facet { span, kind: _ }) => span.strip(),
        }
    }
}

// ---- .patch ----------------------------------------------------------------------

impl StripLayout for PatchFile {
    fn strip(&mut self) {
        let PatchFile {
            span,
            forms,
            trailing_comments,
        } = self;
        span.strip();
        forms.strip();
        strip_comments(trailing_comments);
    }
}

impl StripLayout for PatchForm {
    fn strip(&mut self) {
        let PatchForm {
            span,
            trivia,
            docs,
            target,
            expect,
            body,
        } = self;
        span.strip();
        trivia.strip();
        docs.strip();
        target.strip();
        let ExpectClause { span, trivia, base } = expect;
        span.strip();
        trivia.strip();
        base.strip();
        body.strip();
    }
}

fn strip_layout<T: StripLayout + Clone>(tree: &T) -> T {
    let mut copy = tree.clone();
    copy.strip();
    copy
}

// ---------------------------------------------------------------------------
// Corpora
// ---------------------------------------------------------------------------

fn workspace_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// One corpus root, relative to the workspace, and whether every input in
/// it must parse.
struct Corpus {
    root: &'static str,
    must_parse: bool,
}

const CORPORA: &[Corpus] = &[
    Corpus {
        root: "crates/cpl-syntax/tests/corpus",
        must_parse: true,
    },
    Corpus {
        root: "crates/cpl-syntax/tests/golden",
        must_parse: true,
    },
    Corpus {
        root: "crates/cpl-sema/tests/corpus",
        must_parse: false,
    },
    Corpus {
        root: "crates/cpl-eval/tests/corpus",
        must_parse: false,
    },
    Corpus {
        root: "crates/cpl-index/tests/corpus",
        must_parse: false,
    },
    Corpus {
        root: "examples",
        must_parse: true,
    },
    Corpus {
        root: "std",
        must_parse: true,
    },
];

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = entries.map(|e| e.expect("entry").path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// The outcome of the two assertions for one input, or why it was skipped.
enum Outcome {
    Covered,
    Unparsable(String),
    Failed(String),
}

fn render(label: &str, src: &str, diagnostics: &[cpl_diag::Diagnostic]) -> String {
    let mut map = SourceMap::new();
    map.add(label.to_string(), FileOrigin::Virtual, src.to_string());
    cpl_diag::render_all(diagnostics, &map)
}

fn fmt(label: &str, src: &str, kind: FileKind) -> Result<String, String> {
    format_source(src, kind, FileId(0)).map_err(|d| render(label, src, &d))
}

/// `strip_layout(parse(text))` rendered for comparison, or the parse errors.
fn stripped(label: &str, text: &str, kind: FileKind) -> Result<String, String> {
    fn finish<T: StripLayout + Clone + std::fmt::Debug>(
        label: &str,
        text: &str,
        r: cpl_syntax::ParseResult<T>,
    ) -> Result<String, String> {
        match r.value {
            Some(tree) if r.diagnostics.is_empty() => Ok(format!("{:#?}", strip_layout(&tree))),
            _ => Err(render(label, text, &r.diagnostics)),
        }
    }
    match kind {
        FileKind::Cpl => finish(label, text, parse_module(text, FileId(0))),
        FileKind::Work => finish(label, text, parse_work(text, FileId(0))),
        FileKind::Patch => finish(label, text, parse_patch(text, FileId(0))),
    }
}

fn check(label: &str, x: &str, kind: FileKind) -> Outcome {
    let before = match stripped(label, x, kind) {
        Ok(tree) => tree,
        Err(e) => return Outcome::Unparsable(e),
    };
    let once = match fmt(label, x, kind) {
        Ok(t) => t,
        Err(e) => return Outcome::Failed(format!("{label}: parses but does not format:\n{e}")),
    };
    let twice = match fmt(label, &once, kind) {
        Ok(t) => t,
        Err(e) => {
            return Outcome::Failed(format!(
                "{label}: the formatted text does not parse:\n{once}\n{e}"
            ))
        }
    };
    // Assertion 1: fmt(fmt(x)) == fmt(x).
    if twice != once {
        return Outcome::Failed(format!(
            "{label}: fmt(fmt(x)) != fmt(x)\n--- fmt(x)\n{once}\n--- fmt(fmt(x))\n{twice}"
        ));
    }
    // Assertion 2: strip_layout(parse(fmt(x))) == strip_layout(parse(x)).
    let after = match stripped(label, &once, kind) {
        Ok(tree) => tree,
        Err(e) => return Outcome::Failed(format!("{label}: fmt(x) does not parse:\n{e}")),
    };
    if after != before {
        let first_difference = before
            .lines()
            .zip(after.lines())
            .position(|(a, b)| a != b)
            .unwrap_or(0);
        let context = |text: &str| {
            text.lines()
                .skip(first_difference.saturating_sub(8))
                .take(16)
                .collect::<Vec<_>>()
                .join("\n")
        };
        return Outcome::Failed(format!(
            "{label}: strip_layout(parse(fmt(x))) != strip_layout(parse(x))\n--- fmt(x)\n{once}\n--- parse(x) near the first difference\n{}\n--- parse(fmt(x)) near the first difference\n{}",
            context(&before),
            context(&after)
        ));
    }
    Outcome::Covered
}

#[test]
fn formatting_is_idempotent_over_every_corpus_in_the_workspace() {
    let workspace = workspace_dir();
    let mut covered = 0usize;
    let mut skipped: Vec<String> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    for corpus in CORPORA {
        let mut files = Vec::new();
        walk(&workspace.join(corpus.root), &mut files);
        let mut in_corpus = 0usize;
        for path in files {
            let Some(kind) = FileKind::from_path(&path) else {
                continue;
            };
            let label = path
                .strip_prefix(&workspace)
                .unwrap_or(&path)
                .display()
                .to_string();
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("cannot read {label}: {e}"));
            match check(&label, &text, kind) {
                Outcome::Covered => {
                    covered += 1;
                    in_corpus += 1;
                }
                Outcome::Unparsable(e) if !corpus.must_parse => {
                    skipped.push(format!("{label}: {}", e.lines().next().unwrap_or("")));
                }
                Outcome::Unparsable(e) => {
                    failures.push(format!("{label} must parse (grammar Section 6):\n{e}"))
                }
                Outcome::Failed(e) => failures.push(e),
            }
        }
        println!("{}: {in_corpus} inputs covered", corpus.root);
    }
    println!(
        "formatter idempotency: {covered} inputs covered, {} skipped because they do not parse",
        skipped.len()
    );
    for s in &skipped {
        println!("  skipped {s}");
    }
    assert!(
        failures.is_empty(),
        "{} of {} inputs failed:\n\n{}",
        failures.len(),
        covered + failures.len(),
        failures.join("\n\n")
    );
    // The acceptance corpus alone has seven files and the stubs five; the
    // checker's corpus has well over a hundred.
    assert!(
        covered >= 150,
        "expected every corpus of the workspace, covered only {covered} inputs"
    );
}

/// Toolchain 12 (the `strip_layout` of the idempotency row) and 2.4 rule 1:
/// the formatter drops the trailing spaces and tabs of a doc line, which
/// the lexer keeps in `DocLine::text`, so `strip_layout` trims them and
/// both assertions hold for such an input; other trailing white space
/// (U+00A0) is text, which the formatter and `strip_layout` both keep.
#[test]
fn doc_lines_lose_their_trailing_spaces_and_tabs_and_nothing_else() {
    let x = "module t.docs\n\n/// Trailing spaces.   \n///\t\n/// A tab and a space.\t \n/// A no-break space.\u{a0}\nfn f() -> Int {\n    1\n}\n";
    let once = fmt("docs.cpl", x, FileKind::Cpl).expect("the input formats");
    assert!(
        once.contains("/// Trailing spaces.\n///\n/// A tab and a space.\n"),
        "{once}"
    );
    assert!(once.contains("/// A no-break space.\u{a0}\n"), "{once}");
    assert!(
        !once.lines().any(|l| l.ends_with([' ', '\t'])),
        "trailing whitespace in {once:?}"
    );
    match check("docs.cpl", x, FileKind::Cpl) {
        Outcome::Covered => {}
        Outcome::Unparsable(e) | Outcome::Failed(e) => panic!("{e}"),
    }
    // `strip_layout` itself: spaces and tabs only.
    let doc = |line: &str| {
        let src = format!("module t.d\n\n/// {line}\nfn f() -> Int {{\n    1\n}}\n");
        stripped("d.cpl", &src, FileKind::Cpl).expect("parses")
    };
    assert_eq!(doc("x \t \t"), doc("x"));
    assert_ne!(doc("x\u{a0}"), doc("x"));
    assert_ne!(doc(" x"), doc("x"), "leading white space is text");
}
