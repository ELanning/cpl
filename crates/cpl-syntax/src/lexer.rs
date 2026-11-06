//! The hand-written lexer of `spec/grammar.md` section 1.
//!
//! One lexer serves `.cpl`, `.work`, and `.patch` files. It produces the
//! token kinds of Appendix A plus, out of band, the *trivia* the grammar
//! discards: ordinary `//` comments and blank lines, which the parser
//! attaches to AST nodes so the formatter can reproduce them.
//!
//! Layout rules implemented here (grammar 1.8 and 2.1): at most one
//! [`TokenKind::Newline`] per run of line terminators, none after a doc
//! comment, none before the first token, and no synthetic newline at the end
//! of the file. The `?` rule (1.7) is decided here: `?` directly followed by
//! an identifier start is a [`TokenKind::Hole`].
//!
//! Lexical errors are reported with the codes of [`crate::codes`]. The lexer
//! runs ahead of the parser and keeps lexing after an error (it skips the
//! offending character or closes the literal at the line end) so that the
//! parser can report a syntax error that precedes it in the source; the
//! parser then keeps only the earliest error of the file (grammar D59).

use crate::codes;
use cpl_diag::{DiagCode, Diagnostic, FileId, Span};

/// Token kinds, exactly the set of `spec/grammar.md` Appendix A.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TokenKind {
    // structure
    Newline,
    Eof,
    DocComment,
    // literals and names
    Int,
    Text,
    LowerIdent,
    UpperIdent,
    Underscore,
    Hole,
    ReservedWord,
    // reserved keywords (33)
    KwAnd,
    KwAssert,
    KwCapability,
    KwDecreases,
    KwEffects,
    KwElse,
    KwEnsures,
    KwEnum,
    KwFalse,
    KwFn,
    KwFor,
    KwForall,
    KwIf,
    KwIn,
    KwInvariant,
    KwLet,
    KwMatch,
    KwModule,
    KwNot,
    KwOr,
    KwProperty,
    KwPub,
    KwRequires,
    KwReturn,
    KwStruct,
    KwTest,
    KwTrue,
    KwType,
    KwUse,
    KwUsing,
    KwVar,
    KwWhere,
    KwWhile,
    // operators and punctuation
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    PlusPlus,
    EqEq,
    BangEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Eq,
    Arrow,
    FatArrow,
    Question,
    At,
    Dot,
    DotDot,
    Comma,
    Colon,
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
}

/// The eight spellings reserved for future editions (D4).
pub const RESERVED_FUTURE_WORDS: &[&str] = &[
    "as", "async", "await", "break", "continue", "ghost", "lemma", "loop",
];

impl TokenKind {
    /// The keyword token for a reserved spelling, if `spelling` is one of the
    /// 33 reserved keywords.
    pub fn keyword(spelling: &str) -> Option<TokenKind> {
        Some(match spelling {
            "and" => TokenKind::KwAnd,
            "assert" => TokenKind::KwAssert,
            "capability" => TokenKind::KwCapability,
            "decreases" => TokenKind::KwDecreases,
            "effects" => TokenKind::KwEffects,
            "else" => TokenKind::KwElse,
            "ensures" => TokenKind::KwEnsures,
            "enum" => TokenKind::KwEnum,
            "false" => TokenKind::KwFalse,
            "fn" => TokenKind::KwFn,
            "for" => TokenKind::KwFor,
            "forall" => TokenKind::KwForall,
            "if" => TokenKind::KwIf,
            "in" => TokenKind::KwIn,
            "invariant" => TokenKind::KwInvariant,
            "let" => TokenKind::KwLet,
            "match" => TokenKind::KwMatch,
            "module" => TokenKind::KwModule,
            "not" => TokenKind::KwNot,
            "or" => TokenKind::KwOr,
            "property" => TokenKind::KwProperty,
            "pub" => TokenKind::KwPub,
            "requires" => TokenKind::KwRequires,
            "return" => TokenKind::KwReturn,
            "struct" => TokenKind::KwStruct,
            "test" => TokenKind::KwTest,
            "true" => TokenKind::KwTrue,
            "type" => TokenKind::KwType,
            "use" => TokenKind::KwUse,
            "using" => TokenKind::KwUsing,
            "var" => TokenKind::KwVar,
            "where" => TokenKind::KwWhere,
            "while" => TokenKind::KwWhile,
            _ => return None,
        })
    }

    /// True for the 33 reserved keyword kinds.
    pub fn is_keyword(self) -> bool {
        self >= TokenKind::KwAnd && self <= TokenKind::KwWhile
    }

    /// The fixed spelling of a keyword or punctuation token.
    pub fn spelling(self) -> Option<&'static str> {
        Some(match self {
            TokenKind::KwAnd => "and",
            TokenKind::KwAssert => "assert",
            TokenKind::KwCapability => "capability",
            TokenKind::KwDecreases => "decreases",
            TokenKind::KwEffects => "effects",
            TokenKind::KwElse => "else",
            TokenKind::KwEnsures => "ensures",
            TokenKind::KwEnum => "enum",
            TokenKind::KwFalse => "false",
            TokenKind::KwFn => "fn",
            TokenKind::KwFor => "for",
            TokenKind::KwForall => "forall",
            TokenKind::KwIf => "if",
            TokenKind::KwIn => "in",
            TokenKind::KwInvariant => "invariant",
            TokenKind::KwLet => "let",
            TokenKind::KwMatch => "match",
            TokenKind::KwModule => "module",
            TokenKind::KwNot => "not",
            TokenKind::KwOr => "or",
            TokenKind::KwProperty => "property",
            TokenKind::KwPub => "pub",
            TokenKind::KwRequires => "requires",
            TokenKind::KwReturn => "return",
            TokenKind::KwStruct => "struct",
            TokenKind::KwTest => "test",
            TokenKind::KwTrue => "true",
            TokenKind::KwType => "type",
            TokenKind::KwUse => "use",
            TokenKind::KwUsing => "using",
            TokenKind::KwVar => "var",
            TokenKind::KwWhere => "where",
            TokenKind::KwWhile => "while",
            TokenKind::Underscore => "_",
            TokenKind::Plus => "+",
            TokenKind::Minus => "-",
            TokenKind::Star => "*",
            TokenKind::Slash => "/",
            TokenKind::Percent => "%",
            TokenKind::PlusPlus => "++",
            TokenKind::EqEq => "==",
            TokenKind::BangEq => "!=",
            TokenKind::Lt => "<",
            TokenKind::LtEq => "<=",
            TokenKind::Gt => ">",
            TokenKind::GtEq => ">=",
            TokenKind::Eq => "=",
            TokenKind::Arrow => "->",
            TokenKind::FatArrow => "=>",
            TokenKind::Question => "?",
            TokenKind::At => "@",
            TokenKind::Dot => ".",
            TokenKind::DotDot => "..",
            TokenKind::Comma => ",",
            TokenKind::Colon => ":",
            TokenKind::LParen => "(",
            TokenKind::RParen => ")",
            TokenKind::LBracket => "[",
            TokenKind::RBracket => "]",
            TokenKind::LBrace => "{",
            TokenKind::RBrace => "}",
            _ => return None,
        })
    }

    /// A short description for `expected ...` lists in diagnostics.
    pub fn describe(self) -> String {
        match self {
            TokenKind::Newline => "end of line".to_string(),
            TokenKind::Eof => "end of file".to_string(),
            TokenKind::DocComment => "doc comment".to_string(),
            TokenKind::Int => "integer literal".to_string(),
            TokenKind::Text => "text literal".to_string(),
            TokenKind::LowerIdent => "snake_case identifier".to_string(),
            TokenKind::UpperIdent => "PascalCase identifier".to_string(),
            TokenKind::Hole => "hole".to_string(),
            TokenKind::ReservedWord => "reserved word".to_string(),
            other => format!("`{}`", other.spelling().unwrap_or("?")),
        }
    }
}

/// One token. `text` carries the payload the grammar attaches to the token
/// class: the digit string without separators for `Int`, the decoded string
/// for `Text`, the spelling for identifiers, keywords, and reserved words, the
/// name for `Hole`, and the comment text (after `///`, one leading space
/// removed) for `DocComment`. It is empty for punctuation, `Newline`, and `Eof`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    pub text: String,
}

impl Token {
    /// Describe the token for a `found ...` clause.
    pub fn describe(&self) -> String {
        match self.kind {
            TokenKind::LowerIdent | TokenKind::UpperIdent => format!("identifier `{}`", self.text),
            TokenKind::ReservedWord => format!("reserved word `{}`", self.text),
            TokenKind::Int => format!("integer literal `{}`", self.text),
            TokenKind::Text => format!("text literal {:?}", self.text),
            TokenKind::Hole => format!("hole `?{}`", self.text),
            other => other.describe(),
        }
    }
}

/// What a piece of trivia is.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TriviaKind {
    /// An ordinary `//` comment (its span covers `//` to the end of the line).
    Comment,
    /// A line containing only whitespace (its span covers the terminator).
    BlankLine,
}

/// Trivia the parser attaches to AST nodes. `line_start` is the byte offset
/// of the first character of the line the trivia is on, which lets the
/// parser decide "same line as the preceding token" without a source map.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TriviaToken {
    pub kind: TriviaKind,
    pub span: Span,
    pub line_start: u32,
}

/// The complete lexer output.
#[derive(Clone, Debug, Default)]
pub struct Lexed {
    pub tokens: Vec<Token>,
    pub trivia: Vec<TriviaToken>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Lex `src`, returning tokens (always ending in `Eof`) and diagnostics.
pub fn lex(src: &str, file: FileId) -> (Vec<Token>, Vec<Diagnostic>) {
    let lexed = lex_full(src, file);
    (lexed.tokens, lexed.diagnostics)
}

/// Lex `src`, also returning the comments and blank lines.
pub fn lex_full(src: &str, file: FileId) -> Lexed {
    let mut lexer = Lexer {
        src,
        bytes: src.as_bytes(),
        pos: 0,
        file,
        out: Lexed::default(),
        line_start: 0,
        line_has_content: false,
    };
    lexer.run();
    lexer.out
}

struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
    file: FileId,
    out: Lexed,
    line_start: usize,
    line_has_content: bool,
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// The Unicode bidirectional embedding, override, and isolate characters (U+202A-U+202E,
/// U+2066-U+2069): D75 admits them in neither a comment nor a text literal.
pub fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

impl<'a> Lexer<'a> {
    fn span(&self, start: usize, end: usize) -> Span {
        Span::new(self.file, start as u32, end as u32)
    }

    fn byte(&self, at: usize) -> u8 {
        self.bytes.get(at).copied().unwrap_or(0)
    }

    fn error(&mut self, code: DiagCode, message: impl Into<String>, span: Span) -> &mut Diagnostic {
        self.out
            .diagnostics
            .push(Diagnostic::error(code, codes::CATEGORY, message).with_span(span));
        self.out.diagnostics.last_mut().expect("just pushed")
    }

    fn push(&mut self, kind: TokenKind, start: usize, end: usize, text: String) {
        self.line_has_content = true;
        self.out.tokens.push(Token {
            kind,
            span: self.span(start, end),
            text,
        });
    }

    fn run(&mut self) {
        // A leading byte-order mark is skipped (grammar 1.1).
        if self.src.starts_with('\u{feff}') {
            self.pos = '\u{feff}'.len_utf8();
            self.line_start = self.pos;
        }
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            match b {
                b' ' | b'\t' => self.pos += 1,
                b'\n' => self.line_end(1),
                b'\r' => {
                    if self.byte(self.pos + 1) == b'\n' {
                        self.line_end(2);
                    } else {
                        // Grammar 1.1: a CR not followed by LF is a lexical
                        // error, not a line terminator; the lexer skips it as
                        // an offending character (D70), so the line goes on.
                        self.lone_carriage_return(self.pos);
                        self.pos += 1;
                    }
                }
                b'/' if self.byte(self.pos + 1) == b'/' => self.comment(),
                b'"' => self.text_literal(),
                b'?' => self.question(),
                b'0'..=b'9' => self.int_literal(),
                _ if is_ident_start(b) => self.identifier(),
                _ if b < 0x80 => self.punctuation(),
                _ => {
                    let c = self.src[self.pos..].chars().next().unwrap_or('\u{fffd}');
                    let len = c.len_utf8().max(1);
                    let span = self.span(self.pos, self.pos + len);
                    self.error(
                        codes::NON_ASCII_OUTSIDE_TEXT,
                        format!("non-ASCII character {c:?} is permitted only inside text literals and comments"),
                        span,
                    )
                    .data
                    .insert("found".into(), c.to_string().into());
                    self.pos += len;
                }
            }
        }
        let end = self.bytes.len();
        self.out.tokens.push(Token {
            kind: TokenKind::Eof,
            span: self.span(end, end),
            text: String::new(),
        });
    }

    /// Handle a line terminator of `len` bytes at `self.pos` (grammar 1.8 step 2).
    fn line_end(&mut self, len: usize) {
        let start = self.pos;
        if !self.line_has_content {
            self.out.trivia.push(TriviaToken {
                kind: TriviaKind::BlankLine,
                span: self.span(start, start + len),
                line_start: self.line_start as u32,
            });
        }
        let emit = match self.out.tokens.last() {
            None => false,
            Some(t) => !matches!(t.kind, TokenKind::Newline | TokenKind::DocComment),
        };
        if emit {
            self.out.tokens.push(Token {
                kind: TokenKind::Newline,
                span: self.span(start, start + len),
                text: String::new(),
            });
        }
        self.pos = start + len;
        self.line_start = self.pos;
        self.line_has_content = false;
    }

    /// `invalid_line_terminator` for the lone CR at `at` (grammar 1.1).
    fn lone_carriage_return(&mut self, at: usize) {
        let span = self.span(at, at + 1);
        self.error(
            codes::INVALID_LINE_TERMINATOR,
            "a carriage return must be followed by a line feed",
            span,
        );
    }

    /// Byte offset of the end of the current line: its terminator (LF, or
    /// the CR of a CRLF) or the end of the input. A lone CR is no line
    /// terminator (grammar 1.1), so it does not end the line.
    fn end_of_line(&self) -> usize {
        let mut i = self.pos;
        while i < self.bytes.len() {
            match self.bytes[i] {
                b'\n' => break,
                b'\r' if self.byte(i + 1) == b'\n' => break,
                _ => i += 1,
            }
        }
        i
    }

    /// D75: a control character (C0 other than tab and CR, which has its own error, and DEL) or a
    /// bidirectional formatting character in `start..end` of a comment is `invalid_character`. A
    /// comment is shown to the reader of the source and copied into generated code, where either
    /// can make the text read differently from what it is (CVE-2021-42574, "Trojan Source").
    fn hidden_characters(&mut self, start: usize, end: usize, place: &str) {
        let mut bad = Vec::new();
        for (i, c) in self.src[start..end].char_indices() {
            let control = (c < ' ' && c != '\t' && c != '\r') || c == '\u{7f}';
            if control || is_bidi_control(c) {
                bad.push((start + i, c, control));
            }
        }
        for (at, c, control) in bad {
            let kind = if control {
                "control character"
            } else {
                "bidirectional formatting character"
            };
            let span = self.span(at, at + c.len_utf8());
            self.error(codes::INVALID_CHARACTER, format!("{kind} in {place}"), span)
                .data
                .insert("found".into(), format!("U+{:04X}", c as u32).into());
        }
    }

    fn comment(&mut self) {
        let start = self.pos;
        let mut slashes = 0;
        while self.byte(start + slashes) == b'/' {
            slashes += 1;
        }
        // A comment runs to the end of its line, past any lone CR in it, each
        // of which is a lexical error of its own (grammar 1.1, D70): ending
        // the comment there would turn the rest of the line into tokens.
        let end = self.end_of_line();
        for at in start..end {
            if self.bytes[at] == b'\r' {
                self.lone_carriage_return(at);
            }
        }
        self.hidden_characters(start, end, "comment");
        if slashes == 3 {
            if self.line_has_content {
                let span = self.span(start, start + 3);
                self.error(
                    codes::DOC_COMMENT_POSITION,
                    "doc comment must begin a line",
                    span,
                );
            } else {
                let mut text = &self.src[start + 3..end];
                if let Some(rest) = text.strip_prefix(' ') {
                    text = rest;
                }
                let text = text.to_string();
                self.push(TokenKind::DocComment, start, end, text);
                self.pos = end;
                return;
            }
        }
        self.out.trivia.push(TriviaToken {
            kind: TriviaKind::Comment,
            span: self.span(start, end),
            line_start: self.line_start as u32,
        });
        self.line_has_content = true;
        self.pos = end;
    }

    fn identifier(&mut self) {
        let start = self.pos;
        while is_ident_char(self.byte(self.pos)) {
            self.pos += 1;
        }
        let spelling = &self.src[start..self.pos];
        let kind = classify_identifier(spelling);
        let text = spelling.to_string();
        self.push(kind, start, self.pos, text);
        if self.byte(self.pos) == b'"' {
            let span = self.span(start, self.pos + 1);
            self.error(
                codes::PREFIXED_LITERAL_RESERVED,
                "prefixed string literals are reserved",
                span,
            );
        }
    }

    fn int_literal(&mut self) {
        let start = self.pos;
        let mut digits = String::new();
        loop {
            let b = self.byte(self.pos);
            if b.is_ascii_digit() {
                digits.push(b as char);
                self.pos += 1;
            } else if b == b'_' {
                if !self.byte(self.pos + 1).is_ascii_digit() {
                    let span = self.span(self.pos, self.pos + 1);
                    self.error(
                        codes::INVALID_INT_LITERAL,
                        "a digit separator must be between two digits",
                        span,
                    );
                }
                self.pos += 1;
            } else {
                break;
            }
        }
        if digits.len() > 1 && digits.starts_with('0') {
            let span = self.span(start, self.pos);
            self.error(
                codes::INVALID_INT_LITERAL,
                "leading zeros are not permitted",
                span,
            );
        }
        let end = self.pos;
        self.push(TokenKind::Int, start, end, digits);
        if self.byte(self.pos) == b'.' && self.byte(self.pos + 1).is_ascii_digit() {
            self.pos += 1;
            while self.byte(self.pos).is_ascii_digit() || self.byte(self.pos) == b'_' {
                self.pos += 1;
            }
            let span = self.span(start, self.pos);
            self.error(
                codes::FLOAT_LITERAL_UNSUPPORTED,
                "floating-point literals are not supported in edition 0.1",
                span,
            );
        } else if is_ident_start(self.byte(self.pos)) {
            while is_ident_char(self.byte(self.pos)) {
                self.pos += 1;
            }
            let span = self.span(start, self.pos);
            self.error(
                codes::LITERAL_ADJACENT_IDENTIFIER,
                "a literal must be separated from an identifier",
                span,
            );
        }
    }

    fn text_literal(&mut self) {
        let start = self.pos;
        self.pos += 1;
        let mut decoded = String::new();
        loop {
            if self.pos >= self.bytes.len() {
                let span = self.span(start, self.pos);
                self.error(
                    codes::UNTERMINATED_TEXT_LITERAL,
                    "unterminated text literal",
                    span,
                );
                break;
            }
            let b = self.bytes[self.pos];
            match b {
                b'"' => {
                    self.pos += 1;
                    break;
                }
                b'\n' | b'\r' => {
                    let span = self.span(start, self.pos);
                    self.error(
                        codes::UNTERMINATED_TEXT_LITERAL,
                        "unterminated text literal",
                        span,
                    );
                    break;
                }
                b'\\' => self.escape(&mut decoded),
                0x00..=0x08 | 0x0b | 0x0c | 0x0e..=0x1f | 0x7f => {
                    let span = self.span(self.pos, self.pos + 1);
                    self.error(
                        codes::INVALID_CHARACTER,
                        "control character in text literal",
                        span,
                    )
                    .data
                    .insert("found".into(), format!("U+{b:04X}").into());
                    self.pos += 1;
                }
                _ => {
                    let c = self.src[self.pos..].chars().next().unwrap_or('\u{fffd}');
                    if is_bidi_control(c) {
                        // D75: written raw, it reorders how the line displays; `\u{...}` keeps it
                        let span = self.span(self.pos, self.pos + c.len_utf8());
                        self.error(
                            codes::INVALID_CHARACTER,
                            format!("bidirectional formatting character in text literal (write it as \\u{{{:X}}})", c as u32),
                            span,
                        )
                        .data
                        .insert("found".into(), format!("U+{:04X}", c as u32).into());
                        self.pos += c.len_utf8();
                        continue;
                    }
                    decoded.push(c);
                    self.pos += c.len_utf8().max(1);
                }
            }
        }
        self.push(TokenKind::Text, start, self.pos, decoded);
    }

    /// Decode one escape at `self.pos` (which points at the backslash).
    fn escape(&mut self, decoded: &mut String) {
        let start = self.pos;
        let next = self.byte(start + 1);
        let simple = match next {
            b'\\' => Some('\\'),
            b'"' => Some('"'),
            b'n' => Some('\n'),
            b'r' => Some('\r'),
            b't' => Some('\t'),
            _ => None,
        };
        if let Some(c) = simple {
            decoded.push(c);
            self.pos += 2;
            return;
        }
        if next == b'u' {
            if self.byte(start + 2) != b'{' {
                let span = self.span(start, start + 2);
                self.error(
                    codes::INVALID_ESCAPE,
                    "a unicode escape is written \\u{H}",
                    span,
                )
                .data
                .insert("found".into(), "\\u".into());
                self.pos += 2;
                return;
            }
            let mut i = start + 3;
            let hex_start = i;
            while self.byte(i).is_ascii_hexdigit() {
                i += 1;
            }
            let hex = &self.src[hex_start..i];
            if self.byte(i) != b'}' {
                let span = self.span(start, i);
                let found = self.src[start..i].to_string();
                self.error(codes::INVALID_ESCAPE, "unterminated unicode escape", span)
                    .data
                    .insert("found".into(), found.into());
                self.pos = i;
                return;
            }
            let end = i + 1;
            let span = self.span(start, end);
            let found = self.src[start..end].to_string();
            if hex.is_empty() {
                self.error(
                    codes::INVALID_ESCAPE,
                    "a unicode escape needs at least one hex digit",
                    span,
                )
                .data
                .insert("found".into(), "\\u{}".into());
            } else if hex.len() > 6 {
                self.error(
                    codes::INVALID_ESCAPE,
                    "unicode escape has more than six hex digits",
                    span,
                )
                .data
                .insert("found".into(), found.into());
            } else {
                match u32::from_str_radix(hex, 16).ok().and_then(char::from_u32) {
                    Some(c) => decoded.push(c),
                    None => {
                        self.error(
                            codes::INVALID_ESCAPE,
                            "invalid unicode escape: not a Unicode scalar value",
                            span,
                        )
                        .data
                        .insert("found".into(), found.into());
                    }
                }
            }
            self.pos = end;
            return;
        }
        if start + 1 >= self.bytes.len() || next == b'\n' || next == b'\r' {
            // The backslash is the last character of the line; the caller
            // reports the unterminated literal.
            self.pos += 1;
            return;
        }
        let c = self.src[start + 1..].chars().next().unwrap_or('\u{fffd}');
        let end = start + 1 + c.len_utf8().max(1);
        let span = self.span(start, end);
        self.error(
            codes::INVALID_ESCAPE,
            format!("invalid escape sequence `\\{c}`"),
            span,
        )
        .data
        .insert("found".into(), format!("\\{c}").into());
        self.pos = end;
    }

    fn question(&mut self) {
        let start = self.pos;
        if is_ident_start(self.byte(start + 1)) {
            self.pos += 1;
            while is_ident_char(self.byte(self.pos)) {
                self.pos += 1;
            }
            let name = &self.src[start + 1..self.pos];
            let span = self.span(start, self.pos);
            let bad_case = name == "_" || name.starts_with(|c: char| c.is_ascii_uppercase());
            if bad_case {
                self.error(codes::HOLE_NAME_CASE, "hole names are snake_case", span)
                    .data
                    .insert("name".into(), name.to_string().into());
            } else if TokenKind::keyword(name).is_some() || RESERVED_FUTURE_WORDS.contains(&name) {
                self.error(
                    codes::HOLE_NAME_KEYWORD,
                    "a hole name cannot be a keyword",
                    span,
                )
                .data
                .insert("name".into(), name.to_string().into());
            }
            let name = name.to_string();
            self.push(TokenKind::Hole, start, self.pos, name);
        } else {
            self.push(TokenKind::Question, start, start + 1, String::new());
            self.pos += 1;
        }
    }

    fn punctuation(&mut self) {
        let start = self.pos;
        let b = self.bytes[start];
        let next = self.byte(start + 1);
        let (kind, len) = match (b, next) {
            (b'+', b'+') => (TokenKind::PlusPlus, 2),
            (b'+', _) => (TokenKind::Plus, 1),
            (b'-', b'>') => (TokenKind::Arrow, 2),
            (b'-', _) => (TokenKind::Minus, 1),
            (b'*', _) => (TokenKind::Star, 1),
            (b'/', _) => (TokenKind::Slash, 1),
            (b'%', _) => (TokenKind::Percent, 1),
            (b'=', b'=') => (TokenKind::EqEq, 2),
            (b'=', b'>') => (TokenKind::FatArrow, 2),
            (b'=', _) => (TokenKind::Eq, 1),
            (b'!', b'=') => (TokenKind::BangEq, 2),
            (b'<', b'=') => (TokenKind::LtEq, 2),
            (b'<', _) => (TokenKind::Lt, 1),
            (b'>', b'=') => (TokenKind::GtEq, 2),
            (b'>', _) => (TokenKind::Gt, 1),
            (b'@', _) => (TokenKind::At, 1),
            (b'.', b'.') => (TokenKind::DotDot, 2),
            (b'.', _) => (TokenKind::Dot, 1),
            (b',', _) => (TokenKind::Comma, 1),
            (b':', _) => (TokenKind::Colon, 1),
            (b'(', _) => (TokenKind::LParen, 1),
            (b')', _) => (TokenKind::RParen, 1),
            (b'[', _) => (TokenKind::LBracket, 1),
            (b']', _) => (TokenKind::RBracket, 1),
            (b'{', _) => (TokenKind::LBrace, 1),
            (b'}', _) => (TokenKind::RBrace, 1),
            _ => {
                let message = match b {
                    b';' => "semicolons are not part of CPL".to_string(),
                    b'!' => "use not".to_string(),
                    b'&' | b'|' => "use and / or".to_string(),
                    _ if b.is_ascii_graphic() => format!("unexpected character `{}`", b as char),
                    _ => format!("unexpected character U+{b:04X}"),
                };
                let span = self.span(start, start + 1);
                let found = if b.is_ascii_graphic() {
                    (b as char).to_string()
                } else {
                    format!("U+{b:04X}")
                };
                self.error(codes::INVALID_CHARACTER, message, span)
                    .data
                    .insert("found".into(), found.into());
                self.pos += 1;
                self.line_has_content = true;
                return;
            }
        };
        self.push(kind, start, start + len, String::new());
        self.pos += len;
    }
}

/// Classify an identifier spelling (grammar 1.3 and 1.4).
pub fn classify_identifier(spelling: &str) -> TokenKind {
    if let Some(kw) = TokenKind::keyword(spelling) {
        return kw;
    }
    if RESERVED_FUTURE_WORDS.contains(&spelling) {
        return TokenKind::ReservedWord;
    }
    if spelling == "_" {
        return TokenKind::Underscore;
    }
    if spelling.starts_with(|c: char| c.is_ascii_uppercase()) {
        TokenKind::UpperIdent
    } else {
        TokenKind::LowerIdent
    }
}
