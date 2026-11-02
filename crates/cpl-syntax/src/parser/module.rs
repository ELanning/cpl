//! `.cpl` files: module header, imports, and declarations (grammar 3.1-3.10).

use super::{docs_from, Continuation, Mode, PResult, Parser, Recover};
use crate::ast::{
    Annotation, CapabilityDecl, Comment, ContractClause, ContractKind, DeclKind, Declaration,
    DecreasesClause, DocLine, EffectRef, EffectsClause, EnumDecl, FieldDecl, FnClause, FnDecl,
    ForClause, FunctionMode, Group, Module, ModuleHeader, ModulePath, Name, OpDecl, OpaqueDecl,
    Param, Params, PropertyDecl, RefinementDecl, StructDecl, TestDecl, Trivia, TypeDecl,
    TypeDeclKind, UseDecl, UseItem, UsePath, UsingParam, VariantDecl, WrapperDecl,
};
use crate::codes;
use crate::lexer::{Token, TokenKind as T};
use cpl_diag::{Diagnostic, Span};

fn is_decl_start(kind: T) -> bool {
    matches!(
        kind,
        T::At
            | T::DocComment
            | T::KwPub
            | T::KwType
            | T::KwStruct
            | T::KwEnum
            | T::KwCapability
            | T::KwFn
            | T::KwTest
            | T::KwProperty
    )
}

fn is_contextual_decl(tok: &Token) -> bool {
    tok.kind == T::LowerIdent
        && matches!(
            tok.text.as_str(),
            "pure" | "mutate" | "machine" | "initial" | "program" | "interface"
        )
}

fn is_fn_continuation(tok: &Token) -> bool {
    matches!(
        tok.kind,
        T::LBrace | T::KwEffects | T::KwRequires | T::KwEnsures | T::KwDecreases
    )
}

fn is_op_continuation(tok: &Token) -> bool {
    matches!(tok.kind, T::KwRequires | T::KwEnsures | T::KwEffects)
}

impl<'a> Parser<'a> {
    /// `source_file = DOC_COMMENT* module_header ( NEWLINE use_decl )* ( NEWLINE declaration )* NEWLINE? EOF`
    ///
    /// Returns the module parsed so far; after an error the result is
    /// discarded by `finish` (D59).
    pub(crate) fn parse_source_file(&mut self) -> Module {
        let file = self.file();
        let mut module = Module {
            span: Span::new(file, 0, self.source_len()),
            trivia: Trivia::default(),
            docs: None,
            header: ModuleHeader {
                span: Span::point(file, 0),
                path: ModulePath {
                    span: Span::point(file, 0),
                    segments: Vec::new(),
                },
            },
            uses: Vec::new(),
            declarations: Vec::new(),
            trailing_comments: Vec::new(),
        };
        let _ = self.parse_source_file_into(&mut module);
        module
    }

    fn parse_source_file_into(&mut self, module: &mut Module) -> PResult<()> {
        self.skip_newlines();
        let first = self.peek_span().start;
        self.take_leading(&mut module.trivia, first, false);
        // Grammar 1.2: doc comments before the header document the module, so
        // ones followed by anything else (another token, the end of the file)
        // are `doc comment must precede a declaration`, reported at the doc
        // comment, as in the other admitted positions.
        module.docs = self.parse_docs_before(|t| t.kind == T::KwModule)?;
        let header_start = self.peek_span().start;
        self.take_leading(&mut module.trivia, header_start, false);
        module.header = self.parse_module_header()?;
        self.take_trailing(&mut module.trivia, module.header.span);
        self.expect_item_end("module header")?;

        loop {
            self.skip_newlines();
            let kind = self.peek_kind();
            match kind {
                T::Eof => break,
                T::KwUse => {
                    if !module.declarations.is_empty() {
                        let span = self.peek_span();
                        return Err(self.error(
                            codes::USE_AFTER_DECLARATION,
                            "use declarations must precede all other declarations",
                            span,
                        ));
                    }
                    let u = self.parse_use_decl()?;
                    module.uses.push(u);
                    self.expect_item_end("use declaration")?;
                }
                k if is_decl_start(k) || is_contextual_decl(self.peek()) => {
                    let d = self.parse_declaration()?;
                    module.declarations.push(d);
                    self.expect_item_end("declaration")?;
                }
                T::KwElse => return Err(self.error_misplaced_else()),
                _ => {
                    return Err(self.error_expected(
                        "a declaration (`type`, `struct`, `enum`, `capability`, `fn`, `test`, or `property`)",
                    ))
                }
            }
        }
        module.trailing_comments = self.take_dangling(u32::MAX);
        Ok(())
    }

    fn expect_item_end(&mut self, what: &str) -> PResult<()> {
        if matches!(self.peek_kind(), T::Newline | T::Eof) {
            Ok(())
        } else {
            Err(self.error_expected(&format!("end of line after the {what}")))
        }
    }

    /// `module_header = "module" module_path`
    fn parse_module_header(&mut self) -> PResult<ModuleHeader> {
        let kw = self.expect(
            T::KwModule,
            "`module` (every .cpl file starts with its module header)",
        )?;
        let path = self.parse_module_path()?;
        Ok(ModuleHeader {
            span: kw.span.join(path.span),
            path,
        })
    }

    /// `module_path = LOWER_IDENT ( "." LOWER_IDENT )*`
    fn parse_module_path(&mut self) -> PResult<ModulePath> {
        let mut segments = vec![self.parse_lower_name("a module path segment")?];
        while self.at(T::Dot) {
            self.bump();
            segments.push(self.parse_lower_name("a module path segment")?);
        }
        let span = segments[0].span.join(segments[segments.len() - 1].span);
        Ok(ModulePath { span, segments })
    }

    /// `use_decl = "use" use_path "." "{" commas1(use_item) "}" | "use" use_path "." use_item`
    pub(crate) fn parse_use_decl(&mut self) -> PResult<UseDecl> {
        let start = self.peek_span().start;
        let mut trivia = Trivia::default();
        self.take_leading(&mut trivia, start, true);
        let kw = self.expect(T::KwUse, "`use`")?;
        let mut segments: Vec<Name> = vec![self.parse_lower_name("a module path")?];
        let mut braced = false;
        let mut items: Vec<UseItem> = Vec::new();
        let mut group = Group::default();
        let mut trailing_comments: Vec<Comment> = Vec::new();
        let mut end = segments[0].span;
        loop {
            if !self.at(T::Dot) {
                break;
            }
            // The segment before a `.` belongs to the use path, whose segments
            // are module names (grammar 3.2, `use_path = LOWER_IDENT ( "."
            // LOWER_IDENT )*`). Checking it here, before anything after the
            // `.` is read, reports an upper-case segment ahead of every later
            // error, as the earliest error rule requires (grammar D70
            // [D-226]): `use a.B.{}` is reported at `B`, not at the `}` of
            // the empty list.
            if let Some(bad) = segments.last().filter(|s| s.is_upper()) {
                let found = format!("identifier `{}`", bad.text);
                let span = bad.span;
                return Err(self.error_expected_found(
                    "a module name (snake_case identifier; a use path is made of module names)",
                    &found,
                    span,
                ));
            }
            if self.nth_kind(1) == T::LBrace {
                self.bump();
                braced = true;
                let list =
                    self.bracketed_group(T::LBrace, T::RBrace, Mode::Insignificant, |p| {
                        if p.at(T::RBrace) {
                            return Err(p.error_expected(
                                "an imported name (a use list names at least one item)",
                            ));
                        }
                        p.parse_commas(T::RBrace, |p| p.parse_use_item())
                    })?;
                items = list.inner;
                group = list.group;
                trailing_comments = list.trailing_comments;
                end = list.span;
                break;
            }
            self.bump();
            match self.peek_kind() {
                T::LowerIdent | T::UpperIdent => {
                    let tok = self.bump();
                    end = tok.span;
                    segments.push(self.name_from(&tok));
                }
                _ => return Err(self.error_expected("an imported name or `{`")),
            }
        }
        if !braced {
            if segments.len() < 2 {
                return Err(self.error_expected(
                    "`.` (a use declaration names a module and an item: `use a.b.c` or `use a.b.{c}`)",
                ));
            }
            let item = segments.pop().expect("at least two segments");
            items.push(UseItem {
                span: item.span,
                trivia: Trivia::default(),
                name: item,
            });
        }
        // Every segment left in `segments` was followed by a `.` and checked
        // in the loop above.
        let path_span = segments[0].span.join(segments[segments.len() - 1].span);
        let span = kw.span.join(end);
        let mut decl = UseDecl {
            span,
            trivia,
            path: UsePath {
                span: path_span,
                segments,
            },
            braced,
            items,
            group,
            trailing_comments,
        };
        self.take_trailing(&mut decl.trivia, span);
        Ok(decl)
    }

    fn parse_use_item(&mut self) -> PResult<UseItem> {
        let start = self.peek_span().start;
        let mut trivia = Trivia::default();
        self.take_leading(&mut trivia, start, false);
        let name = match self.peek_kind() {
            T::LowerIdent | T::UpperIdent => {
                let tok = self.bump();
                self.name_from(&tok)
            }
            _ => return Err(self.error_expected("an imported name")),
        };
        let mut item = UseItem {
            span: name.span,
            trivia,
            name,
        };
        self.take_trailing(&mut item.trivia, item.span);
        Ok(item)
    }

    /// `declaration = decl_prefix ( type_decl | struct_decl | enum_decl | capability_decl | fn_decl | test_decl | property_decl )`
    pub(crate) fn parse_declaration(&mut self) -> PResult<Declaration> {
        let start = self.peek_span().start;
        let mut trivia = Trivia::default();
        self.take_leading(&mut trivia, start, true);
        let mut doc_lines: Vec<DocLine> = Vec::new();
        let mut annotations: Vec<Annotation> = Vec::new();
        loop {
            match self.peek_kind() {
                T::DocComment => {
                    let tok = self.bump();
                    doc_lines.push(DocLine {
                        span: tok.span,
                        text: tok.text,
                    });
                }
                T::At => annotations.push(self.parse_annotation(&mut trivia)?),
                _ => break,
            }
        }
        let keyword_start = self.peek_span().start;
        self.take_leading(&mut trivia, keyword_start, false);
        let starts_declaration = is_contextual_decl(self.peek())
            || matches!(
                self.peek_kind(),
                T::KwPub
                    | T::KwType
                    | T::KwStruct
                    | T::KwEnum
                    | T::KwCapability
                    | T::KwFn
                    | T::KwTest
                    | T::KwProperty
            );
        let docs = docs_from(doc_lines);
        if !starts_declaration {
            if let Some(docs) = &docs {
                return Err(self.error(
                    codes::DOC_COMMENT_POSITION,
                    "doc comment must precede a declaration",
                    docs.span,
                ));
            }
        }
        let is_pub = self.eat(T::KwPub).is_some();
        let kind = match self.peek_kind() {
            T::LowerIdent if self.at_word("program") => {
                DeclKind::Struct(self.parse_struct_decl(is_pub, keyword_start)?)
            }
            T::LowerIdent if self.at_word("interface") => {
                DeclKind::Capability(self.parse_capability_decl(is_pub, keyword_start)?)
            }
            T::LowerIdent if is_contextual_decl(self.peek()) => {
                DeclKind::Fn(self.parse_fn_decl(is_pub, keyword_start)?)
            }
            T::KwType => DeclKind::Type(self.parse_type_decl(is_pub, keyword_start)?),
            T::KwStruct => DeclKind::Struct(self.parse_struct_decl(is_pub, keyword_start)?),
            T::KwEnum => DeclKind::Enum(self.parse_enum_decl(is_pub, keyword_start)?),
            T::KwCapability => DeclKind::Capability(self.parse_capability_decl(is_pub, keyword_start)?),
            T::KwFn => DeclKind::Fn(self.parse_fn_decl(is_pub, keyword_start)?),
            T::KwTest | T::KwProperty => {
                if is_pub {
                    return Err(self.error_expected(
                        "`type`, `struct`, `enum`, `capability`, or `fn` after `pub` (test and property declarations take no `pub`, D67)",
                    ));
                }
                if self.at(T::KwTest) {
                    DeclKind::Test(self.parse_test_decl()?)
                } else {
                    DeclKind::Property(self.parse_property_decl()?)
                }
            }
            _ => {
                return Err(self.error_expected(
                    "a declaration (`type`, `struct`, `enum`, `capability`, `fn`, `test`, or `property`)",
                ))
            }
        };
        let span = self.span_from(start);
        let mut decl = Declaration {
            span,
            trivia,
            docs,
            annotations,
            kind,
        };
        self.take_trailing(&mut decl.trivia, span);
        Ok(decl)
    }

    /// `annotation = "@" ( LOWER_IDENT | keyword ) ( "(" commas(literal) ")" )? NEWLINE`
    ///
    /// The name is a `LOWER_IDENT` or any of the 33 reserved keywords
    /// (grammar D71: `@property` in the standard library's stubs); a
    /// reserved-future word is `reserved_word` (D4), any other token the
    /// generic syntax error. Comments above the annotation are leading
    /// trivia of the declaration (`decl_trivia`); a comment on the
    /// annotation's own line is its trailing trivia.
    fn parse_annotation(&mut self, decl_trivia: &mut Trivia) -> PResult<Annotation> {
        let at_start = self.peek_span().start;
        self.take_leading(decl_trivia, at_start, false);
        let at = self.expect(T::At, "`@`")?;
        let name = match self.peek_kind() {
            T::LowerIdent => {
                let tok = self.bump();
                self.name_from(&tok)
            }
            kind if kind.is_keyword() => {
                let tok = self.bump();
                self.name_from(&tok)
            }
            _ => {
                return Err(
                    self.error_expected("an annotation name (snake_case identifier or keyword)")
                )
            }
        };
        let mut span = at.span.join(name.span);
        let mut args = Vec::new();
        let mut group = Group::default();
        let parens = self.at(T::LParen);
        if parens {
            let list = self.bracketed_full(T::LParen, T::RParen, Mode::Insignificant, |p| {
                p.parse_commas(T::RParen, |p| p.parse_literal())
            })?;
            args = list.inner;
            group = list.group;
            span = span.join(list.span);
        }
        let mut annotation = Annotation {
            span,
            trivia: Trivia::default(),
            name,
            args,
            group,
            parens,
        };
        self.take_trailing(&mut annotation.trivia, span);
        match self.peek_kind() {
            T::Newline => {
                self.bump();
            }
            T::DocComment | T::Eof => {}
            _ => {
                return Err(self.error_expected(
                    "end of line after the annotation (an annotation occupies its own line)",
                ))
            }
        }
        Ok(annotation)
    }

    /// `type_decl = "pub"? "type" UPPER_IDENT ( wrapper_decl | refinement_decl | opaque_decl )`
    fn parse_type_decl(&mut self, is_pub: bool, start: u32) -> PResult<TypeDecl> {
        self.expect(T::KwType, "`type`")?;
        let name = self.parse_upper_name("a type name")?;
        let kind = match self.peek_kind() {
            T::LParen => {
                let (span, payload) =
                    self.bracketed(T::LParen, T::RParen, Mode::Insignificant, |p| {
                        p.parse_type()
                    })?;
                TypeDeclKind::Wrapper(WrapperDecl { span, payload })
            }
            T::Eq => {
                let eq = self.bump();
                let base = self.parse_type()?;
                if !self.at(T::KwWhere) {
                    let span = self.peek_span();
                    return Err(self.syntax_error(
                        "a type declaration is a wrapper, type A(T), or a refinement, type A = T where ...",
                        span,
                    ));
                }
                self.bump();
                let predicate = self.parse_expr()?;
                TypeDeclKind::Refinement(RefinementDecl {
                    span: eq.span.join(predicate.span),
                    base,
                    predicate,
                })
            }
            // `opaque_decl = generic_params?`: a definition-less type (3.5);
            // `cpl check` reports `missing_body` outside the standard library.
            T::Lt => {
                let generic_params = self.parse_generic_params()?;
                TypeDeclKind::Opaque(OpaqueDecl {
                    span: generic_params.span,
                    generic_params: Some(generic_params),
                })
            }
            _ => TypeDeclKind::Opaque(OpaqueDecl {
                span: Span::point(self.file(), name.span.end),
                generic_params: None,
            }),
        };
        Ok(TypeDecl {
            span: self.span_from(start),
            is_pub,
            name,
            kind,
        })
    }

    /// `struct_decl = "pub"? "struct" UPPER_IDENT generic_params? NEWLINE? "{" lines(field_decl) "}"`
    fn parse_struct_decl(&mut self, is_pub: bool, start: u32) -> PResult<StructDecl> {
        let is_program = self.at_word("program");
        if is_program {
            self.bump();
        } else {
            self.expect(T::KwStruct, "`struct`")?;
        }
        let name = self.parse_upper_name("a struct name")?;
        let generic_params = if self.at(T::Lt) {
            Some(self.parse_generic_params()?)
        } else {
            None
        };
        if self.continuation(|t| t.kind == T::LBrace) == Continuation::None {
            return Err(self.error_expected_header("`{`"));
        }
        let (body_span, fields, trailing_comments) =
            self.parse_lines("field", |p| p.parse_field_decl())?;
        let invariant = if self.continuation(|t| t.kind == T::KwWhere) != Continuation::None {
            let mut trivia = Trivia::default();
            let start = self.peek_span().start;
            self.take_leading(&mut trivia, start, true);
            let kw = self.bump();
            let expr = self.parse_expr()?;
            let span = kw.span.join(expr.span);
            self.take_trailing(&mut trivia, span);
            Some(crate::ast::InvariantClause { span, trivia, expr })
        } else {
            None
        };
        Ok(StructDecl {
            span: Span::new(
                self.file(),
                start,
                invariant.as_ref().map_or(body_span.end, |i| i.span.end),
            ),
            is_pub,
            is_program,
            name,
            generic_params,
            fields,
            invariant,
            trailing_comments,
        })
    }

    /// `field_decl = DOC_COMMENT* LOWER_IDENT ":" type`
    fn parse_field_decl(&mut self) -> PResult<FieldDecl> {
        let start = self.peek_span().start;
        let mut trivia = Trivia::default();
        self.take_leading(&mut trivia, start, true);
        let docs = self.parse_docs_before(|t| t.kind == T::LowerIdent)?;
        let after_docs = self.peek_span().start;
        self.take_leading(&mut trivia, after_docs, false);
        let name = self.parse_lower_name("a field name")?;
        self.expect(T::Colon, "`:`")?;
        let ty = self.parse_type()?;
        let span = Span::new(self.file(), start, ty.span.end);
        let mut field = FieldDecl {
            span,
            trivia,
            docs,
            name,
            ty,
        };
        self.take_trailing(&mut field.trivia, span);
        Ok(field)
    }

    /// `enum_decl = "pub"? "enum" UPPER_IDENT generic_params? NEWLINE? "{" lines(variant_decl) "}"`
    fn parse_enum_decl(&mut self, is_pub: bool, start: u32) -> PResult<EnumDecl> {
        self.expect(T::KwEnum, "`enum`")?;
        let name = self.parse_upper_name("an enum name")?;
        let generic_params = if self.at(T::Lt) {
            Some(self.parse_generic_params()?)
        } else {
            None
        };
        if self.continuation(|t| t.kind == T::LBrace) == Continuation::None {
            return Err(self.error_expected_header("`{`"));
        }
        let (body_span, variants, trailing_comments) =
            self.parse_lines("variant", |p| p.parse_variant_decl())?;
        Ok(EnumDecl {
            span: Span::new(self.file(), start, body_span.end),
            is_pub,
            name,
            generic_params,
            variants,
            trailing_comments,
        })
    }

    /// `variant_decl = DOC_COMMENT* UPPER_IDENT ( "(" commas1(type) ")" )?`
    fn parse_variant_decl(&mut self) -> PResult<VariantDecl> {
        let start = self.peek_span().start;
        let mut trivia = Trivia::default();
        self.take_leading(&mut trivia, start, true);
        let docs = self.parse_docs_before(|t| t.kind == T::UpperIdent)?;
        let after_docs = self.peek_span().start;
        self.take_leading(&mut trivia, after_docs, false);
        let name = self.parse_upper_name("a variant name")?;
        let mut end = name.span.end;
        let mut payload = Vec::new();
        let mut group = Group::default();
        if self.at(T::LParen) {
            let types = self.bracketed_full(T::LParen, T::RParen, Mode::Insignificant, |p| {
                if p.at(T::RParen) {
                    // `Variant()` is a syntax error (3.7).
                    return Err(p.error_expected(
                        "a payload type (a variant without payload omits the parentheses)",
                    ));
                }
                p.parse_commas(T::RParen, |p| p.parse_type())
            })?;
            payload = types.inner;
            group = types.group;
            end = types.span.end;
        }
        let span = Span::new(self.file(), start, end);
        let mut variant = VariantDecl {
            span,
            trivia,
            docs,
            name,
            payload,
            group,
        };
        self.take_trailing(&mut variant.trivia, span);
        Ok(variant)
    }

    /// `capability_decl = "pub"? "capability" UPPER_IDENT NEWLINE? "{" lines(op_decl) "}"`
    fn parse_capability_decl(&mut self, is_pub: bool, start: u32) -> PResult<CapabilityDecl> {
        let is_interface = self.at_word("interface");
        if is_interface {
            self.bump();
        } else {
            self.expect(T::KwCapability, "`capability`")?;
        }
        let name = self.parse_upper_name("a capability name")?;
        if self.continuation(|t| t.kind == T::LBrace) == Continuation::None {
            return Err(self.error_expected_header("`{`"));
        }
        let (body_span, operations, trailing_comments) =
            self.parse_lines("operation", |p| p.parse_op_decl())?;
        Ok(CapabilityDecl {
            span: Span::new(self.file(), start, body_span.end),
            is_pub,
            is_interface,
            name,
            operations,
            trailing_comments,
        })
    }

    /// `op_decl = DOC_COMMENT* "fn" LOWER_IDENT "(" commas(param) ")" "->" type ( NEWLINE contract_clause )*`
    fn parse_op_decl(&mut self) -> PResult<OpDecl> {
        let start = self.peek_span().start;
        let mut trivia = Trivia::default();
        self.take_leading(&mut trivia, start, true);
        let docs = self.parse_docs_before(|t| matches!(t.kind, T::KwFn | T::KwPub))?;
        let after_docs = self.peek_span().start;
        self.take_leading(&mut trivia, after_docs, false);
        if self.at(T::KwPub) {
            return Err(self.error_expected("`fn` (capability operations take no `pub`)"));
        }
        self.expect(T::KwFn, "`fn`")?;
        let name = self.parse_lower_name("an operation name")?;
        let params = self.parse_params(false)?;
        self.expect(T::Arrow, "`->`")?;
        let ret = self.parse_type()?;
        let mut clauses: Vec<ContractClause> = Vec::new();
        let mut end = ret.span.end;
        loop {
            match self.continuation(is_op_continuation) {
                Continuation::None => break,
                Continuation::SameLine => {
                    return Err(self.error_expected(
                        "end of line (a clause must start its own line, grammar 2.4 rule 1)",
                    ))
                }
                Continuation::AfterNewline => {
                    if self.at(T::KwEffects) {
                        return Err(self.error_expected(
                            "`requires` or `ensures` (capability operations declare no effects clause: the operation is itself the effect)",
                        ));
                    }
                    let clause = self.parse_contract_clause()?;
                    end = clause.span.end;
                    clauses.push(clause);
                }
            }
        }
        let span = Span::new(self.file(), start, end);
        let mut op = OpDecl {
            span,
            trivia,
            docs,
            name,
            params,
            ret,
            clauses,
        };
        self.take_trailing(&mut op.trivia, span);
        Ok(op)
    }

    /// `fn_decl = "pub"? "fn" LOWER_IDENT generic_params? "(" params ")" "->" type fn_tail`
    ///
    /// `fn_tail` is `NEWLINE? block`, clauses then a block on its own line,
    /// or clauses alone: a header followed by neither a clause nor `{` is a
    /// bodiless function (3.9, the standard library's `@builtin` stubs), for
    /// which `cpl check` reports `missing_body` outside the standard library.
    fn parse_fn_decl(&mut self, is_pub: bool, start: u32) -> PResult<FnDecl> {
        let mode = if self.at_word("pure") {
            self.bump();
            FunctionMode::Pure
        } else if self.at_word("mutate") {
            self.bump();
            FunctionMode::Mutate
        } else if self.at_word("machine") {
            self.bump();
            FunctionMode::Machine
        } else if self.at_word("initial") {
            self.bump();
            FunctionMode::Initial
        } else {
            FunctionMode::Inferred
        };
        if mode != FunctionMode::Machine {
            self.expect(T::KwFn, "`fn`")?;
        }
        let name = self.parse_lower_name("a function name")?;
        let generic_params = if self.at(T::Lt) {
            Some(self.parse_generic_params()?)
        } else {
            None
        };
        let params = self.parse_params(true)?;
        if !self.at(T::Arrow) {
            return Err(self.error_expected("`->` (the return type is mandatory; write `-> Unit`)"));
        }
        self.bump();
        let ret = self.parse_type()?;
        let mut clauses: Vec<FnClause> = Vec::new();
        let mut has_effects = false;
        let mut has_decreases = false;
        let mut end = ret.span.end;
        let mut body: Option<crate::ast::Block> = None;
        loop {
            match self.continuation(is_fn_continuation) {
                Continuation::None => break,
                cont => {
                    if self.at(T::LBrace) {
                        if cont == Continuation::SameLine && !clauses.is_empty() {
                            return Err(self.error_expected(
                                "end of line (the body's `{` must start a new line after a clause, grammar 2.4 rule 2)",
                            ));
                        }
                        let block = self.parse_block()?;
                        end = block.span.end;
                        body = Some(block);
                        break;
                    }
                    if cont == Continuation::SameLine {
                        return Err(self.error_expected(
                            "end of line (a clause must start its own line, grammar 2.4 rule 1)",
                        ));
                    }
                    // A second `effects` or `decreases` clause is reported at
                    // its keyword, before its contents are read, so that no
                    // later error inside it can be reported instead (the
                    // earliest error, grammar D70 [D-226]).
                    match self.peek_kind() {
                        T::KwEffects if has_effects => {
                            let span = self.peek_span();
                            return Err(self.duplicate_clause("effects", span));
                        }
                        T::KwDecreases if has_decreases => {
                            let span = self.peek_span();
                            return Err(self.duplicate_clause("decreases", span));
                        }
                        _ => {}
                    }
                    let clause = self.parse_fn_clause()?;
                    match &clause {
                        FnClause::Effects(_) => has_effects = true,
                        FnClause::Decreases(_) => has_decreases = true,
                        FnClause::Contract(_) => {}
                    }
                    end = clause.span().end;
                    clauses.push(clause);
                }
            }
        }
        Ok(FnDecl {
            span: Span::new(self.file(), start, end),
            is_pub,
            mode,
            name,
            generic_params,
            params,
            ret,
            clauses,
            body,
        })
    }

    /// `duplicate effects clause` / `duplicate decreases clause` (3.9, 3.11),
    /// at the keyword of the second clause.
    pub(crate) fn duplicate_clause(&mut self, name: &str, span: Span) -> Recover {
        self.emit(
            Diagnostic::error(
                codes::DUPLICATE_CLAUSE,
                codes::CATEGORY,
                format!("duplicate {name} clause"),
            )
            .with_span(span)
            .with_data("name", name.into()),
        );
        Recover
    }

    /// `fn_clause = effects_clause | contract_clause | decreases_clause`
    fn parse_fn_clause(&mut self) -> PResult<FnClause> {
        match self.peek_kind() {
            T::KwEffects => {
                let start = self.peek_span().start;
                let mut trivia = Trivia::default();
                self.take_leading(&mut trivia, start, true);
                let kw = self.bump();
                let set = self.bracketed_group(T::LBrace, T::RBrace, Mode::Insignificant, |p| {
                    p.parse_commas(T::RBrace, |p| p.parse_effect_ref())
                })?;
                let span = kw.span.join(set.span);
                let mut clause = EffectsClause {
                    span,
                    trivia,
                    effects: set.inner,
                    group: set.group,
                    trailing_comments: set.trailing_comments,
                };
                self.take_trailing(&mut clause.trivia, span);
                Ok(FnClause::Effects(clause))
            }
            T::KwRequires | T::KwEnsures => Ok(FnClause::Contract(self.parse_contract_clause()?)),
            T::KwDecreases => {
                let start = self.peek_span().start;
                let mut trivia = Trivia::default();
                self.take_leading(&mut trivia, start, true);
                let kw = self.bump();
                let expr = self.parse_header_expr()?;
                let span = kw.span.join(expr.span);
                let mut clause = DecreasesClause { span, trivia, expr };
                self.take_trailing(&mut clause.trivia, span);
                Ok(FnClause::Decreases(clause))
            }
            _ => Err(self.error_expected("a clause")),
        }
    }

    /// `contract_clause = ( "requires" | "ensures" ) expr`
    fn parse_contract_clause(&mut self) -> PResult<ContractClause> {
        let start = self.peek_span().start;
        let mut trivia = Trivia::default();
        self.take_leading(&mut trivia, start, true);
        let kind = match self.peek_kind() {
            T::KwRequires => ContractKind::Requires,
            T::KwEnsures => ContractKind::Ensures,
            _ => return Err(self.error_expected("`requires` or `ensures`")),
        };
        let kw = self.bump();
        let expr = self.parse_header_expr()?;
        let span = kw.span.join(expr.span);
        let mut clause = ContractClause {
            span,
            trivia,
            kind,
            expr,
        };
        self.take_trailing(&mut clause.trivia, span);
        Ok(clause)
    }

    /// `effect_ref = LOWER_IDENT "." LOWER_IDENT`
    fn parse_effect_ref(&mut self) -> PResult<EffectRef> {
        let start = self.peek_span().start;
        let mut trivia = Trivia::default();
        self.take_leading(&mut trivia, start, false);
        let param = self.parse_lower_name("a capability parameter name")?;
        self.expect(T::Dot, "`.` (an effect is written parameter.operation)")?;
        let operation = self.parse_lower_name("an operation name")?;
        let span = param.span.join(operation.span);
        let mut effect = EffectRef {
            span,
            trivia,
            param,
            operation,
        };
        self.take_trailing(&mut effect.trivia, span);
        Ok(effect)
    }

    /// `"(" params ")"`: positional parameters, then `using` parameters when `allow_using`.
    fn parse_params(&mut self, allow_using: bool) -> PResult<Params> {
        let list = self.bracketed_group(T::LParen, T::RParen, Mode::Insignificant, |p| {
            let mut positional: Vec<Param> = Vec::new();
            let mut using: Vec<UsingParam> = Vec::new();
            p.parse_commas(T::RParen, |p| {
                let start = p.peek_span().start;
                let mut trivia = Trivia::default();
                p.take_leading(&mut trivia, start, false);
                if p.at(T::KwUsing) {
                    if !allow_using {
                        return Err(p.error_expected(
                            "a parameter name (capability operations take no using parameters, grammar 3.8)",
                        ));
                    }
                    let kw = p.bump();
                    let name = p.parse_lower_name("a parameter name")?;
                    p.expect(T::Colon, "`:`")?;
                    let ty = p.parse_type()?;
                    let span = kw.span.join(ty.span);
                    let mut param = UsingParam {
                        span,
                        trivia,
                        name,
                        ty,
                    };
                    p.take_trailing(&mut param.trivia, span);
                    using.push(param);
                } else {
                    if !using.is_empty() {
                        let span = p.peek_span();
                        return Err(p.error(
                            codes::POSITIONAL_AFTER_USING,
                            "positional parameter after using parameter",
                            span,
                        ));
                    }
                    let name = p.parse_lower_name("a parameter name")?;
                    p.expect(T::Colon, "`:`")?;
                    let ty = p.parse_type()?;
                    let span = name.span.join(ty.span);
                    let mut param = Param {
                        span,
                        trivia,
                        name,
                        ty,
                    };
                    p.take_trailing(&mut param.trivia, span);
                    positional.push(param);
                }
                Ok(())
            })?;
            Ok((positional, using))
        })?;
        let (positional, using) = list.inner;
        Ok(Params {
            span: list.span,
            positional,
            using,
            group: list.group,
            trailing_comments: list.trailing_comments,
        })
    }

    /// `test_decl = "test" TEXT NEWLINE? block`
    fn parse_test_decl(&mut self) -> PResult<TestDecl> {
        let kw = self.expect(T::KwTest, "`test`")?;
        let name = self.parse_text_literal("a test name (text literal)")?;
        if self.continuation(|t| t.kind == T::LBrace) == Continuation::None {
            return Err(self.error_expected_header("`{`"));
        }
        let body = self.parse_block()?;
        Ok(TestDecl {
            span: kw.span.join(body.span),
            name,
            body,
        })
    }

    /// `property_decl = "property" TEXT ( NEWLINE for_clause )+ NEWLINE block`
    fn parse_property_decl(&mut self) -> PResult<PropertyDecl> {
        let kw = self.expect(T::KwProperty, "`property`")?;
        let name = self.parse_text_literal("a property name (text literal)")?;
        let mut for_clauses: Vec<ForClause> = Vec::new();
        let body = loop {
            match self.continuation(|t| matches!(t.kind, T::KwFor | T::LBrace)) {
                Continuation::None => {
                    return Err(self.error_expected_header("a `for` clause or `{`"))
                }
                cont => {
                    if self.at(T::LBrace) {
                        if for_clauses.is_empty() {
                            return Err(self.error_expected(
                                "a `for` clause (a property needs at least one, grammar 3.10)",
                            ));
                        }
                        if cont == Continuation::SameLine {
                            return Err(self.error_expected(
                                "end of line (the body's `{` must start a new line after a clause, grammar 2.4 rule 2)",
                            ));
                        }
                        break self.parse_block()?;
                    }
                    if cont == Continuation::SameLine {
                        return Err(self.error_expected(
                            "end of line (a clause must start its own line, grammar 2.4 rule 1)",
                        ));
                    }
                    for_clauses.push(self.parse_for_clause()?);
                }
            }
        };
        Ok(PropertyDecl {
            span: kw.span.join(body.span),
            name,
            for_clauses,
            body,
        })
    }

    /// `for_clause = "for" LOWER_IDENT "in" expr`
    fn parse_for_clause(&mut self) -> PResult<ForClause> {
        let start = self.peek_span().start;
        let mut trivia = Trivia::default();
        self.take_leading(&mut trivia, start, true);
        let kw = self.expect(T::KwFor, "`for`")?;
        let binder = self.parse_lower_name("a binder name")?;
        self.expect(T::KwIn, "`in`")?;
        let range = self.parse_header_expr()?;
        let span = kw.span.join(range.span);
        let mut clause = ForClause {
            span,
            trivia,
            binder,
            range,
        };
        self.take_trailing(&mut clause.trivia, span);
        Ok(clause)
    }
}
