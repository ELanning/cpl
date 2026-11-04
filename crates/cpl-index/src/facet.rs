//! Facets and content versions (`spec/toolchain.md` 5): which facets a
//! declaration has (5.1), their canonical text (5.2) and content hash
//! (5.3), the `.definition` hash and the facet map of a revision (5.4), and
//! the per-module content hash of semantics S48.
//!
//! Everything here is purely syntactic: it reads `ast::Declaration`s, never
//! a typed program, so it works for revisions that do not typecheck, and
//! the `Program`-level versions of the semantic index are produced by these
//! same functions ([`symbol_version`]) on `Program::ast(def)`, so the two
//! never disagree.
//!
//! Canonical text is `cpl_syntax::format_fragment` in
//! [`FragmentMode::Canonical`]: no ordinary comments, no blank lines, no
//! source line breaks inside a clause, parentheses as written. Examples for
//! the corpus (`examples/reader/src/reader/pages.cpl`):
//!
//! | Declaration | Facet | Canonical text |
//! | - | - | - |
//! | `paginate` | `identity` | `pgn_7c2d` |
//! | `paginate` | `signature` | `pub fn _<T>(items: List<T>, size: Positive) -> List<List<T>>` |
//! | `paginate` | `contract` | `ensures flatten(result) == items` LF `ensures forall page in result: ...` LF ... |
//! | `paginate` | `effects` | (empty) |
//! | `paginate` | `verification_policy` | `@prove` |
//! | `Positive` | `signature` / `contract` | `pub type _ = Int` / `where self > 0` |
//! | `load_page` | `effects` | `effects { db.get }` |
//! | `clean` | `identity` | `reader.pages.clean` (provisional) |
//! | test `pagination handles empty input` | `name` | `"pagination handles empty input"` |

use crate::id::IdKind;
use crate::source::{digest, module_path, project_modules};
use cpl_diag::json::{Digest, FacetHash, Revision, SymbolVersion};
use cpl_diag::{DeclName, Facet, FileId, StableId, SymbolPath};
use cpl_project::{ParsedFile, Project};
use cpl_syntax::ast::{self, DeclKind, Declaration, Docs, LiteralKind, TypeDeclKind};
use cpl_syntax::{format_fragment, format_module, Fragment, FragmentMode};
use std::collections::{BTreeMap, BTreeSet};

// ---------------------------------------------------------------------------
// 5.1 Coverage
// ---------------------------------------------------------------------------

/// `fn`: all eight.
const FN_FACETS: [Facet; 8] = Facet::ALL;
/// A wrapper `type`, a std opaque `type`, `struct`, `enum`.
const DATA_FACETS: [Facet; 4] = [Facet::Identity, Facet::Name, Facet::Signature, Facet::Docs];
/// A refinement `type` (the `where` predicate) and a `capability` (its
/// operations' clauses) also have a `contract`.
const SPECIFIED_FACETS: [Facet; 5] = [
    Facet::Identity,
    Facet::Name,
    Facet::Signature,
    Facet::Contract,
    Facet::Docs,
];
/// `test`: no signature.
const TEST_FACETS: [Facet; 4] = [Facet::Identity, Facet::Name, Facet::Body, Facet::Docs];
/// `property`: its `for` clauses are its signature.
const PROPERTY_FACETS: [Facet; 5] = [
    Facet::Identity,
    Facet::Name,
    Facet::Signature,
    Facet::Body,
    Facet::Docs,
];

/// The facets a declaration of this kind has (Section 5.1), in canonical
/// order; every other facet is absent from its facet map.
///
/// The table's columns decide it, and so does nothing else: a std opaque
/// `type` (`pub type List<T>`) has the facets of a wrapper, and a bodiless
/// `fn` (a std stub) all eight, its `body` text being empty (a bodied `fn`
/// never has an empty body text: an empty block is `{}`).
pub fn applicable_facets(kind: &DeclKind) -> &'static [Facet] {
    match kind {
        DeclKind::Fn(_) => &FN_FACETS,
        DeclKind::Type(t) => match t.kind {
            TypeDeclKind::Refinement(_) => &SPECIFIED_FACETS,
            TypeDeclKind::Wrapper(_) | TypeDeclKind::Opaque(_) => &DATA_FACETS,
        },
        DeclKind::Struct(s) if s.invariant.is_some() => &SPECIFIED_FACETS,
        DeclKind::Struct(_) | DeclKind::Enum(_) => &DATA_FACETS,
        DeclKind::Capability(_) => &SPECIFIED_FACETS,
        DeclKind::Test(_) => &TEST_FACETS,
        DeclKind::Property(_) => &PROPERTY_FACETS,
    }
}

// ---------------------------------------------------------------------------
// Identity and names (4.3, 4.4)
// ---------------------------------------------------------------------------

/// A declaration's stable identity (semantics S2): its `@id` value when it
/// carries a well-formed `@id` (the first `@id` annotation, with exactly one
/// text argument of the accepted syntax of Section 4.1), otherwise its
/// provisional identity, the qualified name ([`symbol_path`] displayed). A
/// malformed `@id` (`@id("Foo")`, `@id(7)`, two arguments, no argument list)
/// is `invalid_annotation_argument` to the checker and identifies nothing.
pub fn declaration_identity(module_path: &[String], decl: &Declaration) -> StableId {
    annotated_id(decl).unwrap_or_else(|| {
        StableId::provisional(&module_path.join("."), &declaration_name(module_path, decl))
    })
}

/// The well-formed `@id` value of a declaration, if any.
fn annotated_id(decl: &Declaration) -> Option<StableId> {
    let annotation = decl.annotation("id")?;
    match annotation.args.as_slice() {
        [arg] => match &arg.kind {
            LiteralKind::Text(value) => StableId::from_annotation(value).ok(),
            _ => None,
        },
        _ => None,
    }
}

/// The qualified name of a declaration (Section 4.4, semantics 1.4):
/// `reader.pages.paginate`, `tests.pagination."pagination handles empty
/// input"`, and for a std `@method`/`@property` stub with a receiver
/// (stdlib S37) `std.prelude.List.take`, the receiver being the head of its
/// first parameter's type. A zero-parameter `@property` (`std.gen.int`,
/// stdlib S40) has no receiver. Outside `std` the two annotations are
/// `unknown_annotation` and change nothing.
pub fn symbol_path(module_path: &[String], decl: &Declaration) -> SymbolPath {
    SymbolPath::new(module_path.to_vec(), declaration_name(module_path, decl))
}

fn declaration_name(module_path: &[String], decl: &Declaration) -> DeclName {
    match &decl.kind {
        DeclKind::Test(t) => DeclName::Text(t.name.value.clone()),
        DeclKind::Property(p) => DeclName::Text(p.name.value.clone()),
        DeclKind::Fn(f) => {
            let in_std = module_path.first().is_some_and(|m| m == "std");
            let member = decl.has_annotation("method") || decl.has_annotation("property");
            match f.params.positional.first() {
                Some(receiver) if in_std && member => DeclName::Member {
                    receiver: receiver.ty.path.name.text.clone(),
                    name: f.name.text.clone(),
                },
                _ => DeclName::Ident(f.name.text.clone()),
            }
        }
        DeclKind::Type(t) => DeclName::Ident(t.name.text.clone()),
        DeclKind::Struct(s) => DeclName::Ident(s.name.text.clone()),
        DeclKind::Enum(e) => DeclName::Ident(e.name.text.clone()),
        DeclKind::Capability(c) => DeclName::Ident(c.name.text.clone()),
    }
}

/// The byte offset of `pub` or the declaration keyword: where its first
/// token after the doc comments and annotations starts (every kind's own
/// span starts there).
pub(crate) fn declaration_keyword_start(decl: &Declaration) -> u32 {
    match &decl.kind {
        DeclKind::Type(d) => d.span.start,
        DeclKind::Struct(d) => d.span.start,
        DeclKind::Enum(d) => d.span.start,
        DeclKind::Capability(d) => d.span.start,
        DeclKind::Fn(d) => d.span.start,
        DeclKind::Test(d) => d.span.start,
        DeclKind::Property(d) => d.span.start,
    }
}

// ---------------------------------------------------------------------------
// 5.2 Canonical text
// ---------------------------------------------------------------------------

/// Canonical text of one facet of a declaration (Section 5.2); `None` when
/// the facet does not apply to that kind ([`applicable_facets`]).
/// `module_path` is the file's `module` header, which the provisional
/// `identity` text needs (Section 4.3).
///
/// | Facet | Text |
/// | - | - |
/// | `identity` | the `@id` value, or the provisional id ([`declaration_identity`]) |
/// | `name` | the identifier; for a test or property its name as a canonically escaped text literal, quotes included |
/// | `signature` | `format_fragment(Signature)`: the header with the name replaced by `_` |
/// | `contract` | `format_fragment(Contract)`: `requires` lines, `ensures` lines, `decreases`; `where ...`; per-operation clauses; each clause begins an unindented line, and one with an `if` or `match` continues over the lines of its blocks (2.4 rule 8: 5.2's "every clause ... is one line" cannot hold for a block of `let` statements and a tail, semantics S32, grammar 2.3) |
/// | `effects` | `format_fragment(Effects)`: `effects { a.x, b.y }` sorted and deduplicated; empty without a clause or for an empty set |
/// | `body` | `format_fragment(Block)` at level 0; empty for a bodiless (std stub) `fn` |
/// | `docs` | the doc lines joined with LF, then for a struct, enum, or capability `LF member <name> LF <lines>` per documented member, in member order; empty without docs |
/// | `verification_policy` | `@prove` when the annotation is present, else empty |
pub fn facet_text(module_path: &[String], decl: &Declaration, facet: Facet) -> Option<String> {
    if !applicable_facets(&decl.kind).contains(&facet) {
        return None;
    }
    let canonical = |fragment: Fragment<'_>| format_fragment(fragment, FragmentMode::Canonical);
    Some(match facet {
        Facet::Identity => declaration_identity(module_path, decl).as_str().to_string(),
        Facet::Name => name_text(decl),
        Facet::Signature => {
            let mut text = canonical(Fragment::Signature(decl));
            append_metadata(
                &mut text,
                decl,
                &["protocol", "transition", "message", "schema", "migration"],
            );
            text
        }
        Facet::Contract => canonical(Fragment::Contract(decl)),
        Facet::Effects => match &decl.kind {
            DeclKind::Fn(f) => {
                let effects = f
                    .effects()
                    .map(|clause| canonical(Fragment::Effects(clause)))
                    .unwrap_or_default();
                if f.mode == ast::FunctionMode::Inferred {
                    effects
                } else if effects.is_empty() {
                    f.mode.prefix().to_string()
                } else {
                    format!("{}\n{effects}", f.mode.prefix())
                }
            }
            _ => String::new(),
        },
        Facet::Body => match &decl.kind {
            DeclKind::Fn(f) => f
                .body
                .as_ref()
                .map(|body| canonical(Fragment::Block(body)))
                .unwrap_or_default(),
            DeclKind::Test(t) => canonical(Fragment::Block(&t.body)),
            DeclKind::Property(p) => canonical(Fragment::Block(&p.body)),
            _ => String::new(),
        },
        Facet::Docs => docs_text(decl),
        Facet::VerificationPolicy => {
            if decl.has_annotation("prove") {
                "@prove".to_string()
            } else {
                String::new()
            }
        }
    })
}

/// The `name` facet: the identifier, or a test's or property's name as the
/// canonically escaped text literal (formatter rule 10), which is the
/// rendering of a `Text` literal expression holding it.
fn name_text(decl: &Declaration) -> String {
    let literal = |name: &ast::TextLiteral| {
        let expr = ast::Expr {
            span: name.span,
            kind: ast::ExprKind::Literal(ast::Literal {
                span: name.span,
                kind: LiteralKind::Text(name.value.clone()),
            }),
        };
        format_fragment(Fragment::Expr(&expr), FragmentMode::Canonical)
    };
    match &decl.kind {
        DeclKind::Test(t) => literal(&t.name),
        DeclKind::Property(p) => literal(&p.name),
        _ => decl.display_name().to_string(),
    }
}

/// The `docs` facet (5.2, T25), read literally: the declaration's doc text
/// (empty when it has none), followed for every member that has docs by LF,
/// `member <name>`, LF, and the member's doc text. A struct whose only docs
/// are on a field therefore has a text that starts with LF.
fn docs_text(decl: &Declaration) -> String {
    let fragment = |docs: &Docs| format_fragment(Fragment::Docs(docs), FragmentMode::Canonical);
    let mut text = decl.docs.as_ref().map(fragment).unwrap_or_default();
    let members: Vec<(&str, &Docs)> = match &decl.kind {
        DeclKind::Struct(s) => s
            .fields
            .iter()
            .filter_map(|f| f.docs.as_ref().map(|d| (f.name.text.as_str(), d)))
            .collect(),
        DeclKind::Enum(e) => e
            .variants
            .iter()
            .filter_map(|v| v.docs.as_ref().map(|d| (v.name.text.as_str(), d)))
            .collect(),
        DeclKind::Capability(c) => c
            .operations
            .iter()
            .filter_map(|op| op.docs.as_ref().map(|d| (op.name.text.as_str(), d)))
            .collect(),
        _ => Vec::new(),
    };
    for (name, docs) in members {
        text.push_str("\nmember ");
        text.push_str(name);
        text.push('\n');
        text.push_str(&fragment(docs));
    }
    append_metadata(&mut text, decl, &["intent", "preserve"]);
    text
}

fn append_metadata(text: &mut String, decl: &Declaration, names: &[&str]) {
    for name in names {
        if let Some(a) = decl.annotation(name) {
            let args: Vec<_> = a
                .args
                .iter()
                .filter_map(|v| match &v.kind {
                    LiteralKind::Text(t) => Some(t),
                    _ => None,
                })
                .collect();
            if !text.is_empty() {
                text.push('\n');
            }
            text.push('@');
            text.push_str(name);
            text.push('(');
            text.push_str(
                &args
                    .iter()
                    .map(|arg| serde_json::to_string(arg).expect("text metadata serializes"))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            text.push(')');
        }
    }
}

/// The effect references of a `fn`'s bound as `param.op` strings
/// ([`FacetEntry::effects`]); empty for every other kind.
fn effect_set(decl: &Declaration) -> BTreeSet<String> {
    match &decl.kind {
        DeclKind::Fn(f) => f
            .effects()
            .map(|clause| {
                clause
                    .effects
                    .iter()
                    .map(|e| format!("{}.{}", e.param.text, e.operation.text))
                    .collect()
            })
            .unwrap_or_default(),
        _ => BTreeSet::new(),
    }
}

// ---------------------------------------------------------------------------
// 5.3 Content hash, 5.4 definition hash
// ---------------------------------------------------------------------------

/// `lowercase_hex(blake3(utf8(text)))[0..32]` (Section 5.3): 128 bits. The
/// empty text hashes to `af1349b9f5f9a1a6a0404dea36dcc949`.
pub fn facet_hash(canonical_text: &str) -> FacetHash {
    let hex = blake3::hash(canonical_text.as_bytes()).to_hex();
    FacetHash(hex[..32].to_string())
}

/// The `.definition` hash (Section 5.4 step 2): [`facet_hash`] of one line
/// `<facet>=<hash>` per facet of the map in `Facet::ALL` order, joined with
/// LF. The map holds exactly the declaration's applicable facets.
pub fn definition_hash(facets: &BTreeMap<Facet, FacetHash>) -> FacetHash {
    let lines: Vec<String> = Facet::ALL
        .iter()
        .filter_map(|facet| {
            facets
                .get(facet)
                .map(|hash| format!("{}={}", facet.as_str(), hash.as_str()))
        })
        .collect();
    facet_hash(&lines.join("\n"))
}

/// A declaration's [`SymbolVersion`]: identity, display path, the hash of
/// every applicable facet, and the `.definition` hash. The one producer of
/// facet versions, for the facet map and for the semantic index alike.
pub fn symbol_version(module_path: &[String], decl: &Declaration) -> SymbolVersion {
    let id = declaration_identity(module_path, decl);
    let facets: BTreeMap<Facet, FacetHash> = applicable_facets(&decl.kind)
        .iter()
        .map(|&facet| {
            let text = facet_text(module_path, decl, facet).expect("the facet applies");
            (facet, facet_hash(&text))
        })
        .collect();
    let definition = definition_hash(&facets);
    SymbolVersion {
        provisional: id.is_provisional(),
        id,
        path: symbol_path(module_path, decl).display(),
        facets,
        definition,
    }
}

/// [`symbol_version`] memoized by the module path and the declaration's source text, which
/// determine its syntax and so every facet. Process-wide and bounded, so that snapshots of one
/// project that share most declarations (a base and its candidate, the sides of a merge) hash
/// each unchanged declaration once.
pub fn symbol_version_cached(
    module_path: &[String],
    decl: &Declaration,
    source: &str,
) -> SymbolVersion {
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<std::collections::HashMap<[u8; 32], SymbolVersion>>> =
        OnceLock::new();
    let mut h = blake3::Hasher::new();
    h.update(b"cpl-symbol-version-v1\0");
    for segment in module_path {
        h.update(segment.as_bytes());
        h.update(b".");
    }
    h.update(b"\0");
    h.update(source.as_bytes());
    let key = *h.finalize().as_bytes();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(v) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return v.clone();
    }
    let version = symbol_version(module_path, decl);
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() >= 200_000 {
        cache.clear();
    }
    cache.insert(key, version.clone());
    version
}

// ---------------------------------------------------------------------------
// 5.4 The facet map of a revision
// ---------------------------------------------------------------------------

/// One declaration of a facet map.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FacetEntry {
    /// `id`, `path`, `provisional`, `facets`, `definition`.
    pub version: SymbolVersion,
    pub kind: IdKind,
    /// The dotted module path: `"reader.pages"`.
    pub module: String,
    /// The declaration name, or a test's or property's name text (unquoted).
    pub name: String,
    /// The project-relative path of the declaring file.
    pub file: String,
    /// `fn` only: the bound's references as `param.op` (`"db.get"`); empty
    /// otherwise. The semantic diff reads effect widening and narrowing
    /// from these sets (T31).
    pub effects: BTreeSet<String>,
}

impl FacetEntry {
    /// The entry of `decl`, declared in `file` under the module header
    /// `module_path`.
    pub fn new(module_path: &[String], decl: &Declaration, file: &str) -> FacetEntry {
        FacetEntry {
            version: symbol_version(module_path, decl),
            kind: IdKind::for_decl(&decl.kind),
            module: module_path.join("."),
            name: decl.display_name().to_string(),
            file: file.to_string(),
            effects: effect_set(decl),
        }
    }

    /// The hash of one facet, `None` when it does not apply.
    pub fn facet(&self, facet: Facet) -> Option<&FacetHash> {
        self.version.facets.get(&facet)
    }
}

/// The facets of every declaration of one revision (Section 5.4).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FacetMap {
    pub revision: Revision,
    /// Keyed by stable id; for an id declared more than once, its first
    /// occurrence in (file path, declaration order).
    pub entries: BTreeMap<StableId, FacetEntry>,
    /// The ids declared more than once in this revision (Section 4.5), in id
    /// order, each with the file of every occurrence in (file path,
    /// declaration order), so a file appears once per declaration.
    pub duplicates: Vec<(StableId, Vec<String>)>,
}

impl FacetMap {
    /// The entry of `id`.
    pub fn get(&self, id: &StableId) -> Option<&FacetEntry> {
        self.entries.get(id)
    }
}

/// The facet map of a project tree (Section 5.4): for every parsed `.cpl`
/// file of the source set in path order and every declaration in order, its
/// [`FacetEntry`], keyed by stable id; a duplicate id keeps the first entry
/// and is recorded in [`FacetMap::duplicates`].
///
/// Purely syntactic: computed from `Project::parsed`, so it works for
/// revisions that do not typecheck, and is the same whether the project was
/// loaded from a directory or a git tree (Section 9.3). A revision is a state
/// of the source set (Section 0), so std, which the project's `parsed` also
/// holds, is not part of it, and a file that failed to parse contributes no
/// declarations.
pub fn facet_map(project: &Project, revision: Revision) -> FacetMap {
    facet_map_using(project, revision, &mut |module, decl| {
        symbol_version(module, decl)
    })
}

/// [`facet_map`] with each declaration's version supplied by `version_of`, so that a caller
/// that already computed [`symbol_version`] of the same declarations reuses it.
pub(crate) fn facet_map_using(
    project: &Project,
    revision: Revision,
    version_of: &mut dyn FnMut(&[String], &Declaration) -> SymbolVersion,
) -> FacetMap {
    let mut entries: BTreeMap<StableId, FacetEntry> = BTreeMap::new();
    let mut occurrences: BTreeMap<StableId, Vec<String>> = BTreeMap::new();
    for file in project_modules(project) {
        let module = module_path(file.module);
        for decl in &file.module.declarations {
            let entry = FacetEntry {
                version: version_of(&module, decl),
                kind: IdKind::for_decl(&decl.kind),
                module: module.join("."),
                name: decl.display_name().to_string(),
                file: file.path.to_string(),
                effects: effect_set(decl),
            };
            let id = entry.version.id.clone();
            occurrences
                .entry(id.clone())
                .or_default()
                .push(file.path.to_string());
            entries.entry(id).or_insert(entry);
        }
    }
    let duplicates = occurrences
        .into_iter()
        .filter(|(_, files)| files.len() > 1)
        .collect();
    FacetMap {
        revision,
        entries,
        duplicates,
    }
}

/// Semantics S48 `content_hash` of one module (Section 1.8): blake3 (64
/// hex) of the `format_module` text of every parsed file among `files`, in
/// path order, joined with LF. Files that are not parsed `.cpl` modules
/// (a file that failed to parse, a `.work` file) and repeated ids are
/// skipped. Milestone 1's stand-in for a dependency revision.
pub fn module_content_hash(project: &Project, files: &[FileId]) -> Digest {
    use std::sync::{Mutex, OnceLock};
    // The formatted text is a function of each file's source text, so the hash is memoized by
    // the paths and texts (process-wide, bounded).
    static CACHE: OnceLock<Mutex<std::collections::HashMap<[u8; 32], Digest>>> = OnceLock::new();
    let mut modules: Vec<(&str, FileId, &std::sync::Arc<cpl_syntax::ast::Module>)> = Vec::new();
    for &id in files {
        let Some(index) = project.files.iter().position(|entry| entry.file == id) else {
            continue;
        };
        if let Some(ParsedFile::Module(module)) = project.parsed.get(index) {
            let path = project.source_map.file(id).path.as_str();
            modules.push((path, id, module));
        }
    }
    modules.sort_by(|a, b| a.0.cmp(b.0).then(a.1.cmp(&b.1)));
    modules.dedup_by_key(|m| m.1);
    let mut key = blake3::Hasher::new();
    key.update(b"cpl-module-content-v1\0");
    for (path, id, _) in &modules {
        let text = project.source_map.file(*id).text();
        key.update(&(path.len() as u64).to_le_bytes());
        key.update(path.as_bytes());
        key.update(&(text.len() as u64).to_le_bytes());
        key.update(text.as_bytes());
    }
    let key = *key.finalize().as_bytes();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(d) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return d.clone();
    }
    let texts: Vec<String> = modules.iter().map(|(_, _, m)| format_module(m)).collect();
    let joined: Vec<&str> = texts.iter().map(String::as_str).collect();
    let d = digest(joined.join("\n").as_bytes());
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() >= 100_000 {
        cache.clear();
    }
    cache.insert(key, d.clone());
    d
}
