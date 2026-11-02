//! cpl-syntax: lexer, parser, AST, and debug dump for CPL 0.1.
//!
//! The authoritative grammar is `spec/grammar.md` (edition 0.1): section 1
//! lexical structure, section 2 the newline/layout algorithm, section 3 the
//! `.cpl` grammar, section 4 the `.work` and `.patch` grammars, section 5
//! the disambiguation rulings, Appendix A the token kinds ([`TokenKind`]),
//! Appendix B the nonterminals ([`ast`]).
//!
//! Entry points:
//!
//! * [`lex`]: tokens and lexical diagnostics for any of the three file forms.
//! * [`parse_module`], [`parse_work`], [`parse_patch`]: one per file form,
//!   each returning a [`ParseResult`] whose `value` is present only when no
//!   error was reported.
//! * [`parse_expr`]: one expression with nothing after it, under the normal
//!   expression grammar, for the constant-expression arguments of `cpl run`
//!   (`spec/toolchain.md` 7.2); same [`ParseResult`] contract.
//! * [`dump_module`], [`dump_work`], [`dump_patch`]: deterministic
//!   S-expression renderings without spans, for golden tests and debugging.
//! * [`format_module`], [`format_work`], [`format_patch`], and
//!   [`format_source`]: the canonical formatter of `spec/toolchain.md` 2.4
//!   ([`formatter`]).
//! * [`format_fragment`]: one [`Fragment`] of a tree (a declaration, a
//!   facet's part of one, a block, an expression, a type, a selector) at
//!   indentation level 0, either as the facet canonical text of
//!   `spec/toolchain.md` 5.2 ([`FragmentMode::Canonical`], comments and
//!   layout dropped, clauses on one line) or as the formatter prints it
//!   with its comments ([`FragmentMode::Source`], for patch application).
//!
//! Diagnostics use the shared [`cpl_diag::Diagnostic`] with category `parse`
//! and the codes listed in [`codes`].

pub mod ast;
pub mod codes;
pub mod dump;
pub mod formatter;
pub mod lexer;
pub mod parser;

pub use dump::{dump_module, dump_patch, dump_work};
pub use formatter::{
    format_fragment, format_module, format_patch, format_source, format_work, FileKind, Fragment,
    FragmentMode,
};
pub use lexer::{lex, lex_full, Lexed, Token, TokenKind, TriviaKind, TriviaToken};
pub use parser::{parse_expr, parse_module, parse_patch, parse_work, ParseResult};
