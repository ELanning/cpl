//! Patterns (grammar 3.13).

use super::{Mode, PResult, Parser};
use crate::ast::{CtorPath, CtorPattern, Group, Name, Pattern, PatternKind};
use crate::lexer::TokenKind as T;

impl<'a> Parser<'a> {
    /// `pattern = "_" | "-"? INT | TEXT | "true" | "false" | LOWER_IDENT | ctor_path ( "(" commas1(pattern) ")" )?`
    pub(crate) fn parse_pattern(&mut self) -> PResult<Pattern> {
        let tok = self.peek().clone();
        match tok.kind {
            T::Underscore => {
                self.bump();
                Ok(Pattern {
                    span: tok.span,
                    kind: PatternKind::Wildcard,
                })
            }
            T::Minus => {
                self.bump();
                let int = self.expect(T::Int, "an integer literal after `-`")?;
                Ok(Pattern {
                    span: tok.span.join(int.span),
                    kind: PatternKind::Int {
                        negative: true,
                        digits: int.text,
                    },
                })
            }
            T::Int => {
                self.bump();
                Ok(Pattern {
                    span: tok.span,
                    kind: PatternKind::Int {
                        negative: false,
                        digits: tok.text,
                    },
                })
            }
            T::Text => {
                self.bump();
                Ok(Pattern {
                    span: tok.span,
                    kind: PatternKind::Text(tok.text),
                })
            }
            T::KwTrue | T::KwFalse => {
                self.bump();
                Ok(Pattern {
                    span: tok.span,
                    kind: PatternKind::Bool(tok.kind == T::KwTrue),
                })
            }
            T::LowerIdent if self.nth_kind(1) != T::Dot => {
                self.bump();
                Ok(Pattern {
                    span: tok.span,
                    kind: PatternKind::Binding(self.name_from(&tok)),
                })
            }
            T::LowerIdent | T::UpperIdent => self.parse_ctor_pattern(),
            _ => Err(self.error_expected("a pattern")),
        }
    }

    fn parse_ctor_pattern(&mut self) -> PResult<Pattern> {
        let path = self.parse_ctor_path()?;
        let mut span = path.span;
        let mut group = Group::default();
        let args = if self.at(T::LParen) {
            let args = self.bracketed_full(T::LParen, T::RParen, Mode::Insignificant, |p| {
                if p.at(T::RParen) {
                    // `commas1(pattern)`: `Ctor()` is a syntax error (3.13).
                    return Err(p.error_expected(
                        "a pattern (a constructor without payload omits the parentheses)",
                    ));
                }
                p.parse_commas(T::RParen, |p| p.parse_pattern())
            })?;
            span = span.join(args.span);
            group = args.group;
            Some(args.inner)
        } else {
            None
        };
        Ok(Pattern {
            span,
            kind: PatternKind::Ctor(CtorPattern { path, args, group }),
        })
    }

    /// `ctor_path = ( LOWER_IDENT "." )* UPPER_IDENT ( "." UPPER_IDENT )*`
    pub(crate) fn parse_ctor_path(&mut self) -> PResult<CtorPath> {
        let mut qualifier: Vec<Name> = Vec::new();
        while self.at(T::LowerIdent) && self.nth_kind(1) == T::Dot {
            let tok = self.bump();
            qualifier.push(self.name_from(&tok));
            self.bump();
        }
        let mut name = self.parse_upper_name("a constructor name")?;
        while self.at(T::Dot) {
            if self.nth_kind(1) == T::UpperIdent {
                self.bump();
                let tok = self.bump();
                qualifier.push(name);
                name = self.name_from(&tok);
            } else {
                self.bump();
                return Err(self.error_expected("a PascalCase constructor name after `.`"));
            }
        }
        let start = qualifier.first().map(|q| q.span).unwrap_or(name.span);
        Ok(CtorPath {
            span: start.join(name.span),
            qualifier,
            name,
        })
    }
}
