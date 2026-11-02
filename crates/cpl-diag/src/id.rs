//! Stable identities and symbol paths (`spec/toolchain.md` 4.1, 4.3, 4.4;
//! semantics S2).
//!
//! A declaration's identity is its `@id("...")` value or, without one, its
//! *provisional identity*: its qualified name exactly as
//! [`SymbolPath::display`] renders it. An accepted `@id` value never
//! contains `.` and a qualified name always does, so the two forms never
//! collide ([`StableId::is_provisional`]).

use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize};
use std::borrow::Borrow;
use std::fmt;
use std::str::FromStr;

/// An `@id("...")` value or a provisional identity (Section 4). Validated
/// on construction (including deserialization); a provisional identity is
/// stored in its canonical [`SymbolPath::display`] spelling.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize)]
#[serde(transparent)]
pub struct StableId(String);

/// Why a string is not a [`StableId`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdError {
    /// An `@id` value with fewer than 4 or more than 64 characters (the
    /// payload is the length found).
    Length(usize),
    /// An `@id` value that does not match `^[a-z][a-z0-9]*(_[a-z0-9]+)+$`.
    Syntax(String),
    /// A provisional identity (it contains `.`) that is not a valid symbol
    /// path; the payload is the [`SymbolPath::parse`] message.
    Path(String),
}

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IdError::Length(n) => write!(f, "an @id value has 4 to 64 characters, found {n}"),
            IdError::Syntax(s) => write!(
                f,
                "`{s}` is not a valid @id value: expected lowercase letters and digits in `_`-separated segments, starting with a letter (like `fn_k7w3qz2mb6ptx4ve`)"
            ),
            IdError::Path(m) => write!(f, "invalid provisional identity: {m}"),
        }
    }
}

impl std::error::Error for IdError {}

/// The accepted syntax of an `@id` argument (Section 4.1):
/// `^[a-z][a-z0-9]*(_[a-z0-9]+)+$`, 4 to 64 characters.
fn check_annotation_value(s: &str) -> Result<(), IdError> {
    let len = s.chars().count();
    if !(4..=64).contains(&len) {
        return Err(IdError::Length(len));
    }
    let segment_ok = |p: &str| {
        !p.is_empty()
            && p.bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    };
    let mut parts = s.split('_');
    let first = parts.next().unwrap_or("");
    let first_ok = first
        .as_bytes()
        .first()
        .is_some_and(|c| c.is_ascii_lowercase())
        && segment_ok(first);
    let mut rest = 0;
    let mut rest_ok = true;
    for p in parts {
        rest += 1;
        rest_ok &= segment_ok(p);
    }
    if first_ok && rest_ok && rest >= 1 {
        Ok(())
    } else {
        Err(IdError::Syntax(s.to_string()))
    }
}

const KIND_PREFIXES: [&str; 5] = ["fn", "ty", "cap", "test", "prop"];

impl StableId {
    /// Either form: a string containing `.` must be a symbol path (Section
    /// 4.3, 4.4) and is stored canonically; any other string must have the
    /// accepted `@id` syntax of Section 4.1.
    pub fn parse(s: &str) -> Result<StableId, IdError> {
        if s.contains('.') {
            let path = SymbolPath::parse(s).map_err(IdError::Path)?;
            Ok(StableId(path.display()))
        } else {
            StableId::from_annotation(s)
        }
    }

    /// Only the `@id` form: the accepted syntax of Section 4.1 (what an
    /// `@id("...")` argument must match; anything else is
    /// `invalid_annotation_argument`).
    pub fn from_annotation(s: &str) -> Result<StableId, IdError> {
        check_annotation_value(s)?;
        Ok(StableId(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// A qualified name: contains `.`, which no accepted `@id` value does.
    pub fn is_provisional(&self) -> bool {
        self.0.contains('.')
    }

    /// `"fn"`, `"ty"`, `"cap"`, `"test"`, or `"prop"` when the `@id` value
    /// starts with one of the five kinds followed by `_`; `""` for
    /// nonstandard ids (`pgn_7c2d`) and provisional ids. The prefix is
    /// informative only (T21).
    pub fn kind_prefix(&self) -> &str {
        if self.is_provisional() {
            return "";
        }
        match self.0.split_once('_') {
            Some((prefix, _)) if KIND_PREFIXES.contains(&prefix) => &self.0[..prefix.len()],
            _ => "",
        }
    }

    /// The qualified name `module.path.name` (semantics S2), Section 4.3:
    /// the single producer of provisional identities. `module_path` is the
    /// dotted module path (`"reader.pages"`).
    pub fn provisional(module_path: &str, name: &DeclName) -> StableId {
        debug_assert!(
            !module_path.is_empty(),
            "a provisional identity needs a module path"
        );
        StableId(format!("{module_path}.{}", name.display()))
    }
}

impl fmt::Display for StableId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for StableId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for StableId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl FromStr for StableId {
    type Err = IdError;
    fn from_str(s: &str) -> Result<StableId, IdError> {
        StableId::parse(s)
    }
}

impl PartialEq<str> for StableId {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for StableId {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl<'de> Deserialize<'de> for StableId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = StableId;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an @id value or a qualified name")
            }
            fn visit_str<E: de::Error>(self, s: &str) -> Result<StableId, E> {
                StableId::parse(s).map_err(E::custom)
            }
        }
        deserializer.deserialize_str(V)
    }
}

/// The name part of a [`SymbolPath`] (Section 4.4).
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub enum DeclName {
    /// fn, type, struct, enum, capability.
    Ident(String),
    /// A test or property: the two share one namespace (semantics S3), so a
    /// path cannot tell them apart.
    Text(String),
    /// A std `@method`/`@property` stub: `std.prelude.List.take` (stdlib
    /// S37). The same form spells a capability operation's qualified name
    /// (`reader.pages.PageRead.get`, semantics 1.4).
    Member { receiver: String, name: String },
}

impl DeclName {
    /// The final segment(s) as they appear in a path: `paginate`,
    /// `"pagination handles empty input"` (canonically escaped), `List.take`.
    pub fn display(&self) -> String {
        match self {
            DeclName::Ident(s) => s.clone(),
            DeclName::Text(s) => quote_text(s),
            DeclName::Member { receiver, name } => format!("{receiver}.{name}"),
        }
    }
}

impl fmt::Display for DeclName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display())
    }
}

/// The fully qualified name of a declaration (Section 4.4): its module path
/// and its name.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct SymbolPath {
    pub module: Vec<String>,
    pub name: DeclName,
}

/// One segment while parsing a path.
enum Segment {
    Ident(String),
    Text(String),
}

fn is_upper(s: &str) -> bool {
    s.starts_with(|c: char| c.is_ascii_uppercase())
}

impl SymbolPath {
    pub fn new(module: Vec<String>, name: DeclName) -> SymbolPath {
        SymbolPath { module, name }
    }

    /// `reader.pages.paginate`, `tests.pagination."pagination handles empty input"`,
    /// `std.prelude.List.take`. A quoted final segment is `Text` (decoded
    /// with the grammar's escapes); an `UPPER_IDENT` segment before a final
    /// `LOWER_IDENT` is a `Member` receiver; every other segment before the
    /// name is a module segment and must be a `LOWER_IDENT`. At least one
    /// module segment is required.
    pub fn parse(s: &str) -> Result<SymbolPath, String> {
        let bytes = s.as_bytes();
        let mut segments = Vec::new();
        let mut i = 0;
        loop {
            if i >= bytes.len() {
                return Err(if s.is_empty() {
                    "empty symbol path".to_string()
                } else {
                    format!("`{s}`: expected a segment after `.`")
                });
            }
            if bytes[i] == b'"' {
                let (text, next) = parse_quoted(s, i)?;
                if next != bytes.len() {
                    return Err(format!("`{s}`: a quoted name must be the last segment"));
                }
                segments.push(Segment::Text(text));
                break;
            }
            let start = i;
            if !(bytes[i].is_ascii_alphabetic() || bytes[i] == b'_') {
                return Err(format!("`{s}`: unexpected character at byte {i}"));
            }
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let ident = &s[start..i];
            if ident == "_" {
                return Err(format!("`{s}`: `_` is not a name"));
            }
            segments.push(Segment::Ident(ident.to_string()));
            if i == bytes.len() {
                break;
            }
            if bytes[i] != b'.' {
                return Err(format!("`{s}`: unexpected character at byte {i}"));
            }
            i += 1;
        }
        let last = segments.pop().expect("at least one segment");
        let name = match last {
            Segment::Text(t) => DeclName::Text(t),
            Segment::Ident(n) => {
                let receiver = match segments.last() {
                    Some(Segment::Ident(r)) if !is_upper(&n) && is_upper(r) => Some(r.clone()),
                    _ => None,
                };
                match receiver {
                    Some(receiver) => {
                        segments.pop();
                        DeclName::Member { receiver, name: n }
                    }
                    None => DeclName::Ident(n),
                }
            }
        };
        let mut module = Vec::with_capacity(segments.len());
        for seg in segments {
            match seg {
                Segment::Ident(m) if !is_upper(&m) => module.push(m),
                Segment::Ident(m) => {
                    return Err(format!("`{s}`: `{m}` is not a module path segment"));
                }
                Segment::Text(_) => unreachable!("a quoted segment is always last"),
            }
        }
        if module.is_empty() {
            return Err(format!("`{s}`: a symbol path starts with a module path"));
        }
        Ok(SymbolPath { module, name })
    }

    /// The same syntax [`SymbolPath::parse`] reads; text segments are
    /// canonically escaped ([`quote_text`]). This is the qualified name and
    /// the provisional identity (Section 4.3).
    pub fn display(&self) -> String {
        format!("{}.{}", self.module.join("."), self.name.display())
    }

    /// The dotted module path: `"reader.pages"`.
    pub fn module_path(&self) -> String {
        self.module.join(".")
    }
}

impl fmt::Display for SymbolPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display())
    }
}

impl FromStr for SymbolPath {
    type Err = String;
    fn from_str(s: &str) -> Result<SymbolPath, String> {
        SymbolPath::parse(s)
    }
}

/// A `TEXT` literal in canonical form (formatter rule 10, `spec/toolchain.md`
/// 2.4): `\\`, `\"`, `\n`, `\r`, `\t`, other control characters below
/// U+0020 and DEL as `\u{h}` with minimal lowercase hex digits, everything
/// else literally; wrapped in double quotes.
pub fn quote_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\x7f' => {
                out.push_str(&format!("\\u{{{:x}}}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Decode the quoted segment starting at byte `open` (a `"`); returns the
/// decoded text and the byte index after the closing quote. Escapes are the
/// grammar's (1.5); raw control characters other than tab are rejected as
/// in a `TEXT` token (D62).
fn parse_quoted(s: &str, open: usize) -> Result<(String, usize), String> {
    let mut out = String::new();
    let mut chars = s[open + 1..].char_indices();
    while let Some((offset, c)) = chars.next() {
        match c {
            '"' => return Ok((out, open + 1 + offset + 1)),
            '\\' => match chars.next() {
                Some((_, '\\')) => out.push('\\'),
                Some((_, '"')) => out.push('"'),
                Some((_, 'n')) => out.push('\n'),
                Some((_, 'r')) => out.push('\r'),
                Some((_, 't')) => out.push('\t'),
                Some((_, 'u')) => {
                    if chars.next().map(|(_, c)| c) != Some('{') {
                        return Err(format!("`{s}`: a unicode escape is written \\u{{H}}"));
                    }
                    let mut hex = String::new();
                    loop {
                        match chars.next() {
                            Some((_, '}')) => break,
                            Some((_, h)) if h.is_ascii_hexdigit() && hex.len() < 6 => hex.push(h),
                            _ => return Err(format!("`{s}`: invalid unicode escape")),
                        }
                    }
                    let decoded = u32::from_str_radix(&hex, 16)
                        .ok()
                        .and_then(char::from_u32)
                        .ok_or_else(|| format!("`{s}`: invalid unicode escape"))?;
                    out.push(decoded);
                }
                Some((_, other)) => return Err(format!("`{s}`: invalid escape `\\{other}`")),
                None => break,
            },
            '\n' | '\r' => return Err(format!("`{s}`: line break in a quoted name")),
            c if ((c as u32) < 0x20 && c != '\t') || c == '\x7f' => {
                return Err(format!("`{s}`: control character in a quoted name"));
            }
            c => out.push(c),
        }
    }
    Err(format!("`{s}`: unterminated quoted name"))
}
