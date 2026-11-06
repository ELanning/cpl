//! Lexer tests: the `?` rule (1.7), escapes and separators (1.5), NEWLINE
//! emission (1.8 / 2.1), `>>` splitting (1.6), and lexical errors.

use cpl_diag::FileId;
use cpl_syntax::lexer::{lex_full, TriviaKind};
use cpl_syntax::{codes, lex, Token, TokenKind as T};

fn tokens(src: &str) -> Vec<Token> {
    let (tokens, diagnostics) = lex(src, FileId(0));
    assert!(
        diagnostics.is_empty(),
        "unexpected lexical errors for {src:?}: {diagnostics:#?}"
    );
    tokens
}

fn kinds(src: &str) -> Vec<T> {
    tokens(src).into_iter().map(|t| t.kind).collect()
}

fn codes_of(src: &str) -> Vec<String> {
    let (_, diagnostics) = lex(src, FileId(0));
    diagnostics.iter().map(|d| d.code.to_string()).collect()
}

fn messages_of(src: &str) -> Vec<String> {
    let (_, diagnostics) = lex(src, FileId(0));
    diagnostics.iter().map(|d| d.message.clone()).collect()
}

#[test]
fn question_rule_distinguishes_holes_from_propagation() {
    let t = tokens("?build_pages");
    assert_eq!(t[0].kind, T::Hole);
    assert_eq!(t[0].text, "build_pages");
    assert_eq!(kinds("x?"), vec![T::LowerIdent, T::Question, T::Eof]);
    assert_eq!(
        kinds("x? y"),
        vec![T::LowerIdent, T::Question, T::LowerIdent, T::Eof]
    );
    assert_eq!(kinds("x?y"), vec![T::LowerIdent, T::Hole, T::Eof]);
    assert_eq!(kinds("? x"), vec![T::Question, T::LowerIdent, T::Eof]);
    assert_eq!(
        kinds("parse_size(text)?"),
        vec![
            T::LowerIdent,
            T::LParen,
            T::LowerIdent,
            T::RParen,
            T::Question,
            T::Eof
        ]
    );
    // `self` and `result` are ordinary identifiers, so these are legal holes (D50).
    assert_eq!(kinds("?self ?result"), vec![T::Hole, T::Hole, T::Eof]);
    assert_eq!(kinds("?_tmp"), vec![T::Hole, T::Eof]);
}

#[test]
fn hole_names_must_be_snake_case_and_not_keywords() {
    for bad in ["?Foo", "?_"] {
        assert_eq!(codes_of(bad), vec![codes::HOLE_NAME_CASE], "{bad}");
        assert_eq!(messages_of(bad), vec!["hole names are snake_case"], "{bad}");
    }
    // `a hole name cannot be a keyword` has its own row in the code
    // catalogue (spec/semantics.md 9.4: `hole_name_keyword`), which D59
    // makes the code of that message.
    for bad in ["?if", "?loop", "?test"] {
        assert_eq!(codes_of(bad), vec![codes::HOLE_NAME_KEYWORD], "{bad}");
        assert_eq!(
            messages_of(bad),
            vec!["a hole name cannot be a keyword"],
            "{bad}"
        );
    }
    // The hole token is still produced so the parser can continue.
    let (t, _) = lex("?Foo", FileId(0));
    assert_eq!(t[0].kind, T::Hole);
}

#[test]
fn text_escapes_decode_exactly_per_grammar() {
    let t = tokens(r#""a\\b\"c\n\r\t\u{41}\u{1F600}\u{7}""#);
    assert_eq!(t[0].kind, T::Text);
    assert_eq!(t[0].text, "a\\b\"c\n\r\t\u{41}\u{1F600}\u{7}");
    // Raw non-ASCII, including C1 controls, is permitted (D62).
    let t = tokens("\"héllo \u{85} 漢字\"");
    assert_eq!(t[0].text, "héllo \u{85} 漢字");
    assert_eq!(tokens(r#""""#)[0].text, "");
}

#[test]
fn bad_escapes_are_lexical_errors() {
    assert_eq!(codes_of(r#""\q""#), vec![codes::INVALID_ESCAPE]);
    assert_eq!(codes_of(r#""\u{110000}""#), vec![codes::INVALID_ESCAPE]);
    assert_eq!(codes_of(r#""\u{D800}""#), vec![codes::INVALID_ESCAPE]);
    assert_eq!(codes_of(r#""\u{1234567}""#), vec![codes::INVALID_ESCAPE]);
    assert_eq!(
        messages_of(r#""\u{1234567}""#),
        vec!["unicode escape has more than six hex digits"]
    );
    assert_eq!(codes_of(r#""\u{}""#), vec![codes::INVALID_ESCAPE]);
    assert_eq!(codes_of(r#""\u41""#), vec![codes::INVALID_ESCAPE]);
    assert_eq!(codes_of(r#""\u{41""#), vec![codes::INVALID_ESCAPE]);
}

#[test]
fn bidirectional_and_control_characters_are_refused_outside_escapes() {
    // D75: Trojan Source: an override in a comment or a literal reorders the displayed line
    for src in ["x // a \u{202e} b", "/// a \u{2066} b\nfn", "x // a \u{1} b", "x // \u{7f}"] {
        assert_eq!(codes_of(src), vec![codes::INVALID_CHARACTER], "{src:?}");
    }
    assert_eq!(
        messages_of("x // \u{202e}"),
        vec!["bidirectional formatting character in comment"]
    );
    assert_eq!(
        messages_of("x // \u{0}"),
        vec!["control character in comment"]
    );
    assert!(messages_of("\"a\u{202e}b\"")[0]
        .starts_with("bidirectional formatting character in text literal"));
    // the escape still writes one, a tab in a comment is fine, and other non-ASCII text is too
    assert!(codes_of("\"a\\u{202E}b\"").is_empty());
    assert_eq!(tokens("\"a\\u{202E}b\"")[0].text, "a\u{202e}b");
    assert!(codes_of("x // a\tb \u{e9} \u{200f}").is_empty());
    // the literal still lexes as one token, without the character
    let (t, _) = lex("\"a\u{202e}b\"", FileId(0));
    assert_eq!(t[0].text, "ab");
}

#[test]
fn unterminated_and_control_characters_in_text() {
    assert_eq!(codes_of("\"abc\nx"), vec![codes::UNTERMINATED_TEXT_LITERAL]);
    assert_eq!(codes_of("\"abc"), vec![codes::UNTERMINATED_TEXT_LITERAL]);
    assert_eq!(
        codes_of("\"abc\r\nx"),
        vec![codes::UNTERMINATED_TEXT_LITERAL]
    );
    assert_eq!(codes_of("\"a\u{1}b\""), vec![codes::INVALID_CHARACTER]);
    assert_eq!(
        messages_of("\"a\u{7f}b\""),
        vec!["control character in text literal"]
    );
    // A tab is permitted raw.
    assert_eq!(tokens("\"a\tb\"")[0].text, "a\tb");
    // The lexer recovers: the rest of the line still lexes.
    let (t, _) = lex("\"abc\nlet", FileId(0));
    let k: Vec<T> = t.iter().map(|t| t.kind).collect();
    assert_eq!(k, vec![T::Text, T::Newline, T::KwLet, T::Eof]);
}

#[test]
fn int_literals_and_separators() {
    let t = tokens("1_000_000 0 42");
    assert_eq!(t[0].text, "1000000");
    assert_eq!(t[1].text, "0");
    assert_eq!(t[2].text, "42");
    assert_eq!(codes_of("007"), vec![codes::INVALID_INT_LITERAL]);
    assert_eq!(messages_of("007"), vec!["leading zeros are not permitted"]);
    assert_eq!(codes_of("1__0"), vec![codes::INVALID_INT_LITERAL]);
    assert_eq!(codes_of("1_"), vec![codes::INVALID_INT_LITERAL]);
    assert_eq!(codes_of("0_1"), vec![codes::INVALID_INT_LITERAL]);
    // `_1` is an identifier, not a literal.
    assert_eq!(kinds("_1"), vec![T::LowerIdent, T::Eof]);
    assert_eq!(codes_of("1.5"), vec![codes::FLOAT_LITERAL_UNSUPPORTED]);
    assert_eq!(
        messages_of("1.5"),
        vec!["floating-point literals are not supported in edition 0.1"]
    );
    assert_eq!(codes_of("1e5"), vec![codes::LITERAL_ADJACENT_IDENTIFIER]);
    assert_eq!(codes_of("2x"), vec![codes::LITERAL_ADJACENT_IDENTIFIER]);
    // `x.len` style member access after a literal is a Dot token, not a float.
    assert_eq!(kinds("1.len"), vec![T::Int, T::Dot, T::LowerIdent, T::Eof]);
    // No negative literals: `-5` is Minus then Int (D48).
    assert_eq!(kinds("-5"), vec![T::Minus, T::Int, T::Eof]);
}

#[test]
fn newline_emission_collapses_runs_and_skips_doc_comments() {
    assert_eq!(
        kinds("a\n\n\nb"),
        vec![T::LowerIdent, T::Newline, T::LowerIdent, T::Eof]
    );
    assert_eq!(kinds("\n\n  \na"), vec![T::LowerIdent, T::Eof]);
    assert_eq!(
        kinds("a\n// comment\n\n// more\nb"),
        vec![T::LowerIdent, T::Newline, T::LowerIdent, T::Eof]
    );
    assert_eq!(kinds("a\n"), vec![T::LowerIdent, T::Newline, T::Eof]);
    assert_eq!(kinds("a"), vec![T::LowerIdent, T::Eof]);
    // No NEWLINE after a doc comment, nor for blank lines directly after it.
    let t = tokens("/// doc\n\nfn");
    assert_eq!(
        t.iter().map(|t| t.kind).collect::<Vec<_>>(),
        vec![T::DocComment, T::KwFn, T::Eof]
    );
    assert_eq!(t[0].text, "doc");
    assert_eq!(
        kinds("a\n/// doc\nfn"),
        vec![T::LowerIdent, T::Newline, T::DocComment, T::KwFn, T::Eof]
    );
    // CRLF is one terminator.
    let t = tokens("a\r\nb");
    assert_eq!(t[1].kind, T::Newline);
    assert_eq!((t[1].span.start, t[1].span.end), (1, 3));
    assert_eq!(codes_of("a\rb"), vec![codes::INVALID_LINE_TERMINATOR]);
}

#[test]
fn doc_comment_text_and_position() {
    assert_eq!(tokens("///  two spaces")[0].text, " two spaces");
    assert_eq!(tokens("///no space")[0].text, "no space");
    // Four or more slashes are an ordinary comment (D60).
    assert_eq!(kinds("//// banner\nx"), vec![T::LowerIdent, T::Eof]);
    // A doc comment after code on the same line is an error and is treated as an ordinary comment.
    let (t, d) = lex("x /// d", FileId(0));
    assert_eq!(
        d.iter().map(|d| d.code.as_str()).collect::<Vec<_>>(),
        vec![codes::DOC_COMMENT_POSITION]
    );
    assert_eq!(d[0].message, "doc comment must begin a line");
    assert_eq!(
        t.iter().map(|t| t.kind).collect::<Vec<_>>(),
        vec![T::LowerIdent, T::Eof]
    );
}

#[test]
fn nested_generics_split_into_separate_gt_tokens() {
    assert_eq!(
        kinds("List<List<T>>"),
        vec![
            T::UpperIdent,
            T::Lt,
            T::UpperIdent,
            T::Lt,
            T::UpperIdent,
            T::Gt,
            T::Gt,
            T::Eof
        ]
    );
    assert_eq!(
        kinds("Result<List<List<Int>>, ConstraintError>"),
        vec![
            T::UpperIdent,
            T::Lt,
            T::UpperIdent,
            T::Lt,
            T::UpperIdent,
            T::Lt,
            T::UpperIdent,
            T::Gt,
            T::Gt,
            T::Comma,
            T::UpperIdent,
            T::Gt,
            T::Eof
        ]
    );
    assert_eq!(
        kinds(">= -> => == != .. ++ <= ."),
        vec![
            T::GtEq,
            T::Arrow,
            T::FatArrow,
            T::EqEq,
            T::BangEq,
            T::DotDot,
            T::PlusPlus,
            T::LtEq,
            T::Dot,
            T::Eof
        ]
    );
    // Longest match: `...` is `..` then `.`; `a-1` is a subtraction.
    assert_eq!(kinds("..."), vec![T::DotDot, T::Dot, T::Eof]);
    assert_eq!(kinds("a-1"), vec![T::LowerIdent, T::Minus, T::Int, T::Eof]);
}

#[test]
fn keywords_identifier_classes_and_reserved_words() {
    assert_eq!(
        kinds("fn test tests _ _x Foo loop self result add base"),
        vec![
            T::KwFn,
            T::KwTest,
            T::LowerIdent,
            T::Underscore,
            T::LowerIdent,
            T::UpperIdent,
            T::ReservedWord,
            T::LowerIdent,
            T::LowerIdent,
            T::LowerIdent,
            T::LowerIdent,
            T::Eof
        ]
    );
    let all = "and assert capability decreases effects else ensures enum false fn for forall if in invariant let \
               match module not or property pub requires return struct test true type use using var where while";
    let k = kinds(all);
    assert_eq!(k.len(), 34);
    assert!(k[..33].iter().all(|k| k.is_keyword()), "{k:?}");
    for word in [
        "as", "async", "await", "break", "continue", "ghost", "lemma", "loop",
    ] {
        assert_eq!(kinds(word), vec![T::ReservedWord, T::Eof], "{word}");
    }
    assert_eq!(T::keyword("fn"), Some(T::KwFn));
    assert_eq!(T::keyword("tests"), None);
    assert_eq!(T::KwFn.spelling(), Some("fn"));
    assert_eq!(T::Plus.spelling(), Some("+"));
}

#[test]
fn punctuation_errors_carry_the_grammar_messages() {
    assert_eq!(messages_of(";"), vec!["semicolons are not part of CPL"]);
    assert_eq!(codes_of(";"), vec![codes::INVALID_CHARACTER]);
    assert_eq!(messages_of("!"), vec!["use not"]);
    assert_eq!(messages_of("&"), vec!["use and / or"]);
    assert_eq!(messages_of("|"), vec!["use and / or"]);
    assert_eq!(messages_of("#"), vec!["unexpected character `#`"]);
    for c in ["$", "'", "`", "^", "~", "\\"] {
        assert_eq!(codes_of(c), vec![codes::INVALID_CHARACTER], "{c}");
    }
    assert_eq!(codes_of("é"), vec![codes::NON_ASCII_OUTSIDE_TEXT]);
    assert_eq!(codes_of("b\"x\""), vec![codes::PREFIXED_LITERAL_RESERVED]);
    assert_eq!(
        messages_of("b\"x\""),
        vec!["prefixed string literals are reserved"]
    );
    // Comments may contain non-ASCII (D1).
    assert_eq!(kinds("// héllo 漢字\nx"), vec![T::LowerIdent, T::Eof]);
    // `!=` is fine.
    assert_eq!(
        kinds("a != b"),
        vec![T::LowerIdent, T::BangEq, T::LowerIdent, T::Eof]
    );
    // The lexer recovers after an error: later tokens are still produced.
    let (t, d) = lex("let x; = 1", FileId(0));
    assert_eq!(d.len(), 1);
    assert_eq!(
        t.iter().map(|t| t.kind).collect::<Vec<_>>(),
        vec![T::KwLet, T::LowerIdent, T::Eq, T::Int, T::Eof]
    );
}

#[test]
fn spans_bom_and_trivia() {
    let t = tokens("\u{feff}module a");
    assert_eq!(t[0].kind, T::KwModule);
    assert_eq!(t[0].span.start, 3);
    let t = tokens("ab cd");
    assert_eq!((t[0].span.start, t[0].span.end), (0, 2));
    assert_eq!((t[1].span.start, t[1].span.end), (3, 5));
    assert_eq!(t[2].kind, T::Eof);
    assert_eq!((t[2].span.start, t[2].span.end), (5, 5));

    let lexed = lex_full("a // c\n\n  \nb // d", FileId(0));
    assert!(lexed.diagnostics.is_empty());
    let kinds: Vec<TriviaKind> = lexed.trivia.iter().map(|t| t.kind).collect();
    assert_eq!(
        kinds,
        vec![
            TriviaKind::Comment,
            TriviaKind::BlankLine,
            TriviaKind::BlankLine,
            TriviaKind::Comment
        ]
    );
    assert_eq!(
        (lexed.trivia[0].span.start, lexed.trivia[0].span.end),
        (2, 6)
    );
    assert_eq!(lexed.trivia[0].line_start, 0);
    assert_eq!(lexed.trivia[3].line_start, 11);
    // Tokens carry their payload text.
    let t = tokens("foo Bar 12 \"x\" ?h");
    let texts: Vec<&str> = t.iter().map(|t| t.text.as_str()).collect();
    assert_eq!(texts, vec!["foo", "Bar", "12", "x", "h", ""]);
}
