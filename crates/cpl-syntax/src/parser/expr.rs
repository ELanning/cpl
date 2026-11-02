//! Expressions (grammar 3.12) with the precedence table, the postfix rules
//! (name chains, struct literals, speculative generic calls), and the
//! disambiguation rulings of section 5.
//!
//! The binary levels of the table are parsed by one precedence-climbing
//! function rather than one function per level, and the alternatives of
//! `primary` and `postfix` live in functions of their own, so that each
//! nesting level of an expression costs few stack frames (see
//! [`super::MAX_DEPTH`]).

use super::{Mode, PResult, Parser};
use crate::ast::{
    Arg, Args, ArmBody, BinaryExpr, BinaryOp, CallExpr, ElseBranch, Expr, ExprKind, FieldInit,
    ForallExpr, GenericArgs, IfExpr, ListElem, ListLiteral, Literal, LiteralKind, MatchArm,
    MatchExpr, MemberExpr, Name, StructEntries, StructLiteral, Trivia, TypePath, UnaryExpr,
    UnaryOp, UsingArg,
};
use crate::codes;
use crate::lexer::TokenKind as T;
use cpl_diag::Span;

/// Precedence of the binary levels of 3.12, higher binds tighter.
const LEVEL_OR: u8 = 1;
const LEVEL_AND: u8 = 2;
const LEVEL_CMP: u8 = 3;
const LEVEL_ADD: u8 = 4;
const LEVEL_MUL: u8 = 5;

fn binary_op(kind: T) -> Option<(BinaryOp, u8)> {
    Some(match kind {
        T::KwOr => (BinaryOp::Or, LEVEL_OR),
        T::KwAnd => (BinaryOp::And, LEVEL_AND),
        T::EqEq => (BinaryOp::Eq, LEVEL_CMP),
        T::BangEq => (BinaryOp::Ne, LEVEL_CMP),
        T::Lt => (BinaryOp::Lt, LEVEL_CMP),
        T::LtEq => (BinaryOp::Le, LEVEL_CMP),
        T::Gt => (BinaryOp::Gt, LEVEL_CMP),
        T::GtEq => (BinaryOp::Ge, LEVEL_CMP),
        T::Plus => (BinaryOp::Add, LEVEL_ADD),
        T::Minus => (BinaryOp::Sub, LEVEL_ADD),
        T::PlusPlus => (BinaryOp::Concat, LEVEL_ADD),
        T::Star => (BinaryOp::Mul, LEVEL_MUL),
        T::Slash => (BinaryOp::Div, LEVEL_MUL),
        T::Percent => (BinaryOp::Rem, LEVEL_MUL),
        _ => return None,
    })
}

fn binary(op: BinaryOp, op_span: Span, break_before_op: bool, lhs: Expr, rhs: Expr) -> Expr {
    let span = lhs.span.join(rhs.span);
    Expr {
        span,
        kind: ExprKind::Binary(Box::new(BinaryExpr {
            span,
            op,
            op_span,
            break_before_op,
            lhs,
            rhs,
        })),
    }
}

/// The names of a pure name chain (`a.b.C`), innermost first.
fn chain_names(expr: &Expr, out: &mut Vec<Name>) -> bool {
    match &expr.kind {
        ExprKind::Name(n) => {
            out.push(n.clone());
            true
        }
        ExprKind::Member(m) => {
            if !chain_names(&m.target, out) {
                return false;
            }
            out.push(m.member.clone());
            true
        }
        _ => false,
    }
}

fn last_segment_is_upper(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Name(n) => n.is_upper(),
        ExprKind::Member(m) => m.member.is_upper(),
        _ => false,
    }
}

impl<'a> Parser<'a> {
    /// `expr = forall_expr | or_expr`
    #[inline(always)]
    pub(crate) fn parse_expr(&mut self) -> PResult<Expr> {
        self.begin_expression();
        let result = if self.at_word("parallel") && self.nth_kind(1) == T::KwFor {
            self.parse_parallel()
        } else if self.at(T::KwForall) {
            self.parse_forall()
        } else {
            self.parse_binary(LEVEL_OR)
        };
        self.end_expression();
        result
    }

    /// A header expression (D41): struct literals disabled at the top level.
    #[inline(always)]
    pub(crate) fn parse_header_expr(&mut self) -> PResult<Expr> {
        let saved = self.set_struct_literals(false);
        let result = self.parse_expr();
        self.restore_struct_literals(saved);
        result
    }

    /// `NEWLINE? expr NEWLINE? EOF`, the input of [`super::parse_expr`]: one
    /// expression read as an expression statement reads it (the parser's
    /// initial state: newlines significant at the top level, struct
    /// literals allowed), then nothing but line breaks.
    pub(crate) fn parse_expr_input(&mut self) -> PResult<Expr> {
        self.skip_newlines();
        let expr = self.parse_expr()?;
        self.skip_newlines();
        if self.peek_kind() != T::Eof {
            return Err(self.error_expected("end of input after the expression"));
        }
        Ok(expr)
    }

    fn parse_parallel(&mut self) -> PResult<Expr> {
        let kw = self.bump();
        self.expect(T::KwFor, "`for`")?;
        let binder = self.parse_lower_name("a parallel binder")?;
        self.expect(T::KwIn, "`in`")?;
        let collection = self.parse_binary(LEVEL_OR)?;
        self.expect(T::Colon, "`:`")?;
        self.enter()?;
        let predicate = self.parse_expr();
        self.leave();
        let predicate = predicate?;
        let span = kw.span.join(predicate.span);
        Ok(Expr {
            span,
            kind: ExprKind::Parallel(Box::new(ForallExpr {
                span,
                binder,
                collection,
                predicate,
            })),
        })
    }

    /// `forall_expr = "forall" LOWER_IDENT "in" or_expr ":" expr` (D45)
    fn parse_forall(&mut self) -> PResult<Expr> {
        let kw = self.expect(T::KwForall, "`forall`")?;
        let binder = self.parse_lower_name("a quantifier binder")?;
        self.expect(T::KwIn, "`in`")?;
        let collection = self.parse_binary(LEVEL_OR)?;
        self.expect(T::Colon, "`:`")?;
        self.enter()?;
        let predicate = self.parse_expr();
        self.leave();
        let predicate = predicate?;
        let span = kw.span.join(predicate.span);
        Ok(Expr {
            span,
            kind: ExprKind::Forall(Box::new(ForallExpr {
                span,
                binder,
                collection,
                predicate,
            })),
        })
    }

    /// The binary levels `or_expr` .. `mul_expr` of 3.12 by precedence
    /// climbing: every operator at or above `min_level` is applied, left
    /// associatively, and a comparison never chains (`cmp_expr = add_expr
    /// ( cmp_op add_expr )?`).
    fn parse_binary(&mut self, min_level: u8) -> PResult<Expr> {
        let mut lhs = self.parse_unary()?;
        while let Some((op, level)) = binary_op(self.peek_kind()) {
            if level < min_level {
                break;
            }
            let op_tok = self.bump();
            let brk = self.last_token_began_line();
            self.count_op(op_tok.span)?;
            let rhs = self.parse_binary(level + 1)?;
            lhs = binary(op, op_tok.span, brk, lhs, rhs);
            if level == LEVEL_CMP && matches!(binary_op(self.peek_kind()), Some((_, LEVEL_CMP))) {
                return Err(self.error_expected(
                    "`and`, `or`, or the end of the expression (comparisons do not chain: write `a <= b and b <= c`)",
                ));
            }
        }
        Ok(lhs)
    }

    /// `unary_expr = "not" unary_expr | "-" unary_expr | HOLE | postfix_expr`
    fn parse_unary(&mut self) -> PResult<Expr> {
        match self.peek_kind() {
            T::KwNot | T::Minus => {
                let tok = self.bump();
                let op = if tok.kind == T::KwNot {
                    UnaryOp::Not
                } else {
                    UnaryOp::Neg
                };
                self.enter()?;
                let operand = self.parse_unary();
                self.leave();
                let operand = operand?;
                let span = tok.span.join(operand.span);
                Ok(Expr {
                    span,
                    kind: ExprKind::Unary(Box::new(UnaryExpr {
                        span,
                        op,
                        op_span: tok.span,
                        operand,
                    })),
                })
            }
            T::Hole => {
                let tok = self.bump();
                if matches!(self.peek_kind(), T::Dot | T::LParen | T::Question) {
                    return Err(self.error_expected(
                        "an operator or the end of the expression (a hole admits no postfix operator; parenthesize the hole if needed)",
                    ));
                }
                Ok(Expr {
                    span: tok.span,
                    kind: ExprKind::Hole(Name::new(tok.text, tok.span)),
                })
            }
            _ => self.parse_postfix(),
        }
    }

    /// `postfix_expr = primary postfix*` with the four postfix rules of 3.12.
    fn parse_postfix(&mut self) -> PResult<Expr> {
        let mut expr = self.parse_primary()?;
        let mut pure_chain = matches!(expr.kind, ExprKind::Name(_));
        loop {
            let next_span = self.peek_span();
            match self.peek_kind() {
                T::Dot => {
                    self.count_op(next_span)?;
                    expr = self.postfix_member(expr)?;
                }
                T::LParen => {
                    self.count_op(next_span)?;
                    expr = self.postfix_call(expr, None)?;
                    pure_chain = false;
                }
                T::Question => {
                    self.count_op(next_span)?;
                    let q = self.bump();
                    let span = expr.span.join(q.span);
                    expr = Expr {
                        span,
                        kind: ExprKind::Propagate(Box::new(expr)),
                    };
                    pure_chain = false;
                }
                // D42: a generic call when `generic_args "("` follows;
                // otherwise the `<` is left to `cmp_expr`, which counts it as
                // the one binary operator it then is (D73).
                T::Lt if pure_chain && self.at_generic_call() => {
                    self.count_op(next_span)?;
                    let generic_args = self.parse_generic_args()?;
                    expr = self.postfix_call(expr, Some(generic_args))?;
                    pure_chain = false;
                }
                T::LBrace
                    if pure_chain
                        && self.struct_literals_allowed()
                        && last_segment_is_upper(&expr) =>
                {
                    self.count_op(next_span)?;
                    expr = self.postfix_struct(expr)?;
                    pure_chain = false;
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    /// `"." member_name`
    fn postfix_member(&mut self, target: Expr) -> PResult<Expr> {
        self.bump();
        let member = self.parse_member_name()?;
        let span = target.span.join(member.span);
        Ok(Expr {
            span,
            kind: ExprKind::Member(Box::new(MemberExpr {
                span,
                target,
                member,
            })),
        })
    }

    /// `generic_args? "(" args ")"`
    fn postfix_call(&mut self, callee: Expr, generic_args: Option<GenericArgs>) -> PResult<Expr> {
        let args = self.parse_args()?;
        let span = callee.span.join(args.span);
        Ok(Expr {
            span,
            kind: ExprKind::Call(Box::new(CallExpr {
                span,
                callee,
                generic_args,
                args,
            })),
        })
    }

    /// `struct_literal`: the name chain so far becomes the `type_path`.
    fn postfix_struct(&mut self, chain: Expr) -> PResult<Expr> {
        let brace = self.peek_span();
        let path = self.chain_to_type_path(&chain, brace)?;
        let entries = self.parse_struct_entries()?;
        let span = chain.span.join(entries.span);
        Ok(Expr {
            span,
            kind: ExprKind::Struct(Box::new(StructLiteral {
                span,
                path,
                entries,
            })),
        })
    }

    /// Reinterpret a pure name chain as a `type_path` (3.12 rule 2). A chain
    /// with an `UPPER_IDENT` before its last segment names a variant, and
    /// then "the `{` is a syntax error": the diagnostic's span is the `{`
    /// (`brace`), the offending token (D59), so that an error between the
    /// chain and the `{` is the earlier one (D70).
    fn chain_to_type_path(&mut self, expr: &Expr, brace: Span) -> PResult<TypePath> {
        let mut names = Vec::new();
        if !chain_names(expr, &mut names) || names.is_empty() {
            return Err(self.error_expected("a struct literal type"));
        }
        let name = names.pop().expect("non-empty chain");
        if names.iter().any(Name::is_upper) {
            return Err(self.syntax_error(
                "a struct literal names a type; variants have positional payloads",
                brace,
            ));
        }
        Ok(TypePath {
            span: expr.span,
            modules: names,
            name,
        })
    }

    /// `"(" args ")"`: positional arguments then `using` arguments.
    pub(crate) fn parse_args(&mut self) -> PResult<Args> {
        let list = self.bracketed_group(T::LParen, T::RParen, Mode::Insignificant, |p| {
            let mut positional: Vec<Arg> = Vec::new();
            let mut using: Vec<UsingArg> = Vec::new();
            p.parse_commas(T::RParen, |p| {
                let start = p.peek_span().start;
                let mut trivia = Trivia::default();
                p.take_leading(&mut trivia, start, false);
                if p.at(T::KwUsing) {
                    let kw = p.bump();
                    let expr = p.parse_expr()?;
                    let span = kw.span.join(expr.span);
                    let mut arg = UsingArg { span, trivia, expr };
                    p.take_trailing(&mut arg.trivia, span);
                    using.push(arg);
                } else {
                    if !using.is_empty() {
                        let span = p.peek_span();
                        return Err(p.error(
                            codes::POSITIONAL_AFTER_USING,
                            "positional argument after using argument",
                            span,
                        ));
                    }
                    let expr = p.parse_expr()?;
                    let span = expr.span;
                    let mut arg = Arg { span, trivia, expr };
                    p.take_trailing(&mut arg.trivia, span);
                    positional.push(arg);
                }
                Ok(())
            })?;
            Ok((positional, using))
        })?;
        let (positional, using) = list.inner;
        Ok(Args {
            span: list.span,
            positional,
            using,
            group: list.group,
            trailing_comments: list.trailing_comments,
        })
    }

    /// `"{" struct_entries "}"` (D29: the spread is first and unique).
    fn parse_struct_entries(&mut self) -> PResult<StructEntries> {
        let entries = self.bracketed_group(T::LBrace, T::RBrace, Mode::Insignificant, |p| {
            let mut spread: Option<Expr> = None;
            let mut spread_trivia = Trivia::default();
            let mut fields: Vec<FieldInit> = Vec::new();
            if p.at(T::DotDot) {
                let start = p.peek_span().start;
                p.take_leading(&mut spread_trivia, start, false);
                p.bump();
                let expr = p.parse_expr()?;
                let span = Span::new(p.file(), start, expr.span.end);
                p.take_trailing(&mut spread_trivia, span);
                spread = Some(expr);
                if p.eat(T::Comma).is_none() && !p.at(T::RBrace) {
                    return Err(p.error_expected("`,` or `}`"));
                }
            }
            p.parse_commas(T::RBrace, |p| {
                if p.at(T::DotDot) {
                    return Err(p.error_expected(
                        "a field initializer (a spread `..base` must be the first entry of a struct literal)",
                    ));
                }
                let start = p.peek_span().start;
                let mut trivia = Trivia::default();
                p.take_leading(&mut trivia, start, false);
                let name = p.parse_lower_name("a field name")?;
                p.expect(T::Colon, "`:`")?;
                let value = p.parse_expr()?;
                let span = name.span.join(value.span);
                let mut field = FieldInit {
                    span,
                    trivia,
                    name,
                    value,
                };
                p.take_trailing(&mut field.trivia, span);
                fields.push(field);
                Ok(())
            })?;
            Ok((spread, spread_trivia, fields))
        })?;
        let (spread, spread_trivia, fields) = entries.inner;
        Ok(StructEntries {
            span: entries.span,
            spread,
            spread_trivia,
            fields,
            group: entries.group,
            trailing_comments: entries.trailing_comments,
        })
    }

    /// `primary = literal | name | list_literal | paren_expr | if_expr | match_expr`
    /// (struct literals are reached through the postfix loop).
    fn parse_primary(&mut self) -> PResult<Expr> {
        match self.peek_kind() {
            T::Int | T::Text | T::KwTrue | T::KwFalse => self.primary_literal(),
            T::LowerIdent | T::UpperIdent => {
                let tok = self.bump();
                Ok(Expr {
                    span: tok.span,
                    kind: ExprKind::Name(self.name_from(&tok)),
                })
            }
            T::Hole => {
                let tok = self.bump();
                Ok(Expr {
                    span: tok.span,
                    kind: ExprKind::Hole(Name::new(tok.text, tok.span)),
                })
            }
            T::LParen => self.primary_paren(),
            T::LBracket => self.primary_list(),
            T::KwIf => {
                let if_expr = self.parse_if()?;
                Ok(Expr {
                    span: if_expr.span,
                    kind: ExprKind::If(if_expr),
                })
            }
            T::KwMatch => self.parse_match(),
            T::KwForall => Err(self.error_expected(
                "an expression (`forall` is the lowest-precedence form: parenthesize it to use it as an operand)",
            )),
            // `expected expression, found end of line` (grammar 2.3).
            T::Newline => Err(self.error_expected("expression")),
            _ => Err(self.error_expected("an expression")),
        }
    }

    fn primary_literal(&mut self) -> PResult<Expr> {
        let tok = self.bump();
        let kind = match tok.kind {
            T::Int => LiteralKind::Int(tok.text),
            T::Text => LiteralKind::Text(tok.text),
            kind => LiteralKind::Bool(kind == T::KwTrue),
        };
        Ok(Expr {
            span: tok.span,
            kind: ExprKind::Literal(Literal {
                span: tok.span,
                kind,
            }),
        })
    }

    /// `paren_expr = "(" expr ")"`
    fn primary_paren(&mut self) -> PResult<Expr> {
        let (span, inner) = self.bracketed(T::LParen, T::RParen, Mode::Insignificant, |p| {
            p.parse_expr()
        })?;
        Ok(Expr {
            span,
            kind: ExprKind::Paren(Box::new(inner)),
        })
    }

    /// `list_literal = "[" commas(expr) "]"`
    fn primary_list(&mut self) -> PResult<Expr> {
        let list = self.bracketed_group(T::LBracket, T::RBracket, Mode::Insignificant, |p| {
            p.parse_commas(T::RBracket, |p| {
                let start = p.peek_span().start;
                let mut trivia = Trivia::default();
                p.take_leading(&mut trivia, start, false);
                let expr = p.parse_expr()?;
                let mut elem = ListElem {
                    span: expr.span,
                    trivia,
                    expr,
                };
                p.take_trailing(&mut elem.trivia, elem.span);
                Ok(elem)
            })
        })?;
        Ok(Expr {
            span: list.span,
            kind: ExprKind::List(Box::new(ListLiteral {
                span: list.span,
                elements: list.inner,
                group: list.group,
                trailing_comments: list.trailing_comments,
            })),
        })
    }

    /// `if_expr = "if" expr NEWLINE? block ( "else" ( NEWLINE? block | if_expr ) )?`
    ///
    /// Boxed as the AST stores it, so that nested `if`s move a pointer
    /// rather than the node between stack frames.
    #[inline(always)]
    pub(crate) fn parse_if(&mut self) -> PResult<Box<IfExpr>> {
        self.enter()?;
        let result = self.parse_if_inner();
        self.leave();
        result
    }

    fn parse_if_inner(&mut self) -> PResult<Box<IfExpr>> {
        let kw = self.expect(T::KwIf, "`if`")?;
        let condition = self.parse_header_expr()?;
        self.expect_header_brace()?;
        let then_block = self.parse_block()?;
        let mut span = kw.span.join(then_block.span);
        let else_branch = if self.at(T::KwElse) {
            self.bump();
            if self.at(T::KwIf) {
                let nested = self.parse_if()?;
                span = span.join(nested.span);
                Some(ElseBranch::If(nested))
            } else {
                self.expect_header_brace()?;
                let block = self.parse_block()?;
                span = span.join(block.span);
                Some(ElseBranch::Block(block))
            }
        } else {
            None
        };
        Ok(Box::new(IfExpr {
            span,
            condition,
            then_block,
            else_branch,
        }))
    }

    /// `match_expr = "match" expr NEWLINE? "{" lines(match_arm) "}"`
    #[inline(always)]
    fn parse_match(&mut self) -> PResult<Expr> {
        self.enter()?;
        let result = self.parse_match_inner();
        self.leave();
        result
    }

    fn parse_match_inner(&mut self) -> PResult<Expr> {
        let kw = self.expect(T::KwMatch, "`match`")?;
        let scrutinee = self.parse_header_expr()?;
        self.expect_header_brace()?;
        let (body_span, arms, trailing_comments) =
            self.parse_lines("match arm", |p| p.parse_match_arm())?;
        let span = kw.span.join(body_span);
        Ok(Expr {
            span,
            kind: ExprKind::Match(Box::new(MatchExpr {
                span,
                scrutinee,
                arms,
                trailing_comments,
            })),
        })
    }

    /// `match_arm = pattern "=>" ( block | expr )`
    fn parse_match_arm(&mut self) -> PResult<MatchArm> {
        let start = self.peek_span().start;
        let mut trivia = Trivia::default();
        self.take_leading(&mut trivia, start, true);
        let pattern = self.parse_pattern()?;
        self.expect(T::FatArrow, "`=>`")?;
        let body = if self.at(T::LBrace) {
            ArmBody::Block(self.parse_block()?)
        } else {
            ArmBody::Expr(self.parse_expr()?)
        };
        let end = match &body {
            ArmBody::Block(b) => b.span,
            ArmBody::Expr(e) => e.span,
        };
        let span = pattern.span.join(end);
        let mut arm = MatchArm {
            span,
            trivia,
            pattern,
            body,
        };
        self.take_trailing(&mut arm.trivia, span);
        Ok(arm)
    }
}
