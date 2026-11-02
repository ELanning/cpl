//! Stable identities (`spec/toolchain.md` 4): the kind of an identity
//! (4.1), generated `@id` values (4.1), and `cpl id assign` (4.2) as a
//! plan ([`plan_id_assignments`]) and a byte-exact text edit
//! ([`apply_id_assignments`]). Provisional identities (4.3) are derived by
//! [`StableId::provisional`]; the identity of a declaration is
//! [`crate::declaration_identity`].

use crate::facet::{declaration_keyword_start, symbol_path};
use crate::source::{module_path, project_modules};
use cpl_diag::json::IdAssignmentJson;
use cpl_diag::{quote_text, FileId, SourceMap};
use cpl_project::{ParsedFile, Project};
use cpl_syntax::ast;
use std::collections::BTreeSet;
use std::fmt;

pub use cpl_diag::StableId;

/// The kind of a declaration's identity (Section 4.1): the prefix of a
/// generated `@id`, and the `kind` of every symbol block, diff statement,
/// and id assignment (`"fn"`, `"ty"`, `"cap"`, `"test"`, `"prop"`).
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub enum IdKind {
    Fn,
    Ty,
    Cap,
    Test,
    Prop,
}

impl IdKind {
    /// The five kinds, in the order of the Section 4.1 table.
    pub const ALL: [IdKind; 5] = [
        IdKind::Fn,
        IdKind::Ty,
        IdKind::Cap,
        IdKind::Test,
        IdKind::Prop,
    ];

    /// `type`/`struct`/`enum` -> `Ty`, `capability` -> `Cap`, `fn` -> `Fn`,
    /// `test` -> `Test`, `property` -> `Prop`.
    pub fn for_decl(kind: &ast::DeclKind) -> IdKind {
        match kind {
            ast::DeclKind::Type(_) | ast::DeclKind::Struct(_) | ast::DeclKind::Enum(_) => {
                IdKind::Ty
            }
            ast::DeclKind::Capability(_) => IdKind::Cap,
            ast::DeclKind::Fn(_) => IdKind::Fn,
            ast::DeclKind::Test(_) => IdKind::Test,
            ast::DeclKind::Property(_) => IdKind::Prop,
        }
    }

    /// `"fn"`, `"ty"`, `"cap"`, `"test"`, `"prop"`.
    pub fn prefix(self) -> &'static str {
        match self {
            IdKind::Fn => "fn",
            IdKind::Ty => "ty",
            IdKind::Cap => "cap",
            IdKind::Test => "test",
            IdKind::Prop => "prop",
        }
    }

    /// The kind with this prefix.
    pub fn from_prefix(prefix: &str) -> Option<IdKind> {
        IdKind::ALL.into_iter().find(|k| k.prefix() == prefix)
    }
}

impl fmt::Display for IdKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.prefix())
    }
}

/// RFC 4648 base32, lowercase, no padding.
const BASE32: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
/// Random bytes per generated id: 80 bits, exactly 16 base32 characters.
const ID_BYTES: usize = 10;
/// Base32 characters per generated id.
const ID_CHARS: usize = 16;

/// A fresh identity of `kind` (Section 4.1): 10 bytes drawn from `rng`
/// (`fill_bytes`), encoded as 16 base32 characters (80 bits, no padding,
/// most significant bit first), after the kind prefix and `_`:
/// `fn_mzxw6ytbmzxw6ytb` for the bytes `foobafooba`.
///
/// `rng` is a `rand::Rng`, the infallible generator trait of rand 0.10
/// (Section 1.8 names it `RngCore`, its rand 0.9 name, which rand 0.10 no
/// longer exports at its root): a seeded `rand_chacha::ChaCha20Rng` in
/// tests; `rand::rng()` or `rand::rand_core::UnwrapErr(rand::rngs::SysRng)`
/// (rand 0.10's `OsRng`) in the CLI.
pub fn generate_id(kind: IdKind, rng: &mut dyn rand::Rng) -> StableId {
    let mut bytes = [0u8; ID_BYTES];
    rng.fill_bytes(&mut bytes);
    let mut bits = bytes
        .iter()
        .fold(0u128, |acc, &b| (acc << 8) | u128::from(b));
    let mut encoded = [0u8; ID_CHARS];
    for slot in encoded.iter_mut().rev() {
        *slot = BASE32[(bits & 31) as usize];
        bits >>= 5;
    }
    let mut text = String::with_capacity(kind.prefix().len() + 1 + ID_CHARS);
    text.push_str(kind.prefix());
    text.push('_');
    text.extend(encoded.iter().map(|&c| char::from(c)));
    StableId::from_annotation(&text).expect("a generated id has the accepted @id syntax")
}

/// True when `id` has the *generated format* of Section 4.1,
/// `^(fn|ty|cap|test|prop)_[a-z2-7]{16}$`, which is stricter than the
/// accepted `@id` syntax: `pgn_7c2d` is accepted but not generated, and so
/// is a provisional identity. This is what `id_kind_mismatch` (T21) tests.
pub fn is_generated_format(id: &StableId) -> bool {
    let Some((prefix, rest)) = id.as_str().split_once('_') else {
        return false;
    };
    IdKind::from_prefix(prefix).is_some()
        && rest.len() == ID_CHARS
        && rest.bytes().all(|c| BASE32.contains(&c))
}

/// One `@id` line `cpl id assign` inserts (Section 4.2).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct IdAssignment {
    pub file: FileId,
    /// The display symbol path of the declaration (Section 4.4).
    pub path: String,
    pub kind: IdKind,
    pub id: StableId,
    /// Byte offset of the declaration's first annotation or, if it has none,
    /// of `pub` or its keyword (after its doc comments).
    pub insert_at: u32,
    /// The horizontal whitespace before `insert_at` on its line, the
    /// declaration's indentation: `""` for a declaration at column 0, as in
    /// every formatted file. The inserted line goes before it, at column 0
    /// (Section 4.2 step 3), so the declaration's line keeps every byte.
    pub indent: String,
}

impl IdAssignment {
    /// The `assignments` entry of `cpl id assign --json` (Section 7.2): the
    /// file's project-relative path, the 1-based line of `insert_at` (the
    /// declaration's line before the insertion, `src/reader/pages.cpl:21`
    /// for `clean`), the symbol path, the kind, and the id.
    pub fn to_json(&self, source_map: &SourceMap) -> IdAssignmentJson {
        IdAssignmentJson {
            path: source_map.file(self.file).path.clone(),
            line: source_map.line_col(self.file, self.insert_at).line,
            symbol: self.path.clone(),
            kind: self.kind.prefix().to_string(),
            id: self.id.clone(),
        }
    }
}

/// One assignment per top-level declaration of the source set whose
/// annotations contain no `id`, in (file path, source order) order
/// (Section 4.2 step 2). Std files are never touched and files that failed
/// to parse are skipped (the caller reports their diagnostics). Every id is
/// generated by [`generate_id`] from `rng`, one draw per declaration, so the
/// plan is deterministic given `rng`; an id that equals an `@id` value
/// already written in the project or planned earlier is drawn again, so no
/// id is ever reused.
pub fn plan_id_assignments(project: &Project, rng: &mut dyn rand::Rng) -> Vec<IdAssignment> {
    let mut taken: BTreeSet<String> = BTreeSet::new();
    for parsed in &project.parsed {
        if let ParsedFile::Module(module) = parsed {
            for decl in &module.declarations {
                for annotation in decl.annotations.iter().filter(|a| a.name.text == "id") {
                    for arg in &annotation.args {
                        if let ast::LiteralKind::Text(value) = &arg.kind {
                            taken.insert(value.clone());
                        }
                    }
                }
            }
        }
    }
    let mut plan = Vec::new();
    for file in project_modules(project) {
        let module = module_path(file.module);
        for decl in &file.module.declarations {
            if decl.annotations.iter().any(|a| a.name.text == "id") {
                continue;
            }
            let kind = IdKind::for_decl(&decl.kind);
            let id = loop {
                let id = generate_id(kind, rng);
                if taken.insert(id.as_str().to_string()) {
                    break id;
                }
            };
            let insert_at = decl
                .annotations
                .first()
                .map_or_else(|| declaration_keyword_start(decl), |a| a.span.start);
            plan.push(IdAssignment {
                file: file.file,
                path: symbol_path(&module, decl).display(),
                kind,
                id,
                insert_at,
                indent: indent_before(file.text, insert_at as usize).to_string(),
            });
        }
    }
    plan
}

/// The run of spaces and tabs that ends at `offset`. The first annotation
/// or keyword of a top-level declaration always starts its line apart from
/// such a run: declarations are separated by line terminators and an
/// annotation occupies its own line (grammar 3.3), and doc comments and
/// ordinary comments end at the end of their line.
fn indent_before(text: &str, offset: usize) -> &str {
    let line = &text[..offset];
    let start = line.trim_end_matches([' ', '\t']).len();
    &line[start..]
}

/// Insert one `@id("...")` line per assignment (Section 4.2 step 3): the
/// line `@id("<id>")` and a line feed, at column 0 of the line of each
/// `insert_at`, that is before the assignment's `indent`. "The inserted line
/// is at column 0. No other byte changes": the result differs from `text`
/// only by the inserted lines, and an indented declaration of an unformatted
/// file (indentation is never significant, grammar 1.1) keeps its own line
/// and its indentation below an `@id` line at column 0, which `cpl fmt` then
/// lays out with the rest. Every assignment given is applied to `text`; pass
/// the assignments of one file. Several assignments at one offset are
/// inserted in the order given.
///
/// Panics when the `indent` of an assignment is not the text that ends at
/// its `insert_at` (an assignment planned for another text).
pub fn apply_id_assignments(text: &str, assignments: &[IdAssignment]) -> String {
    let mut ordered: Vec<(usize, &IdAssignment)> = assignments
        .iter()
        .map(|a| (line_start(text, a), a))
        .collect();
    ordered.sort_by_key(|(at, _)| *at);
    let mut out = String::with_capacity(text.len() + assignments.len() * 32);
    let mut copied = 0;
    for (at, assignment) in ordered {
        out.push_str(&text[copied..at]);
        out.push_str("@id(");
        out.push_str(&quote_text(assignment.id.as_str()));
        out.push_str(")\n");
        copied = at;
    }
    out.push_str(&text[copied..]);
    out
}

/// Where the line of an assignment's `@id` goes: `insert_at` less the
/// assignment's `indent`, the start of the declaration's line.
fn line_start(text: &str, assignment: &IdAssignment) -> usize {
    let at = assignment.insert_at as usize;
    let start = at.checked_sub(assignment.indent.len());
    match start {
        Some(start) if text.get(start..at) == Some(assignment.indent.as_str()) => start,
        _ => panic!(
            "@id insertion for {}: the text before offset {at} is not its indent {:?}",
            assignment.path, assignment.indent
        ),
    }
}
