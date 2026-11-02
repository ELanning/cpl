//! The canonical formatter (`spec/toolchain.md` 2.4): idempotency and tree
//! preservation over every corpus file, every standard-library stub, and
//! every input string of the other test files, golden formatted output for
//! the design-document examples, comment preservation, the corpus and the
//! stubs being canonical already, and one test per layout rule.

use cpl_diag::{FileId, FileOrigin, SourceMap};
use cpl_syntax::{
    dump_module, dump_patch, dump_work, format_source, lex_full, parse_module, parse_patch,
    parse_work, FileKind, TriviaKind,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn crate_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn examples_dir() -> PathBuf {
    crate_dir().join("../../examples")
}

/// The standard library's stub files, the normative text of
/// `spec/stdlib.md` Section 11.
fn std_dir() -> PathBuf {
    crate_dir().join("../../std")
}

fn golden_dir() -> PathBuf {
    crate_dir().join("tests").join("golden")
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .map(|e| e.expect("entry").path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// Format, or return the rendered diagnostics.
fn fmt(src: &str, kind: FileKind) -> Result<String, String> {
    let mut map = SourceMap::new();
    let file = map.add("input".to_string(), FileOrigin::Virtual, src.to_string());
    format_source(src, kind, file).map_err(|d| cpl_diag::render_all(&d, &map))
}

#[track_caller]
fn fmt_ok(src: &str, kind: FileKind) -> String {
    match fmt(src, kind) {
        Ok(text) => text,
        Err(rendered) => panic!("expected a clean parse of:\n{src}\n{rendered}"),
    }
}

/// The dump of a clean parse, or `None` when the source has diagnostics.
fn dump(src: &str, kind: FileKind) -> Option<String> {
    match kind {
        FileKind::Cpl => {
            let r = parse_module(src, FileId(0));
            r.diagnostics
                .is_empty()
                .then(|| r.value.map(|m| dump_module(&m)))
                .flatten()
        }
        FileKind::Work => {
            let r = parse_work(src, FileId(0));
            r.diagnostics
                .is_empty()
                .then(|| r.value.map(|w| dump_work(&w)))
                .flatten()
        }
        FileKind::Patch => {
            let r = parse_patch(src, FileId(0));
            r.diagnostics
                .is_empty()
                .then(|| r.value.map(|p| dump_patch(&p)))
                .flatten()
        }
    }
}

/// A dump as a tree, so that layout the formatter may legitimately change
/// can be normalized away structurally.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Sx {
    Atom(String),
    List(Vec<Sx>),
}

impl Sx {
    fn head(&self) -> Option<&str> {
        match self {
            Sx::List(items) => match items.first() {
                Some(Sx::Atom(a)) => Some(a.as_str()),
                _ => None,
            },
            Sx::Atom(_) => None,
        }
    }

    fn render(&self, out: &mut String) {
        match self {
            Sx::Atom(a) => out.push_str(a),
            Sx::List(items) => {
                out.push('(');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(' ');
                    }
                    item.render(out);
                }
                out.push(')');
            }
        }
    }
}

fn parse_sexpr(src: &str) -> Sx {
    fn parse(b: &[u8], i: &mut usize) -> Sx {
        while *i < b.len() && b[*i].is_ascii_whitespace() {
            *i += 1;
        }
        if b[*i] == b'(' {
            *i += 1;
            let mut items = Vec::new();
            loop {
                while *i < b.len() && b[*i].is_ascii_whitespace() {
                    *i += 1;
                }
                if b[*i] == b')' {
                    *i += 1;
                    break;
                }
                items.push(parse(b, i));
            }
            Sx::List(items)
        } else if b[*i] == b'"' {
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
        } else {
            let start = *i;
            while *i < b.len() && !b[*i].is_ascii_whitespace() && b[*i] != b'(' && b[*i] != b')' {
                *i += 1;
            }
            Sx::Atom(String::from_utf8_lossy(&b[start..*i]).into_owned())
        }
    }
    let mut i = 0;
    parse(src.as_bytes(), &mut i)
}

/// The canonical position of a child the formatter is allowed to move:
/// header clauses (grammar D66) and `.work` item kinds (grammar 4.1, 4.2).
fn child_rank(parent: &str, child: &Sx) -> usize {
    let head = match child.head() {
        Some("item") => match child {
            Sx::List(items) => items.get(1).and_then(Sx::head).unwrap_or(""),
            Sx::Atom(_) => "",
        },
        Some(h) => h,
        None => "",
    };
    match (parent, head) {
        ("fn" | "op", "effects") => 1,
        ("fn" | "op", "requires") => 2,
        ("fn" | "op", "ensures") => 3,
        ("fn" | "op", "decreases") => 4,
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

/// A doc line of a dump (`(docs "line" ...)`, the line's text in Rust
/// string-literal escapes) without its trailing spaces and tabs, which the
/// formatter drops (`spec/toolchain.md` 2.4 rule 1 and its round trip
/// modulo `strip_layout`, Section 12). Any other trailing character, U+00A0
/// (`\u{a0}`) included, is text and stays.
fn trim_doc_line(atom: String) -> String {
    let Some(inner) = atom
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    else {
        return atom;
    };
    let mut inner = inner.to_string();
    loop {
        if inner.ends_with(' ') {
            inner.pop();
        } else if let Some(before) = inner.strip_suffix('t') {
            // `\t` is an escaped tab only when an odd number of backslashes
            // precedes the `t` (`\\t` is a backslash and a `t`).
            let backslashes = before.chars().rev().take_while(|&c| c == '\\').count();
            if backslashes % 2 == 1 {
                inner.truncate(inner.len() - 2);
            } else {
                break;
            }
        } else {
            break;
        }
    }
    format!("\"{inner}\"")
}

/// Remove from a dump the layout the formatter may legitimately change:
/// blank-line markers, the source order of header clauses and `.work`
/// items, and the trailing spaces and tabs of doc lines
/// ([`trim_doc_line`]). Everything else, comments included, must survive
/// formatting.
fn normalize(sx: Sx) -> Option<Sx> {
    let Sx::List(items) = sx else {
        return Some(sx);
    };
    let head = items.first().and_then(|h| match h {
        Sx::Atom(a) => Some(a.clone()),
        Sx::List(_) => None,
    });
    let head = head.unwrap_or_default();
    let mut out: Vec<Sx> = Vec::new();
    for (idx, item) in items.into_iter().enumerate() {
        if head == "trivia" && idx > 0 && item == Sx::Atom("blank".into()) {
            continue;
        }
        if head == "docs" && idx > 0 {
            if let Sx::Atom(line) = item {
                out.push(Sx::Atom(trim_doc_line(line)));
                continue;
            }
        }
        if let Some(n) = normalize(item) {
            out.push(n);
        }
    }
    if head == "trivia" && out.len() == 1 {
        return None;
    }
    if head == "lead-blank" {
        out[0] = Sx::Atom("lead".into());
    }
    if matches!(head.as_str(), "fn" | "op" | "while" | "change" | "task") {
        let (first, rest) = out.split_at(1);
        let mut rest = rest.to_vec();
        rest.sort_by_key(|c| child_rank(&head, c));
        out = first.to_vec();
        out.extend(rest);
    }
    Some(Sx::List(out))
}

fn normalize_dump(d: &str) -> String {
    let mut out = String::new();
    if let Some(sx) = normalize(parse_sexpr(d)) {
        sx.render(&mut out);
    }
    out
}

/// A normalized dump without its comments: `lead` and `trail` entries,
/// `trailing-comments` lists, and the wrappers that exist only to carry a
/// comment (`(item x ...)`, `(elem e ...)`, `(arg e ...)`, `(effect r ...)`,
/// `(sel s ...)`) are removed.
fn without_comments(sx: Sx) -> Option<Sx> {
    let Sx::List(items) = sx else {
        return Some(sx);
    };
    let head = match items.first() {
        Some(Sx::Atom(a)) => a.clone(),
        _ => String::new(),
    };
    if matches!(
        head.as_str(),
        "lead" | "lead-blank" | "trail" | "trailing-comments"
    ) {
        return None;
    }
    let mut out: Vec<Sx> = items.into_iter().filter_map(without_comments).collect();
    if head == "trivia" && out.len() == 1 {
        return None;
    }
    if matches!(head.as_str(), "item" | "elem" | "arg" | "effect" | "sel") && out.len() == 2 {
        return out.pop();
    }
    Some(Sx::List(out))
}

/// True when the formatter may legitimately give one of the dump's comments
/// a new owner (`spec/toolchain.md` 2.4 rule 11 with 2.3): a node with two
/// or more trailing comments prints the further ones on lines of their own,
/// where the next parse attaches them to the next node or to a body's or
/// file's trailing comments; and a selector inside `prove(...)`,
/// `test(...)`, or `expect base(...)` has its comments printed around the
/// whole line, where they become the requirement's or the clause's. Such
/// inputs are compared without comments, plus the multiset of comments
/// (toolchain 12: `strip_layout` clears every comment for this reason).
fn comments_may_move(sx: &Sx) -> bool {
    let Sx::List(items) = sx else {
        return false;
    };
    match sx.head() {
        Some("trivia") => items.iter().filter(|i| i.head() == Some("trail")).count() >= 2,
        Some("prove" | "test" | "expect") => {
            items.iter().any(|i| i.head() == Some("sel")) || items.iter().any(comments_may_move)
        }
        _ => items.iter().any(comments_may_move),
    }
}

/// The texts of the ordinary comments of a source, as a sorted multiset.
fn comments_of(src: &str) -> Vec<String> {
    let lexed = lex_full(src, FileId(0));
    let mut out: Vec<String> = lexed
        .trivia
        .iter()
        .filter(|t| t.kind == TriviaKind::Comment)
        .map(|t| {
            src[t.span.start as usize..t.span.end as usize]
                .trim_end()
                .to_string()
        })
        .collect();
    out.sort();
    out
}

#[derive(Clone, Debug)]
struct Input {
    label: String,
    kind: FileKind,
    text: String,
}

/// Every `.cpl`, `.work`, and `.patch` file under `examples/`, `std/`, and
/// `tests/golden/`.
fn corpus_inputs() -> Vec<Input> {
    let mut files = Vec::new();
    walk(&examples_dir(), &mut files);
    walk(&std_dir(), &mut files);
    walk(&golden_dir(), &mut files);
    files
        .into_iter()
        .filter_map(|path| {
            let kind = FileKind::from_path(&path)?;
            Some(Input {
                label: path.display().to_string(),
                kind,
                text: std::fs::read_to_string(&path).expect("read corpus file"),
            })
        })
        .collect()
}

/// Every Rust string literal in this crate's test sources, decoded.
fn string_literals(src: &str) -> Vec<String> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'r' if matches!(b.get(i + 1), Some(b'"') | Some(b'#'))
                && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')) =>
            {
                let mut j = i + 1;
                let mut hashes = 0;
                while b.get(j) == Some(&b'#') {
                    hashes += 1;
                    j += 1;
                }
                if b.get(j) != Some(&b'"') {
                    i += 1;
                    continue;
                }
                j += 1;
                let start = j;
                let close = format!("\"{}", "#".repeat(hashes));
                match src[start..].find(&close) {
                    Some(len) => {
                        out.push(src[start..start + len].to_string());
                        i = start + len + close.len();
                    }
                    None => i = b.len(),
                }
            }
            b'"' => {
                let mut j = i + 1;
                let mut text = String::new();
                while let Some(&c) = b.get(j) {
                    match c {
                        b'"' => {
                            j += 1;
                            break;
                        }
                        b'\\' => {
                            let next = b.get(j + 1).copied().unwrap_or(b'\\');
                            j += 2;
                            match next {
                                b'n' => text.push('\n'),
                                b'r' => text.push('\r'),
                                b't' => text.push('\t'),
                                b'0' => text.push('\0'),
                                b'\\' => text.push('\\'),
                                b'"' => text.push('"'),
                                b'\'' => text.push('\''),
                                b'u' if b.get(j) == Some(&b'{') => {
                                    let end = src[j..].find('}').map(|e| j + e).unwrap_or(j);
                                    if let Some(ch) = u32::from_str_radix(&src[j + 1..end], 16)
                                        .ok()
                                        .and_then(char::from_u32)
                                    {
                                        text.push(ch);
                                    }
                                    j = end + 1;
                                }
                                b'x' => {
                                    if let Ok(ch) = u8::from_str_radix(&src[j..j + 2], 16) {
                                        text.push(ch as char);
                                    }
                                    j += 2;
                                }
                                b'\n' => {
                                    while matches!(b.get(j), Some(b' ' | b'\t' | b'\n' | b'\r')) {
                                        j += 1;
                                    }
                                }
                                other => {
                                    text.push('\\');
                                    text.push(other as char);
                                }
                            }
                        }
                        _ => {
                            let ch = src[j..].chars().next().expect("char");
                            text.push(ch);
                            j += ch.len_utf8();
                        }
                    }
                }
                out.push(text);
                i = j;
            }
            b'\'' => {
                // A char literal (`'"'`, `'\n'`) or a lifetime.
                if b.get(i + 1) == Some(&b'\\') {
                    let mut j = i + 2;
                    while j < b.len() && b[j] != b'\'' {
                        j += 1;
                    }
                    i = j + 1;
                } else if b.get(i + 2) == Some(&b'\'') {
                    i += 3;
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    out
}

/// Interpret a test string as a `.cpl`, `.work`, or `.patch` file, or as a
/// statement list wrapped in a function body; `None` when it is none of
/// those (an error case or an unrelated string).
fn classify(lit: &str) -> Option<(FileKind, String)> {
    for kind in [FileKind::Cpl, FileKind::Work, FileKind::Patch] {
        if dump(lit, kind).is_some() {
            return Some((kind, lit.to_string()));
        }
    }
    let indented: Vec<String> = lit.lines().map(|l| format!("    {l}")).collect();
    let wrapped = format!(
        "module m\n\nfn f() -> Int {{\n{}\n}}\n",
        indented.join("\n")
    );
    if dump(&wrapped, FileKind::Cpl).is_some() {
        return Some((FileKind::Cpl, wrapped));
    }
    None
}

/// Every input string of every test file of this crate that parses cleanly.
fn test_source_inputs() -> Vec<Input> {
    let tests_dir = crate_dir().join("tests");
    let mut sources: Vec<PathBuf> = std::fs::read_dir(&tests_dir)
        .expect("tests dir")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("rs"))
        .collect();
    sources.sort();
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    let mut out = Vec::new();
    for path in sources {
        let src = std::fs::read_to_string(&path).expect("read test source");
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        for (i, lit) in string_literals(&src).into_iter().enumerate() {
            if lit.trim().is_empty() {
                continue;
            }
            if let Some((kind, text)) = classify(&lit) {
                if seen.insert((kind.extension().to_string(), text.clone())) {
                    out.push(Input {
                        label: format!("{name} literal #{i}"),
                        kind,
                        text,
                    });
                }
            }
        }
    }
    out
}

fn all_inputs() -> Vec<Input> {
    let mut inputs = corpus_inputs();
    inputs.extend(test_source_inputs());
    inputs
}

#[track_caller]
fn assert_formats_to(src: &str, kind: FileKind, expected: &str) {
    let actual = fmt_ok(src, kind);
    assert!(
        actual == expected,
        "formatting mismatch\n--- input\n{src}\n--- expected\n{expected}\n--- actual\n{actual}"
    );
}

// ---------------------------------------------------------------------------
// (1) idempotency and (2) round trip over every input
// ---------------------------------------------------------------------------

#[test]
fn formatting_is_idempotent_over_corpus_and_test_inputs() {
    let inputs = all_inputs();
    assert!(
        inputs.len() >= 60,
        "expected the corpus plus the other tests' inputs, found {}",
        inputs.len()
    );
    let mut failures = Vec::new();
    for input in &inputs {
        let once = match fmt(&input.text, input.kind) {
            Ok(t) => t,
            Err(e) => {
                failures.push(format!("{}: input does not format:\n{e}", input.label));
                continue;
            }
        };
        let twice = match fmt(&once, input.kind) {
            Ok(t) => t,
            Err(e) => {
                failures.push(format!(
                    "{}: formatted output does not parse:\n{once}\n{e}",
                    input.label
                ));
                continue;
            }
        };
        if once != twice {
            failures.push(format!(
                "{}: not idempotent\n--- once\n{once}\n--- twice\n{twice}",
                input.label
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} inputs failed:\n{}",
        failures.len(),
        inputs.len(),
        failures.join("\n\n")
    );
}

#[test]
fn formatting_preserves_the_tree() {
    let inputs = all_inputs();
    let mut failures = Vec::new();
    for input in &inputs {
        let before = dump(&input.text, input.kind).expect("input parses");
        let formatted = fmt_ok(&input.text, input.kind);
        let Some(after) = dump(&formatted, input.kind) else {
            failures.push(format!(
                "{}: formatted output does not parse:\n{formatted}",
                input.label
            ));
            continue;
        };
        let before_sx = normalize(parse_sexpr(&before));
        let after_sx = normalize(parse_sexpr(&after));
        if before_sx.as_ref().is_some_and(comments_may_move) {
            let moved = |sx: Option<Sx>| sx.and_then(without_comments);
            if moved(before_sx) != moved(after_sx)
                || comments_of(&input.text) != comments_of(&formatted)
            {
                failures.push(format!(
                    "{}: tree or comments changed\n--- input\n{}\n--- formatted\n{formatted}",
                    input.label, input.text
                ));
            }
            continue;
        }
        if normalize_dump(&before) != normalize_dump(&after) {
            failures.push(format!(
                "{}: tree changed\n--- input\n{}\n--- formatted\n{formatted}\n--- dump before\n{before}\n--- dump after\n{after}",
                input.label, input.text
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} inputs failed:\n{}",
        failures.len(),
        inputs.len(),
        failures.join("\n\n")
    );
}

/// The tree comparison drops exactly the trailing spaces and tabs of a doc
/// line (toolchain 2.4: the round trip holds modulo `strip_layout`, which
/// trims them, Section 12), reading the dump's escapes: `\\t` is a
/// backslash and a `t`, and U+00A0 is text.
#[test]
fn tree_comparison_trims_only_the_trailing_spaces_and_tabs_of_doc_lines() {
    let trim = |dumped: &str| trim_doc_line(dumped.to_string());
    assert_eq!(trim(r#""a \t \t""#), r#""a""#);
    assert_eq!(trim(r#""\t""#), r#""""#);
    assert_eq!(trim(r#""a\\t""#), r#""a\\t""#);
    assert_eq!(trim(r#""a\\\t""#), r#""a\\""#);
    assert_eq!(trim(r#""a\u{a0}""#), r#""a\u{a0}""#);
    assert_eq!(trim(r#"" a""#), r#"" a""#);
    let src = "module m\n\n/// x \t\nfn f() -> Int {\n    1\n}\n";
    let trimmed = "module m\n\n/// x\nfn f() -> Int {\n    1\n}\n";
    let dumped = |s: &str| normalize_dump(&dump(s, FileKind::Cpl).expect("parses"));
    assert_eq!(dumped(src), dumped(trimmed));
    let nbsp = "module m\n\n/// x\u{a0}\nfn f() -> Int {\n    1\n}\n";
    assert_ne!(dumped(nbsp), dumped(trimmed));
}

// ---------------------------------------------------------------------------
// (3) golden formatted output for the design-document examples
// ---------------------------------------------------------------------------

#[test]
fn design_doc_examples_match_golden_formatting() {
    let dir = golden_dir();
    let mut inputs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("tests/golden exists")
        .map(|e| e.expect("entry").path())
        .filter(|p| FileKind::from_path(p).is_some())
        .collect();
    inputs.sort();
    assert!(inputs.len() >= 12, "found {} golden inputs", inputs.len());
    let update = std::env::var("UPDATE_GOLDEN")
        .map(|v| v == "1")
        .unwrap_or(false);
    let mut failures = Vec::new();
    for input in inputs {
        let kind = FileKind::from_path(&input).expect("kind");
        let text = std::fs::read_to_string(&input).expect("read golden input");
        let actual = match fmt(&text, kind) {
            Ok(t) => t,
            Err(e) => {
                failures.push(format!("{}: {e}", input.display()));
                continue;
            }
        };
        let expected_path = input.with_extension("fmt");
        if update {
            std::fs::write(&expected_path, &actual).expect("write golden expectation");
            continue;
        }
        match std::fs::read_to_string(&expected_path) {
            Ok(expected) if expected == actual => {}
            Ok(expected) => failures.push(format!(
                "{} differs from {}\n--- expected\n{expected}\n--- actual\n{actual}",
                input.display(),
                expected_path.display()
            )),
            Err(_) => failures.push(format!(
                "{} has no expectation {} (run with UPDATE_GOLDEN=1 to create it)",
                input.display(),
                expected_path.display()
            )),
        }
    }
    assert!(
        failures.is_empty(),
        "golden mismatches:\n{}",
        failures.join("\n\n")
    );
}

// ---------------------------------------------------------------------------
// (4) comment preservation
// ---------------------------------------------------------------------------

const COMMENT_CASES: &[(FileKind, &str)] = &[
    // leading and trailing comments at every top-level position
    (
        FileKind::Cpl,
        "// license\n/// Module docs\nmodule m // header\n\n// about use\nuse a.b.{c} // trailing use\n\n// about f\n@id(\"x\") // ann\nfn f() -> Int { // after brace\n    // before let\n    let x = 1 // after let\n\n    // before tail\n    x\n    // dangling in block\n} // after fn\n// at end\n",
    ),
    // between a header and its continuation lines, inside parameter lists and effects sets
    (
        FileKind::Cpl,
        "module m\n\nfn f(\n    // the id\n    id: Int, // trailing param\n    // k\n    using db: PageRead,\n    // dangling params\n) -> Int // after header\n    // before requires\n    requires id > 0 // after requires\n    // before effects\n    effects {\n        // in effects\n        db.get, // after effect\n        // dangling effects\n    }\n    // before brace\n{\n    1\n}\n",
    ),
    // inside match arms
    (
        FileKind::Cpl,
        "module m\n\nfn f(r: R) -> Int {\n    match r { // after match brace\n        // first arm\n        Ok(v) => v // ok\n\n        // second arm\n        Err(_) => { // arm block\n            0 // zero\n        } // after arm block\n        // after arms\n    }\n}\n",
    ),
    // inside multi-line list and struct literals
    (
        FileKind::Cpl,
        "module m\n\nfn g(p: Page) -> Page {\n    let l = [ // open list\n        1, // one\n        // two next\n        2,\n        // end list\n    ] // after list\n    Page { // open struct\n        ..p, // spread\n        // text next\n        text: t, // set\n        // end struct\n    } // after struct\n}\n",
    ),
    // struct, enum, and capability bodies, use lists, while clauses
    (
        FileKind::Cpl,
        "module m\n\nuse a.b.{ // open use\n    c, // c\n    // d next\n    D,\n    // end use\n} // after use\n\n// about Page\npub struct Page { // open page\n    /// the id\n    // plain\n    id: PageId // trailing id\n\n    text: Text\n    // dangling field\n}\n\nenum E {\n    /// doc a\n    A(Int) // after a\n    // before b\n    B\n}\n\ncapability C {\n    // before get\n    fn get(id: Int) -> Int // after get\n    fn put(id: Int) -> Int\n        // before requires\n        requires id > 0 // after requires\n    // dangling op\n}\n\nfn loop_it(n: Int) -> Int {\n    var i = n\n    while i > 0 // after cond\n        // before invariant\n        invariant i >= 0 // after invariant\n        decreases i\n    { // after loop brace\n        i = i - 1\n    } // after loop\n    i\n}\n",
    ),
    // .work files: every brace body and selector set
    (
        FileKind::Work,
        "// top\nuse a.b.{f} // uses\n\n/// The change.\nchange c { // after change brace\n    // where\n    base git(\"main\") // b\n    target git(\"main\")\n\n    /// A task.\n    task t after x { // task brace\n        write { // open write\n            f.body, // h\n            // dangling write\n        } // after write\n        // before goal\n        goal \"g\" // k\n    } // after task\n    integrate { // integrate brace\n        require typecheck // n\n        require prove(f) // after prove\n        // dangling integrate\n    }\n    // end of change\n} // after change\n// eof\n",
    ),
    // .patch files
    (
        FileKind::Patch,
        "// before patch\n/// Replace the body.\npatch f.body // after target\n    // check\n    expect base(f.body) // e\n{\n    // inside\n    1 // one\n}\n// end\n",
    ),
];

#[test]
fn every_comment_survives_formatting_exactly_once() {
    let mut cases: Vec<(FileKind, String)> = COMMENT_CASES
        .iter()
        .map(|(k, s)| (*k, s.to_string()))
        .collect();
    cases.extend(corpus_inputs().into_iter().map(|i| (i.kind, i.text)));
    let mut checked = 0;
    for (kind, src) in &cases {
        let before = comments_of(src);
        if before.is_empty() {
            continue;
        }
        let formatted = fmt_ok(src, *kind);
        let after = comments_of(&formatted);
        assert_eq!(
            before, after,
            "comments changed\n--- input\n{src}\n--- formatted\n{formatted}"
        );
        // Formatting the result again keeps them too.
        let again = fmt_ok(&formatted, *kind);
        assert_eq!(formatted, again, "not idempotent:\n{formatted}");
        checked += 1;
    }
    assert!(
        checked >= COMMENT_CASES.len() + 3,
        "checked {checked} cases"
    );
}

#[test]
fn comments_keep_their_attachment() {
    let src = "module m\n\nfn f(\n    a: Int, // a\n    // before b\n    b: Int,\n) -> Int\n    // before requires\n    requires a > 0 // after requires\n{\n    // lead\n    let x = [\n        1, // one\n        // before two\n        2,\n    ] // after list\n    x\n}\n";
    let expected = "module m\n\nfn f(\n    a: Int, // a\n    // before b\n    b: Int,\n) -> Int\n    // before requires\n    requires a > 0 // after requires\n{\n    // lead\n    let x = [\n        1, // one\n        // before two\n        2,\n    ] // after list\n    x\n}\n";
    assert_formats_to(src, FileKind::Cpl, expected);
}

#[test]
fn further_trailing_comments_print_where_the_next_parse_attaches_them() {
    // `spec/toolchain.md` 2.4 rule 11: the first trailing comment of a node
    // follows its last token and a second one goes on a line of its own
    // below it. The parser attaches a comment on its own line to what
    // follows it (2.3 rules C to F), so the formatter prints it where that
    // attachment prints it, and formatting is idempotent (Section 12). A
    // comment inside a single-payload bracket or a parenthesized expression
    // is trailing trivia of the enclosing attachable node (rule G), so each
    // input below gives one node two trailing comments.
    let cases: &[(FileKind, &str, &str)] = &[
        // A top-level declaration followed by another (the defect): the
        // second comment prints after the separating blank line, above the
        // next declaration, instead of gaining a blank line on every run.
        (
            FileKind::Cpl,
            "module m\n\npub type A(Int // c\n) // d\nfn f() -> Int { 1 }\n",
            "module m\n\npub type A(Int) // c\n\n// d\nfn f() -> Int {\n    1\n}\n",
        ),
        // ... also when the next declaration is a `type` on the line below,
        // which the comment between them now separates (T51), and when a
        // blank line preceded the next declaration (kept after the comment,
        // T11).
        (
            FileKind::Cpl,
            "module m\n\npub type A(Int // c\n) // d\npub type B(Int)\n",
            "module m\n\npub type A(Int) // c\n\n// d\npub type B(Int)\n",
        ),
        (
            FileKind::Cpl,
            "module m\n\npub type P = Int where (self // c\n    > 0) // d\n\npub type B(Int)\n",
            "module m\n\npub type P = Int where (self\n    > 0) // c\n\n// d\n\npub type B(Int)\n",
        ),
        // After the last declaration: the file's trailing comments (rule F).
        (
            FileKind::Cpl,
            "module m\n\npub type A(Int // c\n) // d\n// e\n",
            "module m\n\npub type A(Int) // c\n// d\n// e\n",
        ),
        // An annotation: a comment among the prefix lines is the
        // declaration's and prints above its doc comments (rule 5).
        (
            FileKind::Cpl,
            "module m\n\n/// Docs.\n@id(\"x\" // c\n) // d\nfn f() -> Int { 1 }\n",
            "module m\n\n// d\n/// Docs.\n@id(\n    \"x\",\n) // c\nfn f() -> Int {\n    1\n}\n",
        ),
        // The last clause before a body: below the `{` (rule C).
        (
            FileKind::Cpl,
            "module m\n\nfn f(x: Int) -> Int\n    requires (x // c\n        > 0) // d\n{\n    x\n}\n",
            "module m\n\nfn f(x: Int) -> Int\n    requires (x\n        > 0) // c\n{\n    // d\n    x\n}\n",
        ),
        // A clause before another clause: above the next one (rule D).
        (
            FileKind::Cpl,
            "module m\n\nfn f(x: Int) -> Int\n    requires (x // c\n        > 0) // d\n    ensures true\n{\n    x\n}\n",
            "module m\n\nfn f(x: Int) -> Int\n    requires (x\n        > 0) // c\n    // d\n    ensures true\n{\n    x\n}\n",
        ),
        // A loop clause before an empty body, which then holds the comment.
        (
            FileKind::Cpl,
            "module m\n\nfn f(x: Int) -> Int {\n    while c\n        invariant (x // c\n            > 0) // d\n    {\n    }\n    x\n}\n",
            "module m\n\nfn f(x: Int) -> Int {\n    while c\n        invariant (x\n            > 0) // c\n    {\n        // d\n    }\n    x\n}\n",
        ),
        // A `for` clause of a property.
        (
            FileKind::Cpl,
            "module m\n\nproperty \"p\"\n    for x in (xs // c\n    ) // d\n{\n    assert true\n}\n",
            "module m\n\nproperty \"p\"\n    for x in (xs) // c\n{\n    // d\n    assert true\n}\n",
        ),
        // The last clause of a bodiless function: above the next declaration.
        (
            FileKind::Cpl,
            "module m\n\nfn f(x: Int) -> Int\n    requires (x // c\n        > 0) // d\nfn g() -> Int { 1 }\n",
            "module m\n\nfn f(x: Int) -> Int\n    requires (x\n        > 0) // c\n\n// d\nfn g() -> Int {\n    1\n}\n",
        ),
        // A capability operation's last clause: at the operations' level.
        (
            FileKind::Cpl,
            "module m\n\ncapability C {\n    fn a(x: Int) -> Int\n        requires (x // c\n            > 0) // d\n    fn b() -> Int\n}\n",
            "module m\n\ncapability C {\n    fn a(x: Int) -> Int\n        requires (x\n            > 0) // c\n    // d\n    fn b() -> Int\n}\n",
        ),
        // The `expect` line of a patch form: below the body's `{`.
        (
            FileKind::Patch,
            "patch f.body\n    expect base(f.body // c\n    ) // d\n{\n    1\n}\n",
            "patch f.body\n    expect base(f.body) // c\n{\n    // d\n    1\n}\n",
        ),
        // All of it at once: a declaration's further comment, the next
        // one's leading comment, an annotation's further comment, a comment
        // among the prefix lines, and the file's trailing comments.
        (
            FileKind::Cpl,
            "module m\n\npub type A(Int // a1\n) // a2\n// lead b\n\n/// Docs b.\n@id(\"b\" // b1\n) // b2\n// among\n@prove\npub type B(Int // b3\n) // b4\n\n// tail1\n\n// tail2\n",
            "module m\n\npub type A(Int) // a1\n\n// a2\n// lead b\n\n// b2\n// among\n/// Docs b.\n@id(\n    \"b\",\n) // b1\n@prove\npub type B(Int) // b3\n// b4\n\n// tail1\n\n// tail2\n",
        ),
        // Inside a body nothing intervenes: the next lines, as before.
        (
            FileKind::Cpl,
            "module m\n\nfn f(a: Int) -> Int {\n    let x = (a // c\n    ) // d\n    x\n}\n",
            "module m\n\nfn f(a: Int) -> Int {\n    let x = (a) // c\n    // d\n    x\n}\n",
        ),
    ];
    for (kind, src, expected) in cases {
        assert_formats_to(src, *kind, expected);
        assert_formats_to(expected, *kind, expected);
        assert_eq!(
            comments_of(src),
            comments_of(expected),
            "every comment survives:\n{src}"
        );
    }
}

// ---------------------------------------------------------------------------
// (5) the corpus and the standard library are canonical already
// ---------------------------------------------------------------------------

/// Formatting returns every corpus file and every std stub unchanged. Both
/// are fixed text (the design's code blocks byte for byte, and the normative
/// stub text of `spec/stdlib.md` Section 11), so a difference is a formatter
/// defect, never a reason to edit the file.
#[test]
fn examples_and_std_are_already_canonical() {
    let mut files = Vec::new();
    walk(&examples_dir(), &mut files);
    walk(&std_dir(), &mut files);
    let mut checked = 0;
    let mut failures = Vec::new();
    for path in files {
        let Some(kind) = FileKind::from_path(&path) else {
            continue;
        };
        let text = std::fs::read_to_string(&path).expect("read example");
        checked += 1;
        match fmt(&text, kind) {
            Ok(formatted) if formatted == text => {}
            Ok(formatted) => {
                let mut diff = String::new();
                for (i, (a, b)) in text.lines().zip(formatted.lines()).enumerate() {
                    if a != b {
                        diff.push_str(&format!("  line {}:\n    - {a}\n    + {b}\n", i + 1));
                    }
                }
                failures.push(format!(
                    "{} is not canonical:\n{diff}--- formatted\n{formatted}",
                    path.display()
                ));
            }
            Err(e) => failures.push(format!("{}: {e}", path.display())),
        }
    }
    assert!(checked >= 11, "checked {checked} example and std files");
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

// ---------------------------------------------------------------------------
// Layout rules
// ---------------------------------------------------------------------------

#[test]
fn worked_example_of_the_toolchain_specification() {
    // spec/toolchain.md 2.4.1
    let src = "module demo.layout\nuse std.text.{trim}\npub fn load_page(id: PageId,   using db: PageRead) -> Result<Page, StoreError>\n    ensures true\n    effects { db.get }\n{\n    // fetch\n    let page = db.get(id)?   // may fail\n\n\n    Ok(page)\n}\n";
    let expected = "module demo.layout\n\nuse std.text.{trim}\n\npub fn load_page(id: PageId, using db: PageRead) -> Result<Page, StoreError>\n    effects { db.get }\n    ensures true\n{\n    // fetch\n    let page = db.get(id)? // may fail\n\n    Ok(page)\n}\n";
    assert_formats_to(src, FileKind::Cpl, expected);
}

#[test]
fn clauses_are_emitted_in_canonical_order() {
    let src = "module m\n\nfn f(a: Int) -> Int\n    decreases a\n    ensures result > 0\n    requires a > 0\n    ensures result < 10\n    requires a < 10\n    effects { }\n{\n    var x = a\n    while x > 0\n        decreases x\n        invariant x >= 0\n        invariant x <= a\n    {\n        x = x - 1\n    }\n    x\n}\n";
    let expected = "module m\n\nfn f(a: Int) -> Int\n    effects {}\n    requires a > 0\n    requires a < 10\n    ensures result > 0\n    ensures result < 10\n    decreases a\n{\n    var x = a\n    while x > 0\n        invariant x >= 0\n        invariant x <= a\n        decreases x\n    {\n        x = x - 1\n    }\n    x\n}\n";
    assert_formats_to(src, FileKind::Cpl, expected);
}

#[test]
fn blank_lines_collapse_and_body_edges_have_none() {
    let src = "module m\nuse a.b.{c}\n\n\n\nuse a.b.{d}\nfn a() -> Int {\n\n    1\n\n\n}\nfn b() -> Int {\n    let x = 1\n\n\n\n    x\n}\n\n\n\n";
    let expected = "module m\n\nuse a.b.{c}\n\nuse a.b.{d}\n\nfn a() -> Int {\n    1\n}\n\nfn b() -> Int {\n    let x = 1\n\n    x\n}\n";
    assert_formats_to(src, FileKind::Cpl, expected);
}

#[test]
fn declarations_are_separated_by_exactly_one_blank_line() {
    // Rule 3: exactly one blank line between declarations whatever the
    // source had, `type` declarations included: adjacent `fn`s, a `type`
    // after a `struct`, and adjacent `type`s with a doc comment, an
    // annotation, or a comment between them.
    let src = "module m\nfn a() -> Int { 1 }\nfn b() -> Int { 2 }\n\n\n\nfn c() -> Int { 3 }\npub struct Page {\n    id: PageId\n}\npub type Last(Int)\n/// Documented.\npub type Doc(Int)\n@id(\"t_1\")\npub type Annotated(Int)\npub type Plain(Int)\n// about the next type\npub type Commented(Int)\n";
    let expected = "module m\n\nfn a() -> Int {\n    1\n}\n\nfn b() -> Int {\n    2\n}\n\nfn c() -> Int {\n    3\n}\n\npub struct Page {\n    id: PageId\n}\n\npub type Last(Int)\n\n/// Documented.\npub type Doc(Int)\n\n@id(\"t_1\")\npub type Annotated(Int)\n\npub type Plain(Int)\n\n// about the next type\npub type Commented(Int)\n";
    assert_formats_to(src, FileKind::Cpl, expected);
}

#[test]
fn bare_type_declarations_on_adjacent_lines_keep_the_designs_layout() {
    // The one exception to rule 3 (a spec deviation, see the formatter's
    // module documentation): the design writes `pub type PageId(Text)`
    // directly above `pub type Positive = ...` (design lines 53-54, verbatim
    // in examples/reader), so two `type` declarations without doc comments
    // or annotations that the source wrote on adjacent lines stay adjacent.
    // A blank line written between them is kept; any other neighbour gets
    // exactly one.
    let src = "module m\n\npub type PageId(Text)\npub type Positive = Int where self > 0\ntype List3<T>\n\npub type Other(Int)\npub struct Page {\n    id: PageId\n}\npub type Last(Int)\n";
    let expected = "module m\n\npub type PageId(Text)\npub type Positive = Int where self > 0\ntype List3<T>\n\npub type Other(Int)\n\npub struct Page {\n    id: PageId\n}\n\npub type Last(Int)\n";
    assert_formats_to(src, FileKind::Cpl, expected);
    // A comment above the first of the pair or after it on its line does not
    // separate them.
    let commented = "module m\n\n// glue: copied\npub type PageId(Text) // the id\npub type Positive = Int where self > 0\n";
    assert_formats_to(commented, FileKind::Cpl, commented);
}

#[test]
fn a_blank_line_between_leading_comments_and_their_node_is_kept() {
    // T11 and 2.4 rule 11: `// section`, a blank line, then the node (the
    // layout of std/prelude.cpl) keeps both blank lines, for declarations,
    // `use` declarations, statements, members, arms, and `.work`/`.patch`
    // items alike.
    let cpl = "module m\n\n// about the imports\n\nuse a.b.{c}\n// the second import\n\nuse a.b.{d}\n\n// ---- section ----\n\n/// Docs.\n@builtin(\"x\")\npub type A\n\n// two comments\n\n// and a blank\n\nfn f(r: Result<Int, Int>) -> Int {\n    let x = 1\n    // about y\n\n    let y = 2\n    match r {\n        Ok(v) => v\n        // the error arm\n\n        Err(e) => e\n    }\n}\n\npub struct S {\n    a: Int\n    // about b\n\n    b: Int\n}\n";
    assert_formats_to(cpl, FileKind::Cpl, cpl);
    let work = "use a.b.{f}\n\n// about the change\n\nchange c {\n    base git(\"main\")\n    // about the task\n\n    task t {\n        write { f.body }\n        // why\n\n        goal \"g\"\n    }\n}\n";
    assert_formats_to(work, FileKind::Work, work);
    let patch = "patch f.body\n    expect base(f.body)\n{\n    1\n}\n\n// the second form\n\npatch g.body\n    expect base(g.body)\n{\n    2\n}\n";
    assert_formats_to(patch, FileKind::Patch, patch);
}

#[test]
fn the_first_node_of_a_body_never_gets_a_blank_line() {
    // 2.3: the parser records no blank line before the first node of a brace
    // body, so none is printed after its comments either (rule 4: no blank
    // line after `{`).
    let src = "module m\n\nfn f() -> Int {\n    // first\n\n    1\n}\n";
    let expected = "module m\n\nfn f() -> Int {\n    // first\n    1\n}\n";
    assert_formats_to(src, FileKind::Cpl, expected);
    // Rule 12 moves `base` to the front of its change; as the first item it
    // loses the blank line after its comment, and formatting is stable.
    let work = "change c {\n    target git(\"y\")\n    // the base\n\n    base git(\"x\")\n}\n";
    let expected = "change c {\n    // the base\n    base git(\"x\")\n    target git(\"y\")\n}\n";
    assert_formats_to(work, FileKind::Work, expected);
    assert_formats_to(expected, FileKind::Work, expected);
}

#[test]
fn empty_annotation_argument_lists_are_kept() {
    // T14: the formatter never removes a token, so `@name()` keeps its
    // parentheses; `@name` stays bare.
    let src = "module m\n\n@prove()\n@id(\"x\")\n@empty(\n)\n@bare\nfn f() -> Int {\n    1\n}\n";
    let expected = "module m\n\n@prove()\n@id(\"x\")\n@empty()\n@bare\nfn f() -> Int {\n    1\n}\n";
    assert_formats_to(src, FileKind::Cpl, expected);
}

#[test]
fn spacing_is_canonical() {
    let src = "module m\n\nuse a.b.{ c , D }\nuse a.b.e\n\nfn f( x:Int , y : List< Int > ) -> Int {\n    let z = g( x,y )+x*y\n    let l = [ 1,2 ]\n    let p = Page{..p,text:t}\n    let q = Page{}\n    let n = -x ?\n    let w = not  x\n    let h = check< Positive >( n )\n    let c = h( a , using db )\n    forall q in l : q>0\n    z\n}\n";
    let expected = "module m\n\nuse a.b.{c, D}\nuse a.b.e\n\nfn f(x: Int, y: List<Int>) -> Int {\n    let z = g(x, y) + x * y\n    let l = [1, 2]\n    let p = Page { ..p, text: t }\n    let q = Page {}\n    let n = -x?\n    let w = not x\n    let h = check<Positive>(n)\n    let c = h(a, using db)\n    forall q in l: q > 0\n    z\n}\n";
    assert_formats_to(src, FileKind::Cpl, expected);
}

#[test]
fn brace_placement_follows_clauses_and_broken_parameter_lists() {
    let src = "module m\n\nfn a(x: Int) -> Int\n{\n    x\n}\n\nfn b(\n    x: Int) -> Int {\n    x\n}\n\nfn c(x: Int) -> Int\n    requires x > 0\n{\n    x\n}\n\ntest \"t\"\n{\n    assert true\n}\n\nproperty \"p\"\n    for x in gen.int\n{\n    assert true\n}\n\nfn d() -> Unit {}\n";
    let expected = "module m\n\nfn a(x: Int) -> Int {\n    x\n}\n\nfn b(\n    x: Int,\n) -> Int\n{\n    x\n}\n\nfn c(x: Int) -> Int\n    requires x > 0\n{\n    x\n}\n\ntest \"t\" {\n    assert true\n}\n\nproperty \"p\"\n    for x in gen.int\n{\n    assert true\n}\n\nfn d() -> Unit {}\n";
    assert_formats_to(src, FileKind::Cpl, expected);
}

#[test]
fn broken_groups_stay_broken_and_flat_groups_stay_flat() {
    let src = "module m\n\nfn f() -> Int {\n    let a = g(\n        1, 2)\n    let b = [\n        1,\n    ]\n    let c = Page {\n        text: t }\n    let d = g(1,\n        2)\n    a\n}\n";
    let expected = "module m\n\nfn f() -> Int {\n    let a = g(\n        1,\n        2,\n    )\n    let b = [\n        1,\n    ]\n    let c = Page {\n        text: t,\n    }\n    let d = g(\n        1,\n        2,\n    )\n    a\n}\n";
    assert_formats_to(src, FileKind::Cpl, expected);
}

#[test]
fn long_lines_break_the_outermost_group_first() {
    let src = "module m\n\nfn f() -> Int {\n    let value = outer_function(inner_function(argument_one, argument_two), inner_function(argument_three, argument_four))\n    value\n}\n";
    let expected = "module m\n\nfn f() -> Int {\n    let value = outer_function(\n        inner_function(argument_one, argument_two),\n        inner_function(argument_three, argument_four),\n    )\n    value\n}\n";
    assert_formats_to(src, FileKind::Cpl, expected);
    // A line that fits stays flat (99 columns with the indentation).
    let short = "module m\n\nfn f() -> Int {\n    let value = outer_function(inner_function(argument_one, argument_two), inner_function(aaa, bb))\n    value\n}\n";
    assert_eq!(fmt_ok(short, FileKind::Cpl), short);
    // Exactly 100 columns still fits.
    let edge = "module m\n\npub fn a_rather_long_function_name(first_parameter: Int, second_parameter: Int, third: Int) -> Int {\n    1\n}\n";
    assert_eq!(edge.lines().nth(2).unwrap().chars().count(), 100);
    assert_eq!(fmt_ok(edge, FileKind::Cpl), edge);
    // One more column breaks the parameter list, which moves the `{`.
    let header = "module m\n\npub fn a_rather_long_function_name_(first_parameter: Int, second_parameter: Int, third: Int) -> Int {\n    1\n}\n";
    let expected = "module m\n\npub fn a_rather_long_function_name_(\n    first_parameter: Int,\n    second_parameter: Int,\n    third: Int,\n) -> Int\n{\n    1\n}\n";
    assert_formats_to(header, FileKind::Cpl, expected);
}

#[test]
fn parenthesized_operator_breaks_are_kept_and_others_removed() {
    // A break before an operator inside `( )` is kept (rule 9); inside a
    // call's argument list it is a break of the group (rule 6 (i)) and the
    // expression itself is joined; a break after an operator is removed.
    let src = "module m\n\nfn f(a: Int, b: Int) -> Int\n    requires (a > 0\n              and b > 0)\n{\n    let total = (a\n      + b)\n    let other = g(a\n        + b)\n    let third = (a +\n        b)\n    let nested = ((a\n        + b) * (a\n            - b))\n    total\n}\n";
    // The inner `(a - b)` opens on the continuation line (level 2), so its
    // own continuation is one level deeper still.
    let expected = "module m\n\nfn f(a: Int, b: Int) -> Int\n    requires (a > 0\n        and b > 0)\n{\n    let total = (a\n        + b)\n    let other = g(\n        a + b,\n    )\n    let third = (a + b)\n    let nested = ((a\n        + b) * (a\n            - b))\n    total\n}\n";
    assert_formats_to(src, FileKind::Cpl, expected);
}

#[test]
fn text_literals_are_reescaped_canonically() {
    let src = "module m\n\ntest \"a\\u{9}b\" {\n    let s = \"\\u{41}\\u{a}\\u{7f}\\u{22}\\u{5c}\\u{1}caf\\u{e9}\"\n    let t = \"\\t\\r\\n\"\n    assert s != t\n}\n";
    let expected = "module m\n\ntest \"a\\tb\" {\n    let s = \"A\\n\\u{7f}\\\"\\\\\\u{1}café\"\n    let t = \"\\t\\r\\n\"\n    assert s != t\n}\n";
    assert_formats_to(src, FileKind::Cpl, expected);
}

#[test]
fn work_items_are_emitted_in_canonical_order() {
    let src = "use a.b.{f}\nchange c {\n    integrate {\n        require typecheck\n    }\n    task t {\n        goal \"g\"\n        preserve { f.contract }\n        write { f.body }\n    }\n    workers 2\n    target git(\"main\")\n    base git(\"main\")\n}\nchange d {\n    base git(\"x\")\n    target git(\"y\")\n    task u after t, v {\n        write {}\n        add {tests.a.*}\n    }\n}\n";
    let expected = "use a.b.{f}\n\nchange c {\n    base git(\"main\")\n    target git(\"main\")\n    workers 2\n    task t {\n        write { f.body }\n        preserve { f.contract }\n        goal \"g\"\n    }\n    integrate {\n        require typecheck\n    }\n}\n\nchange d {\n    base git(\"x\")\n    target git(\"y\")\n    task u after t, v {\n        write {}\n        add { tests.a.* }\n    }\n}\n";
    assert_formats_to(src, FileKind::Work, expected);
}

#[test]
fn patch_forms_have_the_fixed_layout() {
    let src = "patch f.body\n    expect base(f.body)\n{\n    1\n}\npatch g.body\n    expect base(g.body)\n{\n    var x = 1\n    x\n}\n";
    let expected = "patch f.body\n    expect base(f.body)\n{\n    1\n}\n\npatch g.body\n    expect base(g.body)\n{\n    var x = 1\n    x\n}\n";
    assert_formats_to(src, FileKind::Patch, expected);
}

#[test]
fn else_rejoins_the_closing_brace_inside_parentheses() {
    // Grammar 2.4 rule 4: inside brackets an `else` may start a line; the
    // formatter puts it back after the `}`. The line break was at the
    // argument group's own level, so the group itself stays broken (rule
    // 6 (i)); a block inside a flat group does not break the group.
    let src = "module m\n\nfn f(c: Bool) -> Int {\n    g(if c { 1 }\n    else { 2 })\n}\n";
    let expected = "module m\n\nfn f(c: Bool) -> Int {\n    g(\n        if c {\n            1\n        } else {\n            2\n        },\n    )\n}\n";
    assert_formats_to(src, FileKind::Cpl, expected);
    let flat = "module m\n\nfn f(c: Bool) -> Int {\n    g(if c { 1 } else { 2 }, 3)\n}\n";
    let expected = "module m\n\nfn f(c: Bool) -> Int {\n    g(if c {\n        1\n    } else {\n        2\n    }, 3)\n}\n";
    assert_formats_to(flat, FileKind::Cpl, expected);
    let chain = "module m\n\nfn f(c: Bool) -> Int {\n    let v = if c { 1 } else if not c { 2 } else { 3 }\n    match v { 1 => { 2 } _ => 3 }\n}\n";
    let expected = "module m\n\nfn f(c: Bool) -> Int {\n    let v = if c {\n        1\n    } else if not c {\n        2\n    } else {\n        3\n    }\n    match v {\n        1 => {\n            2\n        }\n        _ => 3\n    }\n}\n";
    // `match v { 1 => { 2 } _ => 3 }` is two arms on one line and therefore
    // not legal input; feed the arms on separate lines instead.
    let chain = chain.replace(
        "match v { 1 => { 2 } _ => 3 }",
        "match v {\n    1 => { 2 }\n    _ => 3 }",
    );
    assert_formats_to(&chain, FileKind::Cpl, expected);
}

#[test]
fn docs_come_before_annotations_in_source_order() {
    let src = "module m\n\n@prove\n/// doc one\n@id(\"x\")\n/// doc two\npub fn f() -> Int {\n    1\n}\n\npub struct S {\n    /// the field\n    a: Int\n}\n";
    let expected = "module m\n\n/// doc one\n/// doc two\n@prove\n@id(\"x\")\npub fn f() -> Int {\n    1\n}\n\npub struct S {\n    /// the field\n    a: Int\n}\n";
    assert_formats_to(src, FileKind::Cpl, expected);
}

#[test]
fn comments_among_prefix_lines_print_above_the_docs_after_the_blank_line() {
    // Rule 5: a comment written among a declaration's prefix lines is its
    // leading comment and prints above the doc comments. The blank line the
    // source had before the declaration's first token (T11) stays before
    // the comments written after that token, which is where it was; the
    // ones written above the declaration keep it after them.
    let cases = [
        (
            "module m\n\n@id(\"x\")\n// why\nfn f() -> Int { 1 }\n",
            "module m\n\n// why\n@id(\"x\")\nfn f() -> Int {\n    1\n}\n",
        ),
        (
            "module m\n\n// section\n\n/// Docs.\n// note\n@prove\nfn f() -> Int { 1 }\n",
            "module m\n\n// section\n\n// note\n/// Docs.\n@prove\nfn f() -> Int {\n    1\n}\n",
        ),
    ];
    for (src, expected) in cases {
        assert_formats_to(src, FileKind::Cpl, expected);
        assert_formats_to(expected, FileKind::Cpl, expected);
    }
}

#[test]
fn file_ends_with_exactly_one_newline() {
    let no_newline = "module m\n\nfn f() -> Int {\n    1\n}";
    let formatted = fmt_ok(no_newline, FileKind::Cpl);
    assert!(formatted.ends_with("}\n") && !formatted.ends_with("\n\n"));
    assert_eq!(
        fmt_ok(&format!("{no_newline}\n\n\n"), FileKind::Cpl),
        formatted
    );
    assert_eq!(fmt_ok("module m", FileKind::Cpl), "module m\n");
    assert!(formatted.lines().all(|l| l == l.trim_end()));
}

#[test]
fn format_source_refuses_input_it_cannot_parse() {
    let mut map = SourceMap::new();
    let src = "module m\n\nfn f() -> Int {\n    let x = \n}\n";
    let file = map.add(
        "broken.cpl".to_string(),
        FileOrigin::Virtual,
        src.to_string(),
    );
    let err = format_source(src, FileKind::Cpl, file).expect_err("syntax error");
    assert!(err
        .iter()
        .any(|d| d.code == cpl_syntax::codes::SYNTAX_ERROR));
    assert!(format_source("", FileKind::Work, file).is_err());
    assert!(format_source("module m\n", FileKind::Patch, file).is_err());
}

#[test]
fn file_kind_from_path() {
    assert_eq!(
        FileKind::from_path(Path::new("src/reader/pages.cpl")),
        Some(FileKind::Cpl)
    );
    assert_eq!(
        FileKind::from_path(Path::new("work/pagination.work")),
        Some(FileKind::Work)
    );
    assert_eq!(
        FileKind::from_path(Path::new("work/pagination.patch")),
        Some(FileKind::Patch)
    );
    assert_eq!(FileKind::from_path(Path::new("cpl.toml")), None);
    assert_eq!(FileKind::from_path(Path::new("README")), None);
    assert_eq!(FileKind::Cpl.extension(), "cpl");
}

#[test]
fn test_source_literals_are_harvested() {
    let inputs = test_source_inputs();
    assert!(
        inputs.len() >= 40,
        "expected many inputs from the other test files, found {}",
        inputs.len()
    );
    assert!(inputs.iter().any(|i| i.kind == FileKind::Work));
    assert!(inputs.iter().any(|i| i.kind == FileKind::Patch));
    // The scanner decodes escapes: the trivia test's first input is present verbatim.
    assert!(inputs
        .iter()
        .any(|i| i.text.starts_with("// license\nmodule m // header\n")));
}

#[test]
fn a_broken_header_group_moves_the_brace_except_inside_a_comma_group() {
    // Rule 7, "headers whose group broke": an `if`, `else if`, `match`, or
    // `while` header whose comma group prints broken puts its `{` on a line
    // of its own. Inside a comma group the `{` stays on the header's line: a
    // line break before it would lie at the group's own level, the next
    // parse would record the group as broken (2.2 `Group::multiline`), and
    // formatting would not be idempotent (Section 12).
    let src = "module m\n\nfn f() -> Int {\n    let x = g(if f(\n        a) {\n        1\n    } else {\n        2\n    })\n    let y = if f(\n        a) {\n        1\n    } else {\n        2\n    }\n    x\n}\n";
    let expected = "module m\n\nfn f() -> Int {\n    let x = g(if f(\n        a,\n    ) {\n        1\n    } else {\n        2\n    })\n    let y = if f(\n        a,\n    )\n    {\n        1\n    } else {\n        2\n    }\n    x\n}\n";
    assert_formats_to(src, FileKind::Cpl, expected);
    assert_formats_to(expected, FileKind::Cpl, expected);
}

#[test]
fn a_comment_belongs_to_the_nodes_of_its_own_bracket_sequence() {
    // `spec/toolchain.md` 2.3 rules D and G: a comment written before a
    // nested bracket pair is not claimed by a node inside the pair, so the
    // pair's layout is left alone and the comment trails the innermost
    // enclosing attachable node.
    assert_formats_to(
        "module m\n\nfn f() -> Int {\n    let p = (// c\n        [b])\n    let q = [Page // d\n        { text: t }]\n    p\n}\n",
        FileKind::Cpl,
        "module m\n\nfn f() -> Int {\n    let p = ([b]) // c\n    let q = [\n        Page { text: t }, // d\n    ]\n    p\n}\n",
    );
}
