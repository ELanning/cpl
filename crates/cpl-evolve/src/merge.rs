//! Three-way semantic merge keyed by stable identity. Facets merge independently; text taken from
//! either side is re-rendered against the merged program's names, so a rename on one side follows
//! references the other side added. Imports merge as sets of resolved targets. The composed program
//! is checked and its affected tests run on it; a test that passes on both sides but fails after
//! composition is a regression, a semantic conflict that no text merge reports.
use crate::{
    error, query,
    verify::{self, Evidence, VerifyConfig},
    Snapshot,
};
use cpl_diag::json::TestStatus;
use cpl_diag::{Diagnostic, Facet, FileId, FileOrigin, Span, StableId};
use cpl_hir::{DefId, ImportTarget, Program, RefKind};
use cpl_index::{FacetEntry, Index};
use cpl_project::{LoadOptions, VirtualTree};
use cpl_syntax::{
    ast,
    lexer::{lex, TokenKind},
};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Which side a merged facet, placement, or text came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Both,
    Left,
    Right,
}

#[derive(Debug, Serialize)]
pub struct MergeReport {
    pub base: String,
    pub left: String,
    pub right: String,
    /// The composed candidate's snapshot digest, when one could be built and checked.
    pub merged: Option<String>,
    /// `conflict`, `check_failed`, `regression`, `failed`, or `tested`.
    pub status: String,
    /// `independent` when no declaration is affected by both sides, otherwise `tested` or `unknown`.
    pub compatibility: String,
    pub accepted: bool,
    /// The facet origins of every declaration either side changed.
    pub decisions: Vec<Value>,
    /// Structural conflicts: incompatible writes the merge cannot combine.
    pub conflicts: Vec<Value>,
    /// Diagnostics of the composed program when it does not check.
    pub diagnostics: Vec<Value>,
    /// Tests that pass on each side containing them but fail on the composition.
    pub regressions: Vec<Value>,
    /// Failing tests that already failed on one side; not attributable to the merge.
    pub inherited_failures: Vec<Value>,
    /// Declarations whose behavior can depend on changes from both sides.
    pub interactions: Vec<Value>,
    pub evidence: Option<Evidence>,
}

pub struct Merge {
    /// The composed tree. Absent when a structural conflict prevents composition.
    pub tree: Option<VirtualTree>,
    pub snapshot: Option<Snapshot>,
    pub report: MergeReport,
    seal: String,
}

impl Merge {
    /// Only an accepted, unmodified merge result whose left side is `left` can be installed.
    pub(crate) fn accepted_for(&self, left: &Snapshot) -> bool {
        self.report.accepted
            && self.report.left == left.digest()
            && self
                .snapshot
                .as_ref()
                .is_some_and(|s| Some(s.digest()) == self.report.merged)
            && self.seal == seal(&self.report)
    }
}

fn seal(report: &MergeReport) -> String {
    blake3::hash(&serde_json::to_vec(report).expect("merge report serializes"))
        .to_hex()
        .to_string()
}

struct Side<'a> {
    snap: &'a Snapshot,
    index: Index<'a>,
    defs: BTreeMap<StableId, DefId>,
    files: BTreeMap<String, FileId>,
}

impl<'a> Side<'a> {
    fn new(snap: &'a Snapshot) -> Self {
        let p = &snap.checked.program;
        let project = |file: FileId| p.source_map.file(file).origin != FileOrigin::Std;
        Side {
            snap,
            index: snap.index(),
            defs: p
                .defs()
                .filter(|d| project(d.file))
                .map(|d| (d.stable_id.clone(), d.id))
                .collect(),
            files: p
                .asts
                .iter()
                .filter(|(f, _)| project(*f))
                .map(|(f, _)| (p.source_map.file(*f).path.clone(), *f))
                .collect(),
        }
    }
    fn p(&self) -> &'a Program {
        &self.snap.checked.program
    }
    fn entry(&self, id: &StableId) -> Option<&FacetEntry> {
        self.index.facet_map().get(id)
    }
    fn decl(&self, id: &StableId) -> &'a ast::Declaration {
        self.p().ast(self.defs[id])
    }
    /// The declared identifier; tests and properties have none.
    fn ident(&self, id: &StableId) -> Option<&'a str> {
        let def = self.p().by_stable_id(id)?;
        self.p().ast(def).name().map(|n| n.text.as_str())
    }
    fn decl_file(&self, id: &StableId) -> Option<FileId> {
        self.defs.get(id).map(|d| self.p().def(*d).file)
    }
    fn text(&self, file: FileId) -> &'a str {
        self.p().source_map.file(file).text()
    }
    fn module(&self, file: FileId) -> Option<&'a ast::Module> {
        self.p().file_ast(file)
    }
    /// Declarations of one file in source order.
    fn declarations(&self, file: FileId) -> Vec<StableId> {
        let mut out: Vec<_> = self
            .p()
            .defs()
            .filter(|d| d.file == file)
            .map(|d| (d.decl_index, d.stable_id.clone()))
            .collect();
        out.sort();
        out.into_iter().map(|(_, id)| id).collect()
    }
}

/// Three-way choice: `None` is a conflict.
fn pick<T: PartialEq>(b: Option<T>, l: Option<T>, r: Option<T>) -> Option<Origin> {
    if l == r {
        Some(Origin::Both)
    } else if l == b {
        Some(Origin::Right)
    } else if r == b {
        Some(Origin::Left)
    } else {
        None
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Hand {
    Left,
    Right,
}

#[derive(Debug)]
struct Plan {
    /// The side whose declaration text is rendered.
    text: Hand,
    /// The side whose body replaces the text side's body.
    body: Option<Hand>,
    /// The destination file.
    file: String,
    /// Facet origins of a declaration both sides changed in non-body facets: its syntax is grafted
    /// facet by facet and every resulting facet hash is checked against its origin.
    graft: Option<BTreeMap<Facet, Origin>>,
}

/// Merged identifiers and modules of every planned declaration.
struct Names {
    idents: BTreeMap<StableId, String>,
    modules: BTreeMap<StableId, String>,
}

fn conflict(id: &StableId, facet: Option<Facet>, reason: &str) -> Value {
    json!({"symbol": id, "facet": facet, "status": "conflict", "reason": reason})
}

fn same(a: &FacetEntry, b: &FacetEntry) -> bool {
    a.version.facets == b.version.facets && a.file == b.file
}

/// Decide every declaration's facets, placement, and text source.
fn plan(
    b: &Side,
    l: &Side,
    r: &Side,
    conflicts: &mut Vec<Value>,
    decisions: &mut Vec<Value>,
) -> BTreeMap<StableId, Plan> {
    let ids: BTreeSet<StableId> = b
        .defs
        .keys()
        .chain(l.defs.keys())
        .chain(r.defs.keys())
        .cloned()
        .collect();
    let mut plans = BTreeMap::new();
    for id in ids {
        let (eb, el, er) = (b.entry(&id), l.entry(&id), r.entry(&id));
        let keep = |hand: Hand, e: &FacetEntry| Plan {
            text: hand,
            body: None,
            file: e.file.clone(),
            graft: None,
        };
        match (eb, el, er) {
            (_, None, None) => {
                if eb.is_some() {
                    decisions.push(json!({"symbol": id, "removed": "both"}));
                }
            }
            (None, Some(el), None) => {
                decisions.push(json!({"symbol": id, "added": "left"}));
                plans.insert(id, keep(Hand::Left, el));
            }
            (None, None, Some(er)) => {
                decisions.push(json!({"symbol": id, "added": "right"}));
                plans.insert(id, keep(Hand::Right, er));
            }
            (None, Some(el), Some(er)) => {
                if same(el, er) {
                    decisions.push(json!({"symbol": id, "added": "both"}));
                    plans.insert(id, keep(Hand::Left, el));
                } else {
                    conflicts.push(conflict(&id, None, "added_differently_on_both_sides"));
                }
            }
            (Some(eb), Some(el), None) => {
                if same(el, eb) {
                    decisions.push(json!({"symbol": id, "removed": "right"}));
                } else {
                    conflicts.push(conflict(&id, None, "removed_on_right_and_changed_on_left"));
                }
            }
            (Some(eb), None, Some(er)) => {
                if same(er, eb) {
                    decisions.push(json!({"symbol": id, "removed": "left"}));
                } else {
                    conflicts.push(conflict(&id, None, "removed_on_left_and_changed_on_right"));
                }
            }
            (Some(eb), Some(el), Some(er)) => {
                if same(el, eb) && same(er, eb) {
                    plans.insert(id, keep(Hand::Left, el));
                    continue;
                }
                let keys = |e: &FacetEntry| e.version.facets.keys().copied().collect::<Vec<_>>();
                if keys(eb) != keys(el) || keys(eb) != keys(er) {
                    // A kind change on either side: the facets are not comparable.
                    if same(el, eb) {
                        plans.insert(id.clone(), keep(Hand::Right, er));
                        decisions.push(json!({"symbol": id, "declaration": "right"}));
                    } else if same(er, eb) || same(el, er) {
                        plans.insert(id.clone(), keep(Hand::Left, el));
                        decisions.push(json!({"symbol": id, "declaration": "left"}));
                    } else {
                        conflicts.push(conflict(
                            &id,
                            None,
                            "kind_changed_on_one_side_and_changed_on_the_other",
                        ));
                    }
                    continue;
                }
                let mut origins = BTreeMap::new();
                let mut clash = false;
                for facet in Facet::ALL {
                    match pick(eb.facet(facet), el.facet(facet), er.facet(facet)) {
                        Some(o) => {
                            if eb.facet(facet).is_some() {
                                origins.insert(facet, o);
                            }
                        }
                        None => {
                            clash = true;
                            conflicts.push(conflict(
                                &id,
                                Some(facet),
                                "different_replacements_of_same_facet",
                            ));
                        }
                    }
                }
                let placement = pick(Some(&eb.file), Some(&el.file), Some(&er.file));
                if placement.is_none() {
                    clash = true;
                    conflicts.push(conflict(&id, None, "moved_to_different_files"));
                }
                if clash {
                    continue;
                }
                let named = l.decl(&id).name().is_some() && r.decl(&id).name().is_some();
                let splices = |hand: Origin| {
                    origins
                        .iter()
                        .filter(|(_, o)| **o == hand)
                        .all(|(f, _)| *f == Facet::Body || (*f == Facet::Name && named))
                };
                let from =
                    |hand: Origin| origins.iter().any(|(f, o)| *f == Facet::Body && *o == hand);
                let file = if placement == Some(Origin::Right) {
                    er.file.clone()
                } else {
                    el.file.clone()
                };
                let plan = if splices(Origin::Right) {
                    Plan {
                        text: Hand::Left,
                        body: from(Origin::Right).then_some(Hand::Right),
                        file,
                        graft: None,
                    }
                } else if splices(Origin::Left) {
                    Plan {
                        text: Hand::Right,
                        body: from(Origin::Left).then_some(Hand::Left),
                        file,
                        graft: None,
                    }
                } else {
                    Plan {
                        text: Hand::Left,
                        body: None,
                        file,
                        graft: Some(origins.clone()),
                    }
                };
                let facets: BTreeMap<_, _> = origins
                    .iter()
                    .filter(|(_, o)| **o != Origin::Both)
                    .map(|(f, o)| (f.as_str(), *o))
                    .collect();
                decisions.push(json!({"symbol": id, "facets": facets, "placement": placement}));
                plans.insert(id, plan);
            }
        }
    }
    plans
}

/// Text edits in one file's byte coordinates.
#[derive(Default)]
struct Edits(Vec<(u32, u32, String)>);
impl Edits {
    fn push(&mut self, start: u32, end: u32, text: String) {
        self.0.push((start, end, text));
    }
    fn overlaps(&self, start: u32, end: u32) -> bool {
        self.0
            .iter()
            .any(|(s, e, _)| start < *e && *s < end || (start == end && *s < start && start < *e))
    }
    /// Apply to `text[start..end]`, whose first byte is at offset `start`.
    fn apply(mut self, text: &str, start: u32, end: u32) -> Result<String, Diagnostic> {
        self.0.sort_by_key(|e| (e.0, e.1));
        self.0.dedup();
        if self.0.windows(2).any(|w| w[0].1 > w[1].0) {
            return Err(error("merge_overlap", "overlapping merge edits"));
        }
        let mut out = String::new();
        let mut at = start;
        for (s, e, replacement) in self.0 {
            out.push_str(&text[at as usize..s as usize]);
            out.push_str(&replacement);
            at = e;
        }
        out.push_str(&text[at as usize..end as usize]);
        Ok(out)
    }
}

/// The last identifier token spelled `name` inside `span`, as an absolute span.
fn token(text: &str, span: Span, name: &str) -> Option<Span> {
    let (tokens, _) = lex(&text[span.start as usize..span.end as usize], span.file);
    tokens
        .iter()
        .rev()
        .find(|t| matches!(t.kind, TokenKind::LowerIdent | TokenKind::UpperIdent) && t.text == name)
        .map(|t| Span {
            file: span.file,
            start: span.start + t.span.start,
            end: span.start + t.span.end,
        })
}

/// Rewrite the references of `id` inside `[start, end)` for the merged program: a renamed target
/// takes its merged name, and a target whose merged module differs from where this reference could
/// name it unqualified is named by its full module path. Field accesses and capability operations
/// name members, never the target declaration.
fn translate(side: &Side, id: &StableId, start: u32, end: u32, names: &Names, edits: &mut Edits) {
    let p = side.p();
    let def = side.defs[id];
    let file = side.decl(id).span.file;
    let text = side.text(file);
    let module_of = |d: DefId| p.module(p.def(d).module).path.join(".");
    let here = module_of(def);
    let destination = names
        .modules
        .get(id)
        .cloned()
        .unwrap_or_else(|| here.clone());
    for r in p.references_from(def) {
        if r.span.file != file
            || r.span.start < start
            || r.span.end > end
            || matches!(r.kind, RefKind::FieldAccess | RefKind::CapabilityOp)
            || edits.overlaps(r.span.start, r.span.end)
        {
            continue;
        }
        let target = &p.def(r.to).stable_id;
        let Some(old) = side.ident(target) else {
            continue;
        };
        let Some(t) = token(text, r.span, old) else {
            continue;
        };
        let new = names.idents.get(target).map_or(old, String::as_str);
        let was = module_of(r.to);
        let now = names
            .modules
            .get(target)
            .cloned()
            .unwrap_or_else(|| was.clone());
        let qualifier = text[r.span.start as usize..t.start as usize].trim_end();
        if qualifier.ends_with('.') && now != was {
            // A qualified reference to a moved declaration: qualify it by its new module.
            edits.push(r.span.start, t.end, format!("{now}.{new}"));
        } else if qualifier.is_empty() && was == here && now != destination {
            // A same-module reference the merge separates: the target is named by path.
            edits.push(t.start, t.end, format!("{now}.{new}"));
        } else if old != new {
            edits.push(t.start, t.end, new.to_string());
        }
    }
}

const SIGNATURE_METADATA: [&str; 5] = ["protocol", "transition", "message", "schema", "migration"];

fn swap_annotations(into: &mut ast::Declaration, from: &ast::Declaration, names: &[&str]) {
    into.annotations
        .retain(|a| !names.contains(&a.name.text.as_str()));
    into.annotations.extend(
        from.annotations
            .iter()
            .filter(|a| names.contains(&a.name.text.as_str()))
            .cloned(),
    );
}

/// Copy the syntax of one facet of `from` into `into`; false when this kind's facet is not separable.
fn graft(into: &mut ast::Declaration, from: &ast::Declaration, facet: Facet) -> bool {
    use ast::DeclKind as K;
    let effects = |c: &&ast::FnClause| matches!(c, ast::FnClause::Effects(_));
    match facet {
        Facet::Identity => true,
        Facet::Docs => {
            into.docs = from.docs.clone();
            swap_annotations(into, from, &["intent", "preserve"]);
            true
        }
        Facet::VerificationPolicy => {
            swap_annotations(into, from, &["prove"]);
            true
        }
        Facet::Name => match (&mut into.kind, &from.kind) {
            (K::Test(a), K::Test(b)) => {
                a.name = b.name.clone();
                true
            }
            (K::Property(a), K::Property(b)) => {
                a.name = b.name.clone();
                true
            }
            // Identifiers already carry their merged name from translation.
            _ => true,
        },
        Facet::Body => match (&mut into.kind, &from.kind) {
            (K::Fn(a), K::Fn(b)) => {
                a.body = b.body.clone();
                true
            }
            (K::Test(a), K::Test(b)) => {
                a.body = b.body.clone();
                true
            }
            (K::Property(a), K::Property(b)) => {
                a.body = b.body.clone();
                true
            }
            _ => false,
        },
        Facet::Contract => match (&mut into.kind, &from.kind) {
            (K::Fn(a), K::Fn(b)) => {
                let kept: Vec<_> = a.clauses.iter().filter(effects).cloned().collect();
                a.clauses = kept
                    .into_iter()
                    .chain(b.clauses.iter().filter(|c| !effects(c)).cloned())
                    .collect();
                true
            }
            (K::Struct(a), K::Struct(b)) => {
                a.invariant = b.invariant.clone();
                true
            }
            _ => false,
        },
        Facet::Effects => match (&mut into.kind, &from.kind) {
            (K::Fn(a), K::Fn(b)) => {
                let kept: Vec<_> = a.clauses.iter().filter(|c| !effects(c)).cloned().collect();
                a.clauses = b
                    .clauses
                    .iter()
                    .filter(effects)
                    .cloned()
                    .chain(kept)
                    .collect();
                true
            }
            _ => false,
        },
        Facet::Signature => {
            swap_annotations(into, from, &SIGNATURE_METADATA);
            match (&mut into.kind, &from.kind) {
                (K::Fn(a), K::Fn(b)) => {
                    a.is_pub = b.is_pub;
                    a.mode = b.mode;
                    a.generic_params = b.generic_params.clone();
                    a.params = b.params.clone();
                    a.ret = b.ret.clone();
                    true
                }
                (K::Struct(a), K::Struct(b)) => {
                    a.is_pub = b.is_pub;
                    a.is_program = b.is_program;
                    a.generic_params = b.generic_params.clone();
                    a.fields = b.fields.clone();
                    true
                }
                (K::Enum(a), K::Enum(b)) => {
                    a.is_pub = b.is_pub;
                    a.generic_params = b.generic_params.clone();
                    a.variants = b.variants.clone();
                    true
                }
                (K::Property(a), K::Property(b)) => {
                    a.for_clauses = b.for_clauses.clone();
                    true
                }
                _ => false,
            }
        }
    }
}

/// Graft a declaration both sides changed: the left text with each right-origin facet's syntax,
/// re-laid out canonically. Every facet hash must equal its origin's, or the merge refuses it.
fn render_graft(
    id: &StableId,
    plan: &Plan,
    origins: &BTreeMap<Facet, Origin>,
    l: &Side,
    r: &Side,
    names: &Names,
) -> Result<String, Diagnostic> {
    let module = names.modules.get(id).cloned().unwrap_or_default();
    let path: Vec<String> = module.split('.').map(str::to_string).collect();
    let text = |hand: Hand| {
        let plain = Plan {
            text: hand,
            body: None,
            file: plan.file.clone(),
            graft: None,
        };
        render(id, &plain, l, r, names).and_then(|t| crate::change::parse_one(&module, &t))
    };
    let (left, right) = (text(Hand::Left)?, text(Hand::Right)?);
    let mut merged = left.clone();
    for (facet, origin) in origins {
        if *origin == Origin::Right && !graft(&mut merged, &right, *facet) {
            return Err(error(
                "merge_graft_unsupported",
                format!("the {facet} facet of this kind of declaration cannot be combined"),
            ));
        }
    }
    let entry = |d: &ast::Declaration| FacetEntry::new(&path, d, &plan.file);
    let (el, er, em) = (entry(&left), entry(&right), entry(&merged));
    for facet in Facet::ALL {
        let expected = match origins.get(&facet) {
            Some(Origin::Right) => er.facet(facet),
            _ => el.facet(facet),
        };
        if em.facet(facet) != expected {
            return Err(error(
                "merge_graft_mismatch",
                format!("combining the facets changed the {facet} facet"),
            ));
        }
    }
    merged.trivia = ast::Trivia::default();
    let text = cpl_syntax::format_fragment(
        cpl_syntax::Fragment::Declaration(&merged),
        cpl_syntax::FragmentMode::Source,
    );
    Ok(text.trim_end().to_string())
}

fn body_span(decl: &ast::Declaration) -> Option<Span> {
    match &decl.kind {
        ast::DeclKind::Fn(f) => f.body.as_ref().map(|b| b.span),
        ast::DeclKind::Test(t) => Some(t.body.span),
        ast::DeclKind::Property(t) => Some(t.body.span),
        _ => None,
    }
}

fn side<'s, 'a>(hand: Hand, l: &'s Side<'a>, r: &'s Side<'a>) -> &'s Side<'a> {
    match hand {
        Hand::Left => l,
        Hand::Right => r,
    }
}

/// The merged text of one declaration: its text side's span, with the other side's body when
/// planned, the merged own name, and every reference renamed to its merged target.
fn render(
    id: &StableId,
    plan: &Plan,
    l: &Side,
    r: &Side,
    names: &Names,
) -> Result<String, Diagnostic> {
    if let Some(origins) = &plan.graft {
        return render_graft(id, plan, origins, l, r, names);
    }
    let s = side(plan.text, l, r);
    let decl = s.decl(id);
    let file = decl.span.file;
    let mut edits = Edits::default();
    if let Some(hand) = plan.body {
        let o = side(hand, l, r);
        let (Some(target), Some(source)) = (body_span(decl), body_span(o.decl(id))) else {
            return Err(error(
                "merge_body",
                "a merged body needs a body on both sides",
            ));
        };
        let mut inner = Edits::default();
        translate(o, id, source.start, source.end, names, &mut inner);
        edits.push(
            target.start,
            target.end,
            inner.apply(o.text(source.file), source.start, source.end)?,
        );
    }
    if let (Some(name), Some(new)) = (decl.name(), names.idents.get(id)) {
        if &name.text != new {
            edits.push(name.span.start, name.span.end, new.clone());
        }
    }
    translate(s, id, decl.span.start, decl.span.end, names, &mut edits);
    edits.apply(s.text(file), decl.span.start, decl.span.end)
}

/// A resolved import item: the key a three-way set merge compares, and how to print it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ImportKey {
    Def(StableId),
    Path(String),
}
struct Item {
    key: ImportKey,
    path: String,
    name: String,
    span: Span,
}

fn imports(s: &Side, file: FileId) -> Vec<Item> {
    let p = s.p();
    let Some(module) = s.module(file) else {
        return vec![];
    };
    let mut out = Vec::new();
    for u in &module.uses {
        for item in &u.items {
            let target = p
                .imports
                .iter()
                .find(|i| i.file == file && i.span == item.span)
                .map(|i| &i.target);
            let key = match target {
                Some(ImportTarget::Def(d)) if s.defs.contains_key(&p.def(*d).stable_id) => {
                    ImportKey::Def(p.def(*d).stable_id.clone())
                }
                _ => ImportKey::Path(format!("{}.{}", u.path.dotted(), item.name.text)),
            };
            out.push(Item {
                key,
                path: u.path.dotted(),
                name: item.name.text.clone(),
                span: item.span,
            });
        }
    }
    out
}

/// The byte range removing a declaration: its leading comments, doc comments, annotations, and
/// lines, with one adjacent blank line, so the remaining file keeps its layout.
pub(crate) fn deletion(text: &str, decl: &ast::Declaration) -> (u32, u32) {
    let leading = decl
        .trivia
        .leading
        .first()
        .map_or(decl.span.start, |c| c.span.start.min(decl.span.start));
    let mut start = line_start(text, leading as usize);
    let mut end = line_end(text, decl.span.end as usize);
    if text[end..].starts_with('\n') {
        end += 1;
    } else if end == text.len() && text[..start].ends_with("\n\n") {
        start -= 1;
    }
    (start as u32, end as u32)
}

pub(crate) fn line_start(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |i| i + 1)
}
pub(crate) fn line_end(text: &str, at: usize) -> usize {
    text[at..].find('\n').map_or(text.len(), |i| at + i + 1)
}

struct FileInputs<'s, 'a> {
    path: &'s str,
    host: &'s Side<'a>,
    hand: Hand,
    file: FileId,
}

/// Assemble one merged file from its host side's text.
fn assemble(
    input: &FileInputs,
    b: &Side,
    l: &Side,
    r: &Side,
    plans: &BTreeMap<StableId, Plan>,
    names: &Names,
) -> Result<String, Diagnostic> {
    let host = input.host;
    let text = host.text(input.file);
    let mut edits = Edits::default();
    // Imports: a three-way set merge of resolved targets. An unchanged set is renamed in place.
    let set = |s: &Side| -> Vec<Item> {
        s.files
            .get(input.path)
            .map(|f| imports(s, *f))
            .unwrap_or_default()
    };
    let (ib, il, ir) = (set(b), set(l), set(r));
    let keys = |items: &[Item]| items.iter().map(|i| i.key.clone()).collect::<BTreeSet<_>>();
    let (kb, kl, kr) = (keys(&ib), keys(&il), keys(&ir));
    let merged: BTreeSet<_> = kl
        .intersection(&kr)
        .chain(kl.difference(&kb))
        .chain(kr.difference(&kb))
        .cloned()
        .collect();
    let own = imports(host, input.file);
    let module = host
        .module(input.file)
        .ok_or_else(|| error("merge_parse", "a merged file did not parse"))?;
    let name_of = |item: &Item| match &item.key {
        ImportKey::Def(id) => names
            .idents
            .get(id)
            .cloned()
            .unwrap_or_else(|| item.name.clone()),
        ImportKey::Path(_) => item.name.clone(),
    };
    // A declaration moved to another module is imported from its merged module.
    let path_of = |item: &Item| match &item.key {
        ImportKey::Def(id) => names
            .modules
            .get(id)
            .cloned()
            .unwrap_or_else(|| item.path.clone()),
        ImportKey::Path(_) => item.path.clone(),
    };
    if keys(&own) == merged && own.iter().all(|item| path_of(item) == item.path) {
        for item in &own {
            let name = name_of(item);
            if name != item.name {
                edits.push(item.span.start, item.span.end, name);
            }
        }
    } else {
        let other = if input.hand == Hand::Left { &ir } else { &il };
        let mut seen = BTreeSet::new();
        let mut groups: Vec<(String, Vec<String>)> = Vec::new();
        for item in own.iter().chain(other.iter()) {
            if !merged.contains(&item.key) || !seen.insert(item.key.clone()) {
                continue;
            }
            let (name, path) = (name_of(item), path_of(item));
            match groups.iter_mut().find(|(p, _)| *p == path) {
                Some((_, names)) => names.push(name),
                None => groups.push((path, vec![name])),
            }
        }
        let rendered: String = groups
            .iter()
            .map(|(path, names)| match names.as_slice() {
                [one] => format!("use {path}.{one}\n"),
                _ => format!("use {path}.{{{}}}\n", names.join(", ")),
            })
            .collect();
        match (module.uses.first(), module.uses.last()) {
            (Some(first), Some(last)) => {
                let start = line_start(text, first.span.start as usize) as u32;
                let end = line_end(text, last.span.end as usize) as u32;
                edits.push(start, end, rendered);
            }
            _ => {
                let at = line_end(text, module.header.span.end as usize) as u32;
                edits.push(at, at, rendered);
            }
        }
    }
    // Declarations: keep, re-render, or delete each host declaration; then insert arrivals.
    let present = host.declarations(input.file);
    let staying: BTreeSet<_> = present
        .iter()
        .filter(|id| plans.get(*id).is_some_and(|p| p.file == input.path))
        .cloned()
        .collect();
    for id in &present {
        let decl = host.decl(id);
        match plans.get(id).filter(|p| p.file == input.path) {
            Some(plan) => {
                let rendered = render(id, plan, l, r, names)?;
                if rendered != text[decl.span.start as usize..decl.span.end as usize] {
                    edits.push(decl.span.start, decl.span.end, rendered);
                }
            }
            None => {
                let (start, end) = deletion(text, decl);
                edits.push(start, end, String::new());
            }
        }
    }
    // Arrivals: declarations placed here that the host file lacks, in the order of the side whose
    // copy of this file holds them, each after its nearest preceding staying declaration.
    let mut arrivals = Vec::new();
    for (id, plan) in plans
        .iter()
        .filter(|(id, p)| p.file == input.path && !staying.contains(*id))
    {
        let placer = [l, r]
            .into_iter()
            .find(|s| {
                s.files
                    .get(input.path)
                    .is_some_and(|f| s.decl_file(id) == Some(*f))
            })
            .unwrap_or_else(|| side(plan.text, l, r));
        let order = placer
            .files
            .get(input.path)
            .map(|f| placer.declarations(*f))
            .unwrap_or_default();
        let position = order.iter().position(|x| x == id).unwrap_or(order.len());
        let anchor = order[..position]
            .iter()
            .rev()
            .find(|x| staying.contains(*x))
            .map(|x| line_end(text, host.decl(x).span.end as usize))
            .unwrap_or(text.len());
        let origin = side(plan.text, l, r);
        let decl = origin.decl(id);
        let source = origin.text(decl.span.file);
        let comments = decl.trivia.leading.first().map_or("", |c| {
            &source[line_start(source, c.span.start as usize)
                ..line_start(source, decl.span.start as usize)]
        });
        let mut block = String::new();
        if anchor > 0 && !text[..anchor].ends_with('\n') {
            block.push('\n');
        }
        block.push('\n');
        block.push_str(comments);
        block.push_str(&render(id, plan, l, r, names)?);
        block.push('\n');
        arrivals.push((anchor, position, block));
    }
    arrivals.sort_by_key(|a| (a.0, a.1));
    for (anchor, _, block) in arrivals {
        edits.push(anchor as u32, anchor as u32, block);
    }
    edits.apply(text, 0, text.len() as u32)
}

/// The merged identifier of every planned declaration: the name facet's origin wins, so a rename
/// on either side reaches text taken from the other.
fn merged_names(
    b: &Side,
    l: &Side,
    r: &Side,
    plans: &BTreeMap<StableId, Plan>,
) -> BTreeMap<StableId, String> {
    let mut names = BTreeMap::new();
    for (id, plan) in plans {
        let from = match (b.entry(id), l.entry(id), r.entry(id)) {
            (Some(eb), Some(el), Some(er)) => {
                match pick(
                    eb.facet(Facet::Name),
                    el.facet(Facet::Name),
                    er.facet(Facet::Name),
                ) {
                    Some(Origin::Right) => Hand::Right,
                    _ => Hand::Left,
                }
            }
            _ => plan.text,
        };
        if let Some(name) = side(from, l, r).ident(id) {
            names.insert(id.clone(), name.to_string());
        }
    }
    names
}

/// Declarations whose facets changed between two snapshots, closed over the reverse references of
/// every given program, so code one side added that depends on the other side's change is included.
/// Header-only changes are not roots here; verification rechecks them conservatively.
fn closure(before: &Snapshot, after: &Snapshot, graphs: &[&Snapshot]) -> BTreeSet<StableId> {
    let delta = cpl_index::semantic_diff(before.index().facet_map(), after.index().facet_map());
    let changed = delta.statements.into_iter().map(|d| d.id).collect();
    query::affected(graphs, &changed)
}

/// Merge `left` and `right`, both descended from `base`, then check and test the composition.
pub fn merge(
    base: &Snapshot,
    left: &Snapshot,
    right: &Snapshot,
    config: &VerifyConfig,
    options: &LoadOptions,
) -> Result<Merge, Diagnostic> {
    config.validate()?;
    let (b, l, r) = (Side::new(base), Side::new(left), Side::new(right));
    let mut conflicts = Vec::new();
    let mut decisions = Vec::new();
    let plans = plan(&b, &l, &r, &mut conflicts, &mut decisions);
    let idents = merged_names(&b, &l, &r, &plans);
    // The module of each declaration's merged placement: the module of the side holding it there.
    let modules: BTreeMap<StableId, String> = plans
        .iter()
        .filter_map(|(id, plan)| {
            [&l, &r]
                .into_iter()
                .find(|s| {
                    s.files
                        .get(&plan.file)
                        .is_some_and(|f| s.decl_file(id) == Some(*f))
                })
                .map(|s| s.p().module(s.p().def(s.defs[id]).module).path.join("."))
                .map(|m| (id.clone(), m))
        })
        .collect();
    let names = Names { idents, modules };
    // A grafted declaration that cannot be combined exactly is a structural conflict.
    for (id, plan) in plans.iter().filter(|(_, p)| p.graft.is_some()) {
        if let Err(d) = render(id, plan, &l, &r, &names) {
            conflicts.push(json!({"symbol": id, "facet": null, "status": "conflict",
                "reason": "both_sides_changed_declaration_level_facets", "detail": d.message}));
        }
    }
    let paths: BTreeSet<&String> = base
        .tree
        .files
        .keys()
        .chain(left.tree.files.keys())
        .chain(right.tree.files.keys())
        .collect();
    let mut tree = VirtualTree::default();
    let mut hosts = Vec::new();
    for path in paths {
        fn get<'s>(s: &'s Snapshot, path: &str) -> Option<&'s Vec<u8>> {
            s.tree.files.get(path)
        }
        let (in_b, in_l, in_r) = (
            b.files.contains_key(path),
            l.files.contains_key(path),
            r.files.contains_key(path),
        );
        // Inputs other than CPL modules merge as whole files.
        if !in_b && !in_l && !in_r {
            let chosen = match pick(get(base, path), get(left, path), get(right, path)) {
                Some(Origin::Right) => get(right, path),
                Some(_) => get(left, path),
                None => {
                    conflicts.push(json!({"file": path, "status": "conflict", "reason": "both_sides_changed_file"}));
                    continue;
                }
            };
            if let Some(bytes) = chosen {
                tree.files.insert(path.clone(), bytes.clone());
            }
            continue;
        }
        if !in_l && !in_r {
            continue;
        }
        if in_b && !(in_l && in_r) {
            // One side deleted the module file. Anything that still belongs here conflicts.
            let (survivor, hand) = if in_l { (&l, "left") } else { (&r, "right") };
            if survivor
                .declarations(survivor.files[path])
                .iter()
                .any(|id| plans.get(id).is_some_and(|p| &p.file == path))
            {
                conflicts.push(json!({"file": path, "status": "conflict", "reason": "file_removed_on_one_side_and_changed_on_the_other", "changed": hand}));
            }
            continue;
        }
        let (host, hand) = if in_l {
            (&l, Hand::Left)
        } else {
            (&r, Hand::Right)
        };
        hosts.push(FileInputs {
            path,
            host,
            hand,
            file: host.files[path],
        });
    }
    // A declaration placed in a file that neither side's copy hosts would be lost; refuse it.
    for (id, plan) in &plans {
        if !hosts.iter().any(|h| *h.path == plan.file) {
            conflicts.push(conflict(id, None, "destination_file_removed"));
        }
    }
    let graphs = [base, left, right];
    let both: BTreeSet<_> = closure(base, left, &graphs)
        .intersection(&closure(base, right, &graphs))
        .cloned()
        .collect();
    let mut report = MergeReport {
        base: base.digest(),
        left: left.digest(),
        right: right.digest(),
        merged: None,
        status: "conflict".into(),
        compatibility: "unknown".into(),
        accepted: false,
        decisions,
        conflicts,
        diagnostics: vec![],
        regressions: vec![],
        inherited_failures: vec![],
        interactions: both
            .iter()
            .map(
                |id| json!({"symbol": id, "status": "unknown", "reason": "affected_by_both_sides"}),
            )
            .collect(),
        evidence: None,
    };
    if !report.conflicts.is_empty() {
        let seal = seal(&report);
        return Ok(Merge {
            tree: None,
            snapshot: None,
            report,
            seal,
        });
    }
    for input in &hosts {
        let text = assemble(input, &b, &l, &r, &plans, &names)?;
        tree.files.insert(input.path.to_string(), text.into_bytes());
    }
    let snapshot = match Snapshot::new(tree.clone(), options) {
        Ok(s) => Some(s),
        Err(d) => {
            report.status = "check_failed".into();
            report.diagnostics = match d.data.get("diagnostics") {
                Some(Value::Array(items)) => items.clone(),
                _ => vec![json!({"code": d.code.as_str(), "message": d.message})],
            };
            None
        }
    };
    if let Some(merged) = &snapshot {
        let evidence = verify::verify(merged, Some(base), config)?;
        let tests = verify::test_config(config);
        let status_on = |s: &Snapshot, id: &StableId| {
            let p = &s.checked.program;
            p.by_stable_id(id)
                .map(|d| cpl_eval::run_test(p, d, &tests).status)
        };
        for record in evidence
            .tests
            .iter()
            .filter(|t| t.status == TestStatus::Failed)
        {
            let sides = [
                ("left", status_on(left, &record.symbol)),
                ("right", status_on(right, &record.symbol)),
            ];
            let failing: Vec<_> = sides
                .iter()
                .filter(|(_, s)| *s == Some(TestStatus::Failed))
                .map(|(name, _)| *name)
                .collect();
            if failing.is_empty() {
                let passes: Vec<_> = sides
                    .iter()
                    .filter(|(_, s)| s.is_some())
                    .map(|(n, _)| *n)
                    .collect();
                report.regressions.push(
                    json!({"symbol": record.symbol, "qualified_name": record.qualified_name,
                    "status": "regression", "passes_on": passes, "fault": record.fault}),
                );
            } else {
                report
                    .inherited_failures
                    .push(json!({"symbol": record.symbol, "fails_on": failing}));
            }
        }
        report.status = if !report.regressions.is_empty() {
            "regression"
        } else if !evidence.accepted {
            "failed"
        } else {
            "tested"
        }
        .into();
        report.accepted = report.status == "tested";
        if report.accepted {
            report.compatibility = if both.is_empty() {
                "independent"
            } else {
                "tested"
            }
            .into();
        }
        report.merged = Some(merged.digest());
        report.evidence = Some(evidence);
    }
    let seal = seal(&report);
    Ok(Merge {
        tree: Some(tree),
        snapshot,
        report,
        seal,
    })
}
