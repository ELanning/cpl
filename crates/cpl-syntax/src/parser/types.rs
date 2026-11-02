//! Types and generic lists (grammar 3.4) and the generic-call lookahead of
//! D42.

use super::{Mode, PResult, Parser};
use crate::ast::{GenericArgs, GenericParams, Name, Type, TypePath};
use crate::lexer::TokenKind as T;

impl<'a> Parser<'a> {
    /// `type = type_path generic_args?`
    pub(crate) fn parse_type(&mut self) -> PResult<Type> {
        let path = self.parse_type_path()?;
        let generic_args = if self.at(T::Lt) {
            Some(self.parse_generic_args()?)
        } else {
            None
        };
        let end = generic_args.as_ref().map(|g| g.span).unwrap_or(path.span);
        Ok(Type {
            span: path.span.join(end),
            path,
            generic_args,
        })
    }

    /// `type_path = ( LOWER_IDENT "." )* UPPER_IDENT`
    pub(crate) fn parse_type_path(&mut self) -> PResult<TypePath> {
        let mut modules: Vec<Name> = Vec::new();
        while self.at(T::LowerIdent) && self.nth_kind(1) == T::Dot {
            let tok = self.bump();
            modules.push(self.name_from(&tok));
            self.bump();
        }
        let name = self.parse_upper_name("a type name")?;
        let start = modules.first().map(|m| m.span).unwrap_or(name.span);
        Ok(TypePath {
            span: start.join(name.span),
            modules,
            name,
        })
    }

    /// `generic_args = "<" type ( "," type )* ">"` (no trailing comma, one line).
    ///
    /// A generic list holds no attachable node, so a comment inside it is
    /// the innermost enclosing attachable node's (`spec/toolchain.md` 2.3
    /// rule G), and so is one written before it that no node claimed.
    pub(crate) fn parse_generic_args(&mut self) -> PResult<GenericArgs> {
        let open = self.expect(T::Lt, "`<`")?;
        self.orphan_pending(open.span.start);
        self.enter()?;
        let args = self.parse_generic_arg_list();
        self.leave();
        let args = args?;
        let close_start = self.peek_span().start;
        self.orphan_pending(close_start);
        let close = self.expect(T::Gt, "`>`")?;
        Ok(GenericArgs {
            span: open.span.join(close.span),
            args,
        })
    }

    fn parse_generic_arg_list(&mut self) -> PResult<Vec<Type>> {
        let mut args = vec![self.parse_type()?];
        while self.eat(T::Comma).is_some() {
            if self.at(T::Gt) {
                return Err(self.error_expected(
                    "a type (a generic list takes no trailing comma, grammar 2.5)",
                ));
            }
            args.push(self.parse_type()?);
        }
        Ok(args)
    }

    /// `generic_params = "<" UPPER_IDENT ( "," UPPER_IDENT )* ">"`
    pub(crate) fn parse_generic_params(&mut self) -> PResult<GenericParams> {
        let open = self.expect(T::Lt, "`<`")?;
        self.orphan_pending(open.span.start);
        let mut params = vec![self.parse_upper_name("a type parameter")?];
        while self.eat(T::Comma).is_some() {
            if self.at(T::Gt) {
                return Err(self.error_expected(
                    "a type parameter (a generic list takes no trailing comma, grammar 2.5)",
                ));
            }
            params.push(self.parse_upper_name("a type parameter")?);
        }
        let close_start = self.peek_span().start;
        self.orphan_pending(close_start);
        let close = self.expect(T::Gt, "`>`")?;
        Ok(GenericParams {
            span: open.span.join(close.span),
            params,
        })
    }

    /// D42: at `<` after a pure name chain, whether `generic_args "("`
    /// follows. The tokens are scanned without being parsed, so the decision
    /// is made exactly as the grammar states it (`generic_args` per 3.4 and
    /// the type grammar, followed by `(`), without building anything,
    /// reporting anything (D70: nothing is reported during the speculation),
    /// or entering a nesting level. When it holds, the caller parses the
    /// generic call for real, and an argument list nested beyond
    /// [`super::MAX_DEPTH`] is then the nesting error of D73 at the token
    /// that opens the excess level, rather than a failed speculation that
    /// rereads the `<`s as comparisons (grammar D73 and D42 do not say how
    /// they interact; this is the reading under which the limit applies to
    /// the one parse the grammar admits, reported as a spec deviation). As in
    /// the parse, line breaks end the list where newlines are significant
    /// and are skipped inside brackets (D42, grammar 2.2).
    pub(crate) fn at_generic_call(&mut self) -> bool {
        self.skip_insignificant();
        let open = self.pos;
        if self.raw(open).kind != T::Lt {
            return false;
        }
        let close = match self.generic_lists.get(&open) {
            Some(&close) => close,
            None => self.scan_generic_list(open),
        };
        close.is_some_and(|gt| self.raw(self.token_after(gt)).kind == T::LParen)
    }

    /// The index of the next token after the one at `index` in the current
    /// newline mode (grammar 2.2).
    fn token_after(&self, index: usize) -> usize {
        let last = self.tokens.len() - 1;
        let mut next = (index + 1).min(last);
        if self.mode() == Mode::Insignificant {
            while self.tokens[next].kind == T::Newline && next < last {
                next += 1;
            }
        }
        next
    }

    /// Scan the generic list that the `<` at token index `open` would open
    /// and return the index of its closing `>`, or `None` when the tokens
    /// are no `generic_args`. Every `<` met on the way is recorded too: a
    /// scan from a nested `<` would follow the same tokens in the same states
    /// until its own list closes, and would fail where this one fails while
    /// that list is still open. So no token is scanned twice, and a run of
    /// comparisons such as `[A < B, C < D, ...]`, where each `<` would
    /// otherwise rescan the rest of the run, costs time linear in its length.
    fn scan_generic_list(&mut self, open: usize) -> Option<usize> {
        let mut opens = vec![open];
        let mut closed: Vec<(usize, usize)> = Vec::new();
        let mut at = open;
        let outcome = 'scan: loop {
            // `type_path = ( LOWER_IDENT "." )* UPPER_IDENT`
            loop {
                at = self.token_after(at);
                match self.tokens[at].kind {
                    T::LowerIdent => {
                        at = self.token_after(at);
                        if self.tokens[at].kind != T::Dot {
                            break 'scan None;
                        }
                    }
                    T::UpperIdent => break,
                    _ => break 'scan None,
                }
            }
            // After a type path: its own `generic_args`, then `,` or `>`.
            at = self.token_after(at);
            loop {
                match self.tokens[at].kind {
                    T::Lt => {
                        opens.push(at);
                        continue 'scan;
                    }
                    T::Comma => continue 'scan,
                    T::Gt => {
                        let lt = opens.pop().expect("an open generic list");
                        closed.push((lt, at));
                        if opens.is_empty() {
                            break 'scan Some(at);
                        }
                        // A nested list closed its type: `,` or `>` follows.
                        at = self.token_after(at);
                        if self.tokens[at].kind == T::Lt {
                            break 'scan None;
                        }
                    }
                    _ => break 'scan None,
                }
            }
        };
        for (lt, gt) in closed {
            self.generic_lists.insert(lt, Some(gt));
        }
        for lt in opens {
            self.generic_lists.insert(lt, None);
        }
        outcome
    }
}
