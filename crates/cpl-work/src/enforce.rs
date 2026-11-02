//! Write/add/preserve enforcement over facet diffs (`spec/toolchain.md`
//! 10.5), run at task-level validation (`Submitted -> Accepted`) and again
//! at integration against the assembled tree.
//!
//! [`enforce`] is a pure function of the baseline's and the candidate's
//! facet maps, the file-level differences of the two trees ([`TreeDiff`],
//! computed by [`tree_diff`]: steps 1 and 5 of the algorithm judge files,
//! which a facet map does not record), and the task's [`Grant`]:
//!
//! 1. every path outside `<sources>/**/*.cpl` whose bytes differ: a
//!    `work/*.patch` file is consumed (applied and removed by the caller,
//!    never a violation); `cpl.toml`, `cpl.lock`, and `work/<stem>.work`
//!    need their file selector in `grant.files`; any other path is
//!    `patch_out_of_scope`;
//! 2. every statement of `semantic_diff(baseline, candidate)`:
//!    `identity_added` needs an `add` namespace covering its module,
//!    `identity_removed` and `moved` write authority over every applicable
//!    facet of the declaration, `identity_renamed` over `name`, and every
//!    facet statement over its facet; else `patch_out_of_scope`;
//! 3. every preserved `(id, facet)` must hash the same on both sides, else
//!    `patch_preserved_facet`;
//! 4. an identity that changed between provisional and `@id` (or between
//!    two `@id` values) is `patch_identity_changed`, and every duplicate id
//!    of the candidate is `duplicate_identity`;
//! 5. every source `.cpl` file that differs must contain a statement step 2
//!    granted and keep its module header, or be new with its module under
//!    an `add` namespace; and no name that the unchanged text of a
//!    declaration present in both trees uses may import something else
//!    than before (a changed `use` line, or a move to another file of the
//!    module, [`ReboundName`]) unless the task may write that facet; else
//!    `patch_out_of_scope`.
//!
//! The candidate is accepted iff there is no violation
//! ([`Enforcement::accepted`]). The caller passes the facet map of the
//! candidate tree after its patches were applied (Section 10.5, patch
//! application), and the source directories of the run's frozen manifest.

use crate::codes;
use crate::model::FileSelector;
use crate::scheduler::Grant;
use cpl_diag::json::{DiffStatement, DiffStatementKind, FacetHash};
use cpl_diag::{Category, Diagnostic, Facet, FileId, StableId};
use cpl_index::{declaration_identity, facet_text, semantic_diff, FacetEntry, FacetMap};
use cpl_project::VirtualTree;
use cpl_syntax::{ast, lex, parse_module, TokenKind};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

/// How a file differs between the baseline tree and the candidate tree.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FileStatus {
    Added,
    Modified,
    Removed,
}

/// One path whose bytes differ between the two trees.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    /// Project-relative, `/`-separated.
    pub path: String,
    pub status: FileStatus,
    /// For a `.cpl` file: the module path of its `module` header on the
    /// baseline side; `None` when that side has no such file or no
    /// readable header.
    pub old_module: Option<Vec<String>>,
    /// The same on the candidate side.
    pub new_module: Option<Vec<String>>,
}

/// A name that an unchanged facet of a declaration present in both trees
/// uses, and that imports something else in the declaration's candidate
/// file than in its baseline file: a `use` line of the file was added,
/// removed, or repointed, or the declaration moved to another file of its
/// module. Imports are scoped to the file (semantics S5), so the facet
/// keeps its text and hash while what it refers to changes; rule 5 of
/// [`enforce`] needs write authority over it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReboundName {
    /// The candidate file of the declaration.
    pub path: String,
    /// The declaration's identity (the same in both trees).
    pub id: StableId,
    /// The unchanged facet whose text uses `name`: `signature`,
    /// `contract`, `effects`, or `body`, the facets that resolve names.
    pub facet: Facet,
    pub name: String,
    /// What `name` imports in the baseline file (`reader.pages.paginate`);
    /// empty when that file does not import it.
    pub old: Vec<String>,
    /// The same in the candidate file.
    pub new: Vec<String>,
}

/// The file-level input of [`enforce`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TreeDiff {
    /// `[build] sources` of the run's frozen manifest: which `.cpl` files
    /// are sources (`"."` is the project root).
    pub sources: Vec<String>,
    /// In path order.
    pub changes: Vec<FileChange>,
    /// Every name that the unchanged text of a declaration of a changed
    /// source file uses and whose import changed ([`ReboundName`]), in
    /// path, id, facet, and name order.
    pub rebound: Vec<ReboundName>,
}

/// Every path of either tree whose bytes differ, in path order, with the
/// module headers of `.cpl` files ([`module_header`]), and the names whose
/// import changed under the unchanged text of a declaration present in
/// both trees ([`TreeDiff::rebound`]).
pub fn tree_diff(baseline: &VirtualTree, candidate: &VirtualTree, sources: &[String]) -> TreeDiff {
    let paths: BTreeSet<&String> = baseline
        .files
        .keys()
        .chain(candidate.files.keys())
        .collect();
    let mut changes = Vec::new();
    for path in paths {
        let old = baseline.files.get(path);
        let new = candidate.files.get(path);
        let status = match (old, new) {
            (Some(o), Some(n)) if o == n => continue,
            (Some(_), Some(_)) => FileStatus::Modified,
            (None, Some(_)) => FileStatus::Added,
            (Some(_), None) => FileStatus::Removed,
            (None, None) => continue,
        };
        let header = |bytes: Option<&Vec<u8>>| {
            if path.ends_with(".cpl") {
                bytes.and_then(|b| module_header(b))
            } else {
                None
            }
        };
        changes.push(FileChange {
            path: path.clone(),
            status,
            old_module: header(old),
            new_module: header(new),
        });
    }
    let rebound = rebound_names(baseline, candidate, &changes, sources);
    TreeDiff {
        sources: sources.to_vec(),
        changes,
        rebound,
    }
}

/// The module path of a `.cpl` file's `module` header (grammar 3.1: doc
/// comments, then `module a.b.c`), read with the lexer, so that an error
/// later in the file does not hide it; `None` for bytes that are not UTF-8
/// or a file that does not start with a header.
pub fn module_header(bytes: &[u8]) -> Option<Vec<String>> {
    let text = std::str::from_utf8(bytes).ok()?;
    let (tokens, _) = lex(text, FileId(0));
    let mut tokens = tokens
        .iter()
        .skip_while(|t| matches!(t.kind, TokenKind::DocComment | TokenKind::Newline));
    if tokens.next()?.kind != TokenKind::KwModule {
        return None;
    }
    let mut segments = Vec::new();
    loop {
        let segment = tokens.next()?;
        if segment.kind != TokenKind::LowerIdent {
            return None;
        }
        segments.push(segment.text.clone());
        match tokens.next() {
            Some(t) if t.kind == TokenKind::Dot => continue,
            _ => return Some(segments),
        }
    }
}

/// The outcome of [`enforce`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Enforcement {
    /// Every violation, in the order of the algorithm's steps; within a
    /// step in path or statement order.
    pub violations: Vec<Diagnostic>,
    /// The `work/*.patch` files the candidate added or changed: consumed
    /// (applied, then removed from the tree), never violations.
    pub consumed: Vec<String>,
    /// `semantic_diff(baseline, candidate)`: the candidate's `facet_diff`.
    pub statements: Vec<DiffStatement>,
}

impl Enforcement {
    /// Accept iff there is no violation.
    pub fn accepted(&self) -> bool {
        self.violations.is_empty()
    }
}

/// Section 10.5 `enforce(candidate, grant)`; see the module documentation
/// for the five steps. Readings of the algorithm (each cited where it is
/// implemented):
///
/// - "every applicable facet" (`identity_removed`, `moved`) excludes
///   `identity`, which is excluded from every grant (Section 10.3), so the
///   rule can be met at all;
/// - step 4 pairs the two identities of one declaration by its qualified
///   name; besides a change between provisional and `@id` (allowed with
///   write authority over `name`), a changed `@id` value is always a
///   violation, because "a `.work` task can never do this" (Section 4.6);
/// - a statement relates to the files of its declaration on both sides
///   (a move touches two files); a removed source file needs a granted
///   statement only, having no header left to compare;
/// - step 5 lets `use` lines ride on any granted statement of their file,
///   but imports are scoped to the file (semantics S5): repointing one
///   changes what the unchanged text of every declaration of the file that
///   uses the name refers to, while no facet changes. The design, which
///   precedes this specification (spec/README.md), makes "Everything
///   outside the granted write and add sets ... read-only" and says an
///   `add` task "cannot change ... unrelated acceptance tests" (design
///   lines 353 and 285), so step 5 also needs write authority over every
///   unchanged facet that uses a name whose import changed
///   ([`TreeDiff::rebound`]).
pub fn enforce(
    baseline: &FacetMap,
    candidate: &FacetMap,
    tree: &TreeDiff,
    grant: &Grant,
) -> Enforcement {
    let mut violations = Vec::new();
    let mut consumed = Vec::new();

    // 1. Files other than <sources>/**/*.cpl.
    let mut source_changes = Vec::new();
    for change in &tree.changes {
        match classify(&change.path, &tree.sources) {
            PathClass::Patch => {
                if change.status != FileStatus::Removed {
                    consumed.push(change.path.clone());
                }
            }
            PathClass::Source => source_changes.push(change),
            PathClass::Granted(selector) => {
                if !grant.may_change_file(&selector) {
                    violations.push(file_violation(
                        change,
                        format!(
                            "`{}` is {}; changing it needs the file selector `{selector}` in the task's grant",
                            change.path,
                            status_text(change.status)
                        ),
                    ));
                }
            }
            PathClass::Other => violations.push(file_violation(
                change,
                format!(
                    "`{}` is {}, but no grant covers it: only the project's `.cpl` sources, granted project files, and consumed `work/*.patch` files may change",
                    change.path,
                    status_text(change.status)
                ),
            )),
        }
    }

    // 2. Facet-level statements.
    let diff = semantic_diff(baseline, candidate);
    let mut granted_files: BTreeSet<&str> = BTreeSet::new();
    for s in &diff.statements {
        let old = baseline.get(&s.id);
        let new = candidate.get(&s.id);
        let refusal = match s.statement {
            DiffStatementKind::IdentityAdded => match new {
                Some(n) if grant.may_add(&module_segments(&n.module)) => None,
                Some(n) => Some(format!(
                    "`{}` is a new declaration in module `{}`, which no `add` namespace of the task covers",
                    s.path, n.module
                )),
                None => Some(format!("`{}` is a new declaration", s.path)),
            },
            DiffStatementKind::IdentityRemoved => match old {
                Some(o) if writes_every_facet(grant, &s.id, o) => None,
                // Named by the baseline's own path: the removal of a kind
                // change is at the id's new path (Section 9.1).
                _ => Some(format!(
                    "removing `{}` needs write authority over every facet of it",
                    old.map_or(&s.path, |o| &o.version.path)
                )),
            },
            DiffStatementKind::IdentityRenamed => (!grant.may_write(&s.id, Facet::Name)).then(|| {
                format!("renaming `{}` needs write authority over its `name`", s.path)
            }),
            DiffStatementKind::Moved => match new {
                Some(n) if writes_every_facet(grant, &s.id, n) => None,
                _ => Some(format!(
                    "moving `{}` to another module needs write authority over every facet of it",
                    s.path
                )),
            },
            kind => {
                let facet = statement_facet(kind);
                (!grant.may_write(&s.id, facet)).then(|| {
                    format!(
                        "`{}`: {} is outside the task's write set (no write authority over `{facet}`)",
                        s.path,
                        statement_name(kind)
                    )
                })
            }
        };
        match refusal {
            None => {
                granted_files.extend(old.map(|o| o.file.as_str()));
                granted_files.extend(new.map(|n| n.file.as_str()));
            }
            Some(message) => {
                let mut d = Diagnostic::error(codes::PATCH_OUT_OF_SCOPE, codes::PATCH, message)
                    .with_symbol(s.id.clone(), s.path.clone(), s.facet)
                    .with_data("statement", json!(s.statement));
                if s.statement == DiffStatementKind::IdentityAdded {
                    if let Some(n) = new {
                        d = d.with_data("module", json!(n.module));
                    }
                }
                violations.push(d);
            }
        }
    }

    // 3. Preserve.
    for (id, facet) in &grant.preserve {
        let before = baseline.get(id).and_then(|e| e.facet(*facet));
        let after = candidate.get(id).and_then(|e| e.facet(*facet));
        if before != after {
            let path = baseline
                .get(id)
                .or_else(|| candidate.get(id))
                .map_or_else(|| id.to_string(), |e| e.version.path.clone());
            violations.push(
                Diagnostic::error(
                    codes::PATCH_PRESERVED_FACET,
                    codes::PATCH,
                    format!(
                        "the `{facet}` of `{path}` changed, but the task must preserve it ({})",
                        hash_change(before, after)
                    ),
                )
                .with_symbol(id.clone(), path, Some(*facet))
                .with_data("expected", json!(before))
                .with_data("actual", json!(after)),
            );
        }
    }

    // 4. Identity integrity.
    let baseline_by_path: BTreeMap<&str, &StableId> = baseline
        .entries
        .iter()
        .map(|(id, e)| (e.version.path.as_str(), id))
        .collect();
    for (id, entry) in &candidate.entries {
        if baseline.entries.contains_key(id) {
            continue;
        }
        let Some(&old_id) = baseline_by_path.get(entry.version.path.as_str()) else {
            continue;
        };
        if candidate.entries.contains_key(old_id) {
            continue;
        }
        let message = if old_id.is_provisional() != id.is_provisional() {
            if grant.may_write(old_id, Facet::Name) || grant.may_write(id, Facet::Name) {
                continue;
            }
            format!(
                "the identity of `{}` changed from `{old_id}` to `{id}` without write authority over its `name`",
                entry.version.path
            )
        } else {
            // Section 4.6: a new `@id` value is a new identity, which no
            // task can write (identity is never granted, Section 10.3).
            format!(
                "the `@id` of `{}` changed from `{old_id}` to `{id}`; identity is never writable",
                entry.version.path
            )
        };
        violations.push(
            Diagnostic::error(codes::PATCH_IDENTITY_CHANGED, codes::PATCH, message)
                .with_symbol(
                    id.clone(),
                    entry.version.path.clone(),
                    Some(Facet::Identity),
                )
                .with_data("previous", json!(old_id)),
        );
    }
    for (id, files) in &candidate.duplicates {
        let path = candidate
            .get(id)
            .map_or_else(|| id.to_string(), |e| e.version.path.clone());
        violations.push(
            Diagnostic::error(
                codes::DUPLICATE_IDENTITY,
                Category::Resolve,
                format!(
                    "stable id `{id}` is declared {} times ({})",
                    files.len(),
                    files.join(", ")
                ),
            )
            .with_symbol(id.clone(), path, None)
            .with_data("files", json!(files)),
        );
    }

    // 5. Non-facet text in .cpl files.
    for change in source_changes {
        let refusal = match change.status {
            FileStatus::Added => match &change.new_module {
                Some(module) if grant.may_add(module) => None,
                Some(module) => Some(format!(
                    "`{}` is a new file of module `{}`, which no `add` namespace of the task covers",
                    change.path,
                    module.join(".")
                )),
                None => Some(format!(
                    "`{}` is a new file without a readable `module` header",
                    change.path
                )),
            },
            FileStatus::Modified => {
                if change.old_module.is_none() || change.old_module != change.new_module {
                    Some(format!("the `module` header of `{}` changed", change.path))
                } else if !granted_files.contains(change.path.as_str()) {
                    Some(format!(
                        "`{}` changed outside every granted facet (use lines, comments, or layout only)",
                        change.path
                    ))
                } else {
                    None
                }
            }
            FileStatus::Removed => (!granted_files.contains(change.path.as_str())).then(|| {
                format!(
                    "`{}` was removed, but no granted change removed its declarations",
                    change.path
                )
            }),
        };
        if let Some(message) = refusal {
            violations.push(file_violation(change, message));
        }
    }
    // 5, the reading of design lines 285 and 353 (see the documentation):
    // an unchanged facet whose import changed needs write authority too.
    for rebound in &tree.rebound {
        if grant.may_write(&rebound.id, rebound.facet) {
            continue;
        }
        let symbol = baseline
            .get(&rebound.id)
            .map_or_else(|| rebound.id.to_string(), |e| e.version.path.clone());
        violations.push(
            Diagnostic::error(
                codes::PATCH_OUT_OF_SCOPE,
                codes::PATCH,
                format!(
                    "`{}`: the unchanged `{}` of `{symbol}` uses `{}`, which now imports {} instead of {}; that changes what the facet refers to, and the task has no write authority over it",
                    rebound.path,
                    rebound.facet,
                    rebound.name,
                    imports_text(&rebound.new),
                    imports_text(&rebound.old)
                ),
            )
            .with_symbol(rebound.id.clone(), symbol, Some(rebound.facet))
            .with_data("path", json!(rebound.path))
            .with_data("name", json!(rebound.name)),
        );
    }

    Enforcement {
        violations,
        consumed,
        statements: diff.statements,
    }
}

/// What step 1 makes of a path.
enum PathClass {
    /// `work/<stem>.patch`: consumed.
    Patch,
    /// `cpl.toml`, `cpl.lock`, `work/<stem>.work`: needs its file selector.
    Granted(FileSelector),
    /// A `.cpl` file under a source directory: steps 2 and 5.
    Source,
    /// Anything else.
    Other,
}

fn classify(path: &str, sources: &[String]) -> PathClass {
    if let Some(name) = path.strip_prefix("work/") {
        if !name.contains('/') && name.len() > ".patch".len() && name.ends_with(".patch") {
            return PathClass::Patch;
        }
    }
    if let Some(selector) = FileSelector::for_path(path) {
        return PathClass::Granted(selector);
    }
    let name = path.rsplit('/').next().unwrap_or(path);
    let cpl = name.len() > ".cpl".len() && name.ends_with(".cpl");
    let under_sources = sources.iter().any(|dir| {
        dir == "."
            || path
                .strip_prefix(dir.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
    });
    if cpl && under_sources {
        PathClass::Source
    } else {
        PathClass::Other
    }
}

fn file_violation(change: &FileChange, message: String) -> Diagnostic {
    Diagnostic::error(codes::PATCH_OUT_OF_SCOPE, codes::PATCH, message)
        .with_data("path", json!(change.path))
}

fn status_text(status: FileStatus) -> &'static str {
    match status {
        FileStatus::Added => "added",
        FileStatus::Modified => "changed",
        FileStatus::Removed => "removed",
    }
}

fn module_segments(module: &str) -> Vec<String> {
    module.split('.').map(str::to_string).collect()
}

/// Write authority over every applicable facet of the entry except
/// `identity` (Section 10.3: identity is excluded from every grant).
fn writes_every_facet(grant: &Grant, id: &StableId, entry: &FacetEntry) -> bool {
    entry
        .version
        .facets
        .keys()
        .filter(|&&facet| facet != Facet::Identity)
        .all(|&facet| grant.may_write(id, facet))
}

/// The facet a facet statement is about (Section 9.1; cpl-index's reading
/// for the effect and policy statements).
fn statement_facet(kind: DiffStatementKind) -> Facet {
    match kind {
        DiffStatementKind::SignatureChanged => Facet::Signature,
        DiffStatementKind::ContractChanged => Facet::Contract,
        DiffStatementKind::EffectBoundWidened | DiffStatementKind::EffectBoundNarrowed => {
            Facet::Effects
        }
        DiffStatementKind::BodyChanged => Facet::Body,
        DiffStatementKind::DocsChanged => Facet::Docs,
        DiffStatementKind::VerificationPolicyChanged => Facet::VerificationPolicy,
        DiffStatementKind::IdentityRenamed => Facet::Name,
        DiffStatementKind::IdentityAdded
        | DiffStatementKind::IdentityRemoved
        | DiffStatementKind::Moved => Facet::Identity,
    }
}

fn statement_name(kind: DiffStatementKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn hash_change(before: Option<&FacetHash>, after: Option<&FacetHash>) -> String {
    let side = |hash: Option<&FacetHash>| hash.map_or("absent".to_string(), |h| h.to_string());
    format!("{} -> {}", side(before), side(after))
}

/// `` `a.b.x` `` (several when a name is imported twice, which the checker
/// rejects), or `nothing`.
fn imports_text(imports: &[String]) -> String {
    if imports.is_empty() {
        "nothing".to_string()
    } else {
        imports
            .iter()
            .map(|i| format!("`{i}`"))
            .collect::<Vec<_>>()
            .join(" and ")
    }
}

// ---------------------------------------------------------------------------
// Rule 5: names whose import changed under unchanged text
// ---------------------------------------------------------------------------

/// The facets whose text resolves names (semantics 2.2): types in a
/// signature, expressions in a contract and a body, capability parameters
/// in an effect bound. `name`, `docs`, `verification_policy`, and
/// `identity` resolve none.
const RESOLVING_FACETS: [Facet; 4] = [
    Facet::Signature,
    Facet::Contract,
    Facet::Effects,
    Facet::Body,
];

/// One parsed side of a changed source file.
struct ParsedSource {
    path: String,
    module: Vec<String>,
    /// Imported name -> every `a.b.x` the file's `use` lines import under it.
    imports: BTreeMap<String, BTreeSet<String>>,
    /// The file's declarations: identity and the canonical text of each
    /// applicable resolving facet (Section 5.2).
    declarations: Vec<(StableId, BTreeMap<Facet, String>)>,
}

/// Parse one side of a source file; `None` when it is not UTF-8 or does
/// not parse (then the facet map has none of its declarations, and step 2
/// judges them as removed or added).
fn parse_source(path: &str, bytes: &[u8]) -> Option<ParsedSource> {
    let text = std::str::from_utf8(bytes).ok()?;
    let module = parse_module(text, FileId(0)).value?;
    let module_path: Vec<String> = module
        .header
        .path
        .segments
        .iter()
        .map(|s| s.text.clone())
        .collect();
    let declarations = module
        .declarations
        .iter()
        .map(|decl| {
            let facets = RESOLVING_FACETS
                .iter()
                .filter_map(|&f| facet_text(&module_path, decl, f).map(|text| (f, text)))
                .collect();
            (declaration_identity(&module_path, decl), facets)
        })
        .collect();
    Some(ParsedSource {
        path: path.to_string(),
        imports: import_bindings(&module),
        module: module_path,
        declarations,
    })
}

/// What each name imported by the file's `use` lines refers to (semantics
/// 1.6): `use a.b.{x}` and `use a.b.x` both import `a.b.x` under `x`.
fn import_bindings(module: &ast::Module) -> BTreeMap<String, BTreeSet<String>> {
    let mut bindings: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for decl in &module.uses {
        let path = decl.path.dotted();
        for item in &decl.items {
            bindings
                .entry(item.name.text.clone())
                .or_default()
                .insert(format!("{path}.{}", item.name.text));
        }
    }
    bindings
}

/// The names a canonical facet text may resolve through its file's scope:
/// every identifier not preceded by `.` (which starts a member, an effect
/// operation, or the rest of a qualified chain, semantics 2.2 and 2.4).
/// Parameters, locals, and field labels are included, so the set
/// over-approximates the names the text resolves through imports; the
/// check fails closed.
fn scope_names(text: &str) -> BTreeSet<String> {
    let (tokens, _) = lex(text, FileId(0));
    let mut names = BTreeSet::new();
    let mut after_dot = false;
    for token in &tokens {
        match token.kind {
            TokenKind::Newline | TokenKind::DocComment => continue,
            TokenKind::LowerIdent | TokenKind::UpperIdent if !after_dot => {
                names.insert(token.text.clone());
            }
            _ => {}
        }
        after_dot = token.kind == TokenKind::Dot;
    }
    names
}

/// [`TreeDiff::rebound`]: for every declaration of a changed source file
/// that is present, under the same identity and module, on both sides,
/// every resolving facet whose text did not change (a changed facet is
/// judged by step 2) and every name that text uses whose imports differ
/// between the declaration's baseline file and its candidate file. A
/// declaration of an unchanged file keeps its file's imports and is never
/// listed; one that moved to another module is judged by step 2 (`moved`).
fn rebound_names(
    baseline: &VirtualTree,
    candidate: &VirtualTree,
    changes: &[FileChange],
    sources: &[String],
) -> Vec<ReboundName> {
    let mut old_files = Vec::new();
    let mut new_files = Vec::new();
    for change in changes {
        if !matches!(classify(&change.path, sources), PathClass::Source) {
            continue;
        }
        let parse = |tree: &VirtualTree| {
            tree.files
                .get(&change.path)
                .and_then(|bytes| parse_source(&change.path, bytes))
        };
        old_files.extend(parse(baseline));
        new_files.extend(parse(candidate));
    }
    // Identity -> (file, declaration); the first declaration of a
    // duplicated identity (step 4 rejects the duplicate itself).
    let index = |files: &[ParsedSource]| {
        let mut by_id: BTreeMap<StableId, (usize, usize)> = BTreeMap::new();
        for (f, file) in files.iter().enumerate() {
            for (d, (id, _)) in file.declarations.iter().enumerate() {
                by_id.entry(id.clone()).or_insert((f, d));
            }
        }
        by_id
    };
    let old_index = index(&old_files);
    let new_index = index(&new_files);
    let mut rebound = Vec::new();
    for (id, &(of, od)) in &old_index {
        let Some(&(nf, nd)) = new_index.get(id) else {
            continue;
        };
        let (old_file, new_file) = (&old_files[of], &new_files[nf]);
        if old_file.module != new_file.module {
            continue;
        }
        let (old_facets, new_facets) = (&old_file.declarations[od].1, &new_file.declarations[nd].1);
        for (facet, text) in old_facets {
            if new_facets.get(facet) != Some(text) {
                continue;
            }
            for name in scope_names(text) {
                let was = old_file.imports.get(&name);
                let is = new_file.imports.get(&name);
                if was != is {
                    let list = |imports: Option<&BTreeSet<String>>| -> Vec<String> {
                        imports.into_iter().flatten().cloned().collect()
                    };
                    rebound.push(ReboundName {
                        path: new_file.path.clone(),
                        id: id.clone(),
                        facet: *facet,
                        name,
                        old: list(was),
                        new: list(is),
                    });
                }
            }
        }
    }
    rebound.sort();
    rebound
}
