//! The source model of `spec/toolchain.md` 1.4 and 3.2: [`FileId`], [`Span`],
//! [`FileOrigin`], [`SourceFile`], and the [`SourceMap`] that turns spans into
//! lines, columns, and text.
//!
//! Line and column rules (semantics S49): lines are 1-based; columns are
//! 1-based and counted in Unicode scalar values from the line start; an
//! end column is exclusive. LF, CRLF (one terminator), and a lone CR each
//! end a line, so the location of the `invalid_line_terminator` diagnostic
//! is still meaningful. A leading byte-order mark is not part of line 1
//! (grammar 1.1 skips it): an offset inside it maps to line 1, column 1.

use crate::diag::{Category, DiagCode, Diagnostic};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

/// Identifies one file registered in a [`SourceMap`]. Ids are assigned in
/// registration order (std files first, then project files in path order,
/// then virtual files; Section 3.2, T15).
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FileId(pub u32);

/// Byte offsets into the file's text, half-open, `start <= end`.
///
/// `Ord` (by file, then start, then end) is provided so that spans can key
/// sorted collections, such as the HIR's references "sorted by (from, span)".
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub struct Span {
    pub file: FileId,
    pub start: u32,
    pub end: u32,
}

impl Span {
    /// `start..end` in `file`.
    pub fn new(file: FileId, start: u32, end: u32) -> Span {
        Span { file, start, end }
    }

    /// The empty span at `at`.
    pub fn point(file: FileId, at: u32) -> Span {
        Span {
            file,
            start: at,
            end: at,
        }
    }

    /// The smallest span covering both spans: minimum start, maximum end.
    /// Both spans must be in the same file (spans never cross files); the
    /// result keeps `self.file`.
    pub fn join(self, other: Span) -> Span {
        debug_assert_eq!(
            self.file, other.file,
            "Span::join across files ({:?} and {:?})",
            self.file, other.file
        );
        Span {
            file: self.file,
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    /// Length in bytes (0 for an empty or inverted span).
    pub fn len(self) -> u32 {
        self.end.saturating_sub(self.start)
    }

    /// True when the span covers no bytes.
    pub fn is_empty(self) -> bool {
        self.end <= self.start
    }
}

/// Where the text of a [`SourceFile`] came from.
///
/// Serialized in `snake_case` like every JSON name of the toolchain
/// (`"project"`, `{"revision": "<40 hex>"}`).
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileOrigin {
    /// Read from the project root (or a `VirtualTree` of it).
    Project,
    /// An embedded standard-library stub.
    Std,
    /// Materialized from a git commit (40-hex id) for `cpl diff` and baselines.
    Revision(String),
    /// Text supplied by a command (a selector argument, a patch body under test) or by a test.
    Virtual,
}

/// One registered file.
#[derive(Clone)]
pub struct SourceFile {
    pub id: FileId,
    /// '/'-separated. Project files: relative to the project root
    /// (`"src/reader/pages.cpl"`). Std files: `"std/<name>.cpl"`. Virtual
    /// files: `"<virtual:NAME>"` by convention.
    pub path: String,
    pub origin: FileOrigin,
    text: Arc<str>,
    /// Byte offset of the first character of every line; `line_starts[0]`
    /// is 0, or the length of a leading byte-order mark.
    line_starts: Vec<u32>,
}

impl fmt::Debug for SourceFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SourceFile")
            .field("id", &self.id)
            .field("path", &self.path)
            .field("origin", &self.origin)
            .field("len", &self.text.len())
            .field("lines", &self.line_starts.len())
            .finish()
    }
}

const BOM: char = '\u{feff}';

impl SourceFile {
    fn new(id: FileId, path: String, origin: FileOrigin, text: String) -> SourceFile {
        let first = if text.starts_with(BOM) {
            BOM.len_utf8() as u32
        } else {
            0
        };
        let mut line_starts = vec![first];
        let bytes = text.as_bytes();
        let mut i = first as usize;
        while i < bytes.len() {
            match bytes[i] {
                b'\n' => line_starts.push((i + 1) as u32),
                b'\r' => {
                    if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                        i += 1;
                    }
                    line_starts.push((i + 1) as u32);
                }
                _ => {}
            }
            i += 1;
        }
        SourceFile {
            id,
            path,
            origin,
            text: Arc::from(text),
            line_starts,
        }
    }

    /// The complete text of the file.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The text as a shared string (cheap to clone).
    pub fn shared_text(&self) -> Arc<str> {
        Arc::clone(&self.text)
    }

    /// Number of lines. A file without a final terminator still counts its
    /// last line, and a final terminator starts one more (empty) line, so
    /// the end-of-file position always has a line; an empty file has one.
    pub fn line_count(&self) -> u32 {
        self.line_starts.len() as u32
    }

    /// The text of a 1-based line without its terminator (`""` when out of range).
    pub fn line_text(&self, line: u32) -> &str {
        if line == 0 || line as usize > self.line_starts.len() {
            return "";
        }
        let start = self.line_starts[line as usize - 1] as usize;
        let end = self
            .line_starts
            .get(line as usize)
            .map(|&s| s as usize)
            .unwrap_or(self.text.len());
        self.text[start..end].trim_end_matches(['\n', '\r'])
    }

    /// Clamp `offset` into the text and back onto a character boundary.
    fn clamp(&self, offset: u32) -> usize {
        let mut o = (offset as usize).min(self.text.len());
        while o > 0 && !self.text.is_char_boundary(o) {
            o -= 1;
        }
        o
    }

    /// 1-based line and column of a byte offset. Offsets past the end clamp
    /// to the end of the file; an offset inside a multi-byte character or
    /// a leading byte-order mark maps to that character's (or line 1's) start.
    pub fn line_col(&self, offset: u32) -> LineCol {
        let o = self.clamp(offset);
        if o < self.line_starts[0] as usize {
            return LineCol { line: 1, col: 1 };
        }
        let line_idx = self.line_starts.partition_point(|&s| (s as usize) <= o) - 1;
        let line_start = self.line_starts[line_idx] as usize;
        let col = self.text[line_start..o].chars().count() as u32 + 1;
        LineCol {
            line: line_idx as u32 + 1,
            col,
        }
    }

    /// The JSON form of a span of this file.
    pub fn location(&self, span: Span) -> Location {
        let start = self.line_col(span.start);
        let end = self.line_col(span.end.max(span.start));
        Location {
            file: self.path.clone(),
            line: start.line,
            col: start.col,
            end_line: end.line,
            end_col: end.col,
        }
    }

    /// The text covered by `start..end`, clamped to the file and to
    /// character boundaries.
    pub fn slice(&self, start: u32, end: u32) -> &str {
        let s = self.clamp(start);
        let e = self.clamp(end.max(start));
        &self.text[s..e]
    }
}

/// The registry of source files: the only way to turn a [`Span`] into
/// lines, columns, and text (nothing downstream of `cpl-syntax` reads file
/// text except through [`SourceMap::text`], Section 3.2).
#[derive(Clone, Debug, Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
}

impl SourceMap {
    pub fn new() -> Self {
        SourceMap::default()
    }

    /// Register a file and return its id (the next unused one).
    pub fn add(&mut self, path: String, origin: FileOrigin, text: String) -> FileId {
        let id = FileId(self.files.len() as u32);
        self.files.push(SourceFile::new(id, path, origin, text));
        id
    }

    /// Register a file read as bytes. Invalid UTF-8 is the parse diagnostic
    /// `invalid_utf8` raised when the bytes are read (grammar 1.1, D70): the
    /// file is still registered, with the bytes decoded lossily (each
    /// invalid sequence becomes U+FFFD), so that it has a [`FileId`] and the
    /// diagnostic a span (the first replacement character). The caller
    /// treats the file as failed and does not lex it.
    pub fn add_bytes(
        &mut self,
        path: String,
        origin: FileOrigin,
        bytes: &[u8],
    ) -> (FileId, Option<Diagnostic>) {
        match std::str::from_utf8(bytes) {
            Ok(text) => (self.add(path, origin, text.to_string()), None),
            Err(err) => {
                let valid = err.valid_up_to();
                let text = String::from_utf8_lossy(bytes).into_owned();
                let id = self.add(path, origin, text);
                let span = Span::new(
                    id,
                    valid as u32,
                    (valid + char::REPLACEMENT_CHARACTER.len_utf8()) as u32,
                );
                let diagnostic = Diagnostic::error(
                    DiagCode("invalid_utf8"),
                    Category::Parse,
                    format!("invalid UTF-8 at byte {valid}"),
                )
                .with_span(span);
                (id, Some(diagnostic))
            }
        }
    }

    /// The file with this id. Panics when the id was not issued by this map.
    pub fn file(&self, id: FileId) -> &SourceFile {
        self.get(id)
            .unwrap_or_else(|| panic!("FileId({}) is not registered in this SourceMap", id.0))
    }

    /// The file with this id, or `None` when the id was not issued by this map.
    pub fn get(&self, id: FileId) -> Option<&SourceFile> {
        self.files.get(id.0 as usize)
    }

    /// Every file in id order.
    pub fn files(&self) -> impl Iterator<Item = &SourceFile> {
        self.files.iter()
    }

    /// The first file registered under `path`.
    pub fn find(&self, path: &str) -> Option<FileId> {
        self.files.iter().find(|f| f.path == path).map(|f| f.id)
    }

    /// The source text a span covers (clamped to the file and to character
    /// boundaries). Panics when the span's file is not registered.
    pub fn text(&self, span: Span) -> &str {
        self.file(span.file).slice(span.start, span.end)
    }

    /// 1-based line and column of a byte offset. Panics when the file is not registered.
    pub fn line_col(&self, file: FileId, offset: u32) -> LineCol {
        self.file(file).line_col(offset)
    }

    /// The JSON form of a span. Panics when the span's file is not registered.
    pub fn location(&self, span: Span) -> Location {
        self.file(span.file).location(span)
    }

    /// Number of registered files.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// 1-based line; 1-based column counted in Unicode scalar values from the line start.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct LineCol {
    pub line: u32,
    pub col: u32,
}

/// The JSON form of a span: `end_col` is exclusive, and a point span has
/// `end_line == line` and `end_col == col` (semantics S49).
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct Location {
    pub file: String,
    pub line: u32,
    pub col: u32,
    pub end_line: u32,
    pub end_col: u32,
}
