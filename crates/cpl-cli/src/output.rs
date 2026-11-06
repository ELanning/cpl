//! Human and JSON emission (`spec/toolchain.md` 1.4.1, 1.4.2, 7.1, 1.10).
//!
//! Every command builds a [`Report`], the parts of an [`Envelope`] plus the
//! human rendering of its result, and [`Output::emit`] prints it in the
//! chosen [`Format`]:
//!
//! | Format | stdout | stderr |
//! | - | - | - |
//! | human (default) | the result text the command rendered from its result | every diagnostic, rendered as Section 1.4.1 lays out |
//! | `--json` | exactly one envelope, pretty-printed with two-space indentation, keys in declaration order | nothing (crash traces only) |
//! | `--jsonl` (`check`, `test`) | one diagnostic object per line in emission order, then one line holding the envelope without `diagnostics` | nothing |
//!
//! The envelope's diagnostics are in `Diagnostics::sorted` order (T46):
//! by file path, span start, severity, and code, spanless ones last in
//! emission order. A command may hold diagnostics only as JSON (the test
//! runner's records and counterexamples are wire types), so every list is
//! converted to [`DiagnosticJson`] first and sorted and rendered from that
//! object, which also makes the text form carry exactly what the JSON does
//! (semantics 9.1). [`render`] reproduces [`cpl_diag::Diagnostic::render`]
//! byte for byte for every diagnostic that came from one (tested below).
//!
//! Writes ignore a closed pipe (`cpl check | head -1` must not panic).

use crate::exit::Exit;
use cpl_diag::json::{DiagnosticJson, Envelope, NoOutput, Revision, Status, ToolchainInfo};
use cpl_diag::{Diagnostic, Location, SourceMap};
use serde::Serialize;
use std::cmp::Ordering;
use std::fmt::Write as _;
use std::io::Write as _;
use std::sync::Arc;

/// How a command prints (Section 7.1).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Format {
    Human,
    Json,
    Jsonl,
}

/// What a command produced: the envelope's parts, the human rendering of
/// its result, and what the human rendering of the diagnostics needs.
#[derive(Debug)]
pub struct Report<T> {
    /// The envelope's `command`.
    pub command: String,
    pub revision: Option<Revision>,
    pub status: Status,
    /// Every diagnostic, in emission order; [`Output::emit`] sorts them.
    pub diagnostics: Vec<DiagnosticJson>,
    pub result: T,
    /// The human rendering of the result, printed on stdout without
    /// `--json` (empty for none).
    pub stdout: String,
    /// Codes whose diagnostics the human stdout already renders, which the
    /// human stderr does not repeat (`test_result` for `cpl test`).
    pub stdout_codes: &'static [&'static str],
    /// Resolves the diagnostics' spans to source lines for the human form.
    pub sources: Option<Arc<SourceMap>>,
    /// True when the command has already streamed every diagnostic as a
    /// JSON line (`--jsonl` while tests run).
    pub streamed: bool,
}

impl<T> Report<T> {
    /// A report without human text, extra codes, or streamed lines.
    pub fn new(
        command: impl Into<String>,
        revision: Option<Revision>,
        status: Status,
        diagnostics: Vec<DiagnosticJson>,
        result: T,
    ) -> Report<T> {
        Report {
            command: command.into(),
            revision,
            status,
            diagnostics,
            result,
            stdout: String::new(),
            stdout_codes: &[],
            sources: None,
            streamed: false,
        }
    }

    pub fn with_stdout(mut self, stdout: String) -> Self {
        self.stdout = stdout;
        self
    }

    pub fn with_sources(mut self, sources: Arc<SourceMap>) -> Self {
        self.sources = Some(sources);
        self
    }
}

/// Why a command stopped before it had a result of its own: the envelope
/// then carries no keys of its own ([`NoOutput`]).
#[derive(Debug)]
pub struct Failure {
    pub status: Status,
    pub revision: Option<Revision>,
    pub diagnostics: Vec<DiagnosticJson>,
    pub sources: Option<Arc<SourceMap>>,
    pub streamed: bool,
}

impl Failure {
    /// A failure with diagnostics whose spans `sources` resolves.
    pub fn new(
        status: Status,
        revision: Option<Revision>,
        diagnostics: &[Diagnostic],
        sources: Option<Arc<SourceMap>>,
    ) -> Failure {
        let empty = SourceMap::new();
        let sm = sources.as_deref().unwrap_or(&empty);
        Failure {
            status,
            revision,
            diagnostics: diagnostics.iter().map(|d| d.to_json(sm)).collect(),
            sources,
            streamed: false,
        }
    }

    /// One span-less diagnostic with exit code 2 (a usage error, a load
    /// failure, an internal error).
    pub fn error(diagnostic: Diagnostic) -> Failure {
        Failure::new(Status::Error, None, &[diagnostic], None)
    }

    /// This failure in a command that runs in the project at `revision`
    /// (the envelope's `revision`, Section 1.4.2).
    pub fn with_revision(mut self, revision: Option<Revision>) -> Failure {
        self.revision = revision;
        self
    }

    /// The report of this failure for `command`.
    pub fn into_report(self, command: impl Into<String>) -> Report<NoOutput> {
        Report {
            command: command.into(),
            revision: self.revision,
            status: self.status,
            diagnostics: self.diagnostics,
            result: NoOutput {},
            stdout: String::new(),
            stdout_codes: &[],
            sources: self.sources,
            streamed: self.streamed,
        }
    }
}

/// The JSONL tail: the envelope without `diagnostics`, keys in the
/// envelope's order (Section 7.1). Composed of `cpl_diag::json` types (T3).
#[derive(Serialize)]
struct EnvelopeTail<'a, T> {
    command: &'a str,
    toolchain: &'a ToolchainInfo,
    revision: &'a Option<Revision>,
    status: Status,
    #[serde(flatten)]
    result: &'a T,
}

/// The printer of one command.
#[derive(Copy, Clone, Debug)]
pub struct Output {
    pub format: Format,
}

impl Output {
    pub fn new(format: Format) -> Output {
        Output { format }
    }

    /// Under `--jsonl`, print one diagnostic as one line now (progress);
    /// otherwise nothing.
    pub fn stream(&self, diagnostic: &DiagnosticJson) {
        if self.format == Format::Jsonl {
            print_stdout(&format!("{}\n", compact(diagnostic)));
        }
    }

    /// Print a report and return its exit code.
    pub fn emit<T: Serialize>(&self, mut report: Report<T>) -> Exit {
        sort_json(&mut report.diagnostics);
        let exit = Exit::from(report.status);
        match self.format {
            Format::Json => {
                let envelope = Envelope {
                    command: report.command,
                    toolchain: ToolchainInfo::current(),
                    revision: report.revision,
                    status: report.status,
                    diagnostics: report.diagnostics,
                    result: report.result,
                };
                print_stdout(&format!("{}\n", pretty(&envelope)));
            }
            Format::Jsonl => {
                let mut out = String::new();
                if !report.streamed {
                    for d in &report.diagnostics {
                        out.push_str(&compact(d));
                        out.push('\n');
                    }
                }
                let toolchain = ToolchainInfo::current();
                let tail = EnvelopeTail {
                    command: &report.command,
                    toolchain: &toolchain,
                    revision: &report.revision,
                    status: report.status,
                    result: &report.result,
                };
                out.push_str(&compact(&tail));
                out.push('\n');
                print_stdout(&out);
            }
            Format::Human => {
                let empty = SourceMap::new();
                let sm = report.sources.as_deref().unwrap_or(&empty);
                let mut err = String::new();
                for d in &report.diagnostics {
                    if !report.stdout_codes.contains(&d.code.as_str()) {
                        err.push_str(&render(d, sm));
                    }
                }
                print_stderr(&err);
                print_stdout(&report.stdout);
            }
        }
        exit
    }
}

/// Pretty JSON with two-space indentation (Section 7.1), the indentation capped at
/// `MAX_INDENT_DEPTH` levels: indenting every line by its depth makes the text of a value nested
/// `d` deep grow with `d²` (a value nested a million levels deep, from an untrusted program,
/// would be terabytes); past the cap it grows with `d`. Shallower documents print exactly as
/// `serde_json::to_string_pretty` prints them.
pub fn pretty<T: Serialize + ?Sized>(value: &T) -> String {
    let mut out = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(&mut out, CappedPretty::default());
    value.serialize(&mut ser).expect("a wire type serializes");
    String::from_utf8(out).expect("JSON is UTF-8")
}

/// The depth beyond which `pretty` indents no further.
const MAX_INDENT_DEPTH: usize = 32;

/// `serde_json::ser::PrettyFormatter` with two spaces, its indentation capped.
#[derive(Default)]
struct CappedPretty {
    depth: usize,
    has_value: bool,
}

impl CappedPretty {
    fn indent<W: ?Sized + std::io::Write>(&self, w: &mut W) -> std::io::Result<()> {
        for _ in 0..self.depth.min(MAX_INDENT_DEPTH) {
            w.write_all(b"  ")?;
        }
        Ok(())
    }
}

impl serde_json::ser::Formatter for CappedPretty {
    fn begin_array<W: ?Sized + std::io::Write>(&mut self, w: &mut W) -> std::io::Result<()> {
        self.depth += 1;
        self.has_value = false;
        w.write_all(b"[")
    }
    fn end_array<W: ?Sized + std::io::Write>(&mut self, w: &mut W) -> std::io::Result<()> {
        self.depth -= 1;
        if self.has_value {
            w.write_all(b"\n")?;
            self.indent(w)?;
        }
        w.write_all(b"]")
    }
    fn begin_array_value<W: ?Sized + std::io::Write>(
        &mut self,
        w: &mut W,
        first: bool,
    ) -> std::io::Result<()> {
        w.write_all(if first { b"\n" } else { b",\n" })?;
        self.indent(w)
    }
    fn end_array_value<W: ?Sized + std::io::Write>(&mut self, _w: &mut W) -> std::io::Result<()> {
        self.has_value = true;
        Ok(())
    }
    fn begin_object<W: ?Sized + std::io::Write>(&mut self, w: &mut W) -> std::io::Result<()> {
        self.depth += 1;
        self.has_value = false;
        w.write_all(b"{")
    }
    fn end_object<W: ?Sized + std::io::Write>(&mut self, w: &mut W) -> std::io::Result<()> {
        self.depth -= 1;
        if self.has_value {
            w.write_all(b"\n")?;
            self.indent(w)?;
        }
        w.write_all(b"}")
    }
    fn begin_object_key<W: ?Sized + std::io::Write>(
        &mut self,
        w: &mut W,
        first: bool,
    ) -> std::io::Result<()> {
        w.write_all(if first { b"\n" } else { b",\n" })?;
        self.indent(w)
    }
    fn begin_object_value<W: ?Sized + std::io::Write>(&mut self, w: &mut W) -> std::io::Result<()> {
        w.write_all(b": ")
    }
    fn end_object_value<W: ?Sized + std::io::Write>(&mut self, _w: &mut W) -> std::io::Result<()> {
        self.has_value = true;
        Ok(())
    }
}

/// One-line JSON (`--jsonl`).
pub fn compact<T: Serialize + ?Sized>(value: &T) -> String {
    serde_json::to_string(value).expect("a wire type serializes")
}

/// Write to stdout, ignoring a closed pipe.
pub fn print_stdout(text: &str) {
    if text.is_empty() {
        return;
    }
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(text.as_bytes());
    let _ = out.flush();
}

/// Write to stderr, ignoring a closed pipe.
pub fn print_stderr(text: &str) {
    if text.is_empty() {
        return;
    }
    let mut err = std::io::stderr().lock();
    let _ = err.write_all(text.as_bytes());
    let _ = err.flush();
}

// ---------------------------------------------------------------------------
// T46 order
// ---------------------------------------------------------------------------

/// Sort into the `Diagnostics::sorted` order of T46, in place and stably:
/// diagnostics with a span by (file path, span start, severity, code), then
/// the span-less ones in their existing (emission) order. Within one file a
/// (line, column) pair orders exactly as the byte offset it was computed
/// from, so this is the order of `cpl_diag::sort_diagnostics` on the
/// diagnostics the objects came from.
pub fn sort_json(diagnostics: &mut [DiagnosticJson]) {
    diagnostics.sort_by(compare_json);
}

fn compare_json(a: &DiagnosticJson, b: &DiagnosticJson) -> Ordering {
    match (&a.span, &b.span) {
        (Some(x), Some(y)) => x
            .file
            .cmp(&y.file)
            .then(x.line.cmp(&y.line))
            .then(x.col.cmp(&y.col))
            .then(a.severity.cmp(&b.severity))
            .then(a.code.cmp(&b.code)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

// ---------------------------------------------------------------------------
// 1.4.1 human rendering
// ---------------------------------------------------------------------------

/// The human form of Section 1.4.1 of a diagnostic object, ending in one
/// newline: line 1 `<severity>[<code>]: <message>`, line 2
/// `  --> <file>:<line>:<col>` (without a span, none), the gutter block of
/// the span's first line with its caret run, span-less notes as
/// `= note:` lines and backtrace frames as `= at` lines, then each located
/// note as a `note:` block with its own `-->` line. The source lines come
/// from `sm`, found by the location's file path; a location whose file `sm`
/// does not hold keeps its `-->` line and has no gutter block. For a
/// diagnostic `d`, `render(&d.to_json(sm), sm) == d.render(sm)`.
pub fn render(d: &DiagnosticJson, sm: &SourceMap) -> String {
    let located: Vec<(&str, &Location)> = d
        .notes
        .iter()
        .filter_map(|n| n.span.as_ref().map(|s| (n.message.as_str(), s)))
        .collect();
    let unlocated: Vec<&str> = d
        .notes
        .iter()
        .filter(|n| n.span.is_none())
        .map(|n| n.message.as_str())
        .collect();
    let mut max_line = 1;
    if let Some(span) = &d.span {
        max_line = max_line.max(span.line);
    }
    for (_, span) in &located {
        max_line = max_line.max(span.line);
    }
    let width = max_line.to_string().len();
    let gutter = " ".repeat(width);

    let mut out = String::new();
    let _ = writeln!(out, "{}[{}]: {}", d.severity.as_str(), d.code, d.message);
    if let Some(span) = &d.span {
        snippet(&mut out, sm, span, width);
    }
    if !unlocated.is_empty() || !d.backtrace.is_empty() {
        if d.span.is_some() {
            let _ = writeln!(out, "{gutter} |");
        }
        for message in &unlocated {
            let _ = writeln!(out, "{gutter} = note: {message}");
        }
        for frame in &d.backtrace {
            match &frame.span {
                Some(span) => {
                    let _ = writeln!(
                        out,
                        "{gutter} = at {} ({}:{})",
                        frame.qualified_name, span.file, span.line
                    );
                }
                None => {
                    let _ = writeln!(out, "{gutter} = at {}", frame.qualified_name);
                }
            }
        }
    }
    for (message, span) in located {
        let _ = writeln!(out, "note: {message}");
        snippet(&mut out, sm, span, width);
    }
    out
}

/// The `-->` line and the gutter block of one location (Section 1.4.1):
/// the location's first line with tabs shown as four columns and a caret
/// run under the located scalar values of that line (at least one).
fn snippet(out: &mut String, sm: &SourceMap, span: &Location, width: usize) {
    let _ = writeln!(out, "  --> {}:{}:{}", span.file, span.line, span.col);
    let Some(file) = sm.find(&span.file).map(|id| sm.file(id)) else {
        return;
    };
    let gutter = " ".repeat(width);
    let chars: Vec<char> = file.line_text(span.line).chars().collect();
    let from = (span.col.saturating_sub(1) as usize).min(chars.len());
    let to = if span.end_line == span.line {
        (span.end_col.saturating_sub(1) as usize).clamp(from, chars.len())
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
        let _ = writeln!(out, "{:>width$} |", span.line);
    } else {
        let _ = writeln!(out, "{:>width$} | {shown}", span.line);
    }
    let _ = writeln!(out, "{gutter} | {}{}", " ".repeat(pad), "^".repeat(carets));
}

/// The two stable lines of Section 1.4.1 (`<severity>[<code>]: <message>`
/// and, with a span, `  --> <file>:<line>:<col>`), each after `indent`:
/// how `cpl test` shows a failure's fault inline.
pub fn headline(d: &DiagnosticJson, indent: &str) -> String {
    let mut out = format!(
        "{indent}{}[{}]: {}\n",
        d.severity.as_str(),
        d.code,
        d.message
    );
    if let Some(span) = &d.span {
        let _ = writeln!(
            out,
            "{indent}  --> {}:{}:{}",
            span.file, span.line, span.col
        );
    }
    out
}

/// `n thing` or `n things`.
pub fn plural(n: u64, singular: &str, plural: &str) -> String {
    if n == 1 {
        format!("1 {singular}")
    } else {
        format!("{n} {plural}")
    }
}

/// The JSON spelling of a field-less wire enum (`contract_changed`,
/// `type_use`, `verifier_unavailable`): what `--json` writes for it, so
/// that the human form uses the same words.
pub fn spelling<T: Serialize + ?Sized>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(text)) => text,
        Ok(other) => other.to_string(),
        Err(_) => String::new(),
    }
}

/// A location in human output: `file:line:col-end_col` for a span on one
/// line (`src/reader/pages.cpl:75:8-37`), else
/// `file:line:col-end_line:end_col` (`src/reader/pages.cpl:43:1-67:2`).
pub fn location_text(location: &Location) -> String {
    if location.end_line == location.line {
        format!(
            "{}:{}:{}-{}",
            location.file, location.line, location.col, location.end_col
        )
    } else {
        format!(
            "{}:{}:{}-{}:{}",
            location.file, location.line, location.col, location.end_line, location.end_col
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpl_diag::{BacktraceFrame, Category, DiagCode, FileId, FileOrigin, Severity, Span};
    use serde_json::json;

    fn map() -> SourceMap {
        let mut sm = SourceMap::new();
        sm.add(
            "src/a.cpl".into(),
            FileOrigin::Project,
            "module a\n\nfn f() -> Int {\n\tlet x = 1\n    x + \"é\"\n}\n".into(),
        );
        sm.add(
            "src/b.cpl".into(),
            FileOrigin::Project,
            "module b\r\nfn g() -> Int {\r\n    7\r\n}\r\n".into(),
        );
        let long: String = (1..=120).map(|i| format!("// line {i}\n")).collect();
        sm.add("src/c.cpl".into(), FileOrigin::Project, long);
        sm
    }

    fn samples() -> Vec<Diagnostic> {
        let code = DiagCode("type_mismatch");
        let base = || Diagnostic::error(code, Category::Type, "expected `Text`, found `Int`");
        vec![
            base(),
            base().with_span(Span::new(FileId(0), 41, 52)),
            base().with_span(Span::new(FileId(0), 27, 30)),
            base().with_span(Span::point(FileId(0), 0)),
            base().with_span(Span::new(FileId(0), 20, 47)),
            base().with_span(Span::new(FileId(1), 26, 27)),
            base()
                .with_span(Span::new(FileId(0), 41, 49))
                .with_note("the tail expression is the function's return value", None)
                .with_note("earlier here", Some(Span::new(FileId(2), 1100, 1104)))
                .with_note("in another file", Some(Span::new(FileId(1), 10, 11))),
            Diagnostic::new(
                DiagCode("contract_violation"),
                Category::Contract,
                Severity::Warning,
                "ensures clause failed",
            )
            .with_span(Span::new(FileId(1), 26, 27))
            .with_backtrace(vec![
                BacktraceFrame {
                    qualified_name: "b.g".into(),
                    span: Some(Span::new(FileId(1), 26, 27)),
                },
                BacktraceFrame {
                    qualified_name: "std.prelude.flatten".into(),
                    span: None,
                },
            ])
            .with_data("clause", json!("result > 0")),
            Diagnostic::info(DiagCode("test_result"), Category::Test, "tested")
                .with_note("span-less", None),
            base().with_span(Span::new(FileId(7), 0, 1)),
            base().with_span(Span::new(FileId(2), 1100, 1104)),
        ]
    }

    #[test]
    fn rendering_the_object_is_rendering_the_diagnostic() {
        let sm = map();
        for d in samples() {
            assert_eq!(render(&d.to_json(&sm), &sm), d.render(&sm), "{d:?}");
        }
    }

    #[test]
    fn sorting_the_objects_is_sorting_the_diagnostics() {
        let sm = map();
        let mut diags = samples();
        diags.push(
            Diagnostic::warning(DiagCode("unused_value"), Category::Type, "w")
                .with_span(Span::new(FileId(0), 41, 52)),
        );
        diags.push(
            Diagnostic::error(DiagCode("arity_mismatch"), Category::Type, "e")
                .with_span(Span::new(FileId(0), 41, 52)),
        );
        let mut json: Vec<DiagnosticJson> = diags.iter().map(|d| d.to_json(&sm)).collect();
        sort_json(&mut json);
        cpl_diag::sort_diagnostics(&mut diags, &sm);
        let expected: Vec<DiagnosticJson> = diags.iter().map(|d| d.to_json(&sm)).collect();
        assert_eq!(json, expected);
    }

    #[test]
    fn headline_has_the_two_stable_lines() {
        let sm = map();
        let d = samples()[1].to_json(&sm);
        assert_eq!(
            headline(&d, "  "),
            "  error[type_mismatch]: expected `Text`, found `Int`\n    --> src/a.cpl:5:5\n"
        );
    }

    #[test]
    fn plurals() {
        assert_eq!(plural(0, "error", "errors"), "0 errors");
        assert_eq!(plural(1, "hole", "holes"), "1 hole");
        assert_eq!(plural(14, "definition", "definitions"), "14 definitions");
    }

    #[test]
    fn spellings_and_locations() {
        use cpl_diag::json::{DiffStatementKind, Relation};
        assert_eq!(
            spelling(&DiffStatementKind::ContractChanged),
            "contract_changed"
        );
        assert_eq!(spelling(&Relation::TypeUse), "type_use");
        let at = |line, col, end_line, end_col| Location {
            file: "src/a.cpl".into(),
            line,
            col,
            end_line,
            end_col,
        };
        assert_eq!(location_text(&at(75, 8, 75, 37)), "src/a.cpl:75:8-37");
        assert_eq!(location_text(&at(43, 1, 67, 2)), "src/a.cpl:43:1-67:2");
    }
}
