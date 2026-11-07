//! The canonical formatter of CPL 0.1.
//!
//! The layout rules are those of `spec/toolchain.md` section 2.4 (rules 1
//! to 12 and decisions T11 to T14), summarised here with the places where
//! this implementation refines them:
//!
//! 1. Four-space indentation, LF line terminators, no trailing whitespace,
//!    exactly one LF at the end of the file.
//! 2. One statement, member, clause, item, arm, or `.work` item per line.
//! 3. File order: module doc comments, `module` header, one blank line, `use`
//!    declarations in source order, one blank line, declarations in source
//!    order separated by exactly one blank line, with the exception of
//!    decision T51 (toolchain 2.4 rule 3): a `type` declaration written on
//!    the line directly below another `type` declaration stays there when
//!    neither carries doc comments or annotations and neither a blank line
//!    nor a comment separates them, which keeps the design's `pub type
//!    PageId(Text)` directly above `pub type Positive = Int where self > 0`
//!    (design lines 53-54, copied byte for byte into
//!    `examples/reader/src/reader/pages.cpl`) canonical. Every other pair of
//!    declarations, adjacent `type` declarations with doc comments,
//!    annotations, or a comment between them included, is separated by
//!    exactly one blank line.
//! 4. Inside a brace body, among `use` declarations, and among `.work`
//!    items: one blank line between consecutive nodes when the source had
//!    one (the node's `Trivia::blank_line_before`, or its first leading
//!    comment's), none otherwise. A blank line between a node's leading
//!    comments and the node itself is kept as well (T11), everywhere the
//!    parser records one: it never does for the first node of a brace body
//!    or file (2.3), so none is printed there, which also keeps the output
//!    idempotent when rule 12 moves a `.work` item to the front. Never a
//!    blank line after `{` or before `}`; clauses never have blank lines
//!    between them.
//! 5. Doc comments first (`/// text`), then annotations one per line in
//!    source order, then the declaration. Ordinary comments written among
//!    the prefix lines print above the doc comments (they are leading
//!    trivia of the declaration).
//! 6. Comma groups (parameters, arguments, list literals, struct literals,
//!    `effects` sets, `use` lists, annotation arguments, selector sets,
//!    variant payloads, constructor-pattern payloads) print flat unless the
//!    source broke them across lines (`Group::multiline`), an element or
//!    the group's `trailing_comments` carries a comment, or the flat line
//!    would exceed [`MAX_WIDTH`] columns; a broken group puts each element
//!    on its own line one level deeper with a trailing comma. When a line
//!    is too long the outermost flat group on it is broken first and the
//!    rule is re-applied. Generic lists never break. "Broke them across
//!    lines" means a line terminator at the group's own bracket level: a
//!    line break inside a nested group or inside a block (`f(if c { 1 }
//!    else { 2 })`, whose blocks the formatter always prints multi-line)
//!    does not break the enclosing group, which keeps the rule idempotent.
//!    Fitting a line unit renders it again for every group it breaks; the
//!    units nested inside it (statements, tails, arms, loop clauses) keep
//!    their final text meanwhile (`Printer::fitted_unit`), so formatting
//!    stays fast at the nesting limit of grammar D73.
//! 7. `pub fn name<T>(params) -> Ret` on one line; clauses follow one per
//!    line indented one level, in canonical order (`effects`, `requires`
//!    in source order, `ensures` in source order, `decreases`); the body's
//!    `{` is on the header line when there are no clauses and the
//!    parameter group is flat, otherwise on its own line. `while` prints
//!    `invariant`s then `decreases`; `property` prints its `for` clauses;
//!    `patch` its `expect` line. Capability operations put their contract
//!    clauses one level deeper than the operation. "Headers whose group
//!    broke" are read as the `if`, `else if`, `match`, and `while` headers
//!    in which a comma group printed broken: their `{` goes on its own line
//!    at the header's indentation, except inside a comma group, where a line
//!    break before the `{` would break that group on the next parse.
//! 8. One space around binary operators, `=`, `->`, `=>`, after `:` and `,`;
//!    none inside `()`, `[]`, `<>`, or a `use` list's braces; one space
//!    inside the braces of a struct literal, an `effects` set, and a
//!    selector set; `?`, `.member`, `..spread`, unary `-`, and `?hole`
//!    attach to their operand; `not` is followed by one space; parentheses
//!    are preserved exactly as written; blocks are multi-line except `{}`.
//! 9. Line breaks inside an expression are removed, except that inside
//!    parentheses a source line break directly before a binary operator is
//!    kept (`BinaryExpr::break_before_op`), the operator starting a
//!    continuation line one level deeper than the line of the `(`.
//! 10. Text literals are re-escaped canonically (`\\`, `\"`, `\n`, `\r`,
//!     `\t`, other control characters and DEL as `\u{h}` with minimal
//!     lowercase hex digits, everything else literally); integers print
//!     their digits without separators.
//! 11. Comments: leading comments on their own lines at the node's
//!     indentation, byte-for-byte after `//`, each preceded by a blank line
//!     when the source had one; a trailing comment after one space;
//!     comments after the last element of a body or group at the inner
//!     indentation. The trailing spaces and tabs of a comment are removed,
//!     and only those: rules 1 and 11 do not define whitespace, and grammar
//!     1.1's horizontal whitespace is space and tab, so a comment or doc
//!     line ending in U+00A0 or U+3000 keeps it (`trim_horizontal`). A
//!     second trailing comment of a node goes on a line of its own below
//!     it, where the next parse attaches it to what follows (toolchain 2.3
//!     rules C to F), so the printer puts it where that attachment prints it
//!     and formatting stays idempotent: after the blank line that separates
//!     two declarations (rule 3), below the `{` of a body that follows a
//!     header clause (rule C), among a declaration's leading comments for
//!     an annotation's (rule 5), and on the next lines anywhere else
//!     (`split_trailing`).
//! 12. `.work`: `use` declarations, one blank line, changes separated by one
//!     blank line; inside `change`: `base`, `target`, `workers`, tasks in
//!     source order, `integrate`; inside `task`: `write`, `add`, `depend`,
//!     `preserve`, `goal`; `after` on the task's header line. `.patch`:
//!     `patch sel`, `    expect base(sel)`, then the block from column 0.
//!
//! The formatter never reorders declarations, members, arms, or set
//! elements (T13), never adds or removes tokens (T14; an annotation written
//! `@name()` keeps its parentheses, `Annotation::parens`), and only ever
//! receives a successfully parsed tree ([`format_source`] returns the parse
//! diagnostics otherwise, T7).
//!
//! # Fragments
//!
//! [`format_fragment`] renders one [`Fragment`] (a declaration or one of its
//! facets, a block, an expression, a type, a selector) on its own, at
//! indentation level 0, with LF line ends and no trailing newline
//! (toolchain 1.5 and 5.2). [`FragmentMode::Canonical`] produces the facet
//! canonical text that `cpl-index` hashes and the canonical clause text of
//! the HIR's `Clause::text` (toolchain 3.3, semantics 9.2 `clause`):
//! comments, blank lines, and source line breaks are dropped and every comma
//! group is flat, so a clause, a signature line, or an `effects` set is one
//! line (blocks inside an expression stay multi-line, rule 8).
//! [`FragmentMode::Source`] is the formatter's own rendering of the same
//! fragment with its comments, the text `cpl-work` splices into a file when
//! it applies a patch (toolchain 10.5).

use crate::ast::*;
use crate::parser::{parse_module, parse_patch, parse_work};
use cpl_diag::{Diagnostic, FileId, Span};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Rule 6 (iii): the column budget of one line, counted in scalar values.
pub const MAX_WIDTH: usize = 100;

const INDENT: &str = "    ";
const NO_COMMENTS: &[Comment] = &[];

/// The three file forms the formatter accepts.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum FileKind {
    Cpl,
    Work,
    Patch,
}

impl FileKind {
    /// The file kind for a path, by extension (`cpl`, `work`, `patch`).
    pub fn from_path(path: &Path) -> Option<FileKind> {
        match path.extension()?.to_str()? {
            "cpl" => Some(FileKind::Cpl),
            "work" => Some(FileKind::Work),
            "patch" => Some(FileKind::Patch),
            _ => None,
        }
    }

    /// The file extension of this kind, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            FileKind::Cpl => "cpl",
            FileKind::Work => "work",
            FileKind::Patch => "patch",
        }
    }
}

/// Canonical text of a `.cpl` module.
pub fn format_module(m: &Module) -> String {
    let mut p = Printer::new(false);
    p.module(m);
    p.finish()
}

/// Canonical text of a `.work` file.
pub fn format_work(w: &WorkFile) -> String {
    let mut p = Printer::new(false);
    p.work_file(w);
    p.finish()
}

/// Canonical text of a `.patch` file.
pub fn format_patch(p: &PatchFile) -> String {
    let mut printer = Printer::new(false);
    printer.patch_file(p);
    printer.finish()
}

/// Parse then format. `Err` carries the parse diagnostics when the file has
/// a syntax error: the formatter never rewrites a file it could not fully
/// parse (`spec/toolchain.md` T7).
pub fn format_source(src: &str, kind: FileKind, file: FileId) -> Result<String, Vec<Diagnostic>> {
    match kind {
        FileKind::Cpl => parse_module(src, file)
            .into_result()
            .map(|m| format_module(&m)),
        FileKind::Work => parse_work(src, file).into_result().map(|w| format_work(&w)),
        FileKind::Patch => parse_patch(src, file)
            .into_result()
            .map(|p| format_patch(&p)),
    }
}

// ---------------------------------------------------------------------------
// Fragments (toolchain 1.5, 5.2)
// ---------------------------------------------------------------------------

/// How [`format_fragment`] treats trivia and layout memory.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum FragmentMode {
    /// Facet canonical text (`spec/toolchain.md` 5.2): every [`Trivia`] and
    /// every `trailing_comments` list is skipped, so no ordinary comment and
    /// no blank line appears; source line breaks are not reproduced
    /// (`Group::multiline` and `BinaryExpr::break_before_op` are ignored);
    /// every comma group prints flat and no line is broken for width, so a
    /// clause, a signature line, or an `effects` set is one line. Blocks
    /// stay multi-line (2.4 rule 8), one statement per line. Parentheses
    /// are kept exactly as written.
    Canonical,
    /// The formatter's own rendering of the fragment, as it appears in a
    /// formatted file at indentation level 0, comments and blank lines
    /// included (2.4): the text patch application splices into a file
    /// (`spec/toolchain.md` 10.5).
    Source,
}

/// One AST fragment for [`format_fragment`] (`spec/toolchain.md` 1.5). The
/// examples show the [`FragmentMode::Canonical`] text for the corpus
/// declarations of `examples/reader/src/reader/pages.cpl`.
#[derive(Copy, Clone, Debug)]
pub enum Fragment<'a> {
    /// The whole declaration, doc comments and annotations included, as the
    /// formatter prints it in a file (in `Source` mode with its leading and
    /// trailing comments): `@id("pgn_7c2d")` LF `@prove` LF `pub fn
    /// paginate<T>(...) -> List<List<T>>` LF `    ensures ...` ... `}`.
    Declaration(&'a Declaration),
    /// The `signature` facet (5.1, 5.2), the declaration name replaced by
    /// `_` (T24) and member doc comments left out (T25):
    /// `pub fn _<T>(items: List<T>, size: Positive) -> List<List<T>>` (the
    /// parameters flat), `pub type _(Text)`, `pub type _ = Int` (the base;
    /// the predicate is the contract), `pub type _<T>` (a std opaque type),
    /// `pub struct _ {` LF `    id: PageId` LF `    text: Text` LF `}`, an
    /// enum likewise with `    Missing(PageId)` and `    Unavailable` lines,
    /// `pub capability _ {` LF `    fn get(id: PageId) -> Result<Page,
    /// StoreError>` LF `}` (no contracts), and for a property one `for x in
    /// <gen>` line per clause. An empty body prints `{}` (`pub struct _ {}`),
    /// as the formatter prints it; a test, which has no signature facet,
    /// gives the empty string.
    Signature(&'a Declaration),
    /// The `contract` facet: for a `fn` every `requires <expr>` in source
    /// order, then every `ensures <expr>` in source order, then `decreases
    /// <expr>`, one per line without indentation (`requires b != 0` LF
    /// `requires a % b == 0` LF `ensures result * b == a`); for a
    /// refinement `where self > 0`; for a capability, for each operation
    /// with clauses, `fn name` LF and its `    requires` / `    ensures`
    /// lines (requires first). The empty string when there are none or the
    /// facet does not apply.
    Contract(&'a Declaration),
    /// The `effects` facet: in `Canonical` mode the references sorted
    /// lexicographically and deduplicated (T26), `effects { codec.dimensions,
    /// db.get }`, and the empty string for an empty set (`effects {}`); in
    /// `Source` mode the clause as the formatter prints it, in the author's
    /// order (T13), with its comments.
    Effects(&'a EffectsClause),
    /// A block at level 0: `{` LF statements indented one level LF `}`, or
    /// `{}`; loop invariants and `decreases` clauses included (the `body`
    /// facet of 5.2; in `Source` mode the text of patch application, 10.5).
    Block(&'a Block),
    /// The `docs` facet text of one doc-comment run: the doc lines joined
    /// with LF, without `///` and without trailing whitespace (rule 1), so
    /// that formatting never changes the facet (`Arbitrary-precision
    /// integer.`). Doc comments are not comments: both modes give this text.
    /// The member part of a struct, enum, or capability `docs` facet (`LF
    /// member <name> LF <lines>`) is assembled by the caller from the
    /// members' `Docs`.
    Docs(&'a Docs),
    /// Annotations one per line in the given order: `@id("pgn_7c2d")` LF
    /// `@prove`; `@name()` keeps its parentheses (T14).
    Annotations(&'a [Annotation]),
    /// One expression; in `Canonical` mode the canonical clause text of
    /// semantics 9.2 and the HIR's `Clause::text` (toolchain 3.3), without
    /// the clause keyword: `flatten(result) == items`, `forall page in
    /// result: page.len > 0 and page.len <= size`, `b != 0`. It is one line
    /// unless the expression contains an `if` or `match`, whose blocks are
    /// always multi-line and whose arms take a line each (rule 8). The
    /// canonically escaped text literal of a test or property name (the
    /// `name` facet, 5.2) is the rendering of a `Text` literal expression
    /// holding the name: `"pagination preserves input"`.
    Expr(&'a Expr),
    /// A type: `List<List<T>>`, `std.text.Text`; identical in both modes.
    Type(&'a Type),
    /// A `.work` or `.patch` selector: `paginate.body`, `tests.pagination.*`
    /// (in `Source` mode with its leading and trailing comments).
    Selector(&'a Selector),
}

/// Formats one fragment in isolation at indentation level 0, with LF line
/// ends and no trailing newline (`spec/toolchain.md` 1.5).
/// [`FragmentMode::Canonical`] omits every ordinary comment and blank line
/// and prints every group flat: the facet canonical text of 5.2 (and the
/// canonical clause text of `Clause::text`, 3.3).
/// [`FragmentMode::Source`] keeps the comments and the formatter's layout:
/// the text of patch application, 10.5. See [`Fragment`] for the text of
/// each variant.
pub fn format_fragment(f: Fragment<'_>, mode: FragmentMode) -> String {
    let mut p = Printer::new(mode == FragmentMode::Canonical);
    match f {
        Fragment::Declaration(d) => {
            // As a file prints it when it is the file's last declaration: its
            // further trailing comments follow it (see `Printer::module`).
            p.declaration_prefix(d, &[]);
            let further = p.declaration(d);
            p.comment_lines(&further, 0, false, false);
        }
        Fragment::Signature(d) => p.signature(d),
        Fragment::Contract(d) => p.contract(d),
        Fragment::Effects(e) => {
            if p.canonical {
                p.effects_canonical(e);
            } else {
                let mut pending = Vec::new();
                p.clause_unit(&e.trivia, 0, &mut pending, |p| p.effects_set(e));
                p.comment_lines(&pending, 0, true, false);
            }
        }
        Fragment::Block(b) => p.block(b, 0),
        Fragment::Docs(d) => return docs_text(d),
        Fragment::Annotations(annotations) => {
            for a in annotations {
                p.annotation(a, 0, true);
            }
        }
        Fragment::Expr(e) => p.fitted(|p| p.expr(e)),
        Fragment::Type(t) => p.ty(t),
        Fragment::Selector(s) => {
            p.comment_lines(&s.trivia.leading, 0, true, true);
            p.indent(0);
            p.selector(s);
            p.end_line(&s.trivia.trailing, 0);
        }
    }
    p.finish_fragment()
}

/// The `docs` facet text of one doc-comment run (5.2): the lines joined
/// with LF, trailing whitespace removed as the formatter removes it (rule 1).
fn docs_text(d: &Docs) -> String {
    d.lines
        .iter()
        .map(|l| trim_horizontal(&l.text))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `text` without its trailing whitespace in the sense of rules 1 and 11 and
/// the `docs` facet (5.2), which `spec/toolchain.md` does not define: the
/// source's horizontal whitespace, space and tab (grammar 1.1). Other Unicode
/// white space (U+00A0, U+3000, U+2028, ...) is part of a comment's or doc
/// line's text, which rule 11 keeps byte for byte; trimming it would change
/// the text formatting must preserve.
fn trim_horizontal(text: &str) -> &str {
    text.trim_end_matches([' ', '\t'])
}

// ---------------------------------------------------------------------------
// Printer state
// ---------------------------------------------------------------------------

/// Where a comma group was printed within one line unit (rule 6 (iii)).
struct GroupRecord {
    id: Span,
    line: usize,
    depth: usize,
    broken: bool,
}

/// A line unit that can be printed inside another one, by kind (statement,
/// block tail, match arm, loop clause), address of its node, and
/// indentation level: everything its text depends on
/// ([`Printer::fitted_unit`]).
type UnitKey = (u8, usize, usize);

const UNIT_STATEMENT: u8 = 0;
const UNIT_TAIL: u8 = 1;
const UNIT_ARM: u8 = 2;
const UNIT_CLAUSE: u8 = 3;

/// The final rendering of a line unit: its text, the number of line
/// terminators in it, and the indentation level of its last line.
struct UnitText {
    text: String,
    lines: usize,
    line_indent: usize,
}

struct Printer {
    out: String,
    /// Number of line terminators written so far (the current line's index).
    line: usize,
    /// Indentation level of the current line.
    line_indent: usize,
    /// Indentation level for operator continuation lines inside the
    /// innermost parenthesized expression (rule 9), `None` outside one.
    cont_indent: Option<usize>,
    /// Nesting depth of comma groups being printed.
    depth: usize,
    /// Groups this line unit must print broken because a flat rendering
    /// overflowed [`MAX_WIDTH`].
    forced: HashSet<Span>,
    records: Vec<GroupRecord>,
    /// [`FragmentMode::Canonical`]: no comments, no blank lines, every group
    /// flat, no width or operator-continuation breaks (5.2).
    canonical: bool,
    /// Comma groups printed broken so far by this printer, not counting
    /// those of the line units inside it (rule 7: a header whose group
    /// broke puts its `{` on a line of its own).
    broken_groups: usize,
    /// Number of line units being fitted around this printer (0 outside
    /// every unit, [`Printer::fitted`]).
    fit_depth: usize,
    /// The final texts of the units printed directly inside the units being
    /// fitted ([`Printer::fitted_unit`]), and their keys in insertion order.
    units: HashMap<UnitKey, UnitText>,
    unit_log: Vec<UnitKey>,
}

/// One element of a comma group together with its trivia.
enum Elem<'a> {
    Param(&'a Param),
    UsingParam(&'a UsingParam),
    Arg(&'a Arg),
    UsingArg(&'a UsingArg),
    Item(&'a ListElem),
    Spread(&'a Expr, &'a Trivia),
    Field(&'a FieldInit),
    Effect(&'a EffectRef),
    UseItem(&'a UseItem),
    Selector(&'a Selector),
    Type(&'a Type),
    Pattern(&'a Pattern),
    Literal(&'a Literal),
}

impl Elem<'_> {
    fn trivia(&self) -> Option<&Trivia> {
        match self {
            Elem::Param(p) => Some(&p.trivia),
            Elem::UsingParam(p) => Some(&p.trivia),
            Elem::Arg(a) => Some(&a.trivia),
            Elem::UsingArg(a) => Some(&a.trivia),
            Elem::Item(e) => Some(&e.trivia),
            Elem::Field(f) => Some(&f.trivia),
            Elem::Effect(e) => Some(&e.trivia),
            Elem::UseItem(u) => Some(&u.trivia),
            Elem::Selector(s) => Some(&s.trivia),
            Elem::Spread(_, t) => Some(t),
            Elem::Type(_) | Elem::Pattern(_) | Elem::Literal(_) => None,
        }
    }

    fn trailing(&self) -> &[Comment] {
        self.trivia().map_or(NO_COMMENTS, |t| &t.trailing)
    }
}

/// How a comma group is bracketed.
struct GroupSpec<'a> {
    /// A span unique to this group within the file (its brackets).
    id: Span,
    /// `Group::multiline` of the source.
    multiline: bool,
    open: &'static str,
    close: &'static str,
    /// One space inside the brackets when flat (struct literals, sets).
    spaced: bool,
    /// Comments after the last element.
    dangling: &'a [Comment],
}

/// Rule 3's one exception (see the module documentation): `d` stays on the
/// line directly below `prev` when both are `type` declarations without doc
/// comments or annotations and the source separated them by a line break
/// alone, with no blank line and no comment.
fn stays_adjacent(prev: &Declaration, d: &Declaration) -> bool {
    let bare_type = |x: &Declaration| {
        matches!(x.kind, DeclKind::Type(_)) && x.docs.is_none() && x.annotations.is_empty()
    };
    bare_type(prev) && bare_type(d) && d.trivia.leading.is_empty() && !d.trivia.blank_line_before
}

/// A node's trailing comments split for rule 11: the first prints after the
/// node's last token, the others ("further" trailing comments) on lines of
/// their own below it. The parser attaches a comment on a line of its own to
/// what follows it (2.3 rules C to F), never to the node above, so the
/// printer puts the further ones where that attachment prints them: above
/// the next node of the same sequence (where nothing else intervenes they are
/// simply the next lines), below the `{` of a body that follows a header
/// clause (rule C), after the blank line that separates two declarations
/// (rule 3), and above the doc comments of the declaration an annotation
/// belongs to (rule 5). Formatting is then idempotent (Section 12), and the
/// comment survives with a new owner, which `strip_layout` permits.
fn split_trailing(trailing: &[Comment]) -> (&[Comment], &[Comment]) {
    trailing.split_at(trailing.len().min(1))
}

/// Rule 10: the canonical spelling of a text literal.
fn escape_text(value: &str, out: &mut String) {
    out.push('"');
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // D75: a bidirectional formatting character is no raw character of a literal
            c if (c as u32) < 0x20 || c == '\x7f' || crate::lexer::is_bidi_control(c) => {
                out.push_str(&format!("\\u{{{:x}}}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

impl Printer {
    fn new(canonical: bool) -> Printer {
        Printer {
            out: String::new(),
            line: 0,
            line_indent: 0,
            cont_indent: None,
            depth: 0,
            forced: HashSet::new(),
            records: Vec::new(),
            canonical,
            broken_groups: 0,
            fit_depth: 0,
            units: HashMap::new(),
            unit_log: Vec::new(),
        }
    }

    /// Rule 1: no trailing whitespace, exactly one final LF.
    fn finish(self) -> String {
        let mut text = String::with_capacity(self.out.len() + 1);
        for line in self.out.split('\n') {
            text.push_str(trim_horizontal(line));
            text.push('\n');
        }
        while text.ends_with("\n\n") {
            text.pop();
        }
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text
    }

    /// A fragment's text (5.2): no trailing whitespace, no trailing newline.
    fn finish_fragment(self) -> String {
        let mut text = self
            .out
            .split('\n')
            .map(trim_horizontal)
            .collect::<Vec<_>>()
            .join("\n");
        while text.ends_with('\n') {
            text.pop();
        }
        text
    }

    // -- low-level emitters ---------------------------------------------------

    fn push(&mut self, s: &str) {
        self.out.push_str(s);
    }

    fn newline(&mut self) {
        self.out.push('\n');
        self.line += 1;
    }

    /// A blank line; only called at the start of a line.
    fn blank_line(&mut self) {
        self.newline();
    }

    fn indent(&mut self, level: usize) {
        for _ in 0..level {
            self.out.push_str(INDENT);
        }
        self.line_indent = level;
    }

    /// The comments to print: none in canonical mode (5.2).
    fn comments<'c>(&self, comments: &'c [Comment]) -> &'c [Comment] {
        if self.canonical {
            NO_COMMENTS
        } else {
            comments
        }
    }

    /// Own-line comments at `level`. A comment preceded by a blank line in
    /// the source keeps one, except the first comment when `first` (no blank
    /// line after `{` or at the start of a sequence) and never when
    /// `!blanks` (clauses, rule 4).
    fn comment_lines(&mut self, comments: &[Comment], level: usize, first: bool, blanks: bool) {
        for (i, c) in self.comments(comments).iter().enumerate() {
            if blanks && c.blank_line_before && !(first && i == 0) {
                self.blank_line();
            }
            self.indent(level);
            self.push(trim_horizontal(&c.text));
            self.newline();
        }
    }

    /// Finish the current line: the first trailing comment after one space,
    /// any further ones on their own lines at `level`.
    fn end_line(&mut self, trailing: &[Comment], level: usize) {
        let mut iter = self.comments(trailing).iter();
        if let Some(first) = iter.next() {
            self.push(" ");
            self.push(trim_horizontal(&first.text));
        }
        self.newline();
        for c in iter {
            self.indent(level);
            self.push(trim_horizontal(&c.text));
            self.newline();
        }
    }

    /// The blank lines and leading comments that precede a node beginning a
    /// line (rules 4 and 11, T11). `first`: nothing of the node's sequence
    /// precedes it here (it follows `{`, starts the file, or follows a
    /// separator that rule 3 or 12 already printed), so neither its own nor
    /// its first comment's preceding blank line is printed.
    /// `blank_after_comments`: print the blank line the parser recorded
    /// between the leading comments and the node.
    fn prefix(&mut self, t: &Trivia, level: usize, first: bool, blank_after_comments: bool) {
        if self.canonical {
            return;
        }
        if t.leading.is_empty() {
            if !first && t.blank_line_before {
                self.blank_line();
            }
            return;
        }
        self.comment_lines(&t.leading, level, first, true);
        if blank_after_comments && t.blank_line_before {
            self.blank_line();
        }
    }

    /// [`Printer::prefix`] for a node of a brace body or of a file's run of
    /// `use` declarations (`first`: the first node printed there). The
    /// parser never records a blank line before the first node of a brace
    /// body or file (2.3), so none is printed between its comments and it
    /// either; a `.work` item that rule 12 moved to the front then formats
    /// the same way twice.
    fn node_prefix(&mut self, t: &Trivia, level: usize, first: bool) {
        self.prefix(t, level, first, !first);
    }

    /// [`Printer::node_prefix`] with `carried` comments (another node's
    /// further trailing comments, [`split_trailing`]) printed first, as the
    /// leading comments the parser attaches to this node.
    fn node_prefix_with(&mut self, t: &Trivia, level: usize, first: bool, carried: &[Comment]) {
        if carried.is_empty() {
            self.node_prefix(t, level, first);
        } else {
            let mut merged = t.clone();
            merged.leading.splice(0..0, carried.iter().cloned());
            self.node_prefix(&merged, level, first);
        }
    }

    /// The comments and blank line above a declaration's doc comments
    /// (rules 3, 5, 11), all of which the parser attaches to the
    /// declaration as leading comments (2.3 rule D: a comment among the
    /// lines of a declaration's prefix is the declaration's): first
    /// `carried`, the previous declaration's further trailing comments, and
    /// the leading comments written above the declaration's first token;
    /// then the blank line the source had before that token (T11); then, in
    /// source order, the comments written among the prefix lines and the
    /// annotations' further trailing comments ([`split_trailing`]), which
    /// the source had after it. The caller has printed the separating blank
    /// line of rule 3, so no blank line follows it directly.
    fn declaration_prefix(&mut self, d: &Declaration, carried: &[Comment]) {
        if self.canonical {
            return;
        }
        let first_token = d.span.start;
        let mut above: Vec<Comment> = carried.to_vec();
        let mut among: Vec<Comment> = Vec::new();
        for c in &d.trivia.leading {
            if c.span.start < first_token {
                above.push(c.clone());
            } else {
                among.push(c.clone());
            }
        }
        for a in &d.annotations {
            among.extend(split_trailing(&a.trivia.trailing).1.iter().cloned());
        }
        among.sort_by_key(|c| c.span.start);
        let blank = !above.is_empty() && d.trivia.blank_line_before;
        let trivia = Trivia {
            leading: above,
            trailing: Vec::new(),
            blank_line_before: blank,
        };
        let printed = !trivia.leading.is_empty();
        self.prefix(&trivia, 0, true, true);
        // No blank line directly after the separator or after the one just
        // printed: rule 4 never prints two in a row.
        self.comment_lines(&among, 0, !printed || blank, true);
    }

    // -- width fitting (rule 6 (iii)) ------------------------------------------

    /// Render one line unit (a construct that starts at the beginning of a
    /// line), breaking the outermost flat group of every over-long line
    /// until each line fits or has nothing left to break. Canonical text is
    /// never broken for width (5.2: a clause is one line).
    fn fitted<R>(&mut self, render: impl Fn(&mut Printer) -> R) -> R {
        if self.canonical {
            return render(self);
        }
        let mut forced: HashSet<Span> = HashSet::new();
        loop {
            let mut sub = Printer {
                out: String::new(),
                line: 0,
                line_indent: self.line_indent,
                cont_indent: None,
                depth: 0,
                forced: forced.clone(),
                records: Vec::new(),
                canonical: false,
                broken_groups: 0,
                fit_depth: self.fit_depth + 1,
                units: std::mem::take(&mut self.units),
                unit_log: std::mem::take(&mut self.unit_log),
            };
            let result = render(&mut sub);
            self.units = std::mem::take(&mut sub.units);
            self.unit_log = std::mem::take(&mut sub.unit_log);
            match sub.overflow_candidate() {
                Some(id) => {
                    forced.insert(id);
                }
                None => {
                    self.out.push_str(&sub.out);
                    self.line += sub.line;
                    self.line_indent = sub.line_indent;
                    if self.fit_depth == 0 {
                        // No unit is being fitted any more: nothing printed so
                        // far is printed again.
                        self.units.clear();
                        self.unit_log.clear();
                    }
                    return result;
                }
            }
        }
    }

    /// [`Printer::fitted`] for a line unit that can be printed inside another
    /// one (a statement, a block's tail, a match arm, a loop clause), whose
    /// text depends only on `key`: its node and its indentation level. When
    /// an enclosing unit is rendered again with one more group broken (rule
    /// 6 (iii)), the unit's final text is reused rather than fitted again.
    /// Without that, every enclosing level doubled the work, and a file of
    /// over-long headers nested to the limit of grammar D73 (64 levels, which
    /// T7 requires `fmt` to format) took hours; with it, each unit is fitted
    /// once per indentation level it is printed at.
    fn fitted_unit(&mut self, key: UnitKey, render: impl Fn(&mut Printer)) {
        if self.canonical || self.fit_depth == 0 {
            // Outside every other unit, a unit is printed exactly once.
            self.fitted(render);
            return;
        }
        if let Some(unit) = self.units.get(&key) {
            self.out.push_str(&unit.text);
            self.line += unit.lines;
            self.line_indent = unit.line_indent;
            return;
        }
        let (start, line, logged) = (self.out.len(), self.line, self.unit_log.len());
        self.fitted(render);
        // The units inside this one are not printed again now that its text
        // is final, so only the units directly inside a unit being fitted
        // are kept.
        for inner in self.unit_log.drain(logged..) {
            self.units.remove(&inner);
        }
        let unit = UnitText {
            text: self.out[start..].to_string(),
            lines: self.line - line,
            line_indent: self.line_indent,
        };
        self.units.insert(key, unit);
        self.unit_log.push(key);
    }

    fn overflow_candidate(&self) -> Option<Span> {
        for (line_no, text) in self.out.split('\n').enumerate() {
            if text.chars().count() <= MAX_WIDTH {
                continue;
            }
            let best = self
                .records
                .iter()
                .filter(|r| r.line == line_no && !r.broken)
                .min_by_key(|r| r.depth);
            if let Some(r) = best {
                return Some(r.id);
            }
        }
        None
    }

    // -- comma groups (rule 6) ---------------------------------------------------

    /// Print a comma group; returns whether it was broken across lines.
    fn group(&mut self, spec: &GroupSpec, elems: &[Elem]) -> bool {
        let dangling = self.comments(spec.dangling);
        if elems.is_empty() && dangling.is_empty() {
            self.push(spec.open);
            self.push(spec.close);
            return false;
        }
        let broken = !self.canonical
            && (spec.multiline
                || !dangling.is_empty()
                || elems
                    .iter()
                    .any(|e| e.trivia().is_some_and(Trivia::has_comments))
                || self.forced.contains(&spec.id));
        self.records.push(GroupRecord {
            id: spec.id,
            line: self.line,
            depth: self.depth,
            broken,
        });
        if broken {
            self.broken_groups += 1;
        }
        let base = self.line_indent;
        let saved_cont = self.cont_indent.take();
        self.depth += 1;
        self.push(spec.open);
        if broken {
            self.newline();
            for (i, e) in elems.iter().enumerate() {
                if let Some(t) = e.trivia() {
                    self.comment_lines(&t.leading, base + 1, i == 0, true);
                }
                self.indent(base + 1);
                self.elem(e);
                self.push(",");
                self.end_line(e.trailing(), base + 1);
            }
            self.comment_lines(dangling, base + 1, elems.is_empty(), true);
            self.indent(base);
            self.push(spec.close);
        } else {
            if spec.spaced {
                self.push(" ");
            }
            for (i, e) in elems.iter().enumerate() {
                if i > 0 {
                    self.push(", ");
                }
                self.elem(e);
            }
            if spec.spaced {
                self.push(" ");
            }
            self.push(spec.close);
        }
        self.depth -= 1;
        self.cont_indent = saved_cont;
        broken
    }

    fn elem(&mut self, e: &Elem) {
        match e {
            Elem::Param(p) => {
                self.push(&p.name.text);
                self.push(": ");
                self.ty(&p.ty);
            }
            Elem::UsingParam(p) => {
                self.push("using ");
                self.push(&p.name.text);
                self.push(": ");
                self.ty(&p.ty);
            }
            Elem::Arg(a) => self.expr(&a.expr),
            Elem::UsingArg(a) => {
                self.push("using ");
                self.expr(&a.expr);
            }
            Elem::Item(el) => self.expr(&el.expr),
            Elem::Spread(s, _) => {
                self.push("..");
                self.expr(s);
            }
            Elem::Field(f) => {
                self.push(&f.name.text);
                self.push(": ");
                self.expr(&f.value);
            }
            Elem::Effect(r) => {
                self.push(&r.param.text);
                self.push(".");
                self.push(&r.operation.text);
            }
            Elem::UseItem(u) => self.push(&u.name.text),
            Elem::Selector(s) => self.selector(s),
            Elem::Type(t) => self.ty(t),
            Elem::Pattern(p) => self.pattern(p),
            Elem::Literal(l) => self.literal(l),
        }
    }

    fn params(&mut self, params: &Params) -> bool {
        let mut elems: Vec<Elem> = params.positional.iter().map(Elem::Param).collect();
        elems.extend(params.using.iter().map(Elem::UsingParam));
        self.group(
            &GroupSpec {
                id: params.span,
                multiline: params.group.multiline,
                open: "(",
                close: ")",
                spaced: false,
                dangling: &params.trailing_comments,
            },
            &elems,
        )
    }

    fn args(&mut self, args: &Args) {
        let mut elems: Vec<Elem> = args.positional.iter().map(Elem::Arg).collect();
        elems.extend(args.using.iter().map(Elem::UsingArg));
        self.group(
            &GroupSpec {
                id: args.span,
                multiline: args.group.multiline,
                open: "(",
                close: ")",
                spaced: false,
                dangling: &args.trailing_comments,
            },
            &elems,
        );
    }

    fn selector_set(&mut self, set: &SelectorSet) {
        let elems: Vec<Elem> = set.selectors.iter().map(Elem::Selector).collect();
        self.group(
            &GroupSpec {
                id: set.span,
                multiline: set.group.multiline,
                open: "{",
                close: "}",
                spaced: true,
                dangling: &set.trailing_comments,
            },
            &elems,
        );
    }

    /// `effects { ... }` in the author's order (T13).
    fn effects_set(&mut self, e: &EffectsClause) {
        self.push("effects ");
        let elems: Vec<Elem> = e.effects.iter().map(Elem::Effect).collect();
        self.group(
            &GroupSpec {
                id: e.span,
                multiline: e.group.multiline,
                open: "{",
                close: "}",
                spaced: true,
                dangling: &e.trailing_comments,
            },
            &elems,
        );
    }

    /// The `effects` facet text (5.2, T26): the references sorted and
    /// deduplicated, nothing for an empty set.
    fn effects_canonical(&mut self, e: &EffectsClause) {
        let mut refs: Vec<String> = e
            .effects
            .iter()
            .map(|r| format!("{}.{}", r.param.text, r.operation.text))
            .collect();
        refs.sort();
        refs.dedup();
        if refs.is_empty() {
            return;
        }
        self.push("effects { ");
        self.push(&refs.join(", "));
        self.push(" }");
    }

    // -- leaves ------------------------------------------------------------------

    fn literal(&mut self, l: &Literal) {
        match &l.kind {
            LiteralKind::Int(digits) => self.push(digits),
            LiteralKind::Text(t) => escape_text(t, &mut self.out),
            LiteralKind::Bool(b) => self.push(if *b { "true" } else { "false" }),
        }
    }

    fn text(&mut self, t: &TextLiteral) {
        escape_text(&t.value, &mut self.out);
    }

    fn ty(&mut self, t: &Type) {
        self.push(&t.path.dotted());
        if let Some(g) = &t.generic_args {
            self.push("<");
            for (i, arg) in g.args.iter().enumerate() {
                if i > 0 {
                    self.push(", ");
                }
                self.ty(arg);
            }
            self.push(">");
        }
    }

    fn generic_params(&mut self, g: &GenericParams) {
        self.push("<");
        for (i, p) in g.params.iter().enumerate() {
            if i > 0 {
                self.push(", ");
            }
            self.push(&p.text);
        }
        self.push(">");
    }

    fn docs(&mut self, d: &Docs, level: usize) {
        for line in &d.lines {
            self.indent(level);
            let text = trim_horizontal(&line.text);
            if text.is_empty() {
                self.push("///");
            } else {
                self.push("/// ");
                self.push(text);
            }
            self.newline();
        }
    }

    fn selector(&mut self, s: &Selector) {
        self.push(&s.path.dotted());
        match &s.kind {
            SelectorKind::Namespace => self.push(".*"),
            SelectorKind::Facet(f) => {
                self.push(".");
                self.push(f.kind.as_str());
            }
            SelectorKind::Declaration => {}
        }
    }

    // -- expressions (rules 8 and 9) ---------------------------------------------

    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Literal(l) => self.literal(l),
            ExprKind::Name(n) => self.push(&n.text),
            ExprKind::Hole(n) => {
                self.push("?");
                self.push(&n.text);
            }
            ExprKind::Forall(f) | ExprKind::Parallel(f) => {
                self.push(if matches!(e.kind, ExprKind::Parallel(_)) {
                    "parallel for "
                } else {
                    "forall "
                });
                self.push(&f.binder.text);
                self.push(" in ");
                self.expr(&f.collection);
                self.push(": ");
                self.expr(&f.predicate);
            }
            ExprKind::Binary(b) => {
                self.expr(&b.lhs);
                match self.cont_indent {
                    Some(level) if b.break_before_op => {
                        self.newline();
                        self.indent(level);
                    }
                    _ => self.push(" "),
                }
                self.push(b.op.symbol());
                self.push(" ");
                self.expr(&b.rhs);
            }
            ExprKind::Unary(u) => {
                self.push(match u.op {
                    UnaryOp::Not => "not ",
                    UnaryOp::Neg => "-",
                });
                self.expr(&u.operand);
            }
            ExprKind::Member(m) => {
                self.expr(&m.target);
                self.push(".");
                self.push(&m.member.text);
            }
            ExprKind::Call(c) => {
                self.expr(&c.callee);
                if let Some(g) = &c.generic_args {
                    self.push("<");
                    for (i, arg) in g.args.iter().enumerate() {
                        if i > 0 {
                            self.push(", ");
                        }
                        self.ty(arg);
                    }
                    self.push(">");
                }
                self.args(&c.args);
            }
            ExprKind::Propagate(inner) => {
                self.expr(inner);
                self.push("?");
            }
            ExprKind::Paren(inner) => {
                self.push("(");
                let saved = self.cont_indent;
                // Canonical text does not reproduce source line breaks (5.2).
                self.cont_indent = if self.canonical {
                    None
                } else {
                    Some(self.line_indent + 1)
                };
                self.expr(inner);
                self.cont_indent = saved;
                self.push(")");
            }
            ExprKind::List(list) => {
                let elems: Vec<Elem> = list.elements.iter().map(Elem::Item).collect();
                self.group(
                    &GroupSpec {
                        id: list.span,
                        multiline: list.group.multiline,
                        open: "[",
                        close: "]",
                        spaced: false,
                        dangling: &list.trailing_comments,
                    },
                    &elems,
                );
            }
            ExprKind::Struct(s) => {
                self.push(&s.path.dotted());
                self.push(" ");
                let mut elems: Vec<Elem> = Vec::new();
                if let Some(spread) = &s.entries.spread {
                    elems.push(Elem::Spread(spread, &s.entries.spread_trivia));
                }
                elems.extend(s.entries.fields.iter().map(Elem::Field));
                self.group(
                    &GroupSpec {
                        id: s.entries.span,
                        multiline: s.entries.group.multiline,
                        open: "{",
                        close: "}",
                        spaced: true,
                        dangling: &s.entries.trailing_comments,
                    },
                    &elems,
                );
            }
            ExprKind::If(i) => self.if_expr(i),
            ExprKind::Match(m) => self.match_expr(m),
        }
    }

    /// A header expression (an `if` or `while` condition, a `match`
    /// scrutinee), returning whether one of its comma groups broke.
    fn header_expr(&mut self, e: &Expr) -> bool {
        let before = self.broken_groups;
        self.expr(e);
        self.broken_groups > before
    }

    /// What separates a header expression from its body's `{` (rule 7: "the
    /// same `{` rule applies to ... headers whose group broke"): one space,
    /// or, when a comma group of the header broke (by the source, a comment,
    /// or width, rule 6), a line break and the header's indentation, as after
    /// a `fn` header whose parameter group broke. Grammar 2.3 and 3.11-3.12
    /// admit the `{` of an `if`, `else if`, `match`, or `while` header on the
    /// next line (`NEWLINE? block`, `NEWLINE? "{"`), for an `if` or `match`
    /// used as an operand too, so the text re-parses to the same tree.
    ///
    /// Inside a comma group the `{` stays on the header's line: there a line
    /// break before it would lie at the group's own level, which the next
    /// parse records as the group's `Group::multiline` (2.2), and a flat
    /// group would come back broken, so formatting would not be idempotent
    /// (Section 12). Rule 7 does not say which headers it means; this is
    /// the reading under which it never changes another node's layout.
    fn before_header_brace(&mut self, level: usize, group_broke: bool) {
        if group_broke && self.depth == 0 {
            self.newline();
            self.indent(level);
        } else {
            self.push(" ");
        }
    }

    fn if_expr(&mut self, i: &IfExpr) {
        let level = self.line_indent;
        self.push("if ");
        let broke = self.header_expr(&i.condition);
        self.before_header_brace(level, broke);
        self.block(&i.then_block, level);
        if let Some(e) = &i.else_branch {
            self.push(" else ");
            match e {
                ElseBranch::Block(b) => self.block(b, level),
                ElseBranch::If(nested) => self.if_expr(nested),
            }
        }
    }

    fn match_expr(&mut self, m: &MatchExpr) {
        let level = self.line_indent;
        self.push("match ");
        let broke = self.header_expr(&m.scrutinee);
        self.before_header_brace(level, broke);
        self.push("{");
        if m.arms.is_empty() && self.comments(&m.trailing_comments).is_empty() {
            self.push("}");
            return;
        }
        self.newline();
        let saved = self.cont_indent.take();
        for (i, arm) in m.arms.iter().enumerate() {
            self.node_prefix(&arm.trivia, level + 1, i == 0);
            let key = (UNIT_ARM, arm as *const MatchArm as usize, level + 1);
            self.fitted_unit(key, |p| {
                p.indent(level + 1);
                p.pattern(&arm.pattern);
                p.push(" => ");
                match &arm.body {
                    ArmBody::Expr(e) => p.expr(e),
                    ArmBody::Block(b) => p.block(b, level + 1),
                }
                p.end_line(&arm.trivia.trailing, level + 1);
            });
        }
        self.comment_lines(&m.trailing_comments, level + 1, m.arms.is_empty(), true);
        self.indent(level);
        self.push("}");
        self.cont_indent = saved;
    }

    fn pattern(&mut self, p: &Pattern) {
        match &p.kind {
            PatternKind::Wildcard => self.push("_"),
            PatternKind::Int { negative, digits } => {
                if *negative {
                    self.push("-");
                }
                self.push(digits);
            }
            PatternKind::Text(t) => escape_text(t, &mut self.out),
            PatternKind::Bool(b) => self.push(if *b { "true" } else { "false" }),
            PatternKind::Binding(n) => self.push(&n.text),
            PatternKind::Ctor(c) => {
                self.push(&c.path.dotted());
                if let Some(args) = &c.args {
                    let elems: Vec<Elem> = args.iter().map(Elem::Pattern).collect();
                    self.group(
                        &GroupSpec {
                            id: p.span,
                            multiline: c.group.multiline,
                            open: "(",
                            close: ")",
                            spaced: false,
                            dangling: NO_COMMENTS,
                        },
                        &elems,
                    );
                }
            }
        }
    }

    // -- blocks and statements -----------------------------------------------------

    /// `{` ... `}` with the statements one level deeper than `level`; the
    /// cursor is left after the `}`.
    fn block(&mut self, b: &Block, level: usize) {
        self.block_with(b, level, &[]);
    }

    /// [`Printer::block`] after a header clause whose further trailing
    /// comments are `carried` ([`split_trailing`]). On the next parse they lie
    /// between the header and a `{` that begins a line, so they are leading
    /// comments of the body's first node, or the body's trailing comments
    /// when it has none (2.3 rule C); they are printed there, below the `{`
    /// (rule 11).
    fn block_with(&mut self, b: &Block, level: usize, carried: &[Comment]) {
        let carried = self.comments(carried);
        self.push("{");
        if b.is_empty() && self.comments(&b.trailing_comments).is_empty() && carried.is_empty() {
            self.push("}");
            return;
        }
        self.newline();
        let saved = self.cont_indent.take();
        let mut first = true;
        for s in &b.statements {
            self.statement_unit(s, level + 1, first, if first { carried } else { &[] });
            first = false;
        }
        if let Some(tail) = &b.tail {
            self.node_prefix_with(
                &tail.trivia,
                level + 1,
                first,
                if first { carried } else { &[] },
            );
            let key = (UNIT_TAIL, tail as *const ExprStmt as usize, level + 1);
            self.fitted_unit(key, |p| {
                p.indent(level + 1);
                p.expr(&tail.expr);
                p.end_line(&tail.trivia.trailing, level + 1);
            });
            first = false;
        }
        if first {
            let mut comments = carried.to_vec();
            comments.extend(b.trailing_comments.iter().cloned());
            self.comment_lines(&comments, level + 1, true, true);
        } else {
            self.comment_lines(&b.trailing_comments, level + 1, false, true);
        }
        self.indent(level);
        self.push("}");
        self.cont_indent = saved;
    }

    fn statement_unit(&mut self, s: &Statement, level: usize, first: bool, carried: &[Comment]) {
        let t = s.trivia();
        self.node_prefix_with(t, level, first, carried);
        let key = (UNIT_STATEMENT, s as *const Statement as usize, level);
        self.fitted_unit(key, |p| {
            p.indent(level);
            p.statement(s, level);
            p.end_line(&t.trailing, level);
        });
    }

    fn statement(&mut self, s: &Statement, level: usize) {
        match s {
            Statement::Let(l) => {
                self.push("let ");
                match &l.binding {
                    LetBinding::Name(n) => self.push(&n.text),
                    LetBinding::Wildcard(_) => self.push("_"),
                }
                if let Some(t) = &l.ty {
                    self.push(": ");
                    self.ty(t);
                }
                self.push(" = ");
                self.expr(&l.value);
            }
            Statement::Var(v) => {
                self.push("var ");
                self.push(&v.name.text);
                if let Some(t) = &v.ty {
                    self.push(": ");
                    self.ty(t);
                }
                self.push(" = ");
                self.expr(&v.value);
            }
            Statement::Assign(a) => {
                self.push(&a.target.text);
                self.push(" = ");
                self.expr(&a.value);
            }
            Statement::Context(c) => {
                self.push("context ");
                self.push(&c.name.text);
                self.push(" = ");
                self.expr(&c.value);
            }
            Statement::Return(r) => {
                self.push("return");
                if let Some(v) = &r.value {
                    self.push(" ");
                    self.expr(v);
                }
            }
            Statement::Assert(a) => {
                self.push("assert ");
                self.expr(&a.condition);
            }
            Statement::While(w) => self.while_stmt(w, level),
            Statement::Concurrent(c) => {
                self.push("concurrent ");
                self.block(&c.body, level);
            }
            Statement::Expr(e) => self.expr(&e.expr),
        }
    }

    fn while_stmt(&mut self, w: &WhileStmt, level: usize) {
        self.push("while ");
        let broke = self.header_expr(&w.condition);
        let mut pending = Vec::new();
        if w.clauses.is_empty() {
            self.before_header_brace(level, broke);
        } else {
            self.newline();
            for c in w.invariants() {
                self.clause_unit(&c.trivia, level + 1, &mut pending, |p| {
                    p.push("invariant ");
                    p.expr(&c.expr);
                });
            }
            if let Some(d) = w.decreases() {
                self.clause_unit(&d.trivia, level + 1, &mut pending, |p| {
                    p.push("decreases ");
                    p.expr(&d.expr);
                });
            }
            self.indent(level);
        }
        self.block_with(&w.body, level, &pending);
    }

    /// A header clause on its own line (rule 7): leading comments without
    /// blank lines, the clause, and its first trailing comment. `pending`
    /// carries the further trailing comments ([`split_trailing`]) from one
    /// clause to the next: the previous clause's are printed here, above
    /// this clause's leading comments, where the parser attaches them (2.3
    /// rule D), and this clause's are left in `pending`, because where they
    /// belong depends on what follows the last clause (a body's `{`, the
    /// next operation, or the next declaration), which only the caller knows.
    fn clause_unit(
        &mut self,
        t: &Trivia,
        level: usize,
        pending: &mut Vec<Comment>,
        body: impl Fn(&mut Printer),
    ) {
        let carried = std::mem::take(pending);
        self.comment_lines(&carried, level, true, false);
        self.comment_lines(&t.leading, level, true, false);
        let (first, further) = split_trailing(&t.trailing);
        let key = (UNIT_CLAUSE, t as *const Trivia as usize, level);
        self.fitted_unit(key, |p| {
            p.indent(level);
            body(p);
            p.end_line(first, level);
        });
        pending.extend(self.comments(further).iter().cloned());
    }

    // -- declarations ------------------------------------------------------------

    /// One annotation line. `all_trailing`: print every trailing comment;
    /// otherwise only the first, the declaration printing the others among
    /// its leading comments ([`Printer::declaration_prefix`]).
    fn annotation(&mut self, a: &Annotation, level: usize, all_trailing: bool) {
        let trailing = if all_trailing {
            &a.trivia.trailing[..]
        } else {
            split_trailing(&a.trivia.trailing).0
        };
        self.fitted(|p| {
            p.indent(level);
            p.push("@");
            p.push(&a.name.text);
            // T14: `@name()` keeps its (empty) argument list.
            if a.parens {
                let elems: Vec<Elem> = a.args.iter().map(Elem::Literal).collect();
                p.group(
                    &GroupSpec {
                        id: a.span,
                        multiline: a.group.multiline,
                        open: "(",
                        close: ")",
                        spaced: false,
                        dangling: NO_COMMENTS,
                    },
                    &elems,
                );
            }
            p.end_line(trailing, level);
        });
    }

    /// A declaration from its doc comments to its last line (the comments
    /// above it are [`Printer::declaration_prefix`]). Returns its further
    /// trailing comments ([`split_trailing`]) for the caller, which prints
    /// them where the next parse attaches them: above the next declaration,
    /// after the separating blank line, or at the end of the file (2.3 rules
    /// D and F).
    fn declaration(&mut self, d: &Declaration) -> Vec<Comment> {
        let level = 0;
        if let Some(docs) = &d.docs {
            self.docs(docs, level);
        }
        for a in &d.annotations {
            self.annotation(a, level, false);
        }
        let (first, further) = split_trailing(&d.trivia.trailing);
        match &d.kind {
            DeclKind::Type(t) => self.fitted(|p| {
                p.indent(level);
                if t.is_pub {
                    p.push("pub ");
                }
                p.push("type ");
                p.push(&t.name.text);
                match &t.kind {
                    TypeDeclKind::Wrapper(w) => {
                        p.push("(");
                        p.ty(&w.payload);
                        p.push(")");
                    }
                    TypeDeclKind::Refinement(r) => {
                        p.push(" = ");
                        p.ty(&r.base);
                        p.push(" where ");
                        p.expr(&r.predicate);
                    }
                    TypeDeclKind::Opaque(o) => {
                        if let Some(g) = &o.generic_params {
                            p.generic_params(g);
                        }
                    }
                }
                p.end_line(first, level);
            }),
            DeclKind::Struct(s) => {
                self.struct_body(s, &s.name.text, true, level);
                if let Some(inv) = &s.invariant {
                    self.end_line(first, level);
                    let mut pending = Vec::new();
                    self.clause_unit(&inv.trivia, level + 1, &mut pending, |p| {
                        p.push("where ");
                        p.expr(&inv.expr);
                    });
                    self.comment_lines(&pending, level + 1, false, false);
                } else {
                    self.end_line(first, level);
                }
            }
            DeclKind::Enum(e) => {
                self.enum_body(e, &e.name.text, true, level);
                self.end_line(first, level);
            }
            DeclKind::Capability(c) => {
                self.capability_body(c, &c.name.text, true, level);
                self.end_line(first, level);
            }
            DeclKind::Fn(f) => return self.fn_decl(d, f, level),
            DeclKind::Test(t) => self.fitted(|p| {
                p.indent(level);
                p.push("test ");
                p.text(&t.name);
                p.push(" ");
                p.block(&t.body, level);
                p.end_line(first, level);
            }),
            DeclKind::Property(pr) => {
                self.fitted(|p| {
                    p.indent(level);
                    p.push("property ");
                    p.text(&pr.name);
                    p.newline();
                });
                let mut pending = Vec::new();
                for fc in &pr.for_clauses {
                    self.for_clause(fc, level + 1, &mut pending);
                }
                self.indent(level);
                self.block_with(&pr.body, level, &pending);
                self.end_line(first, level);
            }
        }
        self.comments(further).to_vec()
    }

    /// `for x in <gen>` on its own line (a clause, rule 7).
    fn for_clause(&mut self, fc: &ForClause, level: usize, pending: &mut Vec<Comment>) {
        self.clause_unit(&fc.trivia, level, pending, |p| {
            p.push("for ");
            p.push(&fc.binder.text);
            p.push(" in ");
            p.expr(&fc.range);
        });
    }

    /// `name<T> {` for struct, enum, and capability bodies.
    fn body_header(
        &mut self,
        is_pub: bool,
        keyword: &str,
        name: &str,
        generics: Option<&GenericParams>,
        level: usize,
    ) {
        self.indent(level);
        if is_pub {
            self.push("pub ");
        }
        self.push(keyword);
        self.push(" ");
        self.push(name);
        if let Some(g) = generics {
            self.generic_params(g);
        }
        self.push(" {");
    }

    /// A struct from its header to its `}` (the cursor is left after it),
    /// named `name`; `member_docs` prints the fields' doc comments (left out
    /// of the signature facet, T25).
    fn struct_body(&mut self, s: &StructDecl, name: &str, member_docs: bool, level: usize) {
        self.body_header(
            s.is_pub,
            if s.is_program { "program" } else { "struct" },
            name,
            s.generic_params.as_ref(),
            level,
        );
        if s.fields.is_empty() && self.comments(&s.trailing_comments).is_empty() {
            self.push("}");
            return;
        }
        self.newline();
        for (i, f) in s.fields.iter().enumerate() {
            self.node_prefix(&f.trivia, level + 1, i == 0);
            if member_docs {
                if let Some(docs) = &f.docs {
                    self.docs(docs, level + 1);
                }
            }
            self.fitted(|p| {
                p.indent(level + 1);
                p.push(&f.name.text);
                p.push(": ");
                p.ty(&f.ty);
                p.end_line(&f.trivia.trailing, level + 1);
            });
        }
        self.comment_lines(&s.trailing_comments, level + 1, s.fields.is_empty(), true);
        self.indent(level);
        self.push("}");
    }

    /// An enum from its header to its `}`; see [`Printer::struct_body`].
    fn enum_body(&mut self, e: &EnumDecl, name: &str, member_docs: bool, level: usize) {
        self.body_header(e.is_pub, "enum", name, e.generic_params.as_ref(), level);
        if e.variants.is_empty() && self.comments(&e.trailing_comments).is_empty() {
            self.push("}");
            return;
        }
        self.newline();
        for (i, v) in e.variants.iter().enumerate() {
            self.node_prefix(&v.trivia, level + 1, i == 0);
            if member_docs {
                if let Some(docs) = &v.docs {
                    self.docs(docs, level + 1);
                }
            }
            self.fitted(|p| {
                p.indent(level + 1);
                p.push(&v.name.text);
                if !v.payload.is_empty() {
                    let elems: Vec<Elem> = v.payload.iter().map(Elem::Type).collect();
                    p.group(
                        &GroupSpec {
                            id: v.span,
                            multiline: v.group.multiline,
                            open: "(",
                            close: ")",
                            spaced: false,
                            dangling: NO_COMMENTS,
                        },
                        &elems,
                    );
                }
                p.end_line(&v.trivia.trailing, level + 1);
            });
        }
        self.comment_lines(&e.trailing_comments, level + 1, e.variants.is_empty(), true);
        self.indent(level);
        self.push("}");
    }

    /// A capability from its header to its `}`. `full`: the declaration,
    /// with the operations' doc comments and contract clauses; otherwise the
    /// signature facet, operation signatures only (5.2, T25).
    fn capability_body(&mut self, c: &CapabilityDecl, name: &str, full: bool, level: usize) {
        self.body_header(
            c.is_pub,
            if c.is_interface {
                "interface"
            } else {
                "capability"
            },
            name,
            None,
            level,
        );
        if c.operations.is_empty() && self.comments(&c.trailing_comments).is_empty() {
            self.push("}");
            return;
        }
        self.newline();
        for (i, op) in c.operations.iter().enumerate() {
            self.node_prefix(&op.trivia, level + 1, i == 0);
            if full {
                if let Some(docs) = &op.docs {
                    self.docs(docs, level + 1);
                }
            }
            let with_clauses = full && !op.clauses.is_empty();
            let header_trailing: &[Comment] = if with_clauses {
                NO_COMMENTS
            } else {
                &op.trivia.trailing
            };
            self.fitted(|p| {
                p.indent(level + 1);
                p.op_signature(op);
                p.end_line(header_trailing, level + 1);
            });
            if with_clauses {
                let clauses: Vec<&ContractClause> = op.clauses.iter().collect();
                let mut pending = Vec::new();
                self.contract_clauses(&clauses, level + 2, &mut pending);
                // The last clause's further trailing comments (and the
                // operation's own) lie between this operation and the next
                // on the next parse: the next operation's leading comments
                // or the body's trailing comments (2.3 rules D and E), both
                // printed at the operations' indentation.
                pending.extend(self.comments(&op.trivia.trailing).iter().cloned());
                self.comment_lines(&pending, level + 1, true, false);
            }
        }
        self.comment_lines(
            &c.trailing_comments,
            level + 1,
            c.operations.is_empty(),
            true,
        );
        self.indent(level);
        self.push("}");
    }

    /// `fn name(params) -> Ret` of a capability operation.
    fn op_signature(&mut self, op: &OpDecl) {
        self.push("fn ");
        self.push(&op.name.text);
        self.params(&op.params);
        self.push(" -> ");
        self.ty(&op.ret);
    }

    /// `requires` clauses in source order, then `ensures` clauses in source
    /// order (rule 7).
    fn contract_clauses(
        &mut self,
        clauses: &[&ContractClause],
        level: usize,
        pending: &mut Vec<Comment>,
    ) {
        for kind in [ContractKind::Requires, ContractKind::Ensures] {
            for c in clauses.iter().filter(|c| c.kind == kind) {
                self.clause_unit(&c.trivia, level, pending, |p| {
                    p.push(kind.keyword());
                    p.push(" ");
                    p.expr(&c.expr);
                });
            }
        }
    }

    /// Rule 7. A bodiless function (grammar 3.9, the standard library's
    /// stub form) prints its header and clauses alone. Returns the further
    /// trailing comments, as [`Printer::declaration`] does.
    fn fn_decl(&mut self, d: &Declaration, f: &FnDecl, level: usize) -> Vec<Comment> {
        let (first, further) = split_trailing(&d.trivia.trailing);
        let header_complete = self.fitted(|p| {
            p.indent(level);
            if f.is_pub {
                p.push("pub ");
            }
            p.push(f.mode.prefix());
            p.push(" ");
            p.push(&f.name.text);
            if let Some(g) = &f.generic_params {
                p.generic_params(g);
            }
            let params_broken = p.params(&f.params);
            p.push(" -> ");
            p.ty(&f.ret);
            match &f.body {
                Some(body) if f.clauses.is_empty() && !params_broken => {
                    p.push(" ");
                    p.block(body, level);
                    p.end_line(first, level);
                    true
                }
                None if f.clauses.is_empty() => {
                    p.end_line(first, level);
                    true
                }
                _ => {
                    p.newline();
                    false
                }
            }
        });
        if header_complete {
            return self.comments(further).to_vec();
        }
        let mut pending = Vec::new();
        if let Some(e) = f.effects() {
            self.clause_unit(&e.trivia, level + 1, &mut pending, |p| p.effects_set(e));
        }
        let contract: Vec<&ContractClause> = f.contract().collect();
        self.contract_clauses(&contract, level + 1, &mut pending);
        if let Some(dc) = f.decreases() {
            self.clause_unit(&dc.trivia, level + 1, &mut pending, |p| {
                p.push("decreases ");
                p.expr(&dc.expr);
            });
        }
        match &f.body {
            Some(body) => {
                self.indent(level);
                self.block_with(body, level, &pending);
                self.end_line(first, level);
                self.comments(further).to_vec()
            }
            // The declaration ends with its last clause: the clause's further
            // trailing comments and the declaration's own trailing comments
            // (which the parser never produces here: a comment on the last
            // clause's line is the clause's) all follow it on lines of their
            // own, so they go to the caller with the declaration's.
            None => {
                pending.extend(self.comments(&d.trivia.trailing).iter().cloned());
                pending
            }
        }
    }

    // -- facet fragments (5.2) -----------------------------------------------------

    /// The `signature` facet: the declaration with its name replaced by `_`
    /// (T24), without bodies, contracts, or member doc comments (T25).
    fn signature(&mut self, d: &Declaration) {
        match &d.kind {
            DeclKind::Fn(f) => self.fitted(|p| {
                p.indent(0);
                if f.is_pub {
                    p.push("pub ");
                }
                p.push(f.mode.prefix());
                p.push(" _");
                if let Some(g) = &f.generic_params {
                    p.generic_params(g);
                }
                p.params(&f.params);
                p.push(" -> ");
                p.ty(&f.ret);
            }),
            DeclKind::Type(t) => {
                self.indent(0);
                if t.is_pub {
                    self.push("pub ");
                }
                self.push("type _");
                match &t.kind {
                    TypeDeclKind::Wrapper(w) => {
                        self.push("(");
                        self.ty(&w.payload);
                        self.push(")");
                    }
                    TypeDeclKind::Refinement(r) => {
                        self.push(" = ");
                        self.ty(&r.base);
                    }
                    TypeDeclKind::Opaque(o) => {
                        if let Some(g) = &o.generic_params {
                            self.generic_params(g);
                        }
                    }
                }
            }
            DeclKind::Struct(s) => self.struct_body(s, "_", false, 0),
            DeclKind::Enum(e) => self.enum_body(e, "_", false, 0),
            DeclKind::Capability(c) => self.capability_body(c, "_", false, 0),
            DeclKind::Property(pr) => {
                let mut pending = Vec::new();
                for fc in &pr.for_clauses {
                    self.for_clause(fc, 0, &mut pending);
                }
                self.comment_lines(&pending, 0, true, false);
            }
            // A test has no signature facet (5.1).
            DeclKind::Test(_) => {}
        }
    }

    /// The `contract` facet (5.2, T23).
    fn contract(&mut self, d: &Declaration) {
        match &d.kind {
            DeclKind::Struct(s) if s.invariant.is_some() => self.fitted(|p| {
                p.indent(0);
                p.push("where ");
                p.expr(&s.invariant.as_ref().unwrap().expr);
            }),
            DeclKind::Fn(f) => {
                let clauses: Vec<&ContractClause> = f.contract().collect();
                let mut pending = Vec::new();
                self.contract_clauses(&clauses, 0, &mut pending);
                if let Some(dc) = f.decreases() {
                    self.clause_unit(&dc.trivia, 0, &mut pending, |p| {
                        p.push("decreases ");
                        p.expr(&dc.expr);
                    });
                }
                self.comment_lines(&pending, 0, true, false);
            }
            DeclKind::Type(TypeDecl {
                kind: TypeDeclKind::Refinement(r),
                ..
            }) => self.fitted(|p| {
                p.indent(0);
                p.push("where ");
                p.expr(&r.predicate);
            }),
            DeclKind::Capability(c) => {
                for op in c.operations.iter().filter(|op| !op.clauses.is_empty()) {
                    self.indent(0);
                    self.push("fn ");
                    self.push(&op.name.text);
                    self.newline();
                    let clauses: Vec<&ContractClause> = op.clauses.iter().collect();
                    let mut pending = Vec::new();
                    self.contract_clauses(&clauses, 1, &mut pending);
                    self.comment_lines(&pending, 1, true, false);
                }
            }
            // No contract facet (5.1).
            _ => {}
        }
    }

    fn use_decl(&mut self, u: &UseDecl, level: usize) {
        self.fitted(|p| {
            p.indent(level);
            p.push("use ");
            p.push(&u.path.dotted());
            p.push(".");
            if u.braced {
                let elems: Vec<Elem> = u.items.iter().map(Elem::UseItem).collect();
                p.group(
                    &GroupSpec {
                        id: u.span,
                        multiline: u.group.multiline,
                        open: "{",
                        close: "}",
                        spaced: false,
                        dangling: &u.trailing_comments,
                    },
                    &elems,
                );
            } else if let Some(item) = u.items.first() {
                p.push(&item.name.text);
            }
            p.end_line(&u.trivia.trailing, level);
        });
    }

    // -- files -------------------------------------------------------------------

    fn module(&mut self, m: &Module) {
        self.comment_lines(&m.trivia.leading, 0, true, true);
        if let Some(docs) = &m.docs {
            self.docs(docs, 0);
        }
        self.indent(0);
        self.push("module ");
        self.push(&m.header.path.dotted());
        self.end_line(&m.trivia.trailing, 0);
        if !m.uses.is_empty() {
            // Rule 3 puts one blank line before the first `use`; rule 4
            // separates the others.
            self.blank_line();
            for (i, u) in m.uses.iter().enumerate() {
                self.prefix(&u.trivia, 0, i == 0, true);
                self.use_decl(u, 0);
            }
        }
        if !m.declarations.is_empty() {
            // Rule 3: one blank line before the first declaration and
            // exactly one between declarations.
            self.blank_line();
            let mut further: Vec<Comment> = Vec::new();
            for (i, d) in m.declarations.iter().enumerate() {
                // The previous declaration's further trailing comments
                // ([`split_trailing`]) are this declaration's leading
                // comments on the next parse (2.3 rule D), so they print
                // after the separating blank line, above this one; like any
                // comment between two `type` declarations they also end
                // their adjacency (T51).
                if i > 0 && (!further.is_empty() || !stays_adjacent(&m.declarations[i - 1], d)) {
                    self.blank_line();
                }
                self.declaration_prefix(d, &further);
                further = self.declaration(d);
            }
            // After the last declaration they are the file's trailing
            // comments on the next parse (2.3 rule F), printed directly below.
            self.comment_lines(&further, 0, false, true);
        }
        self.comment_lines(&m.trailing_comments, 0, false, true);
    }

    fn work_file(&mut self, w: &WorkFile) {
        for (i, u) in w.uses.iter().enumerate() {
            self.node_prefix(&u.trivia, 0, i == 0);
            self.use_decl(u, 0);
        }
        for (i, c) in w.changes.iter().enumerate() {
            if i > 0 || !w.uses.is_empty() {
                self.blank_line();
            }
            self.prefix(&c.trivia, 0, true, true);
            self.change(c);
        }
        self.comment_lines(&w.trailing_comments, 0, false, true);
    }

    fn change(&mut self, c: &ChangeDecl) {
        let level = 0;
        if let Some(docs) = &c.docs {
            self.docs(docs, level);
        }
        self.indent(level);
        self.push("change ");
        self.push(&c.name.text);
        self.push(" {");
        if c.items.is_empty() && c.trailing_comments.is_empty() {
            self.push("}");
            self.end_line(&c.trivia.trailing, level);
            return;
        }
        self.newline();
        let rank = |item: &ChangeItem| match item.kind {
            ChangeItemKind::Base(_) => 0,
            ChangeItemKind::Target(_) => 1,
            ChangeItemKind::Workers(_) => 2,
            ChangeItemKind::Task(_) => 3,
            ChangeItemKind::Integrate(_) => 4,
        };
        let mut ordered: Vec<&ChangeItem> = c.items.iter().collect();
        ordered.sort_by_key(|item| rank(item));
        for (i, item) in ordered.iter().enumerate() {
            self.node_prefix(&item.trivia, level + 1, i == 0);
            self.change_item(item, level + 1);
        }
        self.comment_lines(&c.trailing_comments, level + 1, c.items.is_empty(), true);
        self.indent(level);
        self.push("}");
        self.end_line(&c.trivia.trailing, level);
    }

    fn change_item(&mut self, item: &ChangeItem, level: usize) {
        match &item.kind {
            ChangeItemKind::Base(g) | ChangeItemKind::Target(g) => {
                let keyword = if matches!(item.kind, ChangeItemKind::Base(_)) {
                    "base"
                } else {
                    "target"
                };
                self.indent(level);
                self.push(keyword);
                self.push(" git(");
                self.text(&g.reference);
                self.push(")");
                self.end_line(&item.trivia.trailing, level);
            }
            ChangeItemKind::Workers(w) => {
                self.indent(level);
                self.push("workers ");
                self.push(&w.digits);
                self.end_line(&item.trivia.trailing, level);
            }
            ChangeItemKind::Task(t) => self.task(item, t, level),
            ChangeItemKind::Integrate(d) => {
                self.indent(level);
                self.push("integrate {");
                if d.requirements.is_empty() && d.trailing_comments.is_empty() {
                    self.push("}");
                    self.end_line(&item.trivia.trailing, level);
                    return;
                }
                self.newline();
                for (i, r) in d.requirements.iter().enumerate() {
                    self.node_prefix(&r.trivia, level + 1, i == 0);
                    self.require_item(r, level + 1);
                }
                self.comment_lines(
                    &d.trailing_comments,
                    level + 1,
                    d.requirements.is_empty(),
                    true,
                );
                self.indent(level);
                self.push("}");
                self.end_line(&item.trivia.trailing, level);
            }
        }
    }

    fn task(&mut self, item: &ChangeItem, t: &TaskDecl, level: usize) {
        if let Some(docs) = &t.docs {
            self.docs(docs, level);
        }
        self.indent(level);
        self.push("task ");
        self.push(&t.name.text);
        if let Some(after) = &t.after {
            self.push(" after ");
            for (i, n) in after.tasks.iter().enumerate() {
                if i > 0 {
                    self.push(", ");
                }
                self.push(&n.text);
            }
        }
        self.push(" {");
        if t.items.is_empty() && t.trailing_comments.is_empty() {
            self.push("}");
            self.end_line(&item.trivia.trailing, level);
            return;
        }
        self.newline();
        let rank = |ti: &TaskItem| match ti.kind {
            TaskItemKind::Write(_) => 0,
            TaskItemKind::Add(_) => 1,
            TaskItemKind::Depend(_) => 2,
            TaskItemKind::Preserve(_) => 3,
            TaskItemKind::Goal(_) => 4,
        };
        let mut ordered: Vec<&TaskItem> = t.items.iter().collect();
        ordered.sort_by_key(|ti| rank(ti));
        for (i, ti) in ordered.iter().enumerate() {
            self.node_prefix(&ti.trivia, level + 1, i == 0);
            self.fitted(|p| {
                p.indent(level + 1);
                p.push(ti.kind.keyword());
                p.push(" ");
                match &ti.kind {
                    TaskItemKind::Write(set)
                    | TaskItemKind::Add(set)
                    | TaskItemKind::Depend(set)
                    | TaskItemKind::Preserve(set) => p.selector_set(set),
                    TaskItemKind::Goal(text) => p.text(text),
                }
                p.end_line(&ti.trivia.trailing, level + 1);
            });
        }
        self.comment_lines(&t.trailing_comments, level + 1, t.items.is_empty(), true);
        self.indent(level);
        self.push("}");
        self.end_line(&item.trivia.trailing, level);
    }

    fn require_item(&mut self, r: &RequireItem, level: usize) {
        let inner_selector = match &r.requirement.kind {
            RequirementKind::Prove(s) | RequirementKind::Test(s) => Some(s),
            _ => None,
        };
        if let Some(s) = inner_selector {
            self.comment_lines(&s.trivia.leading, level, true, true);
        }
        self.indent(level);
        self.push("require ");
        match &r.requirement.kind {
            RequirementKind::Typecheck => self.push("typecheck"),
            RequirementKind::NoHoles => self.push("no_holes"),
            RequirementKind::NoUnresolvedAssumptions => self.push("no_unresolved_assumptions"),
            RequirementKind::Prove(s) => {
                self.push("prove(");
                self.selector(s);
                self.push(")");
            }
            RequirementKind::Test(s) => {
                self.push("test(");
                self.selector(s);
                self.push(")");
            }
            RequirementKind::Review(n) => {
                self.push("review(");
                self.push(&n.text);
                self.push(")");
            }
        }
        let mut trailing: Vec<Comment> = Vec::new();
        if let Some(s) = inner_selector {
            trailing.extend(s.trivia.trailing.iter().cloned());
        }
        trailing.extend(r.trivia.trailing.iter().cloned());
        self.end_line(&trailing, level);
    }

    fn patch_file(&mut self, p: &PatchFile) {
        for (i, form) in p.forms.iter().enumerate() {
            if i > 0 {
                self.blank_line();
            }
            self.prefix(&form.trivia, 0, true, true);
            if let Some(docs) = &form.docs {
                self.docs(docs, 0);
            }
            self.comment_lines(&form.target.trivia.leading, 0, true, true);
            self.indent(0);
            self.push("patch ");
            self.selector(&form.target);
            self.end_line(&form.target.trivia.trailing, 0);
            self.comment_lines(&form.expect.trivia.leading, 1, true, false);
            self.comment_lines(&form.expect.base.trivia.leading, 1, true, false);
            self.indent(1);
            self.push("expect base(");
            self.selector(&form.expect.base);
            self.push(")");
            let mut trailing: Vec<Comment> = form.expect.base.trivia.trailing.clone();
            trailing.extend(form.expect.trivia.trailing.iter().cloned());
            // Further trailing comments of the `expect` line are leading
            // comments of the body's first node on the next parse (2.3 rule
            // C), so they print below the `{`.
            let (first, further) = split_trailing(&trailing);
            self.end_line(first, 1);
            self.indent(0);
            self.block_with(&form.body, 0, further);
            self.end_line(&form.trivia.trailing, 0);
        }
        self.comment_lines(&p.trailing_comments, 0, false, true);
    }
}
