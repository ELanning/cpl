//! Diagnostic codes emitted by the lexer and parser.
//!
//! `spec/grammar.md` D59 distinguishes two diagnostic *classes*, lexical
//! and syntax, whose names are never emitted as codes; the code of every
//! diagnostic is the `parse`-category code that `spec/semantics.md` 9.4
//! assigns to its message. Every diagnostic produced by this crate has
//! `category == CATEGORY` and `code` equal to one of the constants below
//! (all listed in [`ALL`]). Messages are the ones `spec/grammar.md` quotes
//! where it quotes one; otherwise they follow the generic shape `expected
//! <tokens>, found <token>` with the `expected` and `found` fields attached
//! (D59). A file carries at most one of them: parsing stops at the first
//! lexical or syntax error (D59).

use cpl_diag::{Category, DiagCode};

/// The category of every diagnostic this crate produces.
pub const CATEGORY: Category = Category::Parse;

/// Invalid UTF-8 in a source file (raised by `cpl_diag::SourceMap::add_bytes`).
pub const INVALID_UTF8: DiagCode = DiagCode("invalid_utf8");
/// A CR that is not followed by LF (grammar 1.1).
pub const INVALID_LINE_TERMINATOR: DiagCode = DiagCode("invalid_line_terminator");
/// A non-ASCII character outside a text literal or comment (grammar 1.1).
pub const NON_ASCII_OUTSIDE_TEXT: DiagCode = DiagCode("non_ascii_outside_text");
/// `;`, `!`, `&`, `|`, a control character, or any other unexpected
/// character (grammar 1.6 messages, D62 for text literals).
pub const INVALID_CHARACTER: DiagCode = DiagCode("invalid_character");
/// A text literal that reaches a line terminator or the end of the file (grammar 1.5).
pub const UNTERMINATED_TEXT_LITERAL: DiagCode = DiagCode("unterminated_text_literal");
/// A bad escape, including surrogate and out-of-range `\u{}` values (grammar 1.5).
pub const INVALID_ESCAPE: DiagCode = DiagCode("invalid_escape");
/// Misplaced digit separators or leading zeros (grammar 1.5).
pub const INVALID_INT_LITERAL: DiagCode = DiagCode("invalid_int_literal");
/// `1.5` (grammar 1.5).
pub const FLOAT_LITERAL_UNSUPPORTED: DiagCode = DiagCode("float_literal_unsupported");
/// `1e5`, `2x` (grammar 1.5).
pub const LITERAL_ADJACENT_IDENTIFIER: DiagCode = DiagCode("literal_adjacent_identifier");
/// An identifier immediately followed by `"` (D8).
pub const PREFIXED_LITERAL_RESERVED: DiagCode = DiagCode("prefixed_literal_reserved");
/// `doc comment must begin a line` / `doc comment must precede a declaration` (grammar 1.2).
pub const DOC_COMMENT_POSITION: DiagCode = DiagCode("doc_comment_position");
/// One of the eight words reserved for a future edition was used (D4).
pub const RESERVED_WORD: DiagCode = DiagCode("reserved_word");
/// `hole names are snake_case` (D10).
pub const HOLE_NAME_CASE: DiagCode = DiagCode("hole_name_case");
/// `a hole name cannot be a keyword` (grammar 1.7).
pub const HOLE_NAME_KEYWORD: DiagCode = DiagCode("hole_name_keyword");
/// Every other grammar violation; carries `expected` and `found`.
pub const SYNTAX_ERROR: DiagCode = DiagCode("syntax_error");
/// A second `effects` or `decreases` clause on one header (grammar 3.9, 3.11).
pub const DUPLICATE_CLAUSE: DiagCode = DiagCode("duplicate_clause");
/// A `use` declaration after a declaration (D13).
pub const USE_AFTER_DECLARATION: DiagCode = DiagCode("use_after_declaration");
/// A positional parameter or argument after a `using` one (grammar 3.9, 3.12).
pub const POSITIONAL_AFTER_USING: DiagCode = DiagCode("positional_after_using");
/// `else` at the start of a line where newlines are significant (D12).
pub const MISPLACED_ELSE: DiagCode = DiagCode("misplaced_else");
/// A `require` form outside the closed set of six (D36).
pub const UNKNOWN_REQUIREMENT: DiagCode = DiagCode("unknown_requirement");
/// Duplicate `base`/`target`/`workers`/`integrate`, duplicate task item kind,
/// or duplicate task name (grammar 4.1, 4.2).
pub const DUPLICATE_WORK_ITEM: DiagCode = DiagCode("duplicate_work_item");

/// Every code this crate can emit.
pub const ALL: &[DiagCode] = &[
    INVALID_UTF8,
    INVALID_LINE_TERMINATOR,
    NON_ASCII_OUTSIDE_TEXT,
    INVALID_CHARACTER,
    UNTERMINATED_TEXT_LITERAL,
    INVALID_ESCAPE,
    INVALID_INT_LITERAL,
    FLOAT_LITERAL_UNSUPPORTED,
    LITERAL_ADJACENT_IDENTIFIER,
    PREFIXED_LITERAL_RESERVED,
    DOC_COMMENT_POSITION,
    RESERVED_WORD,
    HOLE_NAME_CASE,
    HOLE_NAME_KEYWORD,
    SYNTAX_ERROR,
    DUPLICATE_CLAUSE,
    USE_AFTER_DECLARATION,
    POSITIONAL_AFTER_USING,
    MISPLACED_ELSE,
    UNKNOWN_REQUIREMENT,
    DUPLICATE_WORK_ITEM,
];
