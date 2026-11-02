//! The diagnostic model of `spec/toolchain.md` 1.4 and semantics S49:
//! [`Severity`], [`DiagCode`], [`Category`], [`Diagnostic`] with its
//! builder, the human rendering of 1.4.1, and the [`Diagnostics`] list with
//! the stable order of T46.

use crate::facet::Facet;
use crate::id::StableId;
use crate::json::{BacktraceJson, DiagnosticJson, NoteJson};
use crate::source::{SourceFile, SourceMap, Span};
use serde::{Deserialize, Serialize, Serializer};
use std::cmp::Ordering;
use std::fmt::{self, Write as _};
use std::ops::Deref;

/// Diagnostic severity (semantics 9.1). The derived order (`Error` <
/// `Warning` < `Info`) is the severity key of [`Diagnostics::sorted`].
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    /// `"error"`, `"warning"`, or `"info"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A snake_case code: spec/semantics.md 9.4 for the language categories,
/// spec/toolchain.md Appendix B for the toolchain categories. Codes are
/// stable strings: tests and agents match on them.
///
/// A `DiagCode` dereferences to `str`, compares equal to `str`, `&str`, and
/// `String`, displays as the bare code, and serializes as a plain string.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct DiagCode(pub &'static str);

impl DiagCode {
    /// The code string.
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl Deref for DiagCode {
    type Target = str;
    fn deref(&self) -> &str {
        self.0
    }
}

impl AsRef<str> for DiagCode {
    fn as_ref(&self) -> &str {
        self.0
    }
}

impl fmt::Display for DiagCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl PartialEq<str> for DiagCode {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for DiagCode {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl PartialEq<String> for DiagCode {
    fn eq(&self, other: &String) -> bool {
        self.0 == other.as_str()
    }
}

impl PartialEq<DiagCode> for str {
    fn eq(&self, other: &DiagCode) -> bool {
        self == other.0
    }
}

impl PartialEq<DiagCode> for &str {
    fn eq(&self, other: &DiagCode) -> bool {
        *self == other.0
    }
}

impl PartialEq<DiagCode> for String {
    fn eq(&self, other: &DiagCode) -> bool {
        self.as_str() == other.0
    }
}

impl Serialize for DiagCode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

/// The semantics S49 categories plus the toolchain-owned ones (Appendix B).
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Parse,
    Resolve,
    Type,
    Effect,
    Contract,
    Hole,
    Test,
    ReleasePolicy,
    Runtime,
    Internal,
    Project,
    Index,
    Git,
    Work,
    Patch,
    Usage,
    /// Milestone 2 (T76): `cpl build` and the native toolchain; the codes
    /// are `spec/toolchain.md` Appendix B's.
    Build,
    /// Milestone 2 (T76): the C API's error objects and the binding errors
    /// a command reports; the codes are `spec/cabi.md` Section 4's.
    Interop,
}

impl Category {
    /// Every category, in declaration order.
    pub const ALL: [Category; 18] = [
        Category::Parse,
        Category::Resolve,
        Category::Type,
        Category::Effect,
        Category::Contract,
        Category::Hole,
        Category::Test,
        Category::ReleasePolicy,
        Category::Runtime,
        Category::Internal,
        Category::Project,
        Category::Index,
        Category::Git,
        Category::Work,
        Category::Patch,
        Category::Usage,
        Category::Build,
        Category::Interop,
    ];

    /// The JSON spelling: `"parse"`, `"release_policy"`, ...
    pub fn as_str(self) -> &'static str {
        match self {
            Category::Parse => "parse",
            Category::Resolve => "resolve",
            Category::Type => "type",
            Category::Effect => "effect",
            Category::Contract => "contract",
            Category::Hole => "hole",
            Category::Test => "test",
            Category::ReleasePolicy => "release_policy",
            Category::Runtime => "runtime",
            Category::Internal => "internal",
            Category::Project => "project",
            Category::Index => "index",
            Category::Git => "git",
            Category::Work => "work",
            Category::Patch => "patch",
            Category::Usage => "usage",
            Category::Build => "build",
            Category::Interop => "interop",
        }
    }

    /// The category with this JSON spelling.
    pub fn parse(s: &str) -> Option<Category> {
        Category::ALL.into_iter().find(|c| c.as_str() == s)
    }
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The fixed keys of the diagnostic object (T5, semantics S49). A payload
/// key in [`Diagnostic::data`] may not use them: [`Diagnostic::with_data`]
/// asserts it in debug builds and [`Diagnostic::to_json`] drops such keys.
pub const RESERVED_KEYS: [&str; 10] = [
    "code",
    "category",
    "severity",
    "message",
    "span",
    "symbol",
    "qualified_name",
    "facet",
    "notes",
    "backtrace",
];

/// One structured diagnostic (semantics S49). [`Diagnostic::to_json`] is
/// its JSON form and [`Diagnostic::render`] its human form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: DiagCode,
    pub category: Category,
    pub severity: Severity,
    /// One sentence; never the only carrier of a fact a field carries.
    pub message: String,
    /// Primary location.
    pub span: Option<Span>,
    /// Blamed declaration, when known.
    pub symbol: Option<StableId>,
    /// Always set together with `symbol`.
    pub qualified_name: Option<String>,
    /// Blamed facet, when known.
    pub facet: Option<Facet>,
    /// Related locations and explanations.
    pub notes: Vec<Note>,
    /// Faults only; innermost first.
    pub backtrace: Vec<BacktraceFrame>,
    /// Code-specific payload (`clause`, `input`, `actual`, `expected`,
    /// `status`, ...); flattened into the JSON object. Keys never use
    /// [`RESERVED_KEYS`].
    pub data: serde_json::Map<String, serde_json::Value>,
}

/// A related message, optionally pointing at another location.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub message: String,
    pub span: Option<Span>,
}

impl Note {
    /// A note without a location.
    pub fn new(message: impl Into<String>) -> Note {
        Note {
            message: message.into(),
            span: None,
        }
    }

    /// A note pointing at `span`.
    pub fn at(message: impl Into<String>, span: Span) -> Note {
        Note {
            message: message.into(),
            span: Some(span),
        }
    }
}

/// One frame of a fault's backtrace (innermost first).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BacktraceFrame {
    pub qualified_name: String,
    pub span: Option<Span>,
}

impl Diagnostic {
    pub fn new(
        code: DiagCode,
        category: Category,
        severity: Severity,
        message: impl Into<String>,
    ) -> Self {
        Diagnostic {
            code,
            category,
            severity,
            message: message.into(),
            span: None,
            symbol: None,
            qualified_name: None,
            facet: None,
            notes: Vec::new(),
            backtrace: Vec::new(),
            data: serde_json::Map::new(),
        }
    }

    pub fn error(code: DiagCode, category: Category, message: impl Into<String>) -> Self {
        Diagnostic::new(code, category, Severity::Error, message)
    }

    pub fn warning(code: DiagCode, category: Category, message: impl Into<String>) -> Self {
        Diagnostic::new(code, category, Severity::Warning, message)
    }

    pub fn info(code: DiagCode, category: Category, message: impl Into<String>) -> Self {
        Diagnostic::new(code, category, Severity::Info, message)
    }

    /// Set the primary location.
    pub fn with_span(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }

    /// Append a note.
    pub fn with_note(mut self, message: impl Into<String>, span: Option<Span>) -> Self {
        self.notes.push(Note {
            message: message.into(),
            span,
        });
        self
    }

    /// Blame a declaration: sets `symbol` and `qualified_name` together (S2)
    /// and, when `facet` is `Some`, the facet.
    pub fn with_symbol(
        mut self,
        id: StableId,
        qualified_name: impl Into<String>,
        facet: Option<Facet>,
    ) -> Self {
        self.symbol = Some(id);
        self.qualified_name = Some(qualified_name.into());
        if facet.is_some() {
            self.facet = facet;
        }
        self
    }

    /// Set the blamed facet without a symbol.
    pub fn with_facet(mut self, facet: Facet) -> Self {
        self.facet = Some(facet);
        self
    }

    /// Replace the backtrace (innermost frame first).
    pub fn with_backtrace(mut self, frames: Vec<BacktraceFrame>) -> Self {
        self.backtrace = frames;
        self
    }

    /// Add a payload field. `key` must not be one of [`RESERVED_KEYS`].
    pub fn with_data(mut self, key: &str, value: serde_json::Value) -> Self {
        debug_assert!(
            !RESERVED_KEYS.contains(&key),
            "`{key}` is a reserved diagnostic key (T5)"
        );
        self.data.insert(key.to_string(), value);
        self
    }

    /// True for severity `error`.
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// The S49 object with every span resolved through `sm`. A span whose
    /// file `sm` does not know is omitted; payload keys that collide with
    /// [`RESERVED_KEYS`] are dropped.
    pub fn to_json(&self, sm: &SourceMap) -> DiagnosticJson {
        let location =
            |span: Option<Span>| span.and_then(|s| sm.get(s.file).map(|f| f.location(s)));
        DiagnosticJson {
            code: self.code.0.to_string(),
            category: self.category,
            severity: self.severity,
            message: self.message.clone(),
            span: location(self.span),
            symbol: self.symbol.clone(),
            qualified_name: self.qualified_name.clone(),
            facet: self.facet,
            notes: self
                .notes
                .iter()
                .map(|n| NoteJson {
                    message: n.message.clone(),
                    span: location(n.span),
                })
                .collect(),
            backtrace: self
                .backtrace
                .iter()
                .map(|f| BacktraceJson {
                    qualified_name: f.qualified_name.clone(),
                    span: location(f.span),
                })
                .collect(),
            data: self
                .data
                .iter()
                .filter(|(k, _)| !RESERVED_KEYS.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        }
    }

    /// The human form of Section 1.4.1, ending in exactly one newline:
    ///
    /// ```text
    /// error[type_mismatch]: expected `Text`, found `Int`
    ///   --> src/reader/pages.cpl:12:5
    ///    |
    /// 12 |     page.len
    ///    |     ^^^^^^^^
    ///    |
    ///    = note: the tail expression is the function's return value
    /// ```
    ///
    /// Lines 1 (`<severity>[<code>]: <message>`) and 2 (`  --> <file>:<line>:<col>`,
    /// omitted without a span) are the stable contract. Notes without a
    /// span render as `= note:` lines, backtrace frames as
    /// `= at <qualified_name> (<file>:<line>)` lines, and notes with a span
    /// as further `note:` blocks with their own `-->` line. The caret run
    /// covers the span's scalar values on its first line only (at least
    /// one caret); tabs display as four columns; other text is unchanged.
    pub fn render(&self, sm: &SourceMap) -> String {
        let resolve = |span: Option<Span>| span.and_then(|s| sm.get(s.file).map(|f| (f, s)));
        let primary = resolve(self.span);
        let located: Vec<(&Note, &SourceFile, Span)> = self
            .notes
            .iter()
            .filter_map(|n| resolve(n.span).map(|(f, s)| (n, f, s)))
            .collect();
        let unlocated: Vec<&Note> = self
            .notes
            .iter()
            .filter(|n| resolve(n.span).is_none())
            .collect();

        let mut max_line = 1;
        if let Some((f, s)) = primary {
            max_line = max_line.max(f.line_col(s.start).line);
        }
        for (_, f, s) in &located {
            max_line = max_line.max(f.line_col(s.start).line);
        }
        let width = max_line.to_string().len();
        let gutter = " ".repeat(width);

        let mut out = String::new();
        let _ = writeln!(out, "{}[{}]: {}", self.severity, self.code, self.message);
        if let Some((file, span)) = primary {
            render_snippet(&mut out, file, span, width);
        }
        if !unlocated.is_empty() || !self.backtrace.is_empty() {
            if primary.is_some() {
                let _ = writeln!(out, "{gutter} |");
            }
            for note in &unlocated {
                let _ = writeln!(out, "{gutter} = note: {}", note.message);
            }
            for frame in &self.backtrace {
                match resolve(frame.span) {
                    Some((f, s)) => {
                        let _ = writeln!(
                            out,
                            "{gutter} = at {} ({}:{})",
                            frame.qualified_name,
                            f.path,
                            f.line_col(s.start).line
                        );
                    }
                    None => {
                        let _ = writeln!(out, "{gutter} = at {}", frame.qualified_name);
                    }
                }
            }
        }
        for (note, file, span) in located {
            let _ = writeln!(out, "note: {}", note.message);
            render_snippet(&mut out, file, span, width);
        }
        out
    }
}

/// The `-->` line and the gutter block of one span.
fn render_snippet(out: &mut String, file: &SourceFile, span: Span, width: usize) {
    let gutter = " ".repeat(width);
    let start = file.line_col(span.start);
    let end = file.line_col(span.end.max(span.start));
    let _ = writeln!(out, "  --> {}:{}:{}", file.path, start.line, start.col);
    let line = file.line_text(start.line);
    let chars: Vec<char> = line.chars().collect();
    let from = ((start.col - 1) as usize).min(chars.len());
    let to = if end.line == start.line {
        ((end.col - 1) as usize).clamp(from, chars.len())
    } else {
        chars.len()
    };
    let columns = |c: &char| if *c == '\t' { 4 } else { 1 };
    let shown: String = chars
        .iter()
        .map(|&c| {
            if c == '\t' {
                "    ".to_string()
            } else {
                c.to_string()
            }
        })
        .collect();
    let pad: usize = chars[..from].iter().map(columns).sum();
    let carets = chars[from..to].iter().map(columns).sum::<usize>().max(1);
    let _ = writeln!(out, "{gutter} |");
    if shown.is_empty() {
        let _ = writeln!(out, "{:>width$} |", start.line);
    } else {
        let _ = writeln!(out, "{:>width$} | {shown}", start.line);
    }
    let _ = writeln!(out, "{gutter} | {}{}", " ".repeat(pad), "^".repeat(carets));
}

impl fmt::Display for Diagnostic {
    /// Line 1 of the human form: `<severity>[<code>]: <message>`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}[{}]: {}", self.severity, self.code, self.message)
    }
}

/// The order of T46: diagnostics with a span by (file path, span start,
/// severity, code); then those without a span (or whose file `sm` does not
/// know) in their existing order.
fn compare(a: &Diagnostic, b: &Diagnostic, sm: &SourceMap) -> Ordering {
    let key = |d: &Diagnostic| {
        d.span
            .and_then(|s| sm.get(s.file).map(|f| (f.path.as_str(), s.start)))
    };
    match (key(a), key(b)) {
        (Some((pa, sa)), Some((pb, sb))) => pa
            .cmp(pb)
            .then(sa.cmp(&sb))
            .then(a.severity.cmp(&b.severity))
            .then(a.code.0.cmp(b.code.0)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// Sort in place into the [`Diagnostics::sorted`] order (a stable sort).
pub fn sort_diagnostics(diagnostics: &mut [Diagnostic], sm: &SourceMap) {
    diagnostics.sort_by(|a, b| compare(a, b, sm));
}

/// True when any diagnostic has severity `error`.
pub fn has_errors(diagnostics: &[Diagnostic]) -> bool {
    diagnostics.iter().any(Diagnostic::is_error)
}

/// [`Diagnostic::render`] of every diagnostic, concatenated in order.
pub fn render_all(diagnostics: &[Diagnostic], sm: &SourceMap) -> String {
    diagnostics.iter().map(|d| d.render(sm)).collect()
}

/// An ordered collection of diagnostics.
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct Diagnostics(Vec<Diagnostic>);

impl Diagnostics {
    pub fn new() -> Self {
        Diagnostics(Vec::new())
    }

    pub fn push(&mut self, d: Diagnostic) {
        self.0.push(d);
    }

    pub fn extend(&mut self, other: impl IntoIterator<Item = Diagnostic>) {
        self.0.extend(other);
    }

    pub fn has_errors(&self) -> bool {
        has_errors(&self.0)
    }

    pub fn error_count(&self) -> usize {
        self.0
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count()
    }

    pub fn warning_count(&self) -> usize {
        self.0
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .count()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Stable order: (file path, span.start, severity, code); diagnostics
    /// without a span last, in insertion order (T46).
    pub fn sorted(self, sm: &SourceMap) -> Vec<Diagnostic> {
        let mut v = self.0;
        sort_diagnostics(&mut v, sm);
        v
    }

    pub fn iter(&self) -> impl Iterator<Item = &Diagnostic> {
        self.0.iter()
    }

    pub fn as_slice(&self) -> &[Diagnostic] {
        &self.0
    }

    pub fn into_vec(self) -> Vec<Diagnostic> {
        self.0
    }
}

impl From<Vec<Diagnostic>> for Diagnostics {
    fn from(v: Vec<Diagnostic>) -> Self {
        Diagnostics(v)
    }
}

impl From<Diagnostics> for Vec<Diagnostic> {
    fn from(d: Diagnostics) -> Self {
        d.0
    }
}

impl FromIterator<Diagnostic> for Diagnostics {
    fn from_iter<I: IntoIterator<Item = Diagnostic>>(iter: I) -> Self {
        Diagnostics(iter.into_iter().collect())
    }
}

impl Extend<Diagnostic> for Diagnostics {
    fn extend<I: IntoIterator<Item = Diagnostic>>(&mut self, iter: I) {
        self.0.extend(iter);
    }
}

impl IntoIterator for Diagnostics {
    type Item = Diagnostic;
    type IntoIter = std::vec::IntoIter<Diagnostic>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a Diagnostics {
    type Item = &'a Diagnostic;
    type IntoIter = std::slice::Iter<'a, Diagnostic>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}
