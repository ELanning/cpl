//! `.patch` files (grammar 4.5).

use super::{Continuation, Mode, PResult, Parser};
use crate::ast::{ExpectClause, PatchFile, PatchForm, Trivia};
use crate::lexer::TokenKind as T;
use cpl_diag::Span;

impl<'a> Parser<'a> {
    /// `patch_file = patch_form ( NEWLINE patch_form )* NEWLINE? EOF`
    ///
    /// Returns the forms parsed so far; after an error the result is
    /// discarded by `finish` (D59).
    pub(crate) fn parse_patch_file(&mut self) -> PatchFile {
        let mut patch = PatchFile {
            span: Span::new(self.file(), 0, self.source_len()),
            forms: Vec::new(),
            trailing_comments: Vec::new(),
        };
        let _ = self.parse_patch_file_into(&mut patch);
        patch
    }

    fn parse_patch_file_into(&mut self, patch: &mut PatchFile) -> PResult<()> {
        self.sequence_start = true;
        loop {
            self.skip_newlines();
            match self.peek_kind() {
                T::Eof => break,
                T::DocComment => {}
                T::KwElse => return Err(self.error_misplaced_else()),
                _ if self.at_word("patch") => {}
                _ => {
                    let expected = if patch.forms.is_empty() {
                        "`patch` (a .patch file contains at least one patch form)"
                    } else {
                        "`patch`"
                    };
                    return Err(self.error_expected(expected));
                }
            }
            let form = self.parse_patch_form()?;
            patch.forms.push(form);
            if !matches!(self.peek_kind(), T::Newline | T::Eof) {
                return Err(self.error_expected("end of line after the patch form"));
            }
        }
        if patch.forms.is_empty() {
            return Err(
                self.error_expected("`patch` (a .patch file contains at least one patch form)")
            );
        }
        patch.trailing_comments = self.take_dangling(u32::MAX);
        Ok(())
    }

    /// `patch_form = DOC_COMMENT* "patch" selector NEWLINE "expect" "base" "(" selector ")" NEWLINE block`
    fn parse_patch_form(&mut self) -> PResult<PatchForm> {
        let start = self.peek_span().start;
        let mut trivia = Trivia::default();
        self.take_leading(&mut trivia, start, true);
        let docs = self.parse_docs_before(|t| t.kind == T::LowerIdent && t.text == "patch")?;
        let kw_start = self.peek_span().start;
        self.take_leading(&mut trivia, kw_start, false);
        self.expect_word("patch")?;
        let target = self.parse_selector()?;

        match self.continuation(|t| t.kind == T::LowerIdent && t.text == "expect") {
            Continuation::None => {
                return Err(self.error_expected_header("`expect` on its own line"))
            }
            Continuation::SameLine => {
                return Err(self
                    .error_expected("end of line (`expect` must start its own line, grammar 4.5)"))
            }
            Continuation::AfterNewline => {}
        }
        let expect_start = self.peek_span().start;
        let mut expect_trivia = Trivia::default();
        self.take_leading(&mut expect_trivia, expect_start, true);
        let expect_kw = self.bump();
        self.expect_word("base")?;
        let (paren_span, base) =
            self.bracketed(T::LParen, T::RParen, Mode::Insignificant, |p| {
                p.parse_selector()
            })?;
        let expect_span = expect_kw.span.join(paren_span);
        let mut expect = ExpectClause {
            span: expect_span,
            trivia: expect_trivia,
            base,
        };
        self.take_trailing(&mut expect.trivia, expect_span);

        match self.continuation(|t| t.kind == T::LBrace) {
            Continuation::None => return Err(self.error_expected_header("`{` on its own line")),
            Continuation::SameLine => return Err(self.error_expected(
                "end of line (the block's `{` must start a new line after `expect`, grammar 4.5)",
            )),
            Continuation::AfterNewline => {}
        }
        let body = self.parse_block()?;
        let span = Span::new(self.file(), start, body.span.end);
        let mut form = PatchForm {
            span,
            trivia,
            docs,
            target,
            expect,
            body,
        };
        self.take_trailing(&mut form.trivia, span);
        Ok(form)
    }
}
