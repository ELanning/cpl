//! Blocks and statements (grammar 3.11).

use super::{Continuation, PResult, Parser};
use crate::ast::{
    AssertStmt, AssignStmt, Block, Comment, DecreasesClause, ExprStmt, InvariantClause, LetBinding,
    LetStmt, ReturnStmt, Statement, Trivia, VarStmt, WhileClause, WhileStmt,
};
use crate::lexer::{Token, TokenKind as T};
use cpl_diag::Span;

fn is_while_continuation(tok: &Token) -> bool {
    matches!(tok.kind, T::LBrace | T::KwInvariant | T::KwDecreases)
}

/// A block from its parsed lines: a final expression statement becomes its
/// tail expression (grammar 3.11). Out of line, so that its temporaries are
/// not part of the frames that nested blocks stack up (see
/// [`super::MAX_DEPTH`]).
#[inline(never)]
fn block_from(
    span: Span,
    mut statements: Vec<Statement>,
    trailing_comments: Vec<Comment>,
) -> Block {
    let tail = match statements.last() {
        Some(Statement::Expr(_)) => match statements.pop() {
            Some(Statement::Expr(e)) => Some(e),
            _ => None,
        },
        _ => None,
    };
    Block {
        span,
        statements,
        tail,
        trailing_comments,
    }
}

impl<'a> Parser<'a> {
    /// `block = "{" lines(statement) "}"`; a final expression statement
    /// becomes the block's tail expression.
    pub(crate) fn parse_block(&mut self) -> PResult<Block> {
        let (span, statements, trailing_comments) =
            self.parse_lines("statement", |p| p.parse_statement())?;
        Ok(block_from(span, statements, trailing_comments))
    }

    /// `statement = let_stmt | var_stmt | assign_stmt | return_stmt | assert_stmt | while_stmt | expr_stmt`
    pub(crate) fn parse_statement(&mut self) -> PResult<Statement> {
        let start = self.peek_span().start;
        let mut trivia = Trivia::default();
        self.take_leading(&mut trivia, start, true);
        let mut stmt = self.parse_statement_kind()?;
        *stmt.trivia_mut() = trivia;
        let span = stmt.span();
        self.take_trailing(stmt.trivia_mut(), span);
        Ok(stmt)
    }

    /// The statement after its leading trivia. Every arm returns its
    /// production's result as it is, so that no arm adds a statement-sized
    /// temporary to this frame, which nested statements repeat once per
    /// level (see [`super::MAX_DEPTH`]).
    fn parse_statement_kind(&mut self) -> PResult<Statement> {
        match self.peek_kind() {
            T::KwLet => self.parse_let(),
            T::KwVar => self.parse_var(),
            T::KwReturn => self.parse_return(),
            T::KwAssert => self.parse_assert(),
            T::KwWhile => self.parse_while(),
            T::LowerIdent if self.nth_kind(1) == T::Eq => self.parse_assign(),
            T::KwPub
            | T::KwFn
            | T::KwStruct
            | T::KwEnum
            | T::KwType
            | T::KwCapability
            | T::KwTest
            | T::KwProperty => {
                Err(self
                    .error_expected("a statement (declarations are not allowed inside a block)"))
            }
            _ => self.parse_expr_stmt(),
        }
    }

    /// `expr_stmt = expr`
    fn parse_expr_stmt(&mut self) -> PResult<Statement> {
        let expr = self.parse_expr()?;
        Ok(Statement::Expr(ExprStmt {
            span: expr.span,
            trivia: Trivia::default(),
            expr,
        }))
    }

    /// `let_stmt = "let" ( LOWER_IDENT | "_" ) ( ":" type )? "=" expr`
    fn parse_let(&mut self) -> PResult<Statement> {
        let kw = self.expect(T::KwLet, "`let`")?;
        let binding = match self.peek_kind() {
            T::Underscore => LetBinding::Wildcard(self.bump().span),
            T::LowerIdent => LetBinding::Name(self.parse_lower_name("a binding name")?),
            _ => return Err(self.error_expected("a binding name or `_`")),
        };
        let ty = if self.eat(T::Colon).is_some() {
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect(T::Eq, "`=`")?;
        let value = self.parse_expr()?;
        let span = kw.span.join(value.span);
        Ok(Statement::Let(LetStmt {
            span,
            trivia: Trivia::default(),
            binding,
            ty,
            value,
        }))
    }

    /// `var_stmt = "var" LOWER_IDENT ( ":" type )? "=" expr`
    fn parse_var(&mut self) -> PResult<Statement> {
        let kw = self.expect(T::KwVar, "`var`")?;
        let name = self.parse_lower_name("a variable name")?;
        let ty = if self.eat(T::Colon).is_some() {
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect(T::Eq, "`=`")?;
        let value = self.parse_expr()?;
        let span = kw.span.join(value.span);
        Ok(Statement::Var(VarStmt {
            span,
            trivia: Trivia::default(),
            name,
            ty,
            value,
        }))
    }

    /// `assign_stmt = LOWER_IDENT "=" expr`
    fn parse_assign(&mut self) -> PResult<Statement> {
        let target = self.parse_lower_name("a variable name")?;
        self.expect(T::Eq, "`=`")?;
        let value = self.parse_expr()?;
        let span = target.span.join(value.span);
        Ok(Statement::Assign(AssignStmt {
            span,
            trivia: Trivia::default(),
            target,
            value,
        }))
    }

    /// `return_stmt = "return" expr?`
    fn parse_return(&mut self) -> PResult<Statement> {
        let kw = self.expect(T::KwReturn, "`return`")?;
        let value = match self.peek_kind() {
            T::Newline | T::RBrace | T::Eof => None,
            _ => Some(self.parse_expr()?),
        };
        let span = match &value {
            Some(v) => kw.span.join(v.span),
            None => kw.span,
        };
        Ok(Statement::Return(ReturnStmt {
            span,
            trivia: Trivia::default(),
            value,
        }))
    }

    /// `assert_stmt = "assert" expr`
    fn parse_assert(&mut self) -> PResult<Statement> {
        let kw = self.expect(T::KwAssert, "`assert`")?;
        let condition = self.parse_expr()?;
        let span = kw.span.join(condition.span);
        Ok(Statement::Assert(AssertStmt {
            span,
            trivia: Trivia::default(),
            condition,
        }))
    }

    /// `while_stmt = "while" expr while_tail`. The clauses are read out of
    /// line ([`Parser::parse_while_clauses`]), so that this frame, which
    /// nested loops repeat once per level, holds little more than the
    /// condition, the clauses, and the body (see [`super::MAX_DEPTH`]).
    fn parse_while(&mut self) -> PResult<Statement> {
        let kw = self.expect(T::KwWhile, "`while`")?;
        let condition = self.parse_header_expr()?;
        let clauses = self.parse_while_clauses()?;
        let body = self.parse_block()?;
        let span = kw.span.join(body.span);
        Ok(Statement::While(WhileStmt {
            span,
            trivia: Trivia::default(),
            condition,
            clauses,
            body,
        }))
    }

    /// `while_tail` up to the body: the clauses, each on a line of its own,
    /// leaving the parser at the body's `{` (grammar 2.4 rules 1 to 3).
    #[inline(never)]
    fn parse_while_clauses(&mut self) -> PResult<Vec<WhileClause>> {
        let mut clauses: Vec<WhileClause> = Vec::new();
        let mut has_decreases = false;
        loop {
            match self.continuation(is_while_continuation) {
                Continuation::None => {
                    return Err(
                        self.error_expected_header("`{` or a clause (`invariant`, `decreases`)")
                    )
                }
                cont => {
                    if self.at(T::LBrace) {
                        if cont == Continuation::SameLine && !clauses.is_empty() {
                            return Err(self.error_expected(
                                "end of line (the body's `{` must start a new line after a clause, grammar 2.4 rule 2)",
                            ));
                        }
                        return Ok(clauses);
                    }
                    if cont == Continuation::SameLine {
                        return Err(self.error_expected(
                            "end of line (a clause must start its own line, grammar 2.4 rule 1)",
                        ));
                    }
                    // A second `decreases` is reported at its keyword, before
                    // its expression is read (the earliest error, D70).
                    if has_decreases && self.at(T::KwDecreases) {
                        let span = self.peek_span();
                        return Err(self.duplicate_clause("decreases", span));
                    }
                    let clause_start = self.peek_span().start;
                    let mut trivia = Trivia::default();
                    self.take_leading(&mut trivia, clause_start, true);
                    let kw_tok = self.bump();
                    let expr = self.parse_header_expr()?;
                    let span = kw_tok.span.join(expr.span);
                    let clause = if kw_tok.kind == T::KwInvariant {
                        let mut c = InvariantClause { span, trivia, expr };
                        self.take_trailing(&mut c.trivia, span);
                        WhileClause::Invariant(c)
                    } else {
                        has_decreases = true;
                        let mut c = DecreasesClause { span, trivia, expr };
                        self.take_trailing(&mut c.trivia, span);
                        WhileClause::Decreases(c)
                    };
                    clauses.push(clause);
                }
            }
        }
    }
}
