//! The recursive-descent parser of `spec/grammar.md` sections 2 to 5.
//!
//! This module holds the parser state and the helpers shared by the
//! submodules: the newline-mode stack (section 2.2), the
//! `struct_literals_allowed` flag (D41), header continuation lookahead
//! (section 2.4 rule 3), trivia attachment, and diagnostics. The grammar
//! productions live in:
//!
//! * `module` - `.cpl` files, imports, and declarations (3.1-3.10)
//! * `types` - types and generic lists (3.4, D42 speculation)
//! * `stmt` - blocks and statements (3.11)
//! * `expr` - expressions (3.12)
//! * `pattern` - patterns (3.13)
//! * `work` - `.work` files (4.1-4.4)
//! * `patch` - `.patch` files (4.5)
//!
//! # Errors and resynchronisation (grammar D59, D70)
//!
//! A file with a lexical or syntax error yields no tree and exactly one
//! diagnostic, the error with the smallest start offset, carrying its
//! `parse` code of `spec/semantics.md` 9.4 (grammar D59, `spec/toolchain.md`
//! T6). Grammar D70 lets an implementation resynchronise after an error, to
//! stay total over any input and to find that earliest error, provided the
//! result shows nothing of it. This implementation resynchronises in the
//! lexer only:
//!
//! * The lexer ([`crate::lexer`]) runs over the whole input before parsing
//!   starts and keeps going after every lexical error: it skips an offending
//!   character and closes an unterminated text literal at the end of its
//!   line, so every lexical error of the file is recorded and the token
//!   stream always ends with `Eof`.
//! * The parser needs none of the statement-, member-, arm-, or item-level
//!   resynchronisation D70 permits: the first syntax error it detects is
//!   reported and unwinds every production to the file level (`Recover`),
//!   where parsing ends; nothing is recorded after it. For that first error
//!   to be the earliest, every check is made as soon as the token it is
//!   about has been read, never after reading further: the class of a `use`
//!   path segment when the `.` after it is seen, a duplicate `effects` or
//!   `decreases` clause at its keyword, a duplicate task name at the name,
//!   an `add` selector as soon as it ends, and a doc comment in an admitted
//!   position when the token after it is seen.
//!
//! `Parser::finish` then keeps one diagnostic, the one with the smallest
//! start offset among the lexer's errors and the parser's error (a lexical
//! and a syntax error at the same offset are reported as the lexical one,
//! which was recorded first), and returns [`ParseResult::value`] `None`, so
//! downstream crates never see a partial tree and the tree has no
//! error-recovery nodes. Nothing is reported during the generic-call
//! lookahead of D42, which scans tokens without parsing them
//! (`Parser::at_generic_call`). `invalid_utf8` is never raised
//! here: the input is already a `&str`, and the code that reads a file's
//! bytes reports it before lexing (D70, `spec/toolchain.md` 3.1).

mod expr;
mod module;
mod patch;
mod pattern;
mod stmt;
mod types;
mod work;

use crate::ast::{Comment, DocLine, Docs, Group, Literal, LiteralKind, Name, TextLiteral, Trivia};
use crate::codes;
use crate::lexer::{lex_full, Token, TokenKind, TriviaKind, TriviaToken};
use cpl_diag::{DiagCode, Diagnostic, FileId, Span};
use std::collections::HashMap;

/// The outcome of parsing one file (or, for [`parse_expr`], one
/// expression). `value` is `Some` exactly when no error was found;
/// otherwise `diagnostics` holds the single earliest error, by start offset
/// (D59, D70).
#[derive(Clone, Debug)]
pub struct ParseResult<T> {
    pub value: Option<T>,
    pub diagnostics: Vec<Diagnostic>,
}

impl<T> ParseResult<T> {
    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(Diagnostic::is_error)
    }

    /// The tree, or every diagnostic when the parse failed.
    pub fn into_result(self) -> Result<T, Vec<Diagnostic>> {
        match self.value {
            Some(v) => Ok(v),
            None => Err(self.diagnostics),
        }
    }
}

/// Parse a `.cpl` file (`source_file`).
pub fn parse_module(src: &str, file: FileId) -> ParseResult<crate::ast::Module> {
    let mut parser = Parser::new(src, file);
    let module = parser.parse_source_file();
    parser.finish(module)
}

/// Parse a `.work` file (`work_file`).
pub fn parse_work(src: &str, file: FileId) -> ParseResult<crate::ast::WorkFile> {
    let mut parser = Parser::new(src, file);
    let work = parser.parse_work_file();
    parser.finish(work)
}

/// Parse a `.patch` file (`patch_file`).
pub fn parse_patch(src: &str, file: FileId) -> ParseResult<crate::ast::PatchFile> {
    let mut parser = Parser::new(src, file);
    let patch = parser.parse_patch_file();
    parser.finish(patch)
}

/// Parse one expression with nothing after it: the constant-expression
/// arguments of `cpl run` (`spec/toolchain.md` 7.2; semantics S39 and 3.5),
/// such as `[1, 2, 3]`, `-2`, `Some(PageId("p"))`, or
/// `Page { id: PageId("p"), text: "t" }`.
///
/// The text is read with the normal `expr` grammar of grammar 3.12, as an
/// expression statement reads it: struct literals are allowed (D41) and a
/// line break ends the expression outside brackets (2.3), so an operator
/// cannot end or begin a line there. Line breaks and comments before and
/// after the expression are allowed; any other token after it is the
/// syntax error `expected end of input after the expression, found
/// <token>`. No constant-expression restriction is applied here: names,
/// calls, holes, and `if` parse, and the caller's typing rejects them where
/// it must. As for the file parsers, `value` is `Some` exactly when the text
/// lexed and parsed without error, and otherwise `diagnostics` holds the
/// single earliest error (grammar D59, D70).
pub fn parse_expr(src: &str, file: FileId) -> ParseResult<crate::ast::Expr> {
    let mut parser = Parser::new(src, file);
    let expr = parser.parse_expr_input();
    parser.finish_opt(expr.ok())
}

/// Marker returned by a production that has already reported its error; the
/// callers unwind to the file level, where parsing ends (D59).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Recover;

pub(crate) type PResult<T> = Result<T, Recover>;

/// A bracketed production: its span, its layout memory, the inner value,
/// and (for comma groups) the comments before the closing bracket.
pub(crate) struct Bracketed<T> {
    pub span: Span,
    pub group: Group,
    pub inner: T,
    pub trailing_comments: Vec<Comment>,
}

/// Newline modes of grammar section 2.2.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Significant,
    Insignificant,
}

/// Result of the header continuation lookahead (grammar 2.4 rule 3).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Continuation {
    /// Neither the next token nor the token after a `NEWLINE` continues the header.
    None,
    /// The continuation token follows directly (no newline in between).
    SameLine,
    /// A `NEWLINE` was consumed and the continuation token is next.
    AfterNewline,
}

/// Maximum bracket/block/header nesting before the parser gives up on an
/// input (`nesting is too deep`): the minimum grammar D73 guarantees. It
/// keeps pathological inputs from overflowing the stack: at this depth the
/// worst-case debug build stays under 1 MiB of stack (verified by
/// `tests/errors.rs` and, for nested loops, by
/// `tests/adversarial_parser_errors_r2.rs`), because the functions every
/// nesting level passes through keep their frames small and leave the work
/// before and after the nested parse to out-of-line helpers. Real programs
/// stay in single digits.
pub const MAX_DEPTH: u32 = 64;

/// Maximum number of binary and postfix operators along one path of nested
/// expressions in a top-level expression. The parser applies operators
/// iteratively, but the tree they build is left-nested, so every later
/// recursive walk (drop, clone, dump, serialisation) would recurse once per
/// operator; the budget keeps that depth bounded. An expression's operators
/// add to those of the expressions around it, and expressions side by side
/// (match arms, the statements of a block, arguments) count separately, as
/// a walk visits them one after another. Real expressions have a handful of
/// operators.
pub const MAX_EXPR_OPS: u32 = 1000;

pub(crate) struct Parser<'a> {
    src: &'a str,
    file: FileId,
    tokens: Vec<Token>,
    pos: usize,
    modes: Vec<Mode>,
    struct_literals_allowed: bool,
    /// The lexer's diagnostics, then at most one parser error.
    diagnostics: Vec<Diagnostic>,
    trivia: Vec<TriviaToken>,
    trivia_pos: usize,
    /// Comments that rule G of `spec/toolchain.md` 2.3 gives to the innermost
    /// attachable node containing them, in source order: those left inside a
    /// bracket sequence that owns no comments of its own when it closes (a
    /// parenthesized expression, a generic list, a payload group whose
    /// elements carry no trivia, a single-value bracket), and those of an
    /// enclosing sequence written before a nested bracket sequence opened
    /// (no node of that nested sequence claims them, rule D's "within the
    /// same enclosing sequence"). The first node to finish whose span covers
    /// one claims it ([`Parser::take_trailing`]); nodes finish innermost
    /// first, so that node is the innermost one.
    orphans: Vec<Comment>,
    /// The D42 lookahead's findings by the token index of a `<`: the index
    /// of the `>` that closes the generic list it opens, or `None` when no
    /// generic list begins there (`Parser::at_generic_call`).
    generic_lists: HashMap<usize, Option<usize>>,
    /// Set by the first error; nothing is reported afterwards (D59).
    failed: bool,
    depth: u32,
    /// One entry per expression being parsed, outermost first: its own operators and the
    /// most operators along a path of an expression nested in it that has ended.
    op_frames: Vec<(u32, u32)>,
    /// The own operators of every expression being parsed: the path to the current one.
    op_path: u32,
    /// The next node to take leading trivia is the first of its sequence,
    /// whose blank line (if any) is not recorded (`spec/toolchain.md` 2.3).
    sequence_start: bool,
}

impl<'a> Parser<'a> {
    pub(crate) fn new(src: &'a str, file: FileId) -> Parser<'a> {
        let lexed = lex_full(src, file);
        Parser {
            src,
            file,
            tokens: lexed.tokens,
            pos: 0,
            modes: vec![Mode::Significant],
            struct_literals_allowed: true,
            diagnostics: lexed.diagnostics,
            trivia: lexed.trivia,
            trivia_pos: 0,
            orphans: Vec::new(),
            generic_lists: HashMap::new(),
            failed: false,
            depth: 0,
            op_frames: Vec::new(),
            op_path: 0,
            sequence_start: false,
        }
    }

    /// D59, D70: the result carries the earliest error of the file, or the tree.
    fn finish<T>(self, value: T) -> ParseResult<T> {
        self.finish_opt(Some(value))
    }

    /// [`Parser::finish`] for a production that may have failed (`None`).
    fn finish_opt<T>(mut self, value: Option<T>) -> ParseResult<T> {
        if value.is_none() && !self.diagnostics.iter().any(Diagnostic::is_error) {
            // Every production reports its error before it fails, so this is
            // unreachable; it keeps a missing value paired with a diagnostic.
            self.failed = false;
            let _ = self.error_expected("an expression");
        }
        let first = self
            .diagnostics
            .iter()
            .enumerate()
            .filter(|(_, d)| d.is_error())
            .min_by_key(|(_, d)| d.span.map_or(u32::MAX, |s| s.start))
            .map(|(i, _)| i);
        match first {
            Some(i) => ParseResult {
                value: None,
                diagnostics: vec![self.diagnostics.swap_remove(i)],
            },
            None => ParseResult {
                value,
                diagnostics: self.diagnostics,
            },
        }
    }

    pub(crate) fn file(&self) -> FileId {
        self.file
    }

    pub(crate) fn source_len(&self) -> u32 {
        self.src.len() as u32
    }

    // -----------------------------------------------------------------------
    // Token access
    // -----------------------------------------------------------------------

    fn mode(&self) -> Mode {
        self.modes.last().copied().unwrap_or(Mode::Significant)
    }

    fn raw(&self, index: usize) -> &Token {
        let last = self.tokens.len() - 1;
        &self.tokens[index.min(last)]
    }

    /// Discard `NEWLINE` tokens while the top mode is `Insignificant`
    /// (grammar 2.2 `next_token`).
    fn skip_insignificant(&mut self) {
        if self.mode() == Mode::Insignificant {
            while self.raw(self.pos).kind == TokenKind::Newline {
                self.pos += 1;
            }
        }
    }

    pub(crate) fn peek(&mut self) -> &Token {
        self.skip_insignificant();
        self.raw(self.pos)
    }

    pub(crate) fn peek_kind(&mut self) -> TokenKind {
        self.peek().kind
    }

    pub(crate) fn peek_span(&mut self) -> Span {
        self.peek().span
    }

    /// The `n`-th token after the current one under the current mode.
    pub(crate) fn nth(&mut self, n: usize) -> &Token {
        self.skip_insignificant();
        let insignificant = self.mode() == Mode::Insignificant;
        let mut index = self.pos;
        let mut remaining = n;
        while remaining > 0 {
            index += 1;
            if insignificant {
                while self.raw(index).kind == TokenKind::Newline && index < self.tokens.len() - 1 {
                    index += 1;
                }
            }
            remaining -= 1;
        }
        self.raw(index)
    }

    pub(crate) fn nth_kind(&mut self, n: usize) -> TokenKind {
        self.nth(n).kind
    }

    pub(crate) fn at(&mut self, kind: TokenKind) -> bool {
        self.peek_kind() == kind
    }

    /// True when the next token is a `LOWER_IDENT` spelled `word` (contextual keyword).
    pub(crate) fn at_word(&mut self, word: &str) -> bool {
        let tok = self.peek();
        tok.kind == TokenKind::LowerIdent && tok.text == word
    }

    /// Consume and return the next token (`Eof` is never consumed).
    pub(crate) fn bump(&mut self) -> Token {
        self.skip_insignificant();
        let tok = self.raw(self.pos).clone();
        if tok.kind != TokenKind::Eof {
            self.pos += 1;
        }
        tok
    }

    pub(crate) fn eat(&mut self, kind: TokenKind) -> Option<Token> {
        if self.at(kind) {
            Some(self.bump())
        } else {
            None
        }
    }

    pub(crate) fn expect(&mut self, kind: TokenKind, expected: &str) -> PResult<Token> {
        if self.at(kind) {
            Ok(self.bump())
        } else {
            Err(self.error_expected(expected))
        }
    }

    pub(crate) fn expect_word(&mut self, word: &str) -> PResult<Token> {
        if self.at_word(word) {
            Ok(self.bump())
        } else {
            Err(self.error_expected(&format!("`{word}`")))
        }
    }

    /// End offset of the last consumed non-newline token.
    pub(crate) fn prev_end(&self) -> u32 {
        let mut i = self.pos;
        while i > 0 {
            i -= 1;
            if self.tokens[i].kind != TokenKind::Newline {
                return self.tokens[i].span.end;
            }
        }
        0
    }

    pub(crate) fn span_from(&self, start: u32) -> Span {
        Span::new(self.file, start, self.prev_end().max(start))
    }

    /// Skip `NEWLINE` tokens regardless of mode (the leading `NEWLINE?` of
    /// `lines(X)` and the separators between lines).
    pub(crate) fn skip_newlines(&mut self) {
        while self.raw(self.pos).kind == TokenKind::Newline {
            self.pos += 1;
        }
    }

    /// Header continuation lookahead (grammar 2.4 rule 3): if the next token
    /// satisfies `pred` the header continues on the same line; if a `NEWLINE`
    /// followed by a satisfying token is next, the `NEWLINE` is consumed;
    /// otherwise nothing is consumed.
    pub(crate) fn continuation(&mut self, pred: impl Fn(&Token) -> bool) -> Continuation {
        if pred(self.peek()) {
            return Continuation::SameLine;
        }
        if self.peek_kind() == TokenKind::Newline && pred(self.raw(self.pos + 1)) {
            self.pos += 1;
            return Continuation::AfterNewline;
        }
        Continuation::None
    }

    /// Consume a `NEWLINE` that is directly followed by `{` (the `NEWLINE?
    /// block` of `if`, `else`, and `match` headers).
    pub(crate) fn eat_newline_before_brace(&mut self) {
        if self.peek_kind() == TokenKind::Newline
            && self.raw(self.pos + 1).kind == TokenKind::LBrace
        {
            self.pos += 1;
        }
    }

    /// The `NEWLINE? "{"` that ends an `if`, `else`, or `match` header
    /// (grammar 2.3, decided by the 2.4 rule-3 lookahead): consumes a
    /// `NEWLINE` directly followed by `{` and leaves the parser at the `{`.
    /// A missing `{` is reported as after every other `{`-only header
    /// ([`Parser::error_expected_header`], grammar 2.4 rule 3): the
    /// `NEWLINE` is admitted there, so a doc comment that begins the next
    /// line is the offending token (grammar 1.2, `doc comment must precede
    /// a declaration`, also "one between a header and its `{`").
    pub(crate) fn expect_header_brace(&mut self) -> PResult<()> {
        self.eat_newline_before_brace();
        if self.at(TokenKind::LBrace) {
            Ok(())
        } else {
            Err(self.error_expected_header("`{`"))
        }
    }

    /// True when the most recently consumed token began a line (a `NEWLINE`
    /// directly precedes it in the raw token stream).
    pub(crate) fn last_token_began_line(&self) -> bool {
        self.pos >= 2 && self.tokens[self.pos - 2].kind == TokenKind::Newline
    }

    // -----------------------------------------------------------------------
    // Modes, flags, nesting
    // -----------------------------------------------------------------------

    pub(crate) fn struct_literals_allowed(&self) -> bool {
        self.struct_literals_allowed
    }

    /// Set the struct-literal flag (D41), returning the previous value for
    /// [`Parser::restore_struct_literals`].
    pub(crate) fn set_struct_literals(&mut self, allowed: bool) -> bool {
        std::mem::replace(&mut self.struct_literals_allowed, allowed)
    }

    pub(crate) fn restore_struct_literals(&mut self, saved: bool) {
        self.struct_literals_allowed = saved;
    }

    /// Account for one more nesting level (bracket, block, header, prefix
    /// operator); the matching [`Parser::leave`] must follow. Kept as a pair
    /// rather than a closure combinator so that deep recursion costs one
    /// stack frame per level, not several.
    pub(crate) fn enter(&mut self) -> PResult<()> {
        if self.depth >= MAX_DEPTH {
            let span = self.peek_span();
            return Err(self.syntax_error(
                &format!("nesting is too deep (more than {MAX_DEPTH} levels)"),
                span,
            ));
        }
        self.depth += 1;
        Ok(())
    }

    pub(crate) fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// Account for one more binary or postfix operator of the expression
    /// being parsed (see [`MAX_EXPR_OPS`]). The operators of the expressions
    /// around it and the deepest of its nested expressions so far are on one
    /// path with it, so a sum beyond the budget is beyond it at the end too.
    pub(crate) fn count_op(&mut self, span: Span) -> PResult<()> {
        let nested = match self.op_frames.last_mut() {
            Some(frame) => {
                frame.0 += 1;
                frame.1
            }
            None => 0,
        };
        self.op_path += 1;
        if self.op_path.saturating_add(nested) > MAX_EXPR_OPS {
            return Err(self.syntax_error(
                &format!("an expression has too many operators (more than {MAX_EXPR_OPS}); split it into bindings"),
                span,
            ));
        }
        Ok(())
    }

    /// Begin an expression parse; the matching [`Parser::end_expression`]
    /// must follow.
    pub(crate) fn begin_expression(&mut self) {
        self.op_frames.push((0, 0));
    }

    /// End an expression parse: its operators along its deepest path count
    /// toward the expression around it, not toward its siblings.
    pub(crate) fn end_expression(&mut self) {
        if let Some((own, nested)) = self.op_frames.pop() {
            self.op_path -= own;
            if let Some(outer) = self.op_frames.last_mut() {
                outer.1 = outer.1.max(own + nested);
            }
        }
    }

    /// `open ... close` with `mode` pushed and struct literals re-enabled
    /// inside (D41). Returns the bracket span and the inner value.
    pub(crate) fn bracketed<T>(
        &mut self,
        open: TokenKind,
        close: TokenKind,
        mode: Mode,
        f: impl FnOnce(&mut Self) -> PResult<T>,
    ) -> PResult<(Span, T)> {
        let b = self.bracketed_inner(open, close, mode, false, f)?;
        Ok((b.span, b.inner))
    }

    /// [`Parser::bracketed`] also returning the group's layout memory, for
    /// comma groups whose elements carry no trivia of their own (annotation
    /// arguments, variant payloads, constructor-pattern payloads).
    pub(crate) fn bracketed_full<T>(
        &mut self,
        open: TokenKind,
        close: TokenKind,
        mode: Mode,
        f: impl FnOnce(&mut Self) -> PResult<T>,
    ) -> PResult<Bracketed<T>> {
        self.bracketed_inner(open, close, mode, false, f)
    }

    /// [`Parser::bracketed_full`] for a comma-separated group whose elements
    /// take their own leading trivia: also collects the comments between the
    /// last element and the closing bracket (the group's `trailing_comments`).
    /// Not for single-payload brackets (`(Text)`, `@id("x")`, `git("main")`),
    /// where an unclaimed comment belongs to the enclosing node.
    pub(crate) fn bracketed_group<T>(
        &mut self,
        open: TokenKind,
        close: TokenKind,
        mode: Mode,
        f: impl FnOnce(&mut Self) -> PResult<T>,
    ) -> PResult<Bracketed<T>> {
        self.bracketed_inner(open, close, mode, true, f)
    }

    fn bracketed_inner<T>(
        &mut self,
        open: TokenKind,
        close: TokenKind,
        mode: Mode,
        collect_dangling: bool,
        f: impl FnOnce(&mut Self) -> PResult<T>,
    ) -> PResult<Bracketed<T>> {
        let open_tok = self.expect(open, &open.describe())?;
        let open_index = self.pos - 1;
        // `spec/toolchain.md` 2.3 rule D: no node inside the brackets claims a
        // comment of the enclosing sequence written before them.
        self.orphan_pending(open_tok.span.start);
        // Grammar 2.2: the mode is pushed when the bracket is consumed, so the
        // token after it, where D73 reports an excess level, is read in the
        // bracket's own mode (a line break after `(` is no token there).
        self.modes.push(mode);
        if let Err(recover) = self.enter() {
            self.modes.pop();
            return Err(recover);
        }
        let saved = self.set_struct_literals(true);
        let inner = f(self);
        self.restore_struct_literals(saved);
        let result = inner.and_then(|inner| {
            let close_start = self.peek_span().start;
            let trailing_comments = if collect_dangling {
                self.take_dangling(close_start)
            } else {
                // The sequence owns no comments: what its elements left is
                // rule G's (`spec/toolchain.md` 2.3).
                self.orphan_pending(close_start);
                Vec::new()
            };
            let close_tok = self.expect(close, &close.describe())?;
            let close_index = self.pos - 1;
            Ok(Bracketed {
                span: open_tok.span.join(close_tok.span),
                group: Group {
                    multiline: self.newline_between(open_index, close_index),
                },
                inner,
                trailing_comments,
            })
        });
        self.modes.pop();
        self.leave();
        result
    }

    /// The layout memory of a group (`spec/toolchain.md` 2.4 rule 6): a
    /// `NEWLINE` token between the bracket tokens at `open` and `close` that
    /// is at the group's own level, not inside a nested bracket pair or
    /// block (whose line structure the formatter decides independently).
    fn newline_between(&self, open: usize, close: usize) -> bool {
        let mut depth: i32 = 0;
        for tok in &self.tokens[open + 1..close] {
            match tok.kind {
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => depth += 1,
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => depth -= 1,
                TokenKind::Newline if depth == 0 => return true,
                _ => {}
            }
        }
        false
    }

    /// A possibly empty comma-separated list with an optional trailing comma
    /// (`commas(X)`), parsed up to (not including) `close`.
    pub(crate) fn parse_commas<T>(
        &mut self,
        close: TokenKind,
        mut item: impl FnMut(&mut Self) -> PResult<T>,
    ) -> PResult<Vec<T>> {
        let mut items = Vec::new();
        loop {
            if self.at(close) {
                break;
            }
            items.push(item(self)?);
            if self.eat(TokenKind::Comma).is_some() {
                continue;
            }
            if self.at(close) {
                break;
            }
            return Err(self.error_expected(&format!("`,` or {}", close.describe())));
        }
        Ok(items)
    }

    /// `"{" lines(X) "}"` in a significant-newline context: items separated
    /// by newlines. Returns the brace span, the items, and the comments
    /// after the last item.
    pub(crate) fn parse_lines<T>(
        &mut self,
        what: &str,
        mut item: impl FnMut(&mut Self) -> PResult<T>,
    ) -> PResult<(Span, Vec<T>, Vec<Comment>)> {
        let open = self.expect(TokenKind::LBrace, "`{`")?;
        // Grammar 2.2: the body's mode is pushed when its `{` is consumed, so
        // the token after it (D73) is read in that mode. Pending comments are
        // kept: one between a header and this `{` is the first node's
        // (`spec/toolchain.md` 2.3 rule C).
        self.modes.push(Mode::Significant);
        if let Err(recover) = self.enter() {
            self.modes.pop();
            return Err(recover);
        }
        let saved = self.set_struct_literals(true);
        let result = self.parse_lines_inner(open.span.start, what, &mut item);
        self.restore_struct_literals(saved);
        self.modes.pop();
        self.leave();
        result
    }

    /// The lines of a body. The checks around each item live in out-of-line
    /// helpers, so that this frame, which nested bodies repeat once per
    /// level, holds little more than the items (see [`MAX_DEPTH`]).
    fn parse_lines_inner<T>(
        &mut self,
        start: u32,
        what: &str,
        item: &mut impl FnMut(&mut Self) -> PResult<T>,
    ) -> PResult<(Span, Vec<T>, Vec<Comment>)> {
        let mut items = Vec::new();
        while self.line_starts_an_item(what)? {
            self.sequence_start = items.is_empty();
            let parsed = item(self);
            self.sequence_start = false;
            items.push(parsed?);
            self.line_ends_its_item(what)?;
        }
        let (span, trailing) = self.close_lines(start);
        Ok((span, items, trailing))
    }

    /// Before an item of a `lines(X)` body: skips the line breaks and
    /// returns whether an item follows (`false` at the closing `}`).
    #[inline(never)]
    fn line_starts_an_item(&mut self, what: &str) -> PResult<bool> {
        self.skip_newlines();
        match self.peek_kind() {
            TokenKind::RBrace => Ok(false),
            TokenKind::Eof => Err(self.error_expected("`}`")),
            // Members whose production admits `DOC_COMMENT*` check the doc
            // comments themselves; everywhere else one is misplaced (grammar
            // 1.2).
            TokenKind::DocComment
                if matches!(
                    what,
                    "statement" | "match arm" | "task item" | "requirement"
                ) =>
            {
                Err(self.error_expected(what))
            }
            TokenKind::KwElse => Err(self.error_misplaced_else()),
            _ => Ok(true),
        }
    }

    /// After an item of a `lines(X)` body: its line must end there.
    #[inline(never)]
    fn line_ends_its_item(&mut self, what: &str) -> PResult<()> {
        match self.peek_kind() {
            TokenKind::Newline | TokenKind::RBrace | TokenKind::Eof => Ok(()),
            TokenKind::Comma => {
                let span = self.peek_span();
                Err(self.syntax_error(&format!("{what}s are separated by newlines"), span))
            }
            _ => Err(self.error_expected(&format!("end of line after the {what}"))),
        }
    }

    /// The closing `}` of a `lines(X)` body that began at `start`: the
    /// body's span and its dangling comments (`spec/toolchain.md` 2.3 rule E).
    #[inline(never)]
    fn close_lines(&mut self, start: u32) -> (Span, Vec<Comment>) {
        let close_start = self.peek_span().start;
        let trailing = self.take_dangling(close_start);
        let close = self.bump();
        (Span::new(self.file, start, close.span.end), trailing)
    }

    // -----------------------------------------------------------------------
    // Diagnostics
    // -----------------------------------------------------------------------

    /// Record a diagnostic. Only the first error of the file is kept (D59).
    pub(crate) fn emit(&mut self, diag: Diagnostic) {
        if self.failed {
            return;
        }
        if diag.is_error() {
            self.failed = true;
        }
        self.diagnostics.push(diag);
    }

    /// Report an error with the given code and return the recovery marker.
    pub(crate) fn error(
        &mut self,
        code: DiagCode,
        message: impl Into<String>,
        span: Span,
    ) -> Recover {
        self.emit(Diagnostic::error(code, codes::CATEGORY, message).with_span(span));
        Recover
    }

    /// A `syntax_error` with one of the messages `spec/grammar.md` quotes.
    pub(crate) fn syntax_error(&mut self, message: &str, span: Span) -> Recover {
        self.error(codes::SYNTAX_ERROR, message, span)
    }

    /// The generic `expected <tokens>, found <token>` syntax error at the
    /// next token (D59); reserved words and misplaced doc comments get their
    /// own codes.
    pub(crate) fn error_expected(&mut self, expected: &str) -> Recover {
        let tok = self.peek().clone();
        self.error_expected_at(expected, &tok)
    }

    /// [`Parser::error_expected`] at a given token.
    pub(crate) fn error_expected_at(&mut self, expected: &str, tok: &Token) -> Recover {
        match tok.kind {
            TokenKind::ReservedWord => {
                self.emit(
                    Diagnostic::error(
                        codes::RESERVED_WORD,
                        codes::CATEGORY,
                        format!("`{}` is reserved for a future edition", tok.text),
                    )
                    .with_data("name", tok.text.clone().into())
                    .with_span(tok.span),
                );
                Recover
            }
            TokenKind::DocComment => self.error(
                codes::DOC_COMMENT_POSITION,
                "doc comment must precede a declaration",
                tok.span,
            ),
            _ => {
                let found = tok.describe();
                self.error_expected_found(expected, &found, tok.span)
            }
        }
    }

    /// The generic syntax error with explicit `expected` and `found`
    /// descriptions (the two fields `spec/semantics.md` 9.4 lists).
    pub(crate) fn error_expected_found(
        &mut self,
        expected: &str,
        found: &str,
        span: Span,
    ) -> Recover {
        self.emit(
            Diagnostic::error(
                codes::SYNTAX_ERROR,
                codes::CATEGORY,
                format!("expected {expected}, found {found}"),
            )
            .with_data("expected", expected.into())
            .with_data("found", found.into())
            .with_span(span),
        );
        Recover
    }

    /// The error for a header whose rule-3 lookahead (grammar 2.4) found no
    /// continuation. The `NEWLINE` after a header is admitted by the
    /// grammar, so when the next line begins with a doc comment that comment
    /// is the misplaced token (grammar 1.2, `doc comment must precede a
    /// declaration`); otherwise the line end itself is reported.
    pub(crate) fn error_expected_header(&mut self, expected: &str) -> Recover {
        if self.peek_kind() == TokenKind::Newline {
            let next = self.raw(self.pos + 1).clone();
            if next.kind == TokenKind::DocComment {
                return self.error_expected_at(expected, &next);
            }
        }
        self.error_expected(expected)
    }

    /// D12: a line beginning with `else` where newlines are significant.
    pub(crate) fn error_misplaced_else(&mut self) -> Recover {
        let span = self.peek_span();
        self.error(
            codes::MISPLACED_ELSE,
            "`else` must follow the closing `}` of the preceding block on the same line",
            span,
        )
    }

    // -----------------------------------------------------------------------
    // Trivia
    // -----------------------------------------------------------------------

    fn comment_from(&self, t: TriviaToken, blank_line_before: bool) -> Comment {
        let start = (t.span.start as usize).min(self.src.len());
        let end = (t.span.end as usize).min(self.src.len()).max(start);
        Comment {
            span: t.span,
            text: self.src[start..end].to_string(),
            blank_line_before,
        }
    }

    /// Attach every pending comment that starts before `before` as leading
    /// trivia; when `record_blank` is set, note a blank line directly before
    /// the node (never for the first node of a brace body or file,
    /// `spec/toolchain.md` 2.3).
    pub(crate) fn take_leading(&mut self, trivia: &mut Trivia, before: u32, record_blank: bool) {
        let mut pending_blank = false;
        while let Some(t) = self.trivia.get(self.trivia_pos).copied() {
            if t.span.start >= before {
                break;
            }
            self.trivia_pos += 1;
            match t.kind {
                TriviaKind::BlankLine => pending_blank = true,
                TriviaKind::Comment => {
                    let comment = self.comment_from(t, pending_blank);
                    trivia.leading.push(comment);
                    pending_blank = false;
                }
            }
        }
        if record_blank {
            if pending_blank && !self.sequence_start {
                trivia.blank_line_before = true;
            }
            self.sequence_start = false;
        }
    }

    /// Attach the trailing trivia of the node spanning `span`, in source
    /// order (`spec/toolchain.md` 2.3): the comments inside it that no inner
    /// node claimed (rule G: the orphans it covers, then the pending
    /// comments of its own sequence that start inside it), then the comment
    /// that directly follows its last token on that line, with only
    /// horizontal whitespace or one separating comma between them (rule B).
    pub(crate) fn take_trailing(&mut self, trivia: &mut Trivia, span: Span) {
        // The orphans are in source order and none lies after the node yet,
        // so the ones it covers are a suffix of the list.
        let covered = self
            .orphans
            .iter()
            .rposition(|c| c.span.start < span.start)
            .map_or(0, |i| i + 1);
        trivia.trailing.extend(self.orphans.drain(covered..));
        let node_end = span.end;
        while let Some(t) = self.trivia.get(self.trivia_pos).copied() {
            let inside = t.span.start < node_end;
            let directly_follows = t.kind == TriviaKind::Comment
                && t.line_start < node_end
                && self.only_a_comma_between(node_end, t.span.start);
            if !(inside || directly_follows) {
                break;
            }
            self.trivia_pos += 1;
            if t.kind == TriviaKind::Comment {
                let comment = self.comment_from(t, false);
                trivia.trailing.push(comment);
            }
        }
    }

    /// True when no token other than at most one `,` lies between `from` (a
    /// node's end) and `to` (a comment's start on the same line): rule B's
    /// "only horizontal whitespace, or one separating comma". Decided on the
    /// token stream by binary search, so that attaching a trailing comment
    /// costs no scan of the text between the node and the comment (which
    /// made a long one-line group followed by a comment quadratic).
    fn only_a_comma_between(&self, from: u32, to: u32) -> bool {
        let next = self.tokens.partition_point(|t| t.span.start < from);
        let starts_after = |i: usize| self.tokens.get(i).is_none_or(|t| t.span.start >= to);
        starts_after(next)
            || (self
                .tokens
                .get(next)
                .is_some_and(|t| t.kind == TokenKind::Comma)
                && starts_after(next + 1))
    }

    /// Move the pending comments that start before `before` to the orphans
    /// (rule G), dropping the blank lines among them, which no node records.
    pub(crate) fn orphan_pending(&mut self, before: u32) {
        while let Some(t) = self.trivia.get(self.trivia_pos).copied() {
            if t.span.start >= before {
                break;
            }
            self.trivia_pos += 1;
            if t.kind == TriviaKind::Comment {
                let comment = self.comment_from(t, false);
                self.orphans.push(comment);
            }
        }
    }

    /// Comments before `before` that no node claimed (before a closing `}`
    /// or at the end of the file).
    pub(crate) fn take_dangling(&mut self, before: u32) -> Vec<Comment> {
        let mut trivia = Trivia::default();
        self.take_leading(&mut trivia, before, false);
        trivia.leading
    }

    // -----------------------------------------------------------------------
    // Small shared productions
    // -----------------------------------------------------------------------

    pub(crate) fn name_from(&self, tok: &Token) -> Name {
        Name::new(tok.text.clone(), tok.span)
    }

    /// A `LOWER_IDENT`; the class is part of the grammar (D2), so a
    /// `PascalCase` spelling is the generic syntax error like any other token.
    pub(crate) fn parse_lower_name(&mut self, what: &str) -> PResult<Name> {
        if self.at(TokenKind::LowerIdent) {
            let tok = self.bump();
            return Ok(self.name_from(&tok));
        }
        Err(self.error_expected(&format!("{what} (snake_case identifier)")))
    }

    /// An `UPPER_IDENT` (D2).
    pub(crate) fn parse_upper_name(&mut self, what: &str) -> PResult<Name> {
        if self.at(TokenKind::UpperIdent) {
            let tok = self.bump();
            return Ok(self.name_from(&tok));
        }
        Err(self.error_expected(&format!("{what} (PascalCase identifier)")))
    }

    /// `LOWER_IDENT | UPPER_IDENT`; never a keyword (D53).
    pub(crate) fn parse_member_name(&mut self) -> PResult<Name> {
        match self.peek_kind() {
            TokenKind::LowerIdent | TokenKind::UpperIdent => {
                let tok = self.bump();
                Ok(self.name_from(&tok))
            }
            _ => Err(self.error_expected("a member name")),
        }
    }

    /// `DOC_COMMENT*`
    pub(crate) fn parse_docs(&mut self) -> Option<Docs> {
        let mut lines: Vec<DocLine> = Vec::new();
        while self.at(TokenKind::DocComment) {
            let tok = self.bump();
            lines.push(DocLine {
                span: tok.span,
                text: tok.text,
            });
        }
        docs_from(lines)
    }

    /// `DOC_COMMENT*` in one of the positions grammar 1.2 admits: the doc
    /// comments must be followed by the first token of the member or item
    /// they document (`starts`); a doc comment followed by anything else is
    /// `doc comment must precede a declaration`.
    pub(crate) fn parse_docs_before(
        &mut self,
        starts: impl Fn(&Token) -> bool,
    ) -> PResult<Option<Docs>> {
        let docs = self.parse_docs();
        if let Some(d) = &docs {
            if !starts(self.peek()) {
                return Err(self.error(
                    codes::DOC_COMMENT_POSITION,
                    "doc comment must precede a declaration",
                    d.span,
                ));
            }
        }
        Ok(docs)
    }

    pub(crate) fn parse_text_literal(&mut self, what: &str) -> PResult<TextLiteral> {
        if self.at(TokenKind::Text) {
            let tok = self.bump();
            Ok(TextLiteral {
                span: tok.span,
                value: tok.text,
            })
        } else {
            Err(self.error_expected(what))
        }
    }

    /// `INT | TEXT | true | false`
    pub(crate) fn parse_literal(&mut self) -> PResult<Literal> {
        let tok = self.peek().clone();
        let kind = match tok.kind {
            TokenKind::Int => LiteralKind::Int(tok.text.clone()),
            TokenKind::Text => LiteralKind::Text(tok.text.clone()),
            TokenKind::KwTrue => LiteralKind::Bool(true),
            TokenKind::KwFalse => LiteralKind::Bool(false),
            _ => return Err(self.error_expected("a literal")),
        };
        self.bump();
        Ok(Literal {
            span: tok.span,
            kind,
        })
    }
}

pub(crate) fn docs_from(lines: Vec<DocLine>) -> Option<Docs> {
    let first = lines.first()?.span;
    let last = lines.last()?.span;
    Some(Docs {
        span: first.join(last),
        lines,
    })
}
