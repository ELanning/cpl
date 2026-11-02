//! `.work` files (grammar 4.1-4.4): changes, tasks, integration
//! requirements, and selectors. The vocabulary is contextual: the parser
//! compares the spellings of `LOWER_IDENT` tokens.

use super::{Continuation, Mode, PResult, Parser, Recover};
use crate::ast::{
    AfterClause, ChangeDecl, ChangeItem, ChangeItemKind, Facet, FacetKind, GitRef, IntLiteral,
    IntegrateDecl, Name, RequireItem, Requirement, RequirementKind, Selector, SelectorKind,
    SelectorPath, SelectorSet, TaskDecl, TaskItem, TaskItemKind, Trivia, WorkFile,
};
use crate::codes;
use crate::lexer::{Token, TokenKind as T};
use cpl_diag::{Diagnostic, Span};

impl<'a> Parser<'a> {
    /// `work_file = ( use_decl NEWLINE )* change_decl ( NEWLINE change_decl )* NEWLINE? EOF`
    ///
    /// Returns the file parsed so far; after an error the result is
    /// discarded by `finish` (D59).
    pub(crate) fn parse_work_file(&mut self) -> WorkFile {
        let mut work = WorkFile {
            span: Span::new(self.file(), 0, self.source_len()),
            uses: Vec::new(),
            changes: Vec::new(),
            trailing_comments: Vec::new(),
        };
        let _ = self.parse_work_file_into(&mut work);
        work
    }

    fn parse_work_file_into(&mut self, work: &mut WorkFile) -> PResult<()> {
        self.sequence_start = true;
        loop {
            self.skip_newlines();
            match self.peek_kind() {
                T::Eof => break,
                T::KwUse => {
                    if !work.changes.is_empty() {
                        let span = self.peek_span();
                        return Err(self.error(
                            codes::USE_AFTER_DECLARATION,
                            "use declarations must precede all change declarations",
                            span,
                        ));
                    }
                    let u = self.parse_use_decl()?;
                    work.uses.push(u);
                    self.expect_work_item_end()?;
                }
                T::KwElse => return Err(self.error_misplaced_else()),
                T::DocComment => {
                    let c = self.parse_change_decl()?;
                    work.changes.push(c);
                    self.expect_work_item_end()?;
                }
                _ if self.at_word("change") => {
                    let c = self.parse_change_decl()?;
                    work.changes.push(c);
                    self.expect_work_item_end()?;
                }
                _ => {
                    let expected = if work.changes.is_empty() {
                        "`change` (a .work file declares at least one change)"
                    } else {
                        "`change`"
                    };
                    return Err(self.error_expected(expected));
                }
            }
        }
        if work.changes.is_empty() {
            return Err(self.error_expected("`change` (a .work file declares at least one change)"));
        }
        work.trailing_comments = self.take_dangling(u32::MAX);
        Ok(())
    }

    fn expect_work_item_end(&mut self) -> PResult<()> {
        if matches!(self.peek_kind(), T::Newline | T::Eof) {
            Ok(())
        } else {
            Err(self.error_expected("end of line"))
        }
    }

    /// `change_decl = DOC_COMMENT* "change" LOWER_IDENT NEWLINE? "{" lines(change_item) "}"`
    fn parse_change_decl(&mut self) -> PResult<ChangeDecl> {
        let start = self.peek_span().start;
        let mut trivia = Trivia::default();
        self.take_leading(&mut trivia, start, true);
        let docs = self.parse_docs_before(|t| t.kind == T::LowerIdent && t.text == "change")?;
        let kw_start = self.peek_span().start;
        self.take_leading(&mut trivia, kw_start, false);
        self.expect_word("change")?;
        let name = self.parse_lower_name("a change name")?;
        if self.continuation(|t| t.kind == T::LBrace) == Continuation::None {
            return Err(self.error_expected_header("`{`"));
        }
        let mut seen: Vec<&'static str> = Vec::new();
        let mut task_names: Vec<String> = Vec::new();
        let (body_span, items, trailing_comments) = self.parse_lines("change item", |p| {
            p.parse_change_item(&mut seen, &mut task_names)
        })?;
        let span = Span::new(self.file(), start, body_span.end);
        let mut change = ChangeDecl {
            span,
            trivia,
            docs,
            name,
            items,
            trailing_comments,
        };
        self.take_trailing(&mut change.trivia, span);
        Ok(change)
    }

    /// `duplicate base`, `duplicate write set`, duplicate task name (4.1, 4.2).
    fn duplicate(&mut self, message: &str, name: &str, span: Span) -> Recover {
        self.emit(
            Diagnostic::error(codes::DUPLICATE_WORK_ITEM, codes::CATEGORY, message)
                .with_span(span)
                .with_data("name", name.into()),
        );
        Recover
    }

    /// `change_item = "base" git_ref | "target" git_ref | "workers" INT | task_decl | integrate_decl`
    fn parse_change_item(
        &mut self,
        seen: &mut Vec<&'static str>,
        task_names: &mut Vec<String>,
    ) -> PResult<ChangeItem> {
        let start = self.peek_span().start;
        let mut trivia = Trivia::default();
        self.take_leading(&mut trivia, start, true);
        let tok = self.peek().clone();
        let word = if tok.kind == T::LowerIdent || tok.kind == T::DocComment {
            tok.text.as_str()
        } else {
            ""
        };
        let mut mark_unique = |p: &mut Self, key: &'static str, span: Span| -> PResult<()> {
            if seen.contains(&key) {
                Err(p.duplicate(&format!("duplicate {key}"), key, span))
            } else {
                seen.push(key);
                Ok(())
            }
        };
        let kind = match (tok.kind, word) {
            (T::LowerIdent, "base") => {
                self.bump();
                mark_unique(self, "base", tok.span)?;
                ChangeItemKind::Base(self.parse_git_ref()?)
            }
            (T::LowerIdent, "target") => {
                self.bump();
                mark_unique(self, "target", tok.span)?;
                ChangeItemKind::Target(self.parse_git_ref()?)
            }
            (T::LowerIdent, "workers") => {
                self.bump();
                mark_unique(self, "workers", tok.span)?;
                let int = self.expect(T::Int, "an integer (the worker ceiling)")?;
                ChangeItemKind::Workers(IntLiteral {
                    span: int.span,
                    digits: int.text,
                })
            }
            (T::LowerIdent, "integrate") => {
                mark_unique(self, "integrate", tok.span)?;
                ChangeItemKind::Integrate(self.parse_integrate_decl()?)
            }
            (T::LowerIdent, "task") | (T::DocComment, _) => {
                let docs =
                    self.parse_docs_before(|t| t.kind == T::LowerIdent && t.text == "task")?;
                let after_docs = self.peek_span().start;
                self.take_leading(&mut trivia, after_docs, false);
                ChangeItemKind::Task(self.parse_task_decl(docs, task_names)?)
            }
            _ => {
                return Err(
                    self.error_expected("`base`, `target`, `workers`, `task`, or `integrate`")
                )
            }
        };
        let span = self.span_from(start);
        let mut item = ChangeItem { span, trivia, kind };
        self.take_trailing(&mut item.trivia, span);
        Ok(item)
    }

    /// `git_ref = "git" "(" TEXT ")"`
    fn parse_git_ref(&mut self) -> PResult<GitRef> {
        let kw = self.expect_word("git")?;
        let (paren_span, reference) =
            self.bracketed(T::LParen, T::RParen, Mode::Insignificant, |p| {
                p.parse_text_literal("a Git ref (text literal)")
            })?;
        Ok(GitRef {
            span: kw.span.join(paren_span),
            reference,
        })
    }

    /// `task_decl = DOC_COMMENT* "task" LOWER_IDENT after_clause? NEWLINE? "{" lines(task_item) "}"`
    /// (the doc comments have been consumed by the caller). `task_names`
    /// holds the names of the change's earlier tasks (unique, grammar 4.1).
    fn parse_task_decl(
        &mut self,
        docs: Option<crate::ast::Docs>,
        task_names: &mut Vec<String>,
    ) -> PResult<TaskDecl> {
        let start = docs
            .as_ref()
            .map(|d| d.span.start)
            .unwrap_or_else(|| self.peek_span().start);
        let kw = self.expect_word("task")?;
        let name = self.parse_lower_name("a task name")?;
        // A duplicate task name is reported at the name, before the task's
        // body is read (the earliest error, grammar D70 [D-226]).
        if task_names.contains(&name.text) {
            return Err(self.duplicate(
                &format!("duplicate task name `{}`", name.text),
                &name.text,
                name.span,
            ));
        }
        task_names.push(name.text.clone());
        let after = if self.at_word("after") {
            let after_kw = self.bump();
            let mut tasks = vec![self.parse_lower_name("a task name")?];
            while self.eat(T::Comma).is_some() {
                if self.at(T::LBrace) || self.at(T::Newline) {
                    return Err(self.error_expected(
                        "a task name (an `after` list takes no trailing comma, grammar 2.5)",
                    ));
                }
                tasks.push(self.parse_lower_name("a task name")?);
            }
            let span = after_kw.span.join(tasks[tasks.len() - 1].span);
            Some(AfterClause { span, tasks })
        } else {
            None
        };
        if self.continuation(|t| t.kind == T::LBrace) == Continuation::None {
            return Err(self.error_expected_header("`{` (or `after` followed by task names)"));
        }
        let mut seen: Vec<&'static str> = Vec::new();
        let (body_span, items, trailing_comments) =
            self.parse_lines("task item", |p| p.parse_task_item(&mut seen))?;
        let _ = kw;
        Ok(TaskDecl {
            span: Span::new(self.file(), start, body_span.end),
            docs,
            name,
            after,
            items,
            trailing_comments,
        })
    }

    /// `task_item = "write" selector_set | "add" selector_set | "depend" selector_set | "preserve" selector_set | "goal" TEXT`
    fn parse_task_item(&mut self, seen: &mut Vec<&'static str>) -> PResult<TaskItem> {
        let start = self.peek_span().start;
        let mut trivia = Trivia::default();
        self.take_leading(&mut trivia, start, true);
        let tok = self.peek().clone();
        let word = if tok.kind == T::LowerIdent {
            tok.text.as_str()
        } else {
            ""
        };
        let key: &'static str = match word {
            "write" => "write",
            "add" => "add",
            "depend" => "depend",
            "preserve" => "preserve",
            "goal" => "goal",
            _ => return Err(self.error_expected("`write`, `add`, `depend`, `preserve`, or `goal`")),
        };
        self.bump();
        if seen.contains(&key) {
            let message = if key == "goal" {
                "duplicate goal".to_string()
            } else {
                format!("duplicate {key} set")
            };
            return Err(self.duplicate(&message, key, tok.span));
        }
        seen.push(key);
        let kind = match key {
            "goal" => TaskItemKind::Goal(self.parse_text_literal("the goal (text literal)")?),
            _ => {
                let set = self.parse_selector_set(key == "add")?;
                match key {
                    "write" => TaskItemKind::Write(set),
                    "add" => TaskItemKind::Add(set),
                    "depend" => TaskItemKind::Depend(set),
                    _ => TaskItemKind::Preserve(set),
                }
            }
        };
        let span = self.span_from(start);
        let mut item = TaskItem { span, trivia, kind };
        self.take_trailing(&mut item.trivia, span);
        Ok(item)
    }

    /// `selector_set = "{" commas(selector) "}"`. In an `add` set
    /// (`namespaces_only`) every selector must be a namespace wildcard
    /// (grammar 4.2, `add names a namespace; write <path>.*`); each one is
    /// checked as soon as it is read, so that a later error in the set is
    /// never reported in its place (the earliest error, grammar D70).
    fn parse_selector_set(&mut self, namespaces_only: bool) -> PResult<SelectorSet> {
        let set = self.bracketed_group(T::LBrace, T::RBrace, Mode::Insignificant, |p| {
            p.parse_commas(T::RBrace, |p| {
                let selector = p.parse_selector()?;
                if namespaces_only && selector.kind != SelectorKind::Namespace {
                    let path = selector.path.dotted();
                    return Err(p.syntax_error(
                        &format!("add names a namespace; write {path}.*"),
                        selector.span,
                    ));
                }
                Ok(selector)
            })
        })?;
        Ok(SelectorSet {
            span: set.span,
            selectors: set.inner,
            group: set.group,
            trailing_comments: set.trailing_comments,
        })
    }

    /// `integrate_decl = "integrate" NEWLINE? "{" lines(require_item) "}"`
    fn parse_integrate_decl(&mut self) -> PResult<IntegrateDecl> {
        let kw = self.expect_word("integrate")?;
        if self.continuation(|t| t.kind == T::LBrace) == Continuation::None {
            return Err(self.error_expected_header("`{`"));
        }
        let (body_span, requirements, trailing_comments) =
            self.parse_lines("requirement", |p| p.parse_require_item())?;
        Ok(IntegrateDecl {
            span: kw.span.join(body_span),
            requirements,
            trailing_comments,
        })
    }

    /// `require_item = "require" requirement`
    fn parse_require_item(&mut self) -> PResult<RequireItem> {
        let start = self.peek_span().start;
        let mut trivia = Trivia::default();
        self.take_leading(&mut trivia, start, true);
        let kw = self.expect_word("require")?;
        let requirement = self.parse_requirement()?;
        let span = kw.span.join(requirement.span);
        let mut item = RequireItem {
            span,
            trivia,
            requirement,
        };
        self.take_trailing(&mut item.trivia, span);
        Ok(item)
    }

    /// `requirement = "typecheck" | "no_holes" | "no_unresolved_assumptions" | "prove" "(" selector ")" | "test" "(" selector ")" | "review" "(" LOWER_IDENT ")"`
    fn parse_requirement(&mut self) -> PResult<Requirement> {
        let tok = self.peek().clone();
        let word = match tok.kind {
            T::LowerIdent => tok.text.as_str(),
            T::KwTest => "test",
            // No requirement at all (the line ends), or a reserved word (D4):
            // the generic diagnostics, not `unknown_requirement`.
            T::Newline | T::RBrace | T::Eof | T::ReservedWord => {
                return Err(self.error_expected(
                    "a requirement (typecheck, no_holes, no_unresolved_assumptions, prove(selector), test(selector), or review(task))",
                ))
            }
            // Anything else written where a requirement goes is an unknown
            // requirement (D36).
            _ => "",
        };
        let unknown = |p: &mut Self, tok: &Token| -> Recover {
            let found = tok.describe();
            p.emit(
                Diagnostic::error(
                    codes::UNKNOWN_REQUIREMENT,
                    codes::CATEGORY,
                    format!(
                        "unknown requirement {found}; expected typecheck, no_holes, no_unresolved_assumptions, prove(selector), test(selector), or review(task)"
                    ),
                )
                .with_span(tok.span)
                .with_data("found", found.into()),
            );
            Recover
        };
        let kind = match word {
            "typecheck" => {
                self.bump();
                RequirementKind::Typecheck
            }
            "no_holes" => {
                self.bump();
                RequirementKind::NoHoles
            }
            "no_unresolved_assumptions" => {
                self.bump();
                RequirementKind::NoUnresolvedAssumptions
            }
            "prove" => {
                self.bump();
                let (_, selector) =
                    self.bracketed(T::LParen, T::RParen, Mode::Insignificant, |p| {
                        p.parse_selector()
                    })?;
                RequirementKind::Prove(selector)
            }
            "test" => {
                self.bump();
                let (_, selector) =
                    self.bracketed(T::LParen, T::RParen, Mode::Insignificant, |p| {
                        p.parse_selector()
                    })?;
                RequirementKind::Test(selector)
            }
            "review" => {
                self.bump();
                let (_, task) = self.bracketed(T::LParen, T::RParen, Mode::Insignificant, |p| {
                    p.parse_lower_name("a task name")
                })?;
                RequirementKind::Review(task)
            }
            _ => return Err(unknown(self, &tok)),
        };
        Ok(Requirement {
            span: self.span_from(tok.span.start),
            kind,
        })
    }

    /// `selector = selector_path "." "*" | selector_path "." facet | selector_path`
    pub(crate) fn parse_selector(&mut self) -> PResult<Selector> {
        let start = self.peek_span().start;
        let mut trivia = Trivia::default();
        self.take_leading(&mut trivia, start, false);
        let mut segments: Vec<Name> = Vec::new();
        let first = match self.peek_kind() {
            T::LowerIdent | T::UpperIdent => {
                let tok = self.bump();
                self.name_from(&tok)
            }
            _ => {
                return Err(self.error_expected(
                    "a selector (a dotted path, optionally ending in `.*` or a facet)",
                ))
            }
        };
        segments.push(first);
        let mut kind = SelectorKind::Declaration;
        let mut end = segments[0].span;
        while self.at(T::Dot) {
            self.bump();
            match self.peek_kind() {
                T::LowerIdent | T::UpperIdent => {
                    let tok = self.bump();
                    end = tok.span;
                    segments.push(self.name_from(&tok));
                }
                T::Star => {
                    let star = self.bump();
                    end = star.span;
                    kind = SelectorKind::Namespace;
                    if self.at(T::Dot) {
                        return Err(self.error_expected(
                            "the end of the selector (`*` must be its last segment, grammar 4.4)",
                        ));
                    }
                    break;
                }
                T::KwEffects => {
                    let kw = self.bump();
                    end = kw.span;
                    kind = SelectorKind::Facet(Facet {
                        span: kw.span,
                        kind: FacetKind::Effects,
                    });
                    if self.at(T::Dot) {
                        return Err(self.error_expected(
                            "the end of the selector (a facet must be its last segment, grammar 4.4)",
                        ));
                    }
                    break;
                }
                _ => {
                    return Err(
                        self.error_expected("a selector segment, `*`, or a facet name after `.`")
                    )
                }
            }
        }
        if kind == SelectorKind::Declaration && segments.len() >= 2 {
            let last = &segments[segments.len() - 1];
            if let Some(facet) = FacetKind::parse(&last.text) {
                let facet_span = last.span;
                segments.pop();
                kind = SelectorKind::Facet(Facet {
                    span: facet_span,
                    kind: facet,
                });
            }
        }
        let path_span = segments[0].span.join(segments[segments.len() - 1].span);
        let span = Span::new(self.file(), start, end.end);
        let mut selector = Selector {
            span,
            trivia,
            path: SelectorPath {
                span: path_span,
                segments,
            },
            kind,
        };
        self.take_trailing(&mut selector.trivia, span);
        Ok(selector)
    }
}
