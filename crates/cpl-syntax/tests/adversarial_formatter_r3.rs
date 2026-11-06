//! Adversarial tests, round 3, lens: the canonical formatter.
//!
//! Every expectation encodes `spec/toolchain.md` (edition 0.1): Section 2.3
//! (comment attachment, rules A to G, and the blank-line model), Section 2.4
//! (canonical layout, rules 1 to 12, with the decisions T11 [D-146], T12
//! [D-147], T13 [D-148], T14 [D-149], and T51 [D-241]), and the idempotency
//! row of Section 12, together with the grammar sections they rest on
//! (`spec/grammar.md` 1.1, 1.2, 2.1 to 2.5). Each test names its rule in a
//! comment. Where the specifications are silent or contradict each other the
//! comment says so and names the reading chosen; the design document's
//! goals that decide such readings are lines 44 ("The canonical formatter
//! preserves declaration order and formats one statement per line"), 634
//! ("The formatter keeps annotations and these clauses attached to their
//! declaration"), and the Milestone 1 evidence "Formatter is idempotent".
//!
//! Every positive case goes through [`case_failures`], which asserts, besides
//! the exact canonical text the specification implies:
//!
//! * toolchain 12, assertion 1: `fmt(fmt(x)) == fmt(x)`, and the expected
//!   text is itself a fixed point;
//! * toolchain 12, assertion 2: `strip_layout(parse(fmt(x))) ==
//!   strip_layout(parse(x))`, computed on the AST dump with every trivia
//!   list and every `trailing_comments` list removed, the wrappers that only
//!   carry a comment unwrapped, and the clause and `.work` item lists put in
//!   the canonical order T13 lets the formatter impose (the dump carries no
//!   spans, no `Group::multiline`, and no `BinaryExpr::break_before_op`);
//!   doc lines are compared without trailing spaces and tabs, which rule 1
//!   removes although Section 12 does not list them (a spec issue);
//! * 2.4 rule 11 and T11: every comment of the input survives exactly once,
//!   its text unchanged up to trailing spaces and tabs;
//! * T14: the multiset of tokens is unchanged up to line terminators and
//!   trailing commas (rule 6 adds one to every element of a broken group and
//!   prints a flat group without one; grammar 2.5 leaves the comma to the
//!   formatter);
//! * rules 1 and 4 on the output: LF only, no trailing space or tab, no tab
//!   outside a comment, exactly one final LF, no byte-order mark, and no
//!   blank line at the start, after a line ending in `{`, before a line
//!   beginning with `}`, or next to another blank line;
//! * grammar 1.1: the input with CRLF terminators, and the input behind a
//!   byte-order mark, format to the same text.
//!
//! A few inputs are written as `concat!` of two literals that do not parse
//! on their own. `tests/formatter.rs` harvests every string literal of the
//! crate's tests and compares comment and doc-comment texts exactly before
//! and after formatting, and rules 1 and 11 drop their trailing whitespace,
//! so it cannot take these inputs; [`case_failures`] checks them instead.
//!
//! Tests never assert the formatter's current behaviour; a failing test
//! marks a defect or a spec issue reported with the round's results.

use cpl_diag::{Diagnostic, FileId, FileOrigin, SourceMap};
use cpl_syntax::ast::*;
use cpl_syntax::{
    codes, dump_module, dump_patch, dump_work, format_fragment, format_source, lex, lex_full,
    parse_module, parse_patch, parse_work, FileKind, Fragment, FragmentMode, TokenKind, TriviaKind,
};

const CPL: FileKind = FileKind::Cpl;
const WORK: FileKind = FileKind::Work;
const PATCH: FileKind = FileKind::Patch;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn render(src: &str, diagnostics: &[Diagnostic]) -> String {
    let mut map = SourceMap::new();
    map.add("input".to_string(), FileOrigin::Virtual, src.to_string());
    cpl_diag::render_all(diagnostics, &map)
}

fn try_fmt(src: &str, kind: FileKind) -> Result<String, String> {
    format_source(src, kind, FileId(0)).map_err(|d| render(src, &d))
}

#[track_caller]
fn fmt(src: &str, kind: FileKind) -> String {
    match try_fmt(src, kind) {
        Ok(text) => text,
        Err(e) => panic!("expected a clean parse of:\n{src}\n{e}"),
    }
}

/// `stmts`, one per line and unindented, as the body of `fn f() -> Int`.
fn wrap(stmts: &str) -> String {
    let body: Vec<String> = stmts
        .lines()
        .map(|l| {
            if l.is_empty() {
                String::new()
            } else {
                format!("    {l}")
            }
        })
        .collect();
    format!("module m\n\nfn f() -> Int {{\n{}\n}}\n", body.join("\n"))
}

/// The diagnostic of a file the formatter must refuse (T7): exactly one,
/// with a `parse` code (grammar D59, toolchain T6).
#[track_caller]
fn refused(kind: FileKind, src: &str) -> Diagnostic {
    match format_source(src, kind, FileId(0)) {
        Ok(text) => panic!("T7: the formatter must refuse\n{src:?}\nbut printed\n{text}"),
        Err(mut d) => {
            assert_eq!(d.len(), 1, "D59: one diagnostic for {src:?}: {d:#?}");
            assert!(codes::ALL.contains(&d[0].code), "{d:#?}");
            assert_eq!(d[0].category, codes::CATEGORY, "{d:#?}");
            d.remove(0)
        }
    }
}

fn field<'a>(d: &'a Diagnostic, key: &str) -> Option<&'a str> {
    d.data.get(key).and_then(|v| v.as_str())
}

/// The `docs` facet text (toolchain 1.5, `Fragment::Docs`) of declaration
/// `index` of a `.cpl` source.
fn docs_facet(src: &str, index: usize) -> String {
    let m = parse_module(src, FileId(0)).value.expect("parses");
    let docs = m.declarations[index].docs.as_ref().expect("docs");
    format_fragment(Fragment::Docs(docs), FragmentMode::Canonical)
}

// ---- strip_layout on the dump ------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
enum Sx {
    Atom(String),
    List(Vec<Sx>),
}

fn parse_sx(text: &str) -> Sx {
    fn skip(b: &[u8], i: &mut usize) {
        while *i < b.len() && b[*i].is_ascii_whitespace() {
            *i += 1;
        }
    }
    fn parse(b: &[u8], i: &mut usize) -> Sx {
        skip(b, i);
        match b[*i] {
            b'(' => {
                *i += 1;
                let mut items = Vec::new();
                loop {
                    skip(b, i);
                    if b[*i] == b')' {
                        *i += 1;
                        return Sx::List(items);
                    }
                    items.push(parse(b, i));
                }
            }
            b'"' => {
                let start = *i;
                *i += 1;
                while b[*i] != b'"' {
                    if b[*i] == b'\\' {
                        *i += 1;
                    }
                    *i += 1;
                }
                *i += 1;
                Sx::Atom(String::from_utf8_lossy(&b[start..*i]).into_owned())
            }
            _ => {
                let start = *i;
                while *i < b.len() && !b[*i].is_ascii_whitespace() && b[*i] != b'(' && b[*i] != b')'
                {
                    *i += 1;
                }
                Sx::Atom(String::from_utf8_lossy(&b[start..*i]).into_owned())
            }
        }
    }
    let mut i = 0;
    parse(text.as_bytes(), &mut i)
}

/// The canonical position of a child the formatter may move (T13): header
/// clauses (rule 7, grammar D66) and `.work` item kinds (rule 12).
fn clause_rank(parent: &str, child: &Sx) -> usize {
    let Sx::List(items) = child else { return 0 };
    let Some(Sx::Atom(head)) = items.first() else {
        return 0;
    };
    match (parent, head.as_str()) {
        ("fn", "effects") => 1,
        ("fn" | "op", "requires") => 2,
        ("fn" | "op", "ensures") => 3,
        ("fn", "decreases") => 4,
        ("while", "invariant") => 1,
        ("while", "decreases") => 2,
        ("change", "base") => 1,
        ("change", "target") => 2,
        ("change", "workers") => 3,
        ("change", "task") => 4,
        ("change", "integrate") => 5,
        ("task", "write") => 1,
        ("task", "add") => 2,
        ("task", "depend") => 3,
        ("task", "preserve") => 4,
        ("task", "goal") => 5,
        _ => 0,
    }
}

fn strip_sx(sx: Sx) -> Option<Sx> {
    let Sx::List(items) = sx else {
        return Some(sx);
    };
    let head = match items.first() {
        Some(Sx::Atom(a)) => a.clone(),
        _ => String::new(),
    };
    if head == "trivia" || head == "trailing-comments" {
        return None;
    }
    let mut out: Vec<Sx> = items.into_iter().filter_map(strip_sx).collect();
    if matches!(head.as_str(), "item" | "elem" | "arg" | "effect" | "sel") && out.len() == 2 {
        return out.pop();
    }
    if matches!(head.as_str(), "fn" | "op" | "while" | "change" | "task") && !out.is_empty() {
        let mut rest = out.split_off(1);
        rest.sort_by_key(|c| clause_rank(&head, c));
        out.extend(rest);
    }
    Some(Sx::List(out))
}

fn trim_doc_lines(docs: &mut Option<Docs>) {
    if let Some(d) = docs {
        for line in &mut d.lines {
            line.text = line.text.trim_end_matches([' ', '\t']).to_string();
        }
    }
}

fn trim_module_docs(m: &mut Module) {
    trim_doc_lines(&mut m.docs);
    for d in &mut m.declarations {
        trim_doc_lines(&mut d.docs);
        match &mut d.kind {
            DeclKind::Struct(s) => s
                .fields
                .iter_mut()
                .for_each(|f| trim_doc_lines(&mut f.docs)),
            DeclKind::Enum(e) => e
                .variants
                .iter_mut()
                .for_each(|v| trim_doc_lines(&mut v.docs)),
            DeclKind::Capability(c) => c
                .operations
                .iter_mut()
                .for_each(|o| trim_doc_lines(&mut o.docs)),
            _ => {}
        }
    }
}

fn trim_work_docs(w: &mut WorkFile) {
    for c in &mut w.changes {
        trim_doc_lines(&mut c.docs);
        for item in &mut c.items {
            if let ChangeItemKind::Task(t) = &mut item.kind {
                trim_doc_lines(&mut t.docs);
            }
        }
    }
}

/// Toolchain 12's `strip_layout(parse(text))`, as a stripped dump.
fn stripped(text: &str, kind: FileKind) -> Result<Sx, String> {
    let dump = match kind {
        FileKind::Cpl => {
            let r = parse_module(text, FileId(0));
            match r.value {
                Some(mut m) if r.diagnostics.is_empty() => {
                    trim_module_docs(&mut m);
                    dump_module(&m)
                }
                _ => return Err(render(text, &r.diagnostics)),
            }
        }
        FileKind::Work => {
            let r = parse_work(text, FileId(0));
            match r.value {
                Some(mut w) if r.diagnostics.is_empty() => {
                    trim_work_docs(&mut w);
                    dump_work(&w)
                }
                _ => return Err(render(text, &r.diagnostics)),
            }
        }
        FileKind::Patch => {
            let r = parse_patch(text, FileId(0));
            match r.value {
                Some(mut p) if r.diagnostics.is_empty() => {
                    for f in &mut p.forms {
                        trim_doc_lines(&mut f.docs);
                    }
                    dump_patch(&p)
                }
                _ => return Err(render(text, &r.diagnostics)),
            }
        }
    };
    Ok(strip_sx(parse_sx(&dump)).expect("a file node"))
}

/// The ordinary comments of a source, as a sorted multiset of their texts
/// without trailing spaces and tabs.
fn comments_of(src: &str) -> Vec<String> {
    let lexed = lex_full(src, FileId(0));
    let mut out: Vec<String> = lexed
        .trivia
        .iter()
        .filter(|t| t.kind == TriviaKind::Comment)
        .map(|t| {
            src[t.span.start as usize..t.span.end as usize]
                .trim_end_matches([' ', '\t'])
                .to_string()
        })
        .collect();
    out.sort();
    out
}

/// T14: the tokens of a source as a sorted multiset, without line
/// terminators and trailing commas; doc comment texts without trailing
/// spaces and tabs (rule 1).
fn significant_tokens(src: &str) -> Vec<(TokenKind, String)> {
    let (tokens, _) = lex(src, FileId(0));
    let toks: Vec<_> = tokens
        .iter()
        .filter(|t| !matches!(t.kind, TokenKind::Newline | TokenKind::Eof))
        .collect();
    let mut out = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        let closes = toks.get(i + 1).is_some_and(|n| {
            matches!(
                n.kind,
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace
            )
        });
        if t.kind == TokenKind::Comma && closes {
            continue;
        }
        let text = if t.kind == TokenKind::DocComment {
            t.text.trim_end_matches([' ', '\t']).to_string()
        } else {
            t.text.clone()
        };
        out.push((t.kind, text));
    }
    out.sort();
    out
}

/// Rules 1 and 4 on a formatted file.
fn layout_violations(out: &str) -> Vec<String> {
    let mut v = Vec::new();
    if out.starts_with('\u{feff}') {
        v.push("rule 1: the output starts with a byte-order mark".to_string());
    }
    if out.contains('\r') {
        v.push("rule 1: the output contains a carriage return".to_string());
    }
    if !out.ends_with('\n') || out.ends_with("\n\n") {
        v.push("rule 1: the output does not end with exactly one LF".to_string());
    }
    let lexed = lex_full(out, FileId(0));
    let in_comment = |at: usize| {
        let inside = |s: cpl_diag::Span| (s.start as usize) <= at && at < s.end as usize;
        lexed
            .trivia
            .iter()
            .any(|t| t.kind == TriviaKind::Comment && inside(t.span))
            || lexed
                .tokens
                .iter()
                .any(|t| t.kind == TokenKind::DocComment && inside(t.span))
    };
    for (at, _) in out.match_indices('\t') {
        if !in_comment(at) {
            v.push(format!("rule 1: a tab outside a comment at byte {at}"));
        }
    }
    let all: Vec<&str> = out.split('\n').collect();
    let lines = &all[..all.len().saturating_sub(1)];
    for (i, line) in lines.iter().enumerate() {
        if line.ends_with([' ', '\t']) {
            v.push(format!("rule 1: trailing whitespace on line {}", i + 1));
        }
        if !line.is_empty() {
            continue;
        }
        if i == 0 {
            v.push("rule 4: a blank line at the start of the file".to_string());
            continue;
        }
        let prev = lines[i - 1];
        if prev.is_empty() {
            v.push(format!(
                "rule 4: two blank lines in a row at line {}",
                i + 1
            ));
        }
        if prev.ends_with('{') && !prev.contains("//") {
            v.push(format!("rule 4: a blank line after `{{` at line {}", i + 1));
        }
        if lines
            .get(i + 1)
            .is_some_and(|n| n.trim_start().starts_with('}'))
        {
            v.push(format!(
                "rule 4: a blank line before `}}` at line {}",
                i + 1
            ));
        }
    }
    v
}

/// Every check of the module documentation for one input; `expected` is the
/// canonical text, or `None` where the specification does not fix it.
fn case_failures(label: &str, kind: FileKind, src: &str, expected: Option<&str>) -> Vec<String> {
    let mut f = Vec::new();
    let once = match try_fmt(src, kind) {
        Ok(t) => t,
        Err(e) => {
            f.push(format!("{label}: the input does not format:\n{src}\n{e}"));
            return f;
        }
    };
    if let Some(expected) = expected {
        if once != expected {
            f.push(format!(
                "{label}: canonical text differs\n--- input\n{src}\n--- expected\n{expected}\n--- actual\n{once}"
            ));
        }
        match try_fmt(expected, kind) {
            Ok(t) if t == expected => {}
            Ok(t) => f.push(format!(
                "{label}: the expected canonical text is not a fixed point; fmt(expected):\n{t}"
            )),
            Err(e) => f.push(format!("{label}: the expected text does not parse:\n{e}")),
        }
    }
    match try_fmt(&once, kind) {
        Ok(twice) if twice == once => {}
        Ok(twice) => f.push(format!(
            "{label}: toolchain 12 assertion 1, fmt(fmt(x)) != fmt(x)\n--- fmt(x)\n{once}\n--- fmt(fmt(x))\n{twice}"
        )),
        Err(e) => f.push(format!("{label}: fmt(x) does not parse:\n{once}\n{e}")),
    }
    match (stripped(src, kind), stripped(&once, kind)) {
        (Ok(a), Ok(b)) if a == b => {}
        (Ok(a), Ok(b)) => f.push(format!(
            "{label}: toolchain 12 assertion 2, strip_layout(parse(fmt(x))) != strip_layout(parse(x))\n--- parse(x)\n{a:?}\n--- parse(fmt(x))\n{b:?}"
        )),
        (Err(e), _) | (_, Err(e)) => f.push(format!("{label}: does not parse: {e}")),
    }
    let (before, after) = (comments_of(src), comments_of(&once));
    if before != after {
        f.push(format!(
            "{label}: T11 / rule 11, the comments changed\n--- input {before:?}\n--- output {after:?}"
        ));
    }
    if significant_tokens(src) != significant_tokens(&once) {
        f.push(format!(
            "{label}: T14, a token was inserted, removed, or rewritten\n--- input\n{src}\n--- output\n{once}"
        ));
    }
    for v in layout_violations(&once) {
        f.push(format!("{label}: {v}\n{once}"));
    }
    if !src.contains('\r') {
        match try_fmt(&src.replace('\n', "\r\n"), kind) {
            Ok(t) if t == once => {}
            Ok(t) => f.push(format!(
                "{label}: grammar 1.1, the CRLF input formats differently:\n{t}"
            )),
            Err(e) => f.push(format!("{label}: grammar 1.1, the CRLF input fails:\n{e}")),
        }
    }
    if !src.starts_with('\u{feff}') {
        match try_fmt(&format!("\u{feff}{src}"), kind) {
            Ok(t) if t == once => {}
            Ok(t) => f.push(format!(
                "{label}: rule 1, the input behind a byte-order mark formats differently:\n{t}"
            )),
            Err(e) => f.push(format!("{label}: grammar 1.1, the BOM input fails:\n{e}")),
        }
    }
    f
}

#[track_caller]
fn check_cases(cases: &[(FileKind, &str, &str)]) {
    let mut failures = Vec::new();
    for (i, (kind, src, expected)) in cases.iter().enumerate() {
        failures.extend(case_failures(
            &format!("case {i} ({})", kind.extension()),
            *kind,
            src,
            Some(expected),
        ));
    }
    assert!(
        failures.is_empty(),
        "{} failure(s):\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

#[track_caller]
fn check(kind: FileKind, src: &str, expected: &str) {
    check_cases(&[(kind, src, expected)]);
}

/// The reading-independent checks only; returns `fmt(src)`.
#[track_caller]
fn check_stable(kind: FileKind, src: &str) -> String {
    let failures = case_failures("case", kind, src, None);
    assert!(
        failures.is_empty(),
        "{} failure(s):\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    fmt(src, kind)
}

fn longest_line(text: &str) -> usize {
    text.lines().map(|l| l.chars().count()).max().unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Rule 1: indentation, terminators, the final LF, the byte-order mark
// ---------------------------------------------------------------------------

#[test]
fn rule1_tabs_crlf_a_bom_and_trailing_whitespace_normalise_at_every_depth() {
    // Rule 1: four spaces per level and never a tab, LF terminators, no
    // trailing whitespace, one final LF, the byte-order mark removed;
    // grammar 1.1 makes CRLF one terminator and a tab horizontal whitespace,
    // so the tree is unchanged.
    check_cases(&[
        (
            CPL,
            "\u{feff}module m\r\n\r\nfn f(c: Bool) -> Int {\r\n\twhile c {\r\n\t\tif c {\r\n\t\t\tmatch c {\r\n\t\t\t\ttrue => 1   \r\n\t\t\t\t_ => 2\t\r\n\t\t\t}\r\n\t\t}\r\n\t}\r\n\t3 \t\r\n}\r\n\r\n\r\n",
            "module m\n\nfn f(c: Bool) -> Int {\n    while c {\n        if c {\n            match c {\n                true => 1\n                _ => 2\n            }\n        }\n    }\n    3\n}\n",
        ),
        (
            WORK,
            concat!(
                "use\ta.b.{f}\r\n\r\nchange\tc\t{\r\n\tbase\tgit(\"a\")\t// b\t\r\n",
                "\ttask\tt\tafter\tx\t{\r\n\t\twrite\t{\tf.body\t}\r\n\t}\r\n}\r\n",
            ),
            "use a.b.{f}\n\nchange c {\n    base git(\"a\") // b\n    task t after x {\n        write { f.body }\n    }\n}\n",
        ),
        (
            PATCH,
            "patch\tf.body\r\n\texpect\tbase(f.body)\r\n{\r\n\tlet\tx\t=\t1\r\n\tx\r\n}",
            "patch f.body\n    expect base(f.body)\n{\n    let x = 1\n    x\n}\n",
        ),
    ]);
}

#[test]
fn rule1_every_file_form_ends_with_exactly_one_line_feed() {
    // Rule 1 ("The file ends with exactly one LF"); grammar 1.1 makes the
    // final terminator optional.
    check_cases(&[
        (CPL, "module m // c", "module m // c\n"),
        (CPL, "module m\r\n\r\n\r\n", "module m\n"),
        (
            CPL,
            "module m\n\nfn f() -> Int { 1 }\n// end",
            "module m\n\nfn f() -> Int {\n    1\n}\n// end\n",
        ),
        (
            CPL,
            "module m\n\nfn f() -> Int { 1 }\n\n\n  \n\t\n",
            "module m\n\nfn f() -> Int {\n    1\n}\n",
        ),
        (WORK, "change c {}", "change c {}\n"),
        (
            WORK,
            "change c {}\r\n\r\n// end\r\n\r\n",
            "change c {}\n\n// end\n",
        ),
        (
            PATCH,
            "patch f.body\r\n    expect base(f.body)\r\n{\r\n}",
            "patch f.body\n    expect base(f.body)\n{}\n",
        ),
        (
            PATCH,
            "patch f.body\n    expect base(f.body)\n{\n} // done",
            "patch f.body\n    expect base(f.body)\n{} // done\n",
        ),
    ]);
}

#[test]
fn rule1_trailing_spaces_and_tabs_leave_comments_and_doc_comments_but_inner_tabs_stay() {
    // Rule 1: no trailing whitespace; rule 11: a comment's text is kept
    // byte for byte from `//`, trailing whitespace dropped; rule 5: `/// ` +
    // text, or `///` for an empty line. A tab inside a comment or a doc
    // comment is part of its text (rule 11's "byte-for-byte"); rule 1's
    // "tabs are never emitted" is read as a statement about indentation and
    // the space between tokens (reported as a spec issue). The comment among
    // the prefix lines prints above the doc comments (rule 5).
    let src = concat!(
        "module m\n\n/// doc  \t\n///\t\n",
        "//\tbanner\t \nfn f() -> Int { // c\t\n    1 //\tone  \n}\n",
    );
    let expected =
        "module m\n\n//\tbanner\n/// doc\n///\nfn f() -> Int {\n    // c\n    1 //\tone\n}\n";
    check(CPL, src, expected);
    // The `docs` facet (toolchain 1.5: each line without trailing
    // whitespace) does not change, which is why formatting may trim.
    assert_eq!(docs_facet(src, 0), docs_facet(expected, 0));
}

#[test]
fn rule1_only_a_leading_byte_order_mark_is_removed() {
    // Rule 1 removes "a byte-order mark"; grammar 1.1 skips a *leading* one.
    // One inside a comment or a text literal is content (grammar D1, D62,
    // rule 10: every other scalar value prints literally).
    check_cases(&[
        (
            CPL,
            "\u{feff}// c\u{feff}\nmodule m\n",
            "// c\u{feff}\nmodule m\n",
        ),
        (
            CPL,
            "\u{feff}module m\n\nfn f() -> Text {\n    \"\u{feff}\"\n}\n",
            "module m\n\nfn f() -> Text {\n    \"\u{feff}\"\n}\n",
        ),
        (WORK, "\u{feff}change c {}\n", "change c {}\n"),
        (
            PATCH,
            "\u{feff}patch f.body\n    expect base(f.body)\n{\n}\n",
            "patch f.body\n    expect base(f.body)\n{}\n",
        ),
    ]);
    // A second one is a non-ASCII character outside a text literal or a
    // comment (grammar 1.1), and the formatter refuses the file (T7).
    let d = refused(CPL, "\u{feff}\u{feff}module m\n");
    assert_eq!(d.code, codes::NON_ASCII_OUTSIDE_TEXT, "{d:#?}");
    assert_eq!(d.span.map(|s| s.start), Some(3), "{d:#?}");
}

#[test]
fn crlf_comment_and_doc_comment_text_excludes_the_carriage_return() {
    // Toolchain 2.1: `Comment::text` runs "from `//` to the end of the
    // line", and grammar 1.1 makes CRLF one terminator, so the CR is part of
    // the terminator, not of the comment; likewise a doc line's text.
    let m = parse_module(
        "/// d\r\nmodule m // c\r\n\r\n// lead\r\nfn f() -> Int { 1 } // t\r\n",
        FileId(0),
    )
    .value
    .expect("parses");
    assert_eq!(m.trivia.trailing[0].text, "// c");
    assert_eq!(m.docs.as_ref().unwrap().lines[0].text, "d");
    assert_eq!(m.declarations[0].trivia.leading[0].text, "// lead");
    assert_eq!(m.declarations[0].trivia.trailing[0].text, "// t");
}

// ---------------------------------------------------------------------------
// Rule 2: one node per line
// ---------------------------------------------------------------------------

#[test]
fn rule2_members_items_and_arms_written_on_the_brace_lines_get_a_line_each() {
    // Rule 2: one statement, member, clause, item, arm, or `.work` item per
    // line; grammar 2.3 lets the first and last element of a body share the
    // lines of its braces.
    check_cases(&[
        (
            CPL,
            "module m\n\nstruct S { a: Int\n    b: Int }\n\nenum E { A\n    B(Int) }\n\ncapability C { fn a() -> Int\n    fn b() -> Int }\n\nfn f(x: Int) -> Int {\n    match x { 1 => 2\n        _ => 3 }\n}\n",
            "module m\n\nstruct S {\n    a: Int\n    b: Int\n}\n\nenum E {\n    A\n    B(Int)\n}\n\ncapability C {\n    fn a() -> Int\n    fn b() -> Int\n}\n\nfn f(x: Int) -> Int {\n    match x {\n        1 => 2\n        _ => 3\n    }\n}\n",
        ),
        (
            WORK,
            "change c { base git(\"a\")\n    task t { goal \"g\"\n        write {} }\n    integrate { require typecheck } }\n",
            "change c {\n    base git(\"a\")\n    task t {\n        write {}\n        goal \"g\"\n    }\n    integrate {\n        require typecheck\n    }\n}\n",
        ),
        (
            PATCH,
            "patch f.body\n    expect base(f.body)\n{ let x = 1\n    x }\n",
            "patch f.body\n    expect base(f.body)\n{\n    let x = 1\n    x\n}\n",
        ),
    ]);
}

// ---------------------------------------------------------------------------
// Rule 3: file order; T51
// ---------------------------------------------------------------------------

#[test]
fn rule3_one_blank_line_follows_the_header_and_the_uses_whatever_the_source_had() {
    // Rule 3: module doc comments, header, one blank line, `use`
    // declarations, one blank line, items separated by exactly one blank
    // line; no blank line between the doc comments and the header. Rule 12
    // gives `.work` files the same shape.
    check_cases(&[
        (
            CPL,
            "/// Module.\nmodule m\nuse a.b.c\nuse a.b.{d}\nfn f() -> Int { 1 }\n",
            "/// Module.\nmodule m\n\nuse a.b.c\nuse a.b.{d}\n\nfn f() -> Int {\n    1\n}\n",
        ),
        (
            CPL,
            "module m\n\n\n\nuse a.b.c\n\n\n\nfn f() -> Int { 1 }\n\n\n\nfn g() -> Int { 2 }\n",
            "module m\n\nuse a.b.c\n\nfn f() -> Int {\n    1\n}\n\nfn g() -> Int {\n    2\n}\n",
        ),
        (
            CPL,
            "/// d\n\nmodule m\nfn f() -> Int { 1 }",
            "/// d\nmodule m\n\nfn f() -> Int {\n    1\n}\n",
        ),
        (CPL, "module m\nuse a.b.c", "module m\n\nuse a.b.c\n"),
        (
            WORK,
            "use a.b.{f}\nuse a.b.{g}\nchange c {}\nchange d {}\n\n\n\nchange e {}\n",
            "use a.b.{f}\nuse a.b.{g}\n\nchange c {}\n\nchange d {}\n\nchange e {}\n",
        ),
    ]);
}

#[test]
fn rule3_comments_around_the_header_and_between_the_sections_keep_their_places() {
    // 2.3 rule D: the comments above the header (also those between the
    // module's doc comments and the header) are the module's and print above
    // its doc comments, as rule 5 prints a declaration's; a comment between
    // the header and the first `use` or declaration leads that node, after
    // rule 3's blank line; rule F: the file's last comments.
    check(
        CPL,
        "// license\n\n// more\n/// Docs.\n// between docs and header\nmodule m // header\n// about the use\nuse a.b.c // the use\n// about f\nfn f() -> Int { 1 }\n// end\n",
        "// license\n\n// more\n// between docs and header\n/// Docs.\nmodule m // header\n\n// about the use\nuse a.b.c // the use\n\n// about f\nfn f() -> Int {\n    1\n}\n// end\n",
    );
}

#[test]
fn rule3_t51_adjacent_bare_types_stay_together_in_chains_and_after_multi_line_definitions() {
    // T51 [D-241]: a `type` declaration whose first token is on the line
    // directly below the last line of another `type` declaration stays there
    // when neither has docs or annotations and no blank line or comment
    // separates them; every other pair gets exactly one blank line (rule 3).
    let chain = "module m\n\npub type A(Int)\npub type B(Int)\ntype C<T>\npub type P = Int where (self\n    > 0)\npub type D(Int)\n";
    let trailing = "module m\n\npub type A(Int) // a\npub type B(Int) // b\n";
    check_cases(&[
        (CPL, chain, chain),
        (CPL, trailing, trailing),
        (
            CPL,
            "module m\npub type A(Int)\n\n\npub type B(Int)\nstruct S {}\npub type C(Int)\n",
            "module m\n\npub type A(Int)\n\npub type B(Int)\n\nstruct S {}\n\npub type C(Int)\n",
        ),
        (
            CPL,
            "module m\n\npub type A(Int)\n// about B\npub type B(Int)\n",
            "module m\n\npub type A(Int)\n\n// about B\npub type B(Int)\n",
        ),
        (
            CPL,
            "module m\n\n/// a\npub type A(Int)\npub type B(Int)\n",
            "module m\n\n/// a\npub type A(Int)\n\npub type B(Int)\n",
        ),
        (
            CPL,
            "module m\n\nfn f() -> Int { 1 }\npub type A(Int)\n",
            "module m\n\nfn f() -> Int {\n    1\n}\n\npub type A(Int)\n",
        ),
    ]);
}

// ---------------------------------------------------------------------------
// Rule 4: blank lines inside bodies; clauses
// ---------------------------------------------------------------------------

#[test]
fn rule4_blank_lines_collapse_to_one_in_every_brace_body_and_never_touch_its_braces() {
    // Rule 4: one blank line where the source had one or more, none after
    // `{` or before `}`; 2.3: the first node of a body never records one.
    check_cases(&[
        (
            CPL,
            "module m\n\nstruct S {\n\n\n    a: Int\n\n\n\n    b: Int\n\n\n}\n\nenum E {\n\n    A\n\n\n    B\n\n}\n\ncapability C {\n\n    fn a() -> Int\n\n\n    fn b() -> Int\n\n}\n\nfn f(x: Int) -> Int {\n    match x {\n\n        1 => 1\n\n\n        _ => 2\n\n    }\n}\n",
            "module m\n\nstruct S {\n    a: Int\n\n    b: Int\n}\n\nenum E {\n    A\n\n    B\n}\n\ncapability C {\n    fn a() -> Int\n\n    fn b() -> Int\n}\n\nfn f(x: Int) -> Int {\n    match x {\n        1 => 1\n\n        _ => 2\n    }\n}\n",
        ),
        (
            WORK,
            "change c {\n\n\n    base git(\"a\")\n\n\n    target git(\"b\")\n\n    task t {\n\n        write {}\n\n\n        goal \"g\"\n\n    }\n\n    integrate {\n\n        require typecheck\n\n\n        require no_holes\n\n    }\n\n}\n",
            "change c {\n    base git(\"a\")\n\n    target git(\"b\")\n\n    task t {\n        write {}\n\n        goal \"g\"\n    }\n\n    integrate {\n        require typecheck\n\n        require no_holes\n    }\n}\n",
        ),
        (
            PATCH,
            "patch f.body\n    expect base(f.body)\n{\n\n    let x = 1\n\n\n    x\n\n}\n",
            "patch f.body\n    expect base(f.body)\n{\n    let x = 1\n\n    x\n}\n",
        ),
    ]);
}

#[test]
fn rule4_lines_holding_only_spaces_and_tabs_are_blank_lines() {
    // 2.3: a blank line is "a line consisting only of horizontal whitespace"
    // (grammar 1.1: space and tab); T11 keeps one between a comment and a
    // node that is not the first of its body.
    check(
        CPL,
        "module m\n\nfn f() -> Int {\n    let x = 1\n  \t  \n    let y = 2\n\t\n    // c\n \n    x\n}\n",
        "module m\n\nfn f() -> Int {\n    let x = 1\n\n    let y = 2\n\n    // c\n\n    x\n}\n",
    );
}

#[test]
fn rule4_clauses_and_their_comments_never_have_blank_lines_between_them() {
    // Rule 4: clauses ignore `blank_line_before`, "none between them"; a
    // blank line before a clause's leading comment also lies between clauses
    // and is dropped. The `expect` line is a clause (grammar 4.5) with the
    // fixed layout of rule 12.
    check_cases(&[
        (
            CPL,
            "module m\n\nfn f(a: Int) -> Int\n\n    // why\n\n    requires a > 0\n\n\n    // second\n\n    ensures result > 0\n\n    decreases a\n\n{\n    a\n}\n",
            "module m\n\nfn f(a: Int) -> Int\n    // why\n    requires a > 0\n    // second\n    ensures result > 0\n    decreases a\n{\n    a\n}\n",
        ),
        (
            CPL,
            "module m\n\nfn f(a: Int) -> Int {\n    var i = a\n    while i > 0\n\n        invariant i >= 0\n\n        // measure\n\n        decreases i\n\n    {\n        i = i - 1\n    }\n    i\n}\n\nproperty \"p\"\n\n    for x in g\n\n    for y in h\n\n{\n    assert true\n}\n\ncapability C {\n    fn a() -> Int\n\n        requires true\n\n        ensures true\n}\n",
            "module m\n\nfn f(a: Int) -> Int {\n    var i = a\n    while i > 0\n        invariant i >= 0\n        // measure\n        decreases i\n    {\n        i = i - 1\n    }\n    i\n}\n\nproperty \"p\"\n    for x in g\n    for y in h\n{\n    assert true\n}\n\ncapability C {\n    fn a() -> Int\n        requires true\n        ensures true\n}\n",
        ),
        (
            PATCH,
            "patch f.body\n\n    // check\n\n    expect base(f.body)\n\n{\n    1\n}\n",
            "patch f.body\n    // check\n    expect base(f.body)\n{\n    1\n}\n",
        ),
    ]);
}

#[test]
fn rule4_a_blank_line_before_a_bodys_last_comments_stays_and_one_before_its_brace_goes() {
    // 2.3: `Comment::blank_line_before` is recorded as written and preserved
    // (rule 11) for a body's or a group's last comments too; rule 4: never a
    // blank line before `}`.
    check_cases(&[
        (
            CPL,
            "module m\n\nfn f() -> Int {\n    let x = 1\n\n    // done\n\n}\n\nstruct S {\n    a: Int\n\n\n    // more later\n\n}\n\nfn g(x: Int) -> Int {\n    match x {\n        _ => 1\n\n        // no more\n    }\n}\n\nfn h() -> Int {\n    k(\n        a,\n\n        // no more\n\n    )\n}\n",
            "module m\n\nfn f() -> Int {\n    let x = 1\n\n    // done\n}\n\nstruct S {\n    a: Int\n\n    // more later\n}\n\nfn g(x: Int) -> Int {\n    match x {\n        _ => 1\n\n        // no more\n    }\n}\n\nfn h() -> Int {\n    k(\n        a,\n\n        // no more\n    )\n}\n",
        ),
        (
            WORK,
            "change c {\n    base git(\"a\")\n\n    // later\n\n}\n",
            "change c {\n    base git(\"a\")\n\n    // later\n}\n",
        ),
    ]);
}

// ---------------------------------------------------------------------------
// Rule 5: declaration layout
// ---------------------------------------------------------------------------

#[test]
fn rule5_doc_lines_print_as_three_slashes_a_space_and_their_text() {
    // Rule 5: `/// ` + text, or `///` for an empty line; the text is the
    // token's text (grammar 1.2: one leading space removed), so extra leading
    // spaces and a leading tab are kept, and a line of spaces is empty.
    check_cases(&[
        (
            CPL,
            concat!(
                "module m\n\n///plain\n///   indented\n///\n///    \n",
                "/// \ttab\npub fn f() -> Int { 1 }\n\npub struct S {\n    ///field\n    x: Int\n}\n",
            ),
            "module m\n\n/// plain\n///   indented\n///\n///\n/// \ttab\npub fn f() -> Int {\n    1\n}\n\npub struct S {\n    /// field\n    x: Int\n}\n",
        ),
        (
            WORK,
            "///change docs\nchange c {\n    ///task docs\n    task t {}\n}\n",
            "/// change docs\nchange c {\n    /// task docs\n    task t {}\n}\n",
        ),
        (
            PATCH,
            "///form\npatch f.body\n    expect base(f.body)\n{\n}\n",
            "/// form\npatch f.body\n    expect base(f.body)\n{}\n",
        ),
    ]);
}

#[test]
fn rule5_blank_lines_inside_a_prefix_are_dropped() {
    // Rule 5: doc comments, then annotations one per line, then the
    // declaration; the lexer emits no NEWLINE after a doc comment (grammar
    // 2.1) and 2.3 records a blank line only before a node's first token or
    // before a comment, so nothing keeps the blank lines of a prefix.
    check(
        CPL,
        "module m\n\n/// a\n\n/// b\n\n@id(\"x\")\n\n@prove\n\npub fn f() -> Int { 1 }\n\nstruct S {\n    /// c\n\n    /// d\n\n    x: Int\n}\n",
        "module m\n\n/// a\n/// b\n@id(\"x\")\n@prove\npub fn f() -> Int {\n    1\n}\n\nstruct S {\n    /// c\n    /// d\n    x: Int\n}\n",
    );
}

#[test]
fn rule5_a_comment_between_a_nodes_doc_comments_and_the_node_prints_above_the_docs() {
    // 2.3 rule D says a comment "among the lines of a declaration's prefix
    // is the declaration's" and rule 5 prints it above the doc comments; the
    // specification is silent for members, changes, tasks, and patch forms,
    // which carry doc comments the same way, so the same layout is expected
    // (design line 634: the formatter keeps what belongs to a node with it).
    check_cases(&[
        (
            CPL,
            "module m\n\nstruct S {\n    /// d\n    // c\n    x: Int\n}\n\nenum E {\n    /// d\n    // c\n    A\n}\n\ncapability C {\n    /// d\n    // c\n    fn a() -> Int\n}\n",
            "module m\n\nstruct S {\n    // c\n    /// d\n    x: Int\n}\n\nenum E {\n    // c\n    /// d\n    A\n}\n\ncapability C {\n    // c\n    /// d\n    fn a() -> Int\n}\n",
        ),
        (
            WORK,
            "/// d\n// c\nchange c {\n    /// t\n    // u\n    task t {\n        goal \"g\"\n    }\n}\n",
            "// c\n/// d\nchange c {\n    // u\n    /// t\n    task t {\n        goal \"g\"\n    }\n}\n",
        ),
        (
            PATCH,
            "/// d\n// c\npatch f.body\n    expect base(f.body)\n{\n}\n",
            "// c\n/// d\npatch f.body\n    expect base(f.body)\n{}\n",
        ),
    ]);
}

#[test]
fn rule5_and_rule6_annotation_arguments_follow_the_group_rule() {
    // Spec conflict: rule 5 says "Annotation arguments are a flat group
    // unless they contain comments", while rule 6 lists annotation arguments
    // among the groups that print broken when `Group::multiline` is set, and
    // T12 [D-147] says a group stays broken once broken. This test follows
    // rule 6 and the numbered decision T12. The elements of an annotation's
    // argument list carry no trivia (2.2), so a comment inside it is the
    // annotation's trailing comment (2.3 rule G), printed after `)`; an empty
    // list prints `()` (rule 6) and keeps its parentheses (T14).
    check(
        CPL,
        "module m\n\n@id(\n    \"x\"\n)\n@x(1,  2, \"a\", true,)\n@empty(\n)\n@c(\"y\" // inside\n)\n@d( // after paren\n)\nfn f() -> Int { 1 }\n",
        "module m\n\n@id(\n    \"x\",\n)\n@x(1, 2, \"a\", true)\n@empty()\n@c(\n    \"y\",\n) // inside\n@d() // after paren\nfn f() -> Int {\n    1\n}\n",
    );
}

// ---------------------------------------------------------------------------
// Rule 6: groups
// ---------------------------------------------------------------------------

#[test]
fn rule6_every_group_kind_written_across_lines_prints_broken_with_trailing_commas() {
    // Rule 6 (i): a group with a line terminator at its own level prints
    // broken: the opening bracket ends the line, each element on its own line
    // one level deeper followed by `,`, the closing bracket on its own line at
    // the original indentation; parameters, `use` lists, variant payloads,
    // effects sets, list and struct literals, arguments, constructor-pattern
    // payloads, and selector sets alike.
    check_cases(&[
        (
            CPL,
            "module m\n\nuse a.b.{\n    c, D}\n\nenum E {\n    A(Int,\n        Text)\n}\n\nfn f(\n    a: Int) -> Int\n    effects { db.get,\n        db.put }\n{\n    let l = [1,\n        2]\n    let p = Page { ..p,\n        text: t }\n    let c = g(a,\n        using db)\n    match r {\n        Ok(v,\n            w) => 1\n    }\n}\n",
            "module m\n\nuse a.b.{\n    c,\n    D,\n}\n\nenum E {\n    A(\n        Int,\n        Text,\n    )\n}\n\nfn f(\n    a: Int,\n) -> Int\n    effects {\n        db.get,\n        db.put,\n    }\n{\n    let l = [\n        1,\n        2,\n    ]\n    let p = Page {\n        ..p,\n        text: t,\n    }\n    let c = g(\n        a,\n        using db,\n    )\n    match r {\n        Ok(\n            v,\n            w,\n        ) => 1\n    }\n}\n",
        ),
        (
            WORK,
            "change c {\n    task t {\n        write { a.body,\n            b.body }\n        add {\n            tests.*\n        }\n    }\n}\n",
            "change c {\n    task t {\n        write {\n            a.body,\n            b.body,\n        }\n        add {\n            tests.*,\n        }\n    }\n}\n",
        ),
    ]);
}

#[test]
fn rule6_only_a_line_break_at_the_groups_own_level_breaks_it() {
    // Toolchain 2.2 (`Group`): a line break inside a nested bracket pair
    // does not count, so `g(...)` stays flat around its broken arguments.
    check(
        CPL,
        &wrap("let x = g(h(\n    1), [2,\n    3])\nx"),
        "module m\n\nfn f() -> Int {\n    let x = g(h(\n        1,\n    ), [\n        2,\n        3,\n    ])\n    x\n}\n",
    );
}

#[test]
fn rule6_a_comment_breaks_every_group_kind_even_an_empty_one() {
    // Rule 6 (ii): a comment on an element or among the group's
    // `trailing_comments` breaks it; a comment after the opening bracket of
    // an empty group is that group's dangling comment (2.3 rule E), so the
    // empty group prints broken with the comment one level deeper.
    check_cases(&[
        (
            CPL,
            "module m\n\nfn f( // none\n) -> Int\n    effects { // pure\n    }\n{\n    let a = g( // nothing\n    )\n    let l = [ // empty\n    ]\n    let p = Page { // defaults\n    }\n    let u = h(a, // first\n        b)\n    a\n}\n",
            "module m\n\nfn f(\n    // none\n) -> Int\n    effects {\n        // pure\n    }\n{\n    let a = g(\n        // nothing\n    )\n    let l = [\n        // empty\n    ]\n    let p = Page {\n        // defaults\n    }\n    let u = h(\n        a, // first\n        b,\n    )\n    a\n}\n",
        ),
        (
            WORK,
            "use a.b.{c, // c\n    d}\n\nchange c {\n    task t {\n        write { // none\n        }\n        depend {\n            // nothing yet\n        }\n    }\n}\n",
            "use a.b.{\n    c, // c\n    d,\n}\n\nchange c {\n    task t {\n        write {\n            // none\n        }\n        depend {\n            // nothing yet\n        }\n    }\n}\n",
        ),
    ]);
}

#[test]
fn rule6_a_flat_group_drops_its_trailing_comma() {
    // Rule 6: a flat group prints `f(a, b)` (grammar 2.5: the formatter
    // decides when to emit a trailing comma), in every group kind.
    check_cases(&[
        (
            CPL,
            "module m\n\nuse a.b.{c, D,}\n\nenum E {\n    A(Int, Text,)\n}\n\nfn f(a: Int, using db: R,) -> Int\n    effects { db.get, }\n{\n    let l = [1, 2,]\n    let p = Page { ..p, text: t, }\n    let c = g(a, using db,)\n    match r {\n        Ok(v, w,) => 1\n    }\n}\n",
            "module m\n\nuse a.b.{c, D}\n\nenum E {\n    A(Int, Text)\n}\n\nfn f(a: Int, using db: R) -> Int\n    effects { db.get }\n{\n    let l = [1, 2]\n    let p = Page { ..p, text: t }\n    let c = g(a, using db)\n    match r {\n        Ok(v, w) => 1\n    }\n}\n",
        ),
        (
            WORK,
            "change c {\n    task t {\n        write { a.body, }\n        preserve { a.signature, a.contract, }\n    }\n}\n",
            "change c {\n    task t {\n        write { a.body }\n        preserve { a.signature, a.contract }\n    }\n}\n",
        ),
    ]);
}

/// A module whose function body holds `stmt` (already indented one level)
/// and then `x`.
fn module_with(stmt: &str) -> String {
    format!("module m\n\nfn f() -> Int {{\n{stmt}\n    x\n}}\n")
}

#[test]
fn rule6_width_is_counted_in_scalar_values() {
    // Rule 6 (iii): "100 columns (counted in scalar values, including
    // indentation)"; two-byte, four-byte, and combining characters each count
    // one. The line `    let x = f("...", b)` has 20 columns besides the
    // text.
    let mut cases: Vec<(String, String)> = Vec::new();
    for (unit, width) in [("\u{e9}", 1), ("\u{1F600}", 1), ("e\u{301}", 2)] {
        let fits = unit.repeat(80 / width);
        let flat = module_with(&format!("    let x = f(\"{fits}\", b)"));
        assert_eq!(longest_line(&flat), 100);
        cases.push((flat.clone(), flat));
        let over = format!("{fits}{}", if width == 1 { unit } else { "e" });
        let long = module_with(&format!("    let x = f(\"{over}\", b)"));
        assert_eq!(longest_line(&long), 101);
        let broken = module_with(&format!(
            "    let x = f(\n        \"{over}\",\n        b,\n    )"
        ));
        cases.push((long, broken));
    }
    let refs: Vec<(FileKind, &str, &str)> = cases
        .iter()
        .map(|(s, e)| (CPL, s.as_str(), e.as_str()))
        .collect();
    check_cases(&refs);
}

#[test]
fn rule6_width_is_measured_on_the_printed_text() {
    // Rule 6 (iii) speaks of "the flat rendering of the line": the width is
    // that of the output, after rule 10 re-escapes text literals (a raw tab
    // prints as `\t`, two columns; `\u{41}` prints as `A`, one column),
    // rule 10 drops digit separators, and rule 8 normalises spacing.
    let a79 = "a".repeat(79);
    let tab_long = module_with(&format!("    let x = f(\"\t{a79}\", b)"));
    assert_eq!(longest_line(&tab_long), 100, "100 columns in the source");
    let tab_broken = module_with(&format!(
        "    let x = f(\n        \"\\t{a79}\",\n        b,\n    )"
    ));
    let escape = module_with(&format!("    let x = f(\"\\u{{41}}{a79}\", b)"));
    assert_eq!(longest_line(&escape), 105, "105 columns in the source");
    let escape_flat = module_with(&format!("    let x = f(\"A{a79}\", b)"));
    assert_eq!(longest_line(&escape_flat), 100);
    let digits = module_with(&format!("    let x = f(1_000_000, {})", "b".repeat(76)));
    assert_eq!(longest_line(&digits), 102);
    let digits_flat = module_with(&format!("    let x = f(1000000, {})", "b".repeat(76)));
    assert_eq!(longest_line(&digits_flat), 100);
    let spaced = module_with(&format!("    let x = f(  a ,    {}  )", "c".repeat(82)));
    assert_eq!(longest_line(&spaced), 108);
    let spaced_flat = module_with(&format!("    let x = f(a, {})", "c".repeat(82)));
    assert_eq!(longest_line(&spaced_flat), 100);
    check_cases(&[
        (CPL, &tab_long, &tab_broken),
        (CPL, &escape, &escape_flat),
        (CPL, &digits, &digits_flat),
        (CPL, &spaced, &spaced_flat),
    ]);
}

#[test]
fn rule6_width_includes_the_indentation() {
    // Rule 6 (iii): "including indentation". The same 85-letter argument
    // fits at level 1 (100 columns) and not at level 2 (104 columns).
    let arg = "a".repeat(85);
    let level1 = format!("module m\n\nfn f() -> Int {{\n    let x = f({arg})\n    x\n}}\n");
    let level2 = format!(
        "module m\n\nfn f(c: Bool) -> Int {{\n    if c {{\n        let x = f({arg})\n        x\n    }} else {{\n        0\n    }}\n}}\n"
    );
    let level2_broken = format!(
        "module m\n\nfn f(c: Bool) -> Int {{\n    if c {{\n        let x = f(\n            {arg},\n        )\n        x\n    }} else {{\n        0\n    }}\n}}\n"
    );
    check_cases(&[(CPL, &level1, &level1), (CPL, &level2, &level2_broken)]);
}

#[test]
fn rule6_a_trailing_comment_is_part_of_the_line_whose_width_is_measured() {
    // Rule 6 (iii) measures "the flat rendering of the line"; the line as
    // printed includes its trailing comment (rule 11), so a comment that takes
    // the line past 100 columns breaks the line's outermost group, and the
    // comment then follows the closing bracket (where the next parse attaches
    // it to the same statement, rule B). Reading of the specification, which
    // does not say whether a comment counts (reported as a spec issue).
    let fits = module_with(&format!("    let x = f(a, b) // {}", "c".repeat(77)));
    assert_eq!(longest_line(&fits), 100);
    let comment = "c".repeat(78);
    let over = module_with(&format!("    let x = f(a, b) // {comment}"));
    let broken = module_with(&format!(
        "    let x = f(\n        a,\n        b,\n    ) // {comment}"
    ));
    check_cases(&[(CPL, &fits, &fits), (CPL, &over, &broken)]);
}

#[test]
fn rule6_the_outermost_group_breaks_first_and_the_rule_is_reapplied_to_each_line() {
    // Rule 6: "the outermost flat group on the line is broken first and the
    // rule is re-applied to each resulting line".
    let nested = module_with("    let x = outer(inner_function_with_a_long_name(argument_number_one, argument_number_two, argument_number_three_is_long), b)");
    let nested_broken = module_with("    let x = outer(\n        inner_function_with_a_long_name(\n            argument_number_one,\n            argument_number_two,\n            argument_number_three_is_long,\n        ),\n        b,\n    )");
    let enough = module_with("    let x = outer(inner(argument_number_one, argument_number_two), another_inner(argument_three, four))");
    let enough_broken = module_with("    let x = outer(\n        inner(argument_number_one, argument_number_two),\n        another_inner(argument_three, four),\n    )");
    assert!(longest_line(&enough) > 100);
    check_cases(&[
        (CPL, &nested, &nested_broken),
        (CPL, &enough, &enough_broken),
    ]);
    // Two outermost groups on one line: the specification does not say which
    // breaks first (reported as a spec issue); whichever it is, exactly one
    // of them breaks, every line then fits, and the result is stable.
    let two = module_with("    let x = first_function(argument_one, argument_two) + second_function(argument_three, argument_four)");
    assert!(longest_line(&two) > 100);
    let out = check_stable(CPL, &two);
    assert!(longest_line(&out) <= 100, "{out}");
    let first_broken = out.contains("first_function(\n");
    let second_broken = out.contains("second_function(\n");
    assert!(
        first_broken != second_broken,
        "exactly one group breaks:\n{out}"
    );
    // The same holds for a constructor pattern and the arm's call: breaking
    // either one makes the line fit.
    let arm = "module m\n\nfn f(r: R) -> Int {\n    match r {\n        Ok(v) => some_function(argument_number_one, argument_number_two, argument_number_three, abcd)\n        _ => 0\n    }\n}\n";
    assert_eq!(longest_line(arm), 101);
    let out = check_stable(CPL, arm);
    assert!(longest_line(&out) <= 100, "{out}");
    let pattern_broken = out.contains("Ok(\n");
    let call_broken = out.contains("some_function(\n");
    assert!(
        pattern_broken != call_broken,
        "exactly one group breaks:\n{out}"
    );
}

#[test]
fn rule6_a_line_with_no_breakable_group_stays_long() {
    // Rule 6 breaks groups only; rule 9 removes line breaks inside
    // expressions and adds none; generic lists, `after` lists, text literals,
    // and the single-item `use` form are not groups.
    let cases = [
        (CPL, module_with("    let x = aaaaaaaaaaaaaaaaaaaa + bbbbbbbbbbbbbbbbbbbbbbbbbbbb + cccccccccccccccccccccccccccc + dddddddddddddddddddd")),
        (CPL, module_with(&format!("    let x = \"{}\"", "t".repeat(120)))),
        (CPL, module_with("    let x: Result<List<List<SomeVeryLongTypeName>>, AnotherVeryLongTypeName<WithGenerics, AndMore>> = y")),
        (CPL, "module m\n\nuse some_very_long_module_name.another_very_long_module_name.yet_another_long_module_name.AndOneMoreItemName\n".to_string()),
        (CPL, "module m\n\nstruct S {\n    a_longer_field: Result<List<List<SomeVeryLongTypeName>>, AnotherVeryLongTypeName<WithGenerics, AndMore>>\n}\n".to_string()),
        (CPL, format!("module m\n\ntest \"{}\" {{\n    assert true\n}}\n", "n".repeat(100))),
        (CPL, "module m\n\npub type P = Int where self > 0 and self < 1000000 and self != 4242424242 and self != 5353535353 and self != 6\n".to_string()),
        (WORK, "change c {\n    task t after first_task_name, second_task_name, third_task_name, fourth_task_name, fifth_task_name, sixth {\n        goal \"g\"\n    }\n}\n".to_string()),
        (WORK, format!("change c {{\n    base git(\"{}\")\n    task t {{\n        goal \"{}\"\n    }}\n}}\n", "r".repeat(100), "g".repeat(100))),
    ];
    for (_, src) in &cases {
        assert!(longest_line(src) > 100, "not a long line:\n{src}");
    }
    let refs: Vec<(FileKind, &str, &str)> = cases
        .iter()
        .map(|(k, s)| (*k, s.as_str(), s.as_str()))
        .collect();
    check_cases(&refs);
}

#[test]
fn rule6_width_breaks_every_group_kind_at_101_columns_and_none_at_100() {
    // Rule 6 (iii) for each group kind, at the boundary: a line of exactly
    // 100 columns stays flat, one of 101 breaks. Annotation arguments follow
    // rule 6 here as in `rule5_and_rule6_annotation_arguments_follow_the_group_rule`.
    let mut cases: Vec<(FileKind, String, String)> = Vec::new();
    let mut boundary = |kind: FileKind,
                        make: &dyn Fn(&str) -> String,
                        broken: &dyn Fn(&str) -> String,
                        pad_char: &str,
                        base: usize| {
        let fits = format!("{}{}", &pad_char[..1], pad_char[1..].repeat(base - 1));
        let over = format!("{fits}{}", &pad_char[1..]);
        let flat = make(&fits);
        assert_eq!(longest_line(&flat), 100, "{flat}");
        let long = make(&over);
        assert_eq!(longest_line(&long), 101, "{long}");
        cases.push((kind, flat.clone(), flat));
        cases.push((kind, long, broken(&over)));
    };
    boundary(
        CPL,
        &|p| {
            format!("module m\n\nuse reader.pages.{{paginate_items_of_a_page, PageIdentifier, StoreError, load_page_text, {p}}}\n")
        },
        &|p| {
            format!("module m\n\nuse reader.pages.{{\n    paginate_items_of_a_page,\n    PageIdentifier,\n    StoreError,\n    load_page_text,\n    {p},\n}}\n")
        },
        "xx",
        11,
    );
    boundary(
        CPL,
        &|p| {
            format!("module m\n\nfn f(using database: D, using codec: C, using abc: A) -> Int\n    effects {{ database.get_page, database.put_page, codec.dimensions, codec.decode, abc.{p} }}\n{{\n    1\n}}\n")
        },
        &|p| {
            format!("module m\n\nfn f(using database: D, using codec: C, using abc: A) -> Int\n    effects {{\n        database.get_page,\n        database.put_page,\n        codec.dimensions,\n        codec.decode,\n        abc.{p},\n    }}\n{{\n    1\n}}\n")
        },
        "dd",
        10,
    );
    boundary(
        WORK,
        &|p| {
            format!("change c {{\n    task t {{\n        write {{ reader.pages.paginate.body, reader.pages.load_page.body, reader.images.{p} }}\n    }}\n}}\n")
        },
        &|p| {
            format!("change c {{\n    task t {{\n        write {{\n            reader.pages.paginate.body,\n            reader.pages.load_page.body,\n            reader.images.{p},\n        }}\n    }}\n}}\n")
        },
        "dd",
        11,
    );
    boundary(
        CPL,
        &|p| {
            format!("module m\n\nenum E {{\n    Variant(SomeLongTypeName, AnotherLongTypeName, List<YetAnotherLongTypeName>, Text, {p})\n}}\n")
        },
        &|p| {
            format!("module m\n\nenum E {{\n    Variant(\n        SomeLongTypeName,\n        AnotherLongTypeName,\n        List<YetAnotherLongTypeName>,\n        Text,\n        {p},\n    )\n}}\n")
        },
        "In",
        12,
    );
    boundary(
        CPL,
        &|p| {
            format!("module m\n\nfn f(r: R) -> Int {{\n    match r {{\n        Quad(first_binding_name, second_binding_name, third_binding_name, {p}) => 1\n        _ => 0\n    }}\n}}\n")
        },
        &|p| {
            format!("module m\n\nfn f(r: R) -> Int {{\n    match r {{\n        Quad(\n            first_binding_name,\n            second_binding_name,\n            third_binding_name,\n            {p},\n        ) => 1\n        _ => 0\n    }}\n}}\n")
        },
        "ff",
        20,
    );
    boundary(
        CPL,
        &|p| {
            module_with(&format!("    let page = Page {{ ..base_page_value, identifier: page_identifier_value, text: {p} }}"))
        },
        &|p| {
            module_with(&format!("    let page = Page {{\n        ..base_page_value,\n        identifier: page_identifier_value,\n        text: {p},\n    }}"))
        },
        "tt",
        16,
    );
    boundary(
        CPL,
        &|p| {
            module_with(&format!(
                "    let values = [first_value_name, second_value_name, third_value_name, {p}]"
            ))
        },
        &|p| {
            module_with(&format!("    let values = [\n        first_value_name,\n        second_value_name,\n        third_value_name,\n        {p},\n    ]"))
        },
        "vv",
        26,
    );
    boundary(
        CPL,
        &|p| format!("module m\n\n@id(\"{p}\")\nfn f() -> Int {{\n    1\n}}\n"),
        &|p| format!("module m\n\n@id(\n    \"{p}\",\n)\nfn f() -> Int {{\n    1\n}}\n"),
        "aa",
        93,
    );
    let refs: Vec<(FileKind, &str, &str)> = cases
        .iter()
        .map(|(k, s, e)| (*k, s.as_str(), e.as_str()))
        .collect();
    check_cases(&refs);
}

#[test]
fn rule6_t12_a_group_broken_for_width_stays_broken_even_when_it_would_fit_again() {
    // T12 [D-147]: "a group stays broken once broken (rule 6 (i))"; a group
    // the formatter broke for width re-parses as `multiline`.
    let long = module_with("    let x = outer(inner_function_with_a_long_name(argument_number_one, argument_number_two, argument_number_three_is_long), b)");
    let once = fmt(&long, CPL);
    let m = parse_module(&once, FileId(0)).value.expect("parses");
    let DeclKind::Fn(f) = &m.declarations[0].kind else {
        panic!("a fn")
    };
    let Statement::Let(l) = &f.body.as_ref().unwrap().statements[0] else {
        panic!("a let")
    };
    let ExprKind::Call(outer) = &l.value.kind else {
        panic!("a call")
    };
    assert!(outer.args.group.multiline, "{once}");
    let ExprKind::Call(inner) = &outer.args.positional[0].expr.kind else {
        panic!("a call")
    };
    assert!(inner.args.group.multiline, "{once}");
    // Shortening every name afterwards keeps both groups broken.
    let shortened = once
        .replace("inner_function_with_a_long_name", "g")
        .replace("argument_number_", "a");
    assert!(shortened.contains("    let x = outer(\n        g(\n            aone,\n"));
    check(CPL, &shortened, &shortened);
}

// ---------------------------------------------------------------------------
// Rule 7: headers, clauses, and the body's brace
// ---------------------------------------------------------------------------

#[test]
fn rule7_a_while_header_whose_group_broke_puts_its_brace_on_its_own_line() {
    // Rule 7: "The same `{` rule applies to `while` (...), `property` (...),
    // `patch` (`expect`), and headers whose group broke": the body's `{` goes
    // on its own line at the header's indentation when a group of the header
    // is broken, by the source (rule 6 (i)) or for width (rule 6 (iii)), as it
    // does after a `fn` whose parameter group broke.
    let width = "    while some_function(argument_number_one, argument_number_two, argument_number_three, four_xyzw) {\n        a\n    }";
    assert_eq!(longest_line(width), 101);
    check_cases(&[
        (
            CPL,
            "module m\n\nfn f(a: Int) -> Int {\n    while check(\n        a) {\n        a\n    }\n    a\n}\n",
            "module m\n\nfn f(a: Int) -> Int {\n    while check(\n        a,\n    )\n    {\n        a\n    }\n    a\n}\n",
        ),
        (
            CPL,
            &format!("module m\n\nfn f(a: Int) -> Int {{\n{width}\n    a\n}}\n"),
            "module m\n\nfn f(a: Int) -> Int {\n    while some_function(\n        argument_number_one,\n        argument_number_two,\n        argument_number_three,\n        four_xyzw,\n    )\n    {\n        a\n    }\n    a\n}\n",
        ),
    ]);
}

#[test]
fn rule7_if_else_if_and_match_headers_whose_group_broke_put_their_brace_on_its_own_line() {
    // Rule 7, "headers whose group broke": `if`, `else if`, and `match` are
    // headers (grammar 2.4's table) that admit `NEWLINE? {` (grammar 3.12).
    check(
        CPL,
        "module m\n\nfn f(a: Int) -> Int {\n    if check(\n        a) {\n        1\n    } else if check(a,\n        a) {\n        2\n    }\n    match g(\n        a) {\n        _ => 3\n    }\n}\n",
        "module m\n\nfn f(a: Int) -> Int {\n    if check(\n        a,\n    )\n    {\n        1\n    } else if check(\n        a,\n        a,\n    )\n    {\n        2\n    }\n    match g(\n        a,\n    )\n    {\n        _ => 3\n    }\n}\n",
    );
}

#[test]
fn rule7_capability_operation_clauses_take_the_canonical_order_two_levels_deep() {
    // Rule 7: an operation's contract clauses are indented one level deeper
    // than the operation, `requires` in source order before `ensures` in
    // source order (D66); a moved clause keeps its parenthesised line break,
    // one level deeper than its own line (rule 9).
    check(
        CPL,
        "module m\n\ncapability C {\n    fn get(id: Int) -> Int\n        ensures result > 0\n        requires id > 0\n        ensures result < 9\n        requires (id\n            < 9)\n}\n",
        "module m\n\ncapability C {\n    fn get(id: Int) -> Int\n        requires id > 0\n        requires (id\n            < 9)\n        ensures result > 0\n        ensures result < 9\n}\n",
    );
}

#[test]
fn rule7_a_bodiless_function_ends_with_its_last_clause_line() {
    // Rule 7: a bodiless `fn` prints its header and clauses and nothing
    // after them, so the next declaration follows after rule 3's blank line;
    // a definition-less `type` is one line.
    check(
        CPL,
        "module m\n\nfn f(\n    a: Int) -> Int\n    requires a > 0\nfn g() -> Int\npub type L<T>\n",
        "module m\n\nfn f(\n    a: Int,\n) -> Int\n    requires a > 0\n\nfn g() -> Int\n\npub type L<T>\n",
    );
}

#[test]
fn rule7_the_brace_of_a_clause_free_header_rejoins_the_header_line_in_every_form() {
    // Rule 7: the body's `{` goes on the header line when there are no
    // clauses and no broken group (grammar D63 accepts it on the next line);
    // rules 8 and 12 show the same for every other header.
    check_cases(&[
        (
            CPL,
            "module m\n\nstruct S\n{\n}\n\nenum E<T>\n{\n    A(T)\n}\n\ncapability C\n{\n}\n\ntest \"t\"\n{\n}\n\nfn f(c: Bool) -> Int\n{\n    if c\n    {\n        1\n    } else\n    {\n        2\n    }\n}\n",
            "module m\n\nstruct S {}\n\nenum E<T> {\n    A(T)\n}\n\ncapability C {}\n\ntest \"t\" {}\n\nfn f(c: Bool) -> Int {\n    if c {\n        1\n    } else {\n        2\n    }\n}\n",
        ),
        (
            WORK,
            "change c\n{\n    task t after a, b\n    {\n    }\n    integrate\n    {\n    }\n}\n",
            "change c {\n    task t after a, b {}\n    integrate {}\n}\n",
        ),
    ]);
}

// ---------------------------------------------------------------------------
// Rule 8: spacing; parentheses
// ---------------------------------------------------------------------------

#[test]
fn rule8_prefix_operators_postfix_operators_and_keywords_take_canonical_spacing() {
    // Rule 8: `?`, `.member`, `..spread`, and unary `-` attach to their
    // operand, `not` is followed by one space, one space after `:` and around
    // `=`, one space inside the braces of a struct literal and an `effects`
    // set; parentheses are kept, so `not(x)` prints `not (x)`.
    check_cases(&[
        (
            CPL,
            &wrap("let a = not(x)\nlet b = - -x\nlet c = -  (1)\nlet d = p . text\nlet e = f(x) ?\nlet g = Page { .. p }\nlet h = not  not x\nlet i = forall y in l : (y > 0)\nif(c){\n    1\n}\nwhile(c){\n    2\n}\nassert(x)\nreturn(x)"),
            "module m\n\nfn f() -> Int {\n    let a = not (x)\n    let b = --x\n    let c = -(1)\n    let d = p.text\n    let e = f(x)?\n    let g = Page { ..p }\n    let h = not not x\n    let i = forall y in l: (y > 0)\n    if (c) {\n        1\n    }\n    while (c) {\n        2\n    }\n    assert (x)\n    return (x)\n}\n",
        ),
        (
            CPL,
            "module m\n\nfn f(using db: D) -> Int\n    effects{db.get,db.put}\n{\n    1\n}\n",
            "module m\n\nfn f(using db: D) -> Int\n    effects { db.get, db.put }\n{\n    1\n}\n",
        ),
    ]);
}

#[test]
fn rule8_parentheses_are_kept_exactly_as_written() {
    // Rule 8: "Parentheses are preserved exactly as written (never added,
    // never removed)"; T14.
    let src = wrap("let a = ((x))\nlet b = (a) + (b)\nlet c = (forall y in l: y > 0)\nlet d = (?h)\nlet e = ((a + b) * c)\nlet g = (Page { ..p })\n(a)");
    check(CPL, &src, &src);
}

// ---------------------------------------------------------------------------
// Rule 9: operator line breaks
// ---------------------------------------------------------------------------

#[test]
fn rule9_a_break_before_an_operator_survives_only_directly_inside_parentheses() {
    // Rule 9: line breaks inside an expression are removed, except a break
    // immediately before a binary operator inside a `Paren` group; in a list
    // the break is a break of the list (rule 6 (i)) and the expression is
    // joined; a break after an operator or before a unary one is removed.
    check(
        CPL,
        &wrap("let l = [a\n    + b]\nlet m = [(a\n    + b)]\nlet n = f(\n    (a\n+ b),\n)\nlet p = Page { text: (a\n    ++ b) }\nlet q = (\n    -a)\nlet r = (\n    a + b)\nlet s = (a +\n    b)\nlet t = (a\n\n\n    + b)\nl"),
        "module m\n\nfn f() -> Int {\n    let l = [\n        a + b,\n    ]\n    let m = [(a\n        + b)]\n    let n = f(\n        (a\n            + b),\n    )\n    let p = Page { text: (a\n        ++ b) }\n    let q = (-a)\n    let r = (a + b)\n    let s = (a + b)\n    let t = (a\n        + b)\n    l\n}\n",
    );
}

#[test]
fn rule9_the_continuation_is_one_level_deeper_than_the_line_of_the_opening_parenthesis() {
    // Rule 9: "the operator then begins a continuation line indented one
    // level deeper than the line containing the opening parenthesis", in a
    // refinement predicate, an operation's clause, a `for` clause, a `while`
    // condition (a parenthesised condition is not a group, so the `{` stays,
    // rule 7), after a broken group's closing bracket, a `match` scrutinee,
    // and a match arm.
    check(
        CPL,
        "module m\n\npub type P = Int where (self > 0\nand self < 10)\n\ncapability C {\n    fn a(x: Int) -> Int\n        requires (x\n        > 0)\n}\n\nproperty \"p\"\n    for x in (a\n    ++ b)\n{\n    assert true\n}\n\nfn f(x: Int) -> Int {\n    while (x\n    > 0) {\n        x = x - 1\n    }\n    let y = g(\n        a,\n    ) + (b\n    + c)\n    match (x\n    + 1) {\n        1 => (a\n        or b)\n        _ => 0\n    }\n}\n",
        "module m\n\npub type P = Int where (self > 0\n    and self < 10)\n\ncapability C {\n    fn a(x: Int) -> Int\n        requires (x\n            > 0)\n}\n\nproperty \"p\"\n    for x in (a\n        ++ b)\n{\n    assert true\n}\n\nfn f(x: Int) -> Int {\n    while (x\n        > 0) {\n        x = x - 1\n    }\n    let y = g(\n        a,\n    ) + (b\n        + c)\n    match (x\n        + 1) {\n        1 => (a\n            or b)\n        _ => 0\n    }\n}\n",
    );
}

#[test]
fn rule9_a_continuation_line_that_is_too_long_breaks_its_own_group() {
    // Rules 9 and 6 (iii): every resulting line is measured, the operator's
    // continuation line included; its group breaks one level deeper than
    // the continuation line and closes at that line's indentation.
    check(
        CPL,
        &module_with("    let x = (aaaa\n        + some_function(argument_number_one, argument_number_two, argument_number_three, four_five_six))"),
        &module_with("    let x = (aaaa\n        + some_function(\n            argument_number_one,\n            argument_number_two,\n            argument_number_three,\n            four_five_six,\n        ))"),
    );
}

// ---------------------------------------------------------------------------
// Rule 10: literals
// ---------------------------------------------------------------------------

#[test]
fn rule10_text_literals_take_the_minimal_canonical_escapes() {
    // Rule 10: `\\`, `\"`, `\n`, `\r`, `\t`; every other control character
    // and DEL as `\u{H}` with minimal lowercase hex digits; every other scalar
    // value literally (the C1 range, U+2028, a byte-order mark, an emoji, a
    // non-breaking space). A raw tab is legal in a literal (grammar D62) and
    // prints as `\t`.
    check(
        CPL,
        &wrap("let a = \"\\u{0}\\u{000041}\\u{7F}\\u{1b}\\u{1F}\\u{20}\\u{80}\\u{85}\\u{2028}\\u{feff}\\u{1F600}\\u{0009}\\u{D}\\u{A}\"\nlet b = \"raw\u{80}\u{85}\u{2028}\u{a0}tab\there\"\na"),
        "module m\n\nfn f() -> Int {\n    let a = \"\\u{0}A\\u{7f}\\u{1b}\\u{1f} \u{80}\u{85}\u{2028}\u{feff}\u{1F600}\\t\\r\\n\"\n    let b = \"raw\u{80}\u{85}\u{2028}\u{a0}tab\\there\"\n    a\n}\n",
    );
}

#[test]
fn rule10_bidirectional_formatting_characters_stay_escaped() {
    // D75: they are no raw characters of a literal, so the canonical spelling escapes them
    check(
        CPL,
        &wrap("let a = \"x\\u{202E}y\\u{2066}z\\u{200F}\"\na"),
        "module m\n\nfn f() -> Int {\n    let a = \"x\\u{202e}y\\u{2066}z\u{200f}\"\n    a\n}\n",
    );
}

#[test]
fn rule10_literals_are_respelled_in_every_position_that_holds_one() {
    // Rule 10 applies to every text literal (annotation arguments, patterns,
    // test and property names, goals, Git refs) and every integer literal
    // (digit separators dropped, also in patterns and `workers`); T14 lists
    // the spelling of text-literal escapes among what `fmt` may change.
    check_cases(&[
        (
            CPL,
            "module m\n\n@id(\"\\u{78}\")\n@x(1_000, \"\\u{9}\")\nfn f(n: Int) -> Int {\n    let k = 1_000_000\n    match n {\n        1_0 => k\n        - 5 => 2\n        _ => 0\n    }\n}\n\nfn g(x: Text) -> Int {\n    match x {\n        \"\\u{41}\\t\" => 1\n        _ => 0\n    }\n}\n\ntest \"\\u{41}\\u{301}\" {\n    assert \"\\\"q\\\"\" == \"\\u{22}q\\u{22}\"\n}\n\nproperty \"\\u{50}\"\n    for x in g\n{\n    assert true\n}\n",
            "module m\n\n@id(\"x\")\n@x(1000, \"\\t\")\nfn f(n: Int) -> Int {\n    let k = 1000000\n    match n {\n        10 => k\n        -5 => 2\n        _ => 0\n    }\n}\n\nfn g(x: Text) -> Int {\n    match x {\n        \"A\\t\" => 1\n        _ => 0\n    }\n}\n\ntest \"A\u{301}\" {\n    assert \"\\\"q\\\"\" == \"\\\"q\\\"\"\n}\n\nproperty \"P\"\n    for x in g\n{\n    assert true\n}\n",
        ),
        (
            WORK,
            "change c {\n    base git(\"ma\\u{69}n\")\n    workers 1_0\n    task t {\n        goal \"\\u{47}\\\"q\\\"\\u{5c}\"\n    }\n}\n",
            "change c {\n    base git(\"main\")\n    workers 10\n    task t {\n        goal \"G\\\"q\\\"\\\\\"\n    }\n}\n",
        ),
    ]);
}

// ---------------------------------------------------------------------------
// Rule 11 and Section 2.3: comments
// ---------------------------------------------------------------------------

#[test]
fn rule11_comment_text_is_kept_byte_for_byte_from_the_slashes() {
    // Rule 11: "text byte-for-byte from `//` (so `//x` stays `//x` and `// x`
    // stays `// x`; trailing whitespace is dropped)"; D60: four or more
    // slashes are an ordinary comment; D1: comments may hold any Unicode but
    // the control and bidirectional formatting characters of D75.
    check(
        CPL,
        concat!(
            "module m\n\n//x\n//   spaced   \n////\n//// banner ////\n//\n// \u{fc}n\u{ef}c\u{f6}d\u{e9} \u{2713} \u{200f}rtl\u{200e} \u{2028} end\nfn f() -> Int {",
            " 1 } //no space\n",
        ),
        "module m\n\n//x\n//   spaced\n////\n//// banner ////\n//\n// \u{fc}n\u{ef}c\u{f6}d\u{e9} \u{2713} \u{200f}rtl\u{200e} \u{2028} end\nfn f() -> Int {\n    1\n} //no space\n",
    );
}

#[test]
fn rule11_only_spaces_and_tabs_count_as_a_comments_trailing_whitespace() {
    // Rule 11 keeps a comment's text byte for byte and drops only trailing
    // whitespace, and rule 1 forbids trailing whitespace on a line; grammar
    // 1.1 defines whitespace as space and tab, so a comment or doc comment
    // ending in U+00A0, U+3000, or U+2028 keeps that character (the
    // specification does not define "whitespace" for rules 1 and 11;
    // reported as a spec issue). Design: comments are prose for human
    // readers (grammar D1), and formatting must not change their content.
    check(
        CPL,
        concat!(
            "module m\n\n// note\u{a0}\n// \u{6ce8}\u{610f}\u{3000}\n/// doc\u{a0}\nfn f() -> Int {",
            " 1 } // end\u{2028}\n",
        ),
        concat!(
            "module m\n\n// note\u{a0}\n// \u{6ce8}\u{610f}\u{3000}\n/// doc\u{a0}\nfn f() -> Int {\n",
            "    1\n} // end\u{2028}\n",
        ),
    );
}

#[test]
fn rule11_c_a_comment_between_a_header_and_a_brace_on_the_next_line_prints_below_the_brace() {
    // 2.3 rule C and rule 11: a comment between a header's last token or
    // clause and a `{` that begins a new line is the leading comment of the
    // first node inside the braces (or one of the body's trailing comments
    // when it has none) and prints below the `{`; in every header kind of
    // grammar 2.4's table.
    check_cases(&[
        (
            CPL,
            "module m\n\nfn a() -> Int // after header\n{\n    1\n}\n\nfn b(x: Int) -> Int\n    requires x > 0\n// before brace\n{\n    x\n}\n\nstruct S // s\n{\n    x: Int\n}\n\nenum E\n// e\n{\n    A\n}\n\ncapability C // c\n{\n}\n\ntest \"t\" // t\n{\n    assert true\n}\n\nproperty \"p\"\n    for x in g\n    // p\n{\n    assert true\n}\n\nfn w(c: Bool) -> Int {\n    while c // w\n    {\n        1\n    }\n    if c // i\n    {\n        1\n    } else // l\n    {\n        2\n    }\n    match c // m\n    {\n        _ => 3\n    }\n}\n",
            "module m\n\nfn a() -> Int {\n    // after header\n    1\n}\n\nfn b(x: Int) -> Int\n    requires x > 0\n{\n    // before brace\n    x\n}\n\nstruct S {\n    // s\n    x: Int\n}\n\nenum E {\n    // e\n    A\n}\n\ncapability C {\n    // c\n}\n\ntest \"t\" {\n    // t\n    assert true\n}\n\nproperty \"p\"\n    for x in g\n{\n    // p\n    assert true\n}\n\nfn w(c: Bool) -> Int {\n    while c {\n        // w\n        1\n    }\n    if c {\n        // i\n        1\n    } else {\n        // l\n        2\n    }\n    match c {\n        // m\n        _ => 3\n    }\n}\n",
        ),
        (
            WORK,
            "change c // c\n{\n    task t after a // t\n    {\n        goal \"g\"\n    }\n    integrate\n    // i\n    {\n        require typecheck\n    }\n}\n",
            "change c {\n    // c\n    task t after a {\n        // t\n        goal \"g\"\n    }\n    integrate {\n        // i\n        require typecheck\n    }\n}\n",
        ),
        (
            PATCH,
            "patch f.body\n    expect base(f.body)\n// c\n{\n    1\n}\n",
            "patch f.body\n    expect base(f.body)\n{\n    // c\n    1\n}\n",
        ),
    ]);
}

#[test]
fn rule11_e_a_comment_in_an_empty_body_of_every_kind_prints_inside_it() {
    // 2.3 rule E (a comment with no following node is one of the body's
    // trailing comments, also after `{` on its header's line) and rule 8 (a
    // block is multi-line unless it is the empty block `{}`).
    check_cases(&[
        (
            CPL,
            "module m\n\nstruct S { // s\n}\n\nenum E { // e\n}\n\ncapability C { // c\n}\n\ntest \"t\" { // t\n}\n\nproperty \"p\"\n    for x in g\n{ // p\n}\n\nfn f(c: Bool, r: Int) -> Unit { // f\n    while c { // w\n    }\n    if c { // i\n    } else if c { // ei\n    } else { // e2\n    }\n    match r { // m\n    }\n    match r {\n        _ => { // a\n        }\n    }\n}\n",
            "module m\n\nstruct S {\n    // s\n}\n\nenum E {\n    // e\n}\n\ncapability C {\n    // c\n}\n\ntest \"t\" {\n    // t\n}\n\nproperty \"p\"\n    for x in g\n{\n    // p\n}\n\nfn f(c: Bool, r: Int) -> Unit {\n    // f\n    while c {\n        // w\n    }\n    if c {\n        // i\n    } else if c {\n        // ei\n    } else {\n        // e2\n    }\n    match r {\n        // m\n    }\n    match r {\n        _ => {\n            // a\n        }\n    }\n}\n",
        ),
        (
            WORK,
            "change c { // c\n    task t { // t\n    }\n    integrate { // i\n    }\n}\nchange d {\n    // only\n}\n",
            "change c {\n    // c\n    task t {\n        // t\n    }\n    integrate {\n        // i\n    }\n}\n\nchange d {\n    // only\n}\n",
        ),
        (
            PATCH,
            "patch f.body\n    expect base(f.body)\n{ // nothing\n}\n",
            "patch f.body\n    expect base(f.body)\n{\n    // nothing\n}\n",
        ),
    ]);
}

#[test]
fn rule11_g_a_comment_inside_header_parentheses_trails_the_statement_not_the_body() {
    // 2.3 rule G names "a parenthesized expression" and "a `match` scrutinee
    // that spans lines": the comment lies inside the parentheses, where no
    // node follows it (rule D) and no group owns it (rule E), so it is the
    // trailing comment of the innermost attachable node containing it, the
    // statement, and "the formatter prints it after that node's last token",
    // the body's `}`. The body's first statement lies in another sequence and
    // does not claim it.
    check_cases(&[
        (
            CPL,
            &wrap("if (c // i\n) {\n    1\n}\nwhile (c // w\n) {\n    2\n}\nmatch (x // m\n) {\n    _ => 3\n}"),
            "module m\n\nfn f() -> Int {\n    if (c) {\n        1\n    } // i\n    while (c) {\n        2\n    } // w\n    match (x) {\n        _ => 3\n    } // m\n}\n",
        ),
        (
            CPL,
            &wrap("while (c // w\n)\n    invariant c\n{\n    2\n}\n3"),
            "module m\n\nfn f() -> Int {\n    while (c)\n        invariant c\n    {\n        2\n    } // w\n    3\n}\n",
        ),
    ]);
}

#[test]
fn rule11_g_a_comment_inside_parentheses_is_not_claimed_by_a_node_of_a_later_sequence() {
    // 2.3 rules D and G: rule D attaches a comment to an attachable node that
    // starts after it "within the same enclosing sequence", the content of the
    // parentheses here; the argument, element, field initializer, arm, or
    // statement that follows lies in another bracket or brace sequence, so
    // rule G makes the comment the statement's trailing comment, printed
    // after the statement's last token, and no group is broken for it.
    check_cases(&[
        (
            CPL,
            &wrap("let x = (a // c\n) + g(b)\nx"),
            "module m\n\nfn f() -> Int {\n    let x = (a) + g(b) // c\n    x\n}\n",
        ),
        (
            CPL,
            &wrap("let x = (a // c\n) + [b]\nx"),
            "module m\n\nfn f() -> Int {\n    let x = (a) + [b] // c\n    x\n}\n",
        ),
        (
            CPL,
            &wrap("let x = (a // c\n) + Page { text: t }\nx"),
            "module m\n\nfn f() -> Int {\n    let x = (a) + Page { text: t } // c\n    x\n}\n",
        ),
        (
            CPL,
            &wrap("let x = (a // c\n) + match y {\n    _ => 1\n}\nx"),
            "module m\n\nfn f() -> Int {\n    let x = (a) + match y {\n        _ => 1\n    } // c\n    x\n}\n",
        ),
        (
            CPL,
            &wrap("assert (a // c\n) == if b {\n    1\n} else {\n    2\n}\n0"),
            "module m\n\nfn f() -> Int {\n    assert (a) == if b {\n        1\n    } else {\n        2\n    } // c\n    0\n}\n",
        ),
    ]);
}

#[test]
fn rule11_g_a_comment_inside_a_generic_list_trails_the_innermost_node_containing_it() {
    // 2.3 rule G names "a generic list": the comment is the trailing comment
    // of the innermost attachable node containing it: the declaration, the
    // parameter, and the list element (not the argument of the call, which
    // starts after the generic list in the call's own argument sequence).
    check_cases(&[
        (
            CPL,
            "module m\n\npub type A(List<Int // c\n>)\n",
            "module m\n\npub type A(List<Int>) // c\n",
        ),
        (
            CPL,
            "module m\n\nfn f(x: List<Int // d\n>) -> Int { 1 }\n",
            "module m\n\nfn f(\n    x: List<Int>, // d\n) -> Int\n{\n    1\n}\n",
        ),
        (
            CPL,
            &wrap("let y = [g<Int // e\n>(x)]\ny"),
            "module m\n\nfn f() -> Int {\n    let y = [\n        g<Int>(x), // e\n    ]\n    y\n}\n",
        ),
    ]);
}

#[test]
fn rule11_g_comments_inside_payloads_patterns_and_single_value_brackets_trail_their_node() {
    // 2.2: variant payloads, constructor-pattern payloads, `git(...)`, and
    // `review(...)` hold no attachable node, so a comment inside one is the
    // trailing comment of the variant, the arm, or the item (2.3 rule G); a
    // payload or pattern group written across lines prints broken (rule 6).
    check_cases(&[
        (
            CPL,
            "module m\n\nenum E {\n    A(Int // c\n    , Text)\n    B\n}\n\nfn f(r: R) -> Int {\n    match r {\n        Ok(v // o\n        ) => 1\n        _ => 0\n    }\n}\n",
            "module m\n\nenum E {\n    A(\n        Int,\n        Text,\n    ) // c\n    B\n}\n\nfn f(r: R) -> Int {\n    match r {\n        Ok(\n            v,\n        ) => 1 // o\n        _ => 0\n    }\n}\n",
        ),
        (
            WORK,
            "change c {\n    base git( // b\n        \"main\")\n    integrate {\n        require review( // r\n            audit)\n    }\n}\n",
            "change c {\n    base git(\"main\") // b\n    integrate {\n        require review(audit) // r\n    }\n}\n",
        ),
    ]);
}

#[test]
fn rule11_a_comment_between_a_closing_brace_and_else_inside_brackets_survives_formatting() {
    // Grammar 2.4 rule 4: inside brackets an `else` may begin a line, so a
    // comment can sit between the `}` and the `else`. Toolchain 2.3 does not
    // settle whether rule D's "within the same enclosing sequence" lets the
    // `else` block's first statement, in a nested sequence, claim it, or
    // whether rule G makes it the argument's trailing comment (reported as a
    // spec issue); either way formatting is idempotent, keeps the comment,
    // and rejoins the `else` to the `}` (grammar 2.4 rule 4, rule 8).
    let out = check_stable(CPL, &wrap("g(if c { 1 } // c\nelse { 2 })"));
    assert!(out.contains("} else {"), "{out}");
    // In a significant-newline context the same layout is misplaced_else,
    // reported at the `else` (grammar 2.4 rule 4, D12), and `fmt` refuses it.
    let src = wrap("if c {\n    1\n} // c\nelse {\n    2\n}");
    let d = refused(CPL, &src);
    assert_eq!(d.code, codes::MISPLACED_ELSE, "{d:#?}");
    assert_eq!(
        d.span.map(|s| s.start),
        src.find("else").map(|i| i as u32),
        "{d:#?}"
    );
}

#[test]
fn rule11_a_second_trailing_comment_moves_below_its_node_in_every_sequence_kind() {
    // Rule 11: "a second trailing comment of the same node on its own line
    // below it"; the next parse attaches it to what follows (2.3 rules D and
    // E), so the canonical text has it on the next line at the sequence's
    // indentation: for a match arm, a list element, a spread, a variant, a
    // change item, and a requirement.
    check_cases(&[
        (
            CPL,
            &wrap("match r {\n    A => (a // c\n    ) // d\n    B => 2\n}"),
            "module m\n\nfn f() -> Int {\n    match r {\n        A => (a) // c\n        // d\n        B => 2\n    }\n}\n",
        ),
        (
            CPL,
            &wrap("match r {\n    A => (a // c\n    ) // d\n}"),
            "module m\n\nfn f() -> Int {\n    match r {\n        A => (a) // c\n        // d\n    }\n}\n",
        ),
        (
            CPL,
            &wrap("let l = [(a // c\n) // d\n, b]\nl"),
            "module m\n\nfn f() -> Int {\n    let l = [\n        (a), // c\n        // d\n        b,\n    ]\n    l\n}\n",
        ),
        (
            CPL,
            &wrap("let p = Page { ..(p // c\n) // d\n, text: t }\np"),
            "module m\n\nfn f() -> Int {\n    let p = Page {\n        ..(p), // c\n        // d\n        text: t,\n    }\n    p\n}\n",
        ),
        (
            CPL,
            "module m\n\nenum E {\n    A(Int // c\n    ) // d\n    B\n}\n",
            "module m\n\nenum E {\n    A(\n        Int,\n    ) // c\n    // d\n    B\n}\n",
        ),
        (
            WORK,
            "change c {\n    base git(\"a\" // c\n    ) // d\n    target git(\"b\")\n    integrate {\n        require review(a // e\n        ) // f\n    }\n}\n",
            "change c {\n    base git(\"a\") // c\n    // d\n    target git(\"b\")\n    integrate {\n        require review(a) // e\n        // f\n    }\n}\n",
        ),
    ]);
}

#[test]
fn rule11_f_comments_at_the_end_of_every_file_form_keep_their_blank_lines() {
    // 2.3 rule F: comments after the last item are the file's trailing
    // comments, printed at the top level with the blank lines recorded before
    // them (rule 11), several blank lines as one (T11); grammar 1.1: no final
    // terminator needed.
    check_cases(&[
        (
            CPL,
            "module m\nuse a.b.c\n// end",
            "module m\n\nuse a.b.c\n// end\n",
        ),
        (
            CPL,
            "module m\n\nfn f() -> Int { 1 }\n\n\n// a\n// b\n\n\n// c",
            "module m\n\nfn f() -> Int {\n    1\n}\n\n// a\n// b\n\n// c\n",
        ),
        (CPL, "module m\n\n\n// end\n\n", "module m\n\n// end\n"),
        (
            WORK,
            "change c {}\n// a\n\n\n// b",
            "change c {}\n// a\n\n// b\n",
        ),
        (
            PATCH,
            "patch f.body\n    expect base(f.body)\n{\n}\n\n\n// a",
            "patch f.body\n    expect base(f.body)\n{}\n\n// a\n",
        ),
    ]);
}

#[test]
fn rule11_comments_inside_requirement_and_expect_brackets_survive_formatting() {
    // The selector inside `prove(...)`, `test(...)`, and `expect base(...)`
    // is attachable (2.3), but those brackets are not groups (rule 6), and
    // the specification does not say where a comment inside them prints;
    // formatting must keep it, stay idempotent, and keep the requirement on
    // one line (rule 2).
    let out = check_stable(
        WORK,
        "change c {\n    integrate {\n        require prove( // p\n            f)\n        require test(tests.* // t\n        )\n    }\n}\n",
    );
    assert!(out.contains("require prove(f)"), "{out}");
    assert!(out.contains("require test(tests.*)"), "{out}");
    let out = check_stable(
        PATCH,
        "patch f.body\n    expect base( // e\n        f.body)\n{\n    1\n}\n",
    );
    assert!(out.contains("    expect base(f.body)\n{\n"), "{out}");
}

// ---------------------------------------------------------------------------
// T11: blank lines
// ---------------------------------------------------------------------------

#[test]
fn t11_the_first_node_of_a_file_never_keeps_a_blank_line_after_its_comments() {
    // 2.3: `Trivia::blank_line_before` is false on the module of a `.cpl`
    // file, on the first `use` or `change` of a `.work` file, and on the first
    // patch form of a `.patch` file; it is recorded on the first `use` and the
    // first declaration of a `.cpl` file, and on the first `change` after a
    // `.work` file's `use` declarations, which are not first nodes.
    check_cases(&[
        (CPL, "// top\n\nmodule m\n", "// top\nmodule m\n"),
        (
            WORK,
            "// top\n\nuse a.b.{f}\n\nchange c {}\n",
            "// top\nuse a.b.{f}\n\nchange c {}\n",
        ),
        (WORK, "// top\n\nchange c {}\n", "// top\nchange c {}\n"),
        (
            PATCH,
            "// top\n\npatch f.body\n    expect base(f.body)\n{\n}\n",
            "// top\npatch f.body\n    expect base(f.body)\n{}\n",
        ),
        (
            CPL,
            "module m\n// about\n\nuse a.b.c\n",
            "module m\n\n// about\n\nuse a.b.c\n",
        ),
        (
            CPL,
            "module m\n// about\n\nfn f() -> Int { 1 }\n",
            "module m\n\n// about\n\nfn f() -> Int {\n    1\n}\n",
        ),
        (
            WORK,
            "use a.b.{f}\n// about\n\nchange c {}\n",
            "use a.b.{f}\n\n// about\n\nchange c {}\n",
        ),
    ]);
}

#[test]
fn t11_a_blank_line_between_a_comment_and_a_later_node_stays_in_every_body_kind() {
    // T11 [D-146]: every node records a blank line directly before it and
    // every leading comment one before it; the formatter preserves both (2.3,
    // rules 4 and 11) for members, operations, task items, and requirements,
    // but the elements of a comma group never record one (2.3).
    let cpl = "module m\n\nenum E {\n    A\n    // about B\n\n    B\n}\n\ncapability C {\n    fn a() -> Int\n    // about b\n\n    fn b() -> Int\n}\n";
    let work = "change c {\n    task t {\n        write {}\n        // about goal\n\n        goal \"g\"\n    }\n    integrate {\n        require typecheck\n        // about holes\n\n        require no_holes\n    }\n}\n";
    check_cases(&[
        (CPL, cpl, cpl),
        (WORK, work, work),
        (
            CPL,
            &wrap("g(\n    a,\n    // about b\n\n    b,\n)"),
            "module m\n\nfn f() -> Int {\n    g(\n        a,\n        // about b\n        b,\n    )\n}\n",
        ),
    ]);
}

#[test]
fn t11_several_blank_lines_count_as_one_around_comments_everywhere() {
    // 2.3: "several blank lines count as one", before a comment and between
    // a comment and its node.
    check(
        CPL,
        "module m\n\n\n\n// a\n\n\n\n// b\n\n\n\nfn f() -> Int {\n    let x = 1\n\n\n\n    // c\n\n\n\n    x\n}\n",
        "module m\n\n// a\n\n// b\n\nfn f() -> Int {\n    let x = 1\n\n    // c\n\n    x\n}\n",
    );
}

#[test]
fn t11_a_blank_line_before_a_comment_inside_a_group_is_kept() {
    // 2.3: `Comment::blank_line_before` is recorded as written, inside groups
    // too, and preserved (rule 11), except after the opening bracket (rule 4,
    // no blank line after `{`, read for every bracket).
    check(
        CPL,
        &wrap("g(\n\n    // first\n    a,\n\n    // second\n    b,\n\n    // last\n)"),
        "module m\n\nfn f() -> Int {\n    g(\n        // first\n        a,\n\n        // second\n        b,\n\n        // last\n    )\n}\n",
    );
}

// ---------------------------------------------------------------------------
// Rule 12, T13, T14
// ---------------------------------------------------------------------------

#[test]
fn rule12_moved_work_items_take_their_comments_and_lose_a_leading_blank_line() {
    // Rule 12 orders change items (`base`, `target`, `workers`, tasks,
    // `integrate`) and task items (`write`, `add`, `depend`, `preserve`,
    // `goal`); an item's leading comments are its trivia and move with it;
    // a moved-to-the-front item gets no blank line after `{` (rule 4, 2.3's
    // note on rule 12).
    check(
        WORK,
        concat!(
            "change c {\n    // about the task\n    task t {\n",
            "        // about the goal\n        goal \"g\"\n\n        write {}\n    }\n\n    // about the base\n    base git(\"a\")\n}\n",
        ),
        "change c {\n    // about the base\n    base git(\"a\")\n    // about the task\n    task t {\n        write {}\n        // about the goal\n        goal \"g\"\n    }\n}\n",
    );
}

#[test]
fn t13_uses_members_arms_effects_and_set_elements_keep_their_order_and_repetitions() {
    // T13 [D-148]: the formatter never reorders `use` declarations, struct
    // fields, match arms, effect references, or set elements, and T14 never
    // removes a token, so a repeated effect reference stays.
    let cpl = "module m\n\nuse z.y.{b, a}\nuse a.b.c\n\nstruct S {\n    z: Int\n    a: Int\n}\n\nfn f(r: Int, using db: R) -> Int\n    effects { db.put, db.get, db.get }\n{\n    match r {\n        2 => 1\n        1 => 2\n        _ => 3\n    }\n}\n";
    let work = "change c {\n    task t {\n        write { z.body, a.body }\n        depend { b.contract, a.contract }\n    }\n}\n";
    check_cases(&[(CPL, cpl, cpl), (WORK, work, work)]);
}

#[test]
fn t14_the_formatter_never_adds_removes_or_rewrites_an_annotation_or_a_parenthesis() {
    // T14 [D-149]: `fmt` assigns no `@id` (that is `cpl id assign`), keeps
    // `@name()` and every parenthesis, and changes only whitespace, line
    // structure, clause order, and text-literal escapes.
    check(
        CPL,
        "module m\n\nfn f() -> Int { 1 }\n\n@id( \"x\" )\n@prove ( )\n@method\nfn g() -> Int { ((1)) }\n",
        "module m\n\nfn f() -> Int {\n    1\n}\n\n@id(\"x\")\n@prove()\n@method\nfn g() -> Int {\n    ((1))\n}\n",
    );
}

// ---------------------------------------------------------------------------
// T7: files the formatter refuses; Unicode
// ---------------------------------------------------------------------------

#[test]
fn unicode_identifiers_are_refused_and_unicode_in_text_and_comments_is_kept() {
    // Grammar 1.1 and 1.3: identifiers are ASCII and non-ASCII characters are
    // permitted only inside text literals and comments; anywhere else the file
    // has the lexical error `non_ascii_outside_text` with the field `found`
    // (semantics 9.4), at the character, and `fmt` refuses it (T7).
    for (kind, src, needle) in [
        (CPL, "module m\n\nfn caf\u{e9}() -> Int { 1 }\n", "\u{e9}"),
        (
            CPL,
            "module m\n\nfn f() -> Int {\n    let \u{430} = 1\n    1\n}\n",
            "\u{430}",
        ),
        (
            CPL,
            "module m\n\nfn f() -> Int {\n    let x\u{a0}= 1\n    x\n}\n",
            "\u{a0}",
        ),
        (CPL, "module \u{3b1}\u{3b2}\n", "\u{3b1}"),
        (
            WORK,
            "change c {\n    task t {\n        write { caf\u{e9}.body }\n    }\n}\n",
            "\u{e9}",
        ),
        (
            PATCH,
            "patch caf\u{e9}.body\n    expect base(caf\u{e9}.body)\n{\n}\n",
            "\u{e9}",
        ),
    ] {
        let d = refused(kind, src);
        assert_eq!(d.code, codes::NON_ASCII_OUTSIDE_TEXT, "{src:?}: {d:#?}");
        let at = src.find(needle).map(|i| i as u32);
        assert_eq!(d.span.map(|s| s.start), at, "{src:?}: {d:#?}");
        assert!(
            field(&d, "found").is_some_and(|f| f.contains(needle)),
            "{src:?}: {d:#?}"
        );
    }
    // The same characters inside comments and text literals print literally
    // (grammar D1, D62; rule 10; rule 11).
    let kept = "module m\n\n// caf\u{e9} \u{430} \u{3b1}\u{3b2}\nfn f() -> Text {\n    \"caf\u{e9} \u{430}\u{a0}\u{3b1}\" // \u{e9}\n}\n";
    check(CPL, kept, kept);
}

#[test]
fn a_lone_carriage_return_is_refused_even_at_the_end_of_a_comment() {
    // Grammar 1.1: "A CR not followed by LF is a lexical error"
    // (`invalid_line_terminator`, semantics 9.4); a comment runs to the end
    // of its line and does not absorb it; `fmt` refuses the file (T7).
    let d = refused(CPL, "module m // a\rb\n");
    assert_eq!(d.code, codes::INVALID_LINE_TERMINATOR, "{d:#?}");
    assert_eq!(d.span.map(|s| s.start), Some(13), "{d:#?}");
    let d = refused(WORK, "change c {}\r");
    assert_eq!(d.code, codes::INVALID_LINE_TERMINATOR, "{d:#?}");
}

// ---------------------------------------------------------------------------
// Further cases
// ---------------------------------------------------------------------------

#[test]
fn rule7_a_bodiless_header_and_its_last_clause_keep_their_trailing_comments() {
    // 2.3 rule B: a comment after a bodiless `fn`'s header is the
    // declaration's trailing comment (its last token ends the line), one
    // after its last clause the clause's; rule 7: the declaration ends with
    // that line, and rule 3's blank line follows.
    check(
        CPL,
        "module m\n\nfn f() -> Int // c\nfn g(a: Int) -> Int\n    requires a > 0 // d\nfn h() -> Int\n",
        "module m\n\nfn f() -> Int // c\n\nfn g(a: Int) -> Int\n    requires a > 0 // d\n\nfn h() -> Int\n",
    );
}

#[test]
fn rule11_comment_markers_inside_text_literals_are_text() {
    // Grammar 1.2 and 1.5: `//` and `///` inside a text literal are
    // characters of the literal, not comments; rule 10 prints them literally.
    let src = "module m\n\nfn f() -> Text {\n    let a = \"// not a comment\"\n    let b = \"/// nor a doc\" // but this is\n    a ++ b\n}\n";
    check(CPL, src, src);
    assert_eq!(comments_of(src), ["// but this is"]);
}

#[test]
fn rule5_comments_on_annotation_lines_stay_and_comments_between_them_move_above_the_docs() {
    // 2.3 rule B: a comment after an annotation on its line is the
    // annotation's trailing comment and stays there; rule D: one on a line
    // of its own among the prefix lines is the declaration's, printed above
    // the doc comments (rule 5), which move before the annotations.
    check(
        CPL,
        "module m\n\n@id(\"x\") // a\n// b\n@prove // c\n/// d\nfn f() -> Int { 1 }\n",
        "module m\n\n// b\n/// d\n@id(\"x\") // a\n@prove // c\nfn f() -> Int {\n    1\n}\n",
    );
}

#[test]
fn rule4_blank_lines_between_the_uses_of_a_work_file_are_kept() {
    // Rule 4 applies "among `use` declarations"; rule 12 puts one blank line
    // between the uses and the first change.
    check(
        WORK,
        "use a.b.{f}\n\n\nuse c.d.{g}\n// about h\n\nuse e.f.{h}\nchange x {}\n",
        "use a.b.{f}\n\nuse c.d.{g}\n// about h\n\nuse e.f.{h}\n\nchange x {}\n",
    );
}

#[test]
fn rule6_width_breaks_groups_on_clause_and_arm_lines() {
    // Rule 6 (iii) applies to every line, a clause's (rule 7) and a match
    // arm's included; a broken group on a clause line closes at the clause's
    // indentation, and the body's `{` stays on its own line. Each line holds
    // one outermost group (see the next test for a line with two).
    let requires = "module m\n\nfn f(a: Int) -> Int\n    requires check(argument_number_one, argument_number_two, argument_number_three, aaaa_bbbb_cccc_d)\n{\n    a\n}\n";
    let arm = "module m\n\nfn f(r: R) -> Int {\n    match r {\n        v => some_function(argument_number_one, argument_number_two, argument_number_three, abcdefgh)\n        _ => 0\n    }\n}\n";
    let for_clause = "module m\n\nproperty \"p\"\n    for x in gen.list_of(argument_number_one, argument_number_two, argument_number_three, abcdefghij)\n{\n    assert true\n}\n";
    let decreases = "module m\n\nfn f(a: Int) -> Int\n    decreases measure(argument_number_one, argument_number_two, argument_number_three, abcdefghijklm)\n{\n    a\n}\n";
    for src in [requires, arm, for_clause, decreases] {
        assert_eq!(longest_line(src), 101, "{src}");
    }
    check_cases(&[
        (
            CPL,
            requires,
            "module m\n\nfn f(a: Int) -> Int\n    requires check(\n        argument_number_one,\n        argument_number_two,\n        argument_number_three,\n        aaaa_bbbb_cccc_d,\n    )\n{\n    a\n}\n",
        ),
        (
            CPL,
            arm,
            "module m\n\nfn f(r: R) -> Int {\n    match r {\n        v => some_function(\n            argument_number_one,\n            argument_number_two,\n            argument_number_three,\n            abcdefgh,\n        )\n        _ => 0\n    }\n}\n",
        ),
        (
            CPL,
            for_clause,
            "module m\n\nproperty \"p\"\n    for x in gen.list_of(\n        argument_number_one,\n        argument_number_two,\n        argument_number_three,\n        abcdefghij,\n    )\n{\n    assert true\n}\n",
        ),
        (
            CPL,
            decreases,
            "module m\n\nfn f(a: Int) -> Int\n    decreases measure(\n        argument_number_one,\n        argument_number_two,\n        argument_number_three,\n        abcdefghijklm,\n    )\n{\n    a\n}\n",
        ),
    ]);
}

#[test]
fn rule9_every_binary_operator_keeps_a_break_before_it_inside_parentheses() {
    // Rule 9: "inside a `Paren` group, a source line break immediately
    // before a binary operator is preserved", for each of the fourteen
    // binary operators of grammar 3.12 (a `<` after a name is a comparison
    // once the generic-call speculation fails, D42).
    let mut cases = Vec::new();
    for op in [
        "or", "and", "==", "!=", "<", "<=", ">", ">=", "+", "-", "++", "*", "/", "%",
    ] {
        cases.push((
            module_with(&format!("    let x = (a\n    {op} b)")),
            module_with(&format!("    let x = (a\n        {op} b)")),
        ));
    }
    let refs: Vec<(FileKind, &str, &str)> = cases
        .iter()
        .map(|(s, e)| (CPL, s.as_str(), e.as_str()))
        .collect();
    check_cases(&refs);
}

#[test]
fn rule9_breaks_around_postfix_prefix_and_quantifier_parts_are_removed_inside_parentheses() {
    // Rule 9 keeps only a break before a binary operator: a break before
    // `.member`, a call's `(`, or `?`, after `not` or unary `-`, after a
    // quantifier's `:`, or before its `in` is removed (rule 8 attaches the
    // postfix and prefix operators to their operand).
    check(
        CPL,
        &wrap("let a = (p\n    .text)\nlet b = (f\n    (x))\nlet c = (r\n    ?)\nlet d = (not\n    q)\nlet e = (-\n    n)\nlet g = (forall y in l:\n    y > 0)\nlet h = (forall y\n    in l: y > 0)\na"),
        "module m\n\nfn f() -> Int {\n    let a = (p.text)\n    let b = (f(x))\n    let c = (r?)\n    let d = (not q)\n    let e = (-n)\n    let g = (forall y in l: y > 0)\n    let h = (forall y in l: y > 0)\n    a\n}\n",
    );
}

#[test]
fn rule6_width_breaking_stays_fast_at_the_nesting_limit() {
    // Grammar D73: every implementation accepts at least 64 nesting levels,
    // so this file of 24 nested `if`s (49 levels), each with an over-long
    // condition, parses, and T7 has `fmt` format every file that parses. Rule
    // 6 (iii) breaks each condition's group once. The specification states no
    // time bound; D73's rationale (no input "however deep or long" may break
    // a recursive consumer of the tree) and the design's agents, which format
    // on every change, require the work to stay proportional to the output,
    // so a generous bound is asserted. Re-rendering every enclosing line unit
    // once more per group broken inside it doubles the work per level.
    let depth = 24;
    let mut src = String::from("module m\n\nfn f(c: Bool) -> Int {\n");
    for i in 0..depth {
        src.push_str(&"    ".repeat(i + 1));
        src.push_str("if check(argument_number_one_is_long, argument_number_two_is_long, argument_number_three_is_long, x) {\n");
    }
    src.push_str(&"    ".repeat(depth + 1));
    src.push_str("1\n");
    for i in (0..depth).rev() {
        src.push_str(&"    ".repeat(i + 1));
        src.push_str("}\n");
    }
    src.push_str("    0\n}\n");
    assert!(parse_module(&src, FileId(0)).diagnostics.is_empty());
    let (tx, rx) = std::sync::mpsc::channel();
    let input = src.clone();
    std::thread::spawn(move || {
        let _ = tx.send(format_source(&input, CPL, FileId(0)).is_ok());
    });
    let formatted = rx.recv_timeout(std::time::Duration::from_secs(10));
    assert_eq!(
        formatted,
        Ok(true),
        "formatting 24 nested over-long `if` headers did not finish within 10 seconds"
    );
}

#[test]
fn rule6_a_line_break_inside_a_generic_list_never_survives() {
    // Rule 6: "Generic lists `<T, U>` are never groups and never break";
    // grammar 2.2: inside brackets a line break in one is discarded. Whether
    // that break is "at the group's own level" (2.2, `Group`) for the
    // enclosing group is not settled, since a generic list is neither a group
    // nor a mode-changing bracket pair (reported as a spec issue); the
    // generic list itself prints on one line and formatting is idempotent.
    let out = check_stable(CPL, &wrap("let l = [g<\nInt>(x)]\nl"));
    assert!(out.contains("g<Int>(x)"), "{out}");
    let out = check_stable(CPL, "module m\n\nfn h(x: List<\nInt>) -> Int { 1 }\n");
    assert!(out.contains("x: List<Int>"), "{out}");
}

#[test]
fn rule3_t51_a_type_broken_for_width_keeps_the_next_type_below_its_last_line() {
    // T51 decides adjacency from the source (no blank line, no comment, no
    // docs or annotations), and the second declaration then prints directly
    // below the first's last line, which rule 6 (iii) moved down.
    let src = "module m\n\npub type P = Int where check(self, argument_number_one, argument_number_two, argument_number_three_x)\npub type Q(Int)\n";
    assert_eq!(longest_line(src), 101);
    check(
        CPL,
        src,
        "module m\n\npub type P = Int where check(\n    self,\n    argument_number_one,\n    argument_number_two,\n    argument_number_three_x,\n)\npub type Q(Int)\n",
    );
}

#[test]
fn rule11_c_a_comment_after_a_broken_parameter_list_prints_below_the_brace() {
    // 2.3 rule C: the comment after the header's last token, before a `{`
    // that begins a new line, leads the body's first node; rule 7 keeps the
    // `{` on its own line after the broken parameter group.
    check(
        CPL,
        "module m\n\nfn f(\n    a: Int,\n) -> Int // c\n{\n    a\n}\n",
        "module m\n\nfn f(\n    a: Int,\n) -> Int\n{\n    // c\n    a\n}\n",
    );
}

#[test]
fn rule10_raw_tabs_in_work_and_patch_text_literals_print_as_escapes() {
    // Grammar D62 admits a raw tab in a text literal; rule 10 prints it as
    // `\t` in every file form, so no tab is emitted (rule 1).
    check_cases(&[
        (
            WORK,
            "change c {\n    base git(\"a\tb\")\n    task t {\n        goal \"x\ty\"\n    }\n}\n",
            "change c {\n    base git(\"a\\tb\")\n    task t {\n        goal \"x\\ty\"\n    }\n}\n",
        ),
        (
            PATCH,
            "patch f.body\n    expect base(f.body)\n{\n    \"a\tb\"\n}\n",
            "patch f.body\n    expect base(f.body)\n{\n    \"a\\tb\"\n}\n",
        ),
    ]);
}

#[test]
fn rule11_b_a_comment_after_a_closing_brace_stays_after_it() {
    // 2.3 rule B: a comment after a node's last token on the same line is
    // that node's trailing comment, the innermost node whose last token ends
    // the line: an arm whose block body closes the line, an arm with an
    // expression body, and the `match` statement; rule 11 prints it after
    // one space following that token, also when rule 8 spreads the block
    // over several lines.
    check(
        CPL,
        "module m\n\nfn f(r: R) -> Int {\n    match r {\n        A => { 1 } // c\n        B => {\n            2\n        }   // d\n        _ => 3 // e\n    } // f\n}\n",
        "module m\n\nfn f(r: R) -> Int {\n    match r {\n        A => {\n            1\n        } // c\n        B => {\n            2\n        } // d\n        _ => 3 // e\n    } // f\n}\n",
    );
}
