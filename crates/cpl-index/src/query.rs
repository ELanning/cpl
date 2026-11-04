//! The semantic index (`spec/toolchain.md` Section 6 and 1.8): what a query
//! names ([`Target`], 6.1) and asks for ([`IncludeSet`], 6.2), the
//! [`Index`] over a checked `Program` with its callers, dependencies, and
//! tests (6.5), and `cpl inspect`'s response (6.4, [`Index::inspect`]).
//!
//! An `Index` is rebuilt from scratch for each command (6.5: no on-disk
//! cache). Every symbol version in one response comes from the one
//! revision the index was built for, so no context from two snapshots is
//! combined (6.4).
//!
//! Readings of the specification taken here (each is cited at its item):
//!
//! - [`Target`] has a third variant, [`Target::Name`], for the bare names of
//!   6.1 (`paginate`), which neither a `SymbolPath` (it starts with a
//!   module path) nor a `StableId` can hold.
//! - A caller's `call_sites` are the call expressions for `Call` and
//!   `CapabilityOp` references (6.4's example spans `paginate(items,
//!   checked_size)`), and the reference's own span (the type, constructor,
//!   or field name) for the other kinds. A generator call of a `for`
//!   clause is an ordinary call (3.5: `gen.list(g)`) that the HIR lowers to
//!   a span-less `GenExpr` (T18), so its call expression comes from the
//!   syntax: the call whose callee is the reference.
//! - A symbol block carries `signature`, `contract`, and `effects` whenever
//!   the set includes them, for every kind: 6.4's table limits only
//!   `implementation` ("fn, test, property only") and
//!   `verification_policy` ("always for fn") by kind, and 6.2 makes "the
//!   response shape ... a function of the effective include set alone". A
//!   kind without the facet (5.1) has the header `test "<name>"` as its
//!   `signature` and `[]` as its `contract` or `effects`; `facets` shows
//!   which facets it has. `implementation` (with `holes`) stays limited to
//!   functions, tests, and properties.
//! - `holes` needs the check's hole reports (`CheckResult::holes`), which
//!   [`Index::build`] alone does not have: such an index refuses
//!   (`internal_error`, kind `hole_reports_missing`) to list the holes of a
//!   body that has some, rather than answer that it has none.

use crate::codes;
use crate::diff::semantic_diff;
use crate::facet::{
    definition_hash, facet_hash, facet_text, module_content_hash, symbol_version, FacetMap,
};
use crate::obligations::{self, clause_text};
use crate::receipt::{receipt_inputs, ReceiptInputs};
use crate::syntax;
use crate::walk::{self, Visitor};
use cpl_diag::json::{
    Digest, InspectChanges, InspectOutput, Location, Obligation, Relation, Revision, SymbolBlock,
    SymbolVersion, VerificationPolicyJson,
};
use cpl_diag::{
    quote_text, Category, DeclName, Diagnostic, Facet, FileOrigin, Severity, Span, StableId,
    SymbolPath,
};
use cpl_project::Project;
use cpl_sema::hir::{
    Block, Def, DefId, DefKind, DefStatus, Expr, ExprKind, FnBody, ModuleId, Program, RefKind,
};
use cpl_sema::{CheckResult, HoleReport};
use cpl_syntax::ast::{DeclKind, Declaration, TypeDeclKind};
use cpl_syntax::{format_fragment, FileKind, Fragment, FragmentMode};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;

pub use cpl_diag::json::Include;

// ---------------------------------------------------------------------------
// 6.1 Targets
// ---------------------------------------------------------------------------

/// The target of a query (Section 6.1).
///
/// | Variant | 6.1 form | Resolution ([`Index::resolve`]) |
/// | - | - | - |
/// | `Id` | a stable id (`pgn_7c2d`) or a provisional id (`reader.pages.clean`) | `Program::by_stable_id`; a provisional id that names no entry is then read as its qualified name, and an `@id` spelling as a bare name (row 1: "tried first ... and an entry exists") |
/// | `Path` | a qualified symbol path (`reader.pages.paginate`, `tests.pagination."pagination handles empty input"`, `std.prelude.List.take`) | `Program::lookup`, then the provisional id it spells |
/// | `Name` | a bare name (`paginate`, `List.take`, `"pagination handles empty input"`) | every definition so named in the project and std: one resolves, several are `index_ambiguous_symbol`, none `index_unknown_symbol` |
///
/// `Name` is an addition to the two variants of Section 1.8: a bare name is
/// no `SymbolPath` (which starts with a module path) and, without an
/// underscore, no `StableId` either.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Target {
    Path(SymbolPath),
    Id(StableId),
    Name(DeclName),
}

impl Target {
    /// The `<symbol>` argument of `cpl inspect` and `cpl prove`, read by
    /// the rows of 6.1 in order: an accepted `@id` spelling (Section 4.1) is
    /// an [`Target::Id`]; a qualified symbol path (Section 4.4) a
    /// [`Target::Path`]; an identifier, a `Receiver.member` pair, or a
    /// quoted test or property name without a module a [`Target::Name`];
    /// anything else, which no identifier or path spells, can only be a
    /// test or property name written without its quotes (`pagination
    /// handles empty input`, as a shell passes it) and is the `Name` of that
    /// text. Every argument is a target; one that names nothing is
    /// `index_unknown_symbol` when it is resolved.
    pub fn parse(arg: &str) -> Target {
        if let Ok(id) = StableId::from_annotation(arg) {
            return Target::Id(id);
        }
        if let Ok(path) = SymbolPath::parse(arg) {
            return Target::Path(path);
        }
        // A bare name is the final segment of a path in some module.
        if let Ok(path) = SymbolPath::parse(&format!("m.{arg}")) {
            if path.module.len() == 1 {
                return Target::Name(path.name);
            }
        }
        Target::Name(DeclName::Text(arg.to_string()))
    }
}

impl fmt::Display for Target {
    /// The spelling diagnostics quote: the path, the id, or the bare name
    /// (a path built without a module, which `SymbolPath::parse` never
    /// returns, is its name).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Target::Path(path) if path.module.is_empty() => f.write_str(&path.name.display()),
            Target::Path(path) => f.write_str(&path.display()),
            Target::Id(id) => f.write_str(id.as_str()),
            Target::Name(name) => f.write_str(&name.display()),
        }
    }
}

// ---------------------------------------------------------------------------
// 6.2 Include sets
// ---------------------------------------------------------------------------

/// The items a response includes (Section 6.2).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct IncludeSet {
    pub items: BTreeSet<Include>,
}

/// The items `--include` may name, in the order of the Section 6.2 grammar.
/// `interface_dependencies` is not among them: the default set implies it
/// and the command line never names it.
const NAMEABLE: [Include; 10] = [
    Include::Signature,
    Include::Contract,
    Include::Effects,
    Include::Implementation,
    Include::Docs,
    Include::Callers,
    Include::Dependencies,
    Include::Obligations,
    Include::Tests,
    Include::Changes,
];

impl IncludeSet {
    /// The `--include` list (Section 6.2): `include_item ("," include_item)*`
    /// with no spaces, each one of `signature`, `contract`, `effects`,
    /// `implementation`, `docs`, `callers`, `dependencies`, `obligations`,
    /// `tests`, `changes`; a repeated item counts once. `Err` carries the
    /// message of the CLI's `usage_bad_argument`.
    pub fn parse(csv: &str) -> Result<IncludeSet, String> {
        let mut items = BTreeSet::new();
        for item in csv.split(',') {
            let include = NAMEABLE
                .into_iter()
                .find(|i| i.as_str() == item)
                .ok_or_else(|| {
                    let known: Vec<&str> = NAMEABLE.iter().map(|i| i.as_str()).collect();
                    if item.is_empty() {
                        format!(
                            "empty item in the include list `{csv}`; expected a comma-separated list of {}",
                            known.join(", ")
                        )
                    } else {
                        format!(
                            "unknown include item `{item}`; expected one of {}",
                            known.join(", ")
                        )
                    }
                })?;
            items.insert(include);
        }
        Ok(IncludeSet { items })
    }

    /// The set without `--include`: `signature`, `contract`, `effects`,
    /// `docs`, and `interface_dependencies` (Section 6.2, T17).
    pub fn default_set() -> IncludeSet {
        IncludeSet {
            items: [
                Include::Signature,
                Include::Contract,
                Include::Effects,
                Include::Docs,
                Include::InterfaceDependencies,
            ]
            .into_iter()
            .collect(),
        }
    }

    pub fn contains(&self, item: Include) -> bool {
        self.items.contains(&item)
    }
}

// ---------------------------------------------------------------------------
// The index
// ---------------------------------------------------------------------------

/// One dependency of a definition (Section 6.5): the referenced definition
/// with the kind and interface flag of the first reference to it, in span
/// order.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct Dependency {
    pub def: DefId,
    pub kind: RefKind,
    /// The first reference is in the signature, contract, or effects (T17).
    pub interface: bool,
}

/// The semantic index of one checked program at one revision (Section 6).
///
/// Build it with [`Index::from_check`] (or [`Index::build`] and
/// [`Index::with_holes`]) from the project the program was checked from;
/// add the earlier revision `changes` compares with by
/// [`Index::with_changes_base`]. Without the check's hole reports, the
/// `holes` of an implementation that has some are refused
/// ([`Index::inspect`]).
#[derive(Clone, Debug)]
pub struct Index<'p> {
    program: &'p Program,
    /// Everything computed from the program and the project, owned or lent by a cache
    /// ([`Index::with_data`]).
    data: std::borrow::Cow<'p, IndexData>,
    /// The facet map `changes` compares with.
    changes_base: Option<FacetMap>,
    /// The check's hole reports; `None` until [`Index::with_holes`].
    holes: Option<&'p [HoleReport]>,
}

/// The part of an [`Index`] that borrows nothing: computed once per checked program
/// ([`IndexData::build`], [`IndexData::from_check`]) and lent to any number of indexes.
#[derive(Clone, Debug)]
pub struct IndexData {
    revision: Revision,
    /// Per `DefId`: the definition's facet versions.
    versions: Vec<SymbolVersion>,
    /// Per `DefId`: the indices of the references to it.
    incoming: Vec<Vec<usize>>,
    /// The content hash of every project module (semantics S48).
    module_hashes: BTreeMap<ModuleId, Digest>,
    /// The facet map of the revision (Section 5.4), for `changes`.
    facets: FacetMap,
    /// Some definition failed to check, so the reference graph may lack
    /// edges (`index_partial`).
    partial: bool,
    pub(crate) inputs: ReceiptInputs,
    /// `[verification] proof_timeout_ms`.
    pub(crate) proof_timeout_ms: u64,
    /// `[verification] admit`, as stable ids.
    pub(crate) admitted: Vec<StableId>,
    /// The number of `.cpl` files of the source set.
    pub(crate) source_files: u32,
}

impl IndexData {
    /// The owned part of [`Index::build`].
    pub fn build(program: &Program, project: &Project, revision: Revision) -> IndexData {
        // The facet map and the versions describe the same declarations (the program shares the
        // project's parsed modules), so each declaration's canonical texts are hashed once.
        let mut raw: std::collections::HashMap<*const Declaration, SymbolVersion> =
            std::collections::HashMap::with_capacity(program.defs.len());
        let versions: Vec<SymbolVersion> = program
            .defs()
            .map(|def| {
                let decl = program.ast(def.id);
                let module = &program.module(def.module).path;
                let text = program.source_map.file(decl.span.file).text();
                let version = crate::facet::symbol_version_cached(
                    module,
                    decl,
                    &text[decl.span.start as usize..decl.span.end as usize],
                );
                raw.insert(decl as *const Declaration, version.clone());
                adjust_version(version, def)
            })
            .collect();
        let facets =
            crate::facet::facet_map_using(project, revision.clone(), &mut |module, decl| {
                raw.get(&(decl as *const Declaration))
                    .cloned()
                    .unwrap_or_else(|| symbol_version(module, decl))
            });
        let mut incoming = vec![Vec::new(); program.defs.len()];
        for (i, r) in program.references.iter().enumerate() {
            if let Some(list) = incoming.get_mut(r.to.0 as usize) {
                list.push(i);
            }
        }
        let module_hashes = program
            .modules()
            .filter(|m| m.origin != FileOrigin::Std)
            .map(|m| (m.id, module_content_hash(project, &m.files)))
            .collect();
        let verification = &project.manifest.verification;
        let admitted = verification
            .admit
            .iter()
            .filter_map(|id| StableId::parse(id).ok())
            .collect();
        let source_files = project
            .files
            .iter()
            .filter(|entry| entry.origin != FileOrigin::Std && entry.kind == FileKind::Cpl)
            .count() as u32;
        let partial =
            program.defs.is_empty() || program.defs().any(|d| d.status == DefStatus::HasErrors);
        IndexData {
            versions,
            incoming,
            module_hashes,
            facets,
            partial,
            inputs: receipt_inputs(project, revision.clone()),
            proof_timeout_ms: verification.proof_timeout_ms,
            admitted,
            source_files,
            revision,
        }
    }

    /// The owned part of [`Index::from_check`]: partial when the check reported an error.
    pub fn from_check(check: &CheckResult, project: &Project, revision: Revision) -> IndexData {
        let mut data = IndexData::build(&check.program, project, revision);
        data.partial |= check
            .diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error);
        data
    }
}

impl std::ops::Deref for Index<'_> {
    type Target = IndexData;
    fn deref(&self) -> &IndexData {
        &self.data
    }
}

impl<'p> Index<'p> {
    /// The index of `program`, which must have been checked from `project`
    /// (they share the source map), at `revision` (Section 6.3), with the
    /// version of every definition of the project and of std, the reverse
    /// reference table, the content hash of every project module, the
    /// revision's facet map, and the receipt inputs.
    ///
    /// Versions are `symbol_version` of each definition's syntax (Section
    /// 5.4), except that the later occurrence of a duplicated `@id` has the
    /// provisional identity the `Program` gives it (Section 4.5): its `id`,
    /// `provisional` flag, and `identity` hash are those of that identity.
    pub fn build(program: &'p Program, project: &Project, revision: Revision) -> Index<'p> {
        Index {
            program,
            data: std::borrow::Cow::Owned(IndexData::build(program, project, revision)),
            changes_base: None,
            holes: None,
        }
    }

    /// [`Index::build`] over a check's program, with its hole reports
    /// ([`Index::with_holes`]); the index is partial when the check
    /// reported an error.
    pub fn from_check(check: &'p CheckResult, project: &Project, revision: Revision) -> Index<'p> {
        Index {
            program: &check.program,
            data: std::borrow::Cow::Owned(IndexData::from_check(check, project, revision)),
            changes_base: None,
            holes: None,
        }
        .with_holes(&check.holes)
    }

    /// An index of `program` over data computed for it once ([`IndexData::build`] or
    /// [`IndexData::from_check`] of the same program and project); nothing is recomputed.
    pub fn with_data(program: &'p Program, data: &'p IndexData) -> Index<'p> {
        Index {
            program,
            data: std::borrow::Cow::Borrowed(data),
            changes_base: None,
            holes: None,
        }
    }

    /// The hole reports of the check (`CheckResult::holes`), which the
    /// `holes` of an `implementation` read and by which
    /// [`Index::complete_check_diagnostics`] finds a hole's declaration.
    pub fn with_holes(mut self, holes: &'p [HoleReport]) -> Index<'p> {
        self.holes = Some(holes);
        self
    }

    /// The facet map of the revision `changes` compares with (`--since`,
    /// [`crate::changes_base`]).
    pub fn with_changes_base(mut self, base: FacetMap) -> Index<'p> {
        self.changes_base = Some(base);
        self
    }

    pub fn program(&self) -> &'p Program {
        self.program
    }

    /// The revision every version of this index belongs to.
    pub fn revision(&self) -> &Revision {
        &self.revision
    }

    /// The facet map of the revision (Section 5.4).
    pub fn facet_map(&self) -> &FacetMap {
        &self.facets
    }

    /// The receipt inputs of the project at the revision (Section 8).
    pub fn receipt_inputs(&self) -> &ReceiptInputs {
        &self.inputs
    }

    /// The hole reports given with [`Index::with_holes`] (none without).
    pub fn holes(&self) -> &'p [HoleReport] {
        self.holes.unwrap_or_default()
    }

    /// The content hash of a project module (semantics S48); `None` for a
    /// std module, which `toolchain.std` covers.
    pub fn module_hash(&self, module: ModuleId) -> Option<&Digest> {
        self.module_hashes.get(&module)
    }

    /// The facet versions of a definition. Panics on an id out of range.
    pub fn version(&self, def: DefId) -> &SymbolVersion {
        &self.versions[def.0 as usize]
    }

    /// The warning `index_partial` (Section 7.2 `cpl inspect`) when the
    /// project does not check without errors, so that callers,
    /// dependencies, and tests may be incomplete; `None` otherwise.
    pub fn partial(&self) -> Option<Diagnostic> {
        self.partial.then(|| {
            Diagnostic::warning(
                codes::INDEX_PARTIAL,
                codes::INDEX,
                "the project has errors, so callers, dependencies, and tests may be incomplete",
            )
        })
    }

    // -----------------------------------------------------------------------
    // 6.1 Resolution
    // -----------------------------------------------------------------------

    /// The definition a target names (Section 6.1; [`Target`] gives the
    /// rule per form): `index_unknown_symbol` when it names none, and
    /// `index_ambiguous_symbol` (with `candidates`, the qualified names in
    /// order, and a note at each declaration) when a bare name names
    /// several. Definitions sharing one qualified name (a
    /// `duplicate_declaration`) count once, as the first, which is what
    /// `Program::lookup` finds.
    pub fn resolve(&self, target: &Target) -> Result<DefId, Diagnostic> {
        let program = self.program;
        match target {
            Target::Id(id) => {
                if let Some(def) = program.by_stable_id(id) {
                    return Ok(def);
                }
                if id.is_provisional() {
                    return SymbolPath::parse(id.as_str())
                        .ok()
                        .and_then(|path| program.lookup(&path))
                        .ok_or_else(|| self.unknown(target));
                }
                // Section 4.1's accepted syntax is a subset of the identifiers,
                // so an `@id` spelling without an entry is a bare name.
                self.resolve_name(&DeclName::Ident(id.as_str().to_string()), target)
            }
            // A path without a module is a bare name (6.1 row 4).
            Target::Path(path) if path.module.is_empty() => {
                self.resolve_name(&path.name, &Target::Name(path.name.clone()))
            }
            Target::Path(path) => program
                .lookup(path)
                .or_else(|| {
                    program.by_stable_id(&StableId::provisional(&path.module_path(), &path.name))
                })
                .ok_or_else(|| self.unknown(target)),
            Target::Name(name) => self.resolve_name(name, target),
        }
    }

    /// Every definition a bare name names (6.1 row 4).
    fn resolve_name(&self, name: &DeclName, target: &Target) -> Result<DefId, Diagnostic> {
        let mut seen = BTreeSet::new();
        let mut candidates: Vec<DefId> = self
            .program
            .defs()
            .filter(|def| bare_name_matches(&def.name, name))
            .filter(|def| seen.insert(self.version(def.id).path.clone()))
            .map(|def| def.id)
            .collect();
        self.sort_by_path(&mut candidates);
        match candidates.as_slice() {
            [] => Err(self.unknown(target)),
            [one] => Ok(*one),
            many => Err(self.ambiguous(target, many)),
        }
    }

    fn unknown(&self, target: &Target) -> Diagnostic {
        let name = target.to_string();
        let mut d = Diagnostic::error(
            codes::INDEX_UNKNOWN_SYMBOL,
            codes::INDEX,
            format!("no declaration `{name}` in the project or the standard library"),
        )
        .with_data("name", json!(name));
        if let Target::Path(path) = target {
            let program = self.program;
            match program.module_by_path(&path.module) {
                None => {
                    d = d.with_note(format!("there is no module `{}`", path.module_path()), None);
                }
                Some(_) => {
                    if let DeclName::Member { receiver, name } = &path.name {
                        let owner =
                            SymbolPath::new(path.module.clone(), DeclName::Ident(receiver.clone()));
                        let is_operation = program
                            .lookup(&owner)
                            .and_then(|d| program.def(d).kind.as_capability())
                            .is_some_and(|c| c.ops.iter().any(|op| &op.name == name));
                        if is_operation {
                            d = d.with_note(
                                format!(
                                    "`{}` is an operation of the capability `{}`, which is the declaration to query",
                                    path.display(),
                                    owner.display()
                                ),
                                None,
                            );
                        }
                    }
                }
            }
        }
        d
    }

    fn ambiguous(&self, target: &Target, candidates: &[DefId]) -> Diagnostic {
        let name = target.to_string();
        let paths: Vec<String> = candidates
            .iter()
            .map(|&d| self.version(d).path.clone())
            .collect();
        let mut d = Diagnostic::error(
            codes::INDEX_AMBIGUOUS_SYMBOL,
            codes::INDEX,
            format!(
                "`{name}` names {} declarations: {}; name one by its qualified name",
                paths.len(),
                paths.join(", ")
            ),
        )
        .with_data("name", json!(name))
        .with_data("candidates", json!(paths));
        for (&def, path) in candidates.iter().zip(&paths) {
            let span = name_span(self.program.ast(def));
            d = d.with_note(format!("`{path}` is declared here"), Some(span));
        }
        d
    }

    // -----------------------------------------------------------------------
    // 6.5 Callers, dependencies, tests
    // -----------------------------------------------------------------------

    /// Every definition that references `def` (Section 6.5: the distinct
    /// `from` of the references to it, of every kind), in display-path
    /// order. A recursive definition is its own caller.
    pub fn callers(&self, def: DefId) -> Vec<DefId> {
        let mut out: Vec<DefId> = self.incoming[def.0 as usize]
            .iter()
            .map(|&i| self.program.references[i].from)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        self.sort_by_path(&mut out);
        out
    }

    /// Every definition `def` references (Section 6.5), interface and body,
    /// each with the kind and interface flag of its first reference in span
    /// order, in display-path order.
    pub fn dependencies(&self, def: DefId) -> Vec<Dependency> {
        let mut seen = BTreeSet::new();
        let mut out: Vec<Dependency> = self
            .program
            .references_from(def)
            .iter()
            .filter(|r| seen.insert(r.to))
            .map(|r| Dependency {
                def: r.to,
                kind: r.kind,
                interface: r.interface,
            })
            .collect();
        out.sort_by(|a, b| self.path_order(a.def, b.def));
        out
    }

    /// The dependencies whose first reference is an interface reference
    /// (T17): the default inspect set's `interface_dependencies`.
    pub fn interface_dependencies(&self, def: DefId) -> Vec<Dependency> {
        self.dependencies(def)
            .into_iter()
            .filter(|d| d.interface)
            .collect()
    }

    /// Every test and property whose static call closure reaches `def`
    /// (Section 6.5: `def` is reached from it by `Call` and `CapabilityOp`
    /// edges), in display-path order.
    pub fn tests_for(&self, def: DefId) -> Vec<DefId> {
        let mut seen = BTreeSet::from([def]);
        let mut queue = VecDeque::from([def]);
        let mut tests = Vec::new();
        while let Some(d) = queue.pop_front() {
            for &i in &self.incoming[d.0 as usize] {
                let r = &self.program.references[i];
                if !is_call_edge(r.kind) || !seen.insert(r.from) {
                    continue;
                }
                if self.program.def(r.from).kind.is_test() {
                    tests.push(r.from);
                }
                queue.push_back(r.from);
            }
        }
        self.sort_by_path(&mut tests);
        tests
    }

    /// `direct` when `test` itself calls `def` (a `Call` or `CapabilityOp`
    /// reference), else `transitive` (the `relation` of a `tests` entry).
    fn test_relation(&self, test: DefId, def: DefId) -> Relation {
        let direct = self
            .program
            .references_from(test)
            .iter()
            .any(|r| r.to == def && is_call_edge(r.kind));
        if direct {
            Relation::Direct
        } else {
            Relation::Transitive
        }
    }

    /// The static call closure of `roots`: the roots and every definition
    /// they reach by `Call` and `CapabilityOp` edges (the edges of
    /// `tests_for`, Section 6.5), the closure of semantics S59's receipt
    /// `modules` and of a test receipt's `tested` (Section 8.2).
    pub fn call_closure(&self, roots: &[DefId]) -> BTreeSet<DefId> {
        let mut seen: BTreeSet<DefId> = roots.iter().copied().collect();
        let mut queue: VecDeque<DefId> = roots.iter().copied().collect();
        while let Some(d) = queue.pop_front() {
            for r in self.program.references_from(d) {
                if is_call_edge(r.kind) && seen.insert(r.to) {
                    queue.push_back(r.to);
                }
            }
        }
        seen
    }

    /// The proof obligations of a function (Section 8.1), in the order of
    /// this table, each with an id stable across runs for an unchanged
    /// definition:
    ///
    /// | Obligation id | `kind` | `clause` | `location` |
    /// | - | - | - | - |
    /// | `<id>#ensures/<i>` | `ensures` | the clause text | the clause, from its keyword |
    /// | `<id>#decreases` | `decreases` | the measure | the clause |
    /// | `<id>#termination` (a recursive function without a measure) | `termination` | `""`: there is no clause | the function's name |
    /// | `<id>#loop/<n>/invariant/<i>/init`, `.../preserve` | `invariant_init`, `invariant_preserve` | the invariant | the clause |
    /// | `<id>#loop/<n>/decreases` | `loop_decreases` | the measure | the clause |
    /// | `<id>#call/<n>/requires/<i>` | `call_requires` | the callee's clause | the call |
    /// | `<id>#div/<n>` | `division` | `<divisor> != 0` | the division |
    /// | `<id>#refine/<n>` | `refinement` | the refinement's predicate | the coerced expression |
    /// | `<id>#assert/<n>` | `assert` | the asserted expression | the statement, from `assert` |
    ///
    /// `<i>` counts clauses in canonical order and `<n>` counts in body
    /// order: the function's `requires`, `ensures`, and `decreases` clauses
    /// first, then its body (pre-order, as `cpl_hir`'s `Expr::walk`), so
    /// that a division in a clause is numbered as 8.1 says ("body order,
    /// clauses included"). Calls are counted the same way, since a call in
    /// a clause has preconditions that clause's evaluation relies on, and
    /// every call site counts (`CapCall`s included) whether or not its
    /// callee has a precondition, so that `<n>` depends on the function's
    /// own text alone.
    ///
    /// Milestone 1 has no verifier: every obligation is `unknown` with
    /// reason `verifier_unavailable`, except `termination` (`unknown`,
    /// reason `no_measure`) and `refinement`, which the type checker
    /// established by the constant proof of semantics S16 (T39): `proved`,
    /// with evidence `{"by": "literal"}`. Only functions with a body have
    /// obligations ("obligations of a function"): a `@builtin` stub's
    /// clauses are axioms about it (semantics S61, stdlib S41).
    pub fn obligations(&self, def: DefId) -> Vec<Obligation> {
        obligations::of(self, def)
    }

    /// Where `caller` references `def`, in span order: for a call, the call
    /// expression (`paginate(items, checked_size)`, `xs.take(1)`, `xs.len`);
    /// for any other reference, its own span (the type, constructor, or
    /// field name). Equal sites (`check<Positive>` is a type use and a
    /// check) count once.
    ///
    /// A call expression is the smallest call of `def` in the HIR that
    /// contains the reference, else the call of the syntax whose callee is
    /// the reference: the generator calls of a `for` clause
    /// (`gen.range(0, limit())`, `gen.list(gen.int)`) are ordinary calls
    /// (3.5) that the HIR keeps only as span-less `GenExpr`s (T18).
    pub fn call_sites(&self, caller: DefId, def: DefId) -> Vec<Location> {
        let mut calls = CallExprs::default();
        walk::definition(self.program.def(caller), &mut calls);
        let mut syntax_calls: Option<Vec<(Span, Span)>> = None;
        let mut spans: Vec<Span> = Vec::new();
        for r in self.program.references_from(caller) {
            if r.to != def {
                continue;
            }
            let site = match r.kind {
                RefKind::Call | RefKind::CapabilityOp => {
                    let lowered = calls
                        .sites
                        .iter()
                        .filter(|(target, span)| *target == def && contains(*span, r.span))
                        .map(|(_, span)| *span)
                        .min_by_key(|span| span.len());
                    lowered.or_else(|| {
                        syntax_calls
                            .get_or_insert_with(|| syntax::calls(self.program.ast(caller)))
                            .iter()
                            .find(|(callee, _)| *callee == r.span)
                            .map(|(_, call)| *call)
                    })
                }
                _ => None,
            }
            .unwrap_or(r.span);
            if !spans.contains(&site) {
                spans.push(site);
            }
        }
        spans.sort();
        let sm = self.program.source_map();
        spans.into_iter().map(|s| sm.location(s)).collect()
    }

    // -----------------------------------------------------------------------
    // 6.4 Responses
    // -----------------------------------------------------------------------

    /// The `cpl inspect` response for `target` (Sections 6.2, 6.4): the
    /// target's symbol block with the facet texts the set includes, and one
    /// list per included item. Dependency blocks (both lists) carry their
    /// `signature` and `contract` (6.2) and, when the set includes them,
    /// `effects` and `docs` (so the default set gives each "the same
    /// four"); caller and test blocks carry the version, kind, and location
    /// with `call_sites` or `relation`.
    ///
    /// Errors: the target's resolution ([`Index::resolve`]); with
    /// `implementation` included, a target whose body has holes, and an
    /// index without the check's hole reports ([`Index::build`] without
    /// [`Index::with_holes`]), `internal_error` (kind
    /// `hole_reports_missing`): the holes inside the body (6.4) are the
    /// check's reports, which the program alone does not hold; with
    /// `changes` included and no base given ([`Index::with_changes_base`]),
    /// `git_not_a_repository`, the reason a command has none.
    pub fn inspect(
        &self,
        target: &Target,
        include: &IncludeSet,
    ) -> Result<InspectOutput, Diagnostic> {
        let def = self.resolve(target)?;
        if include.contains(Include::Implementation)
            && self.holes.is_none()
            && self.body_has_holes(def)
        {
            return Err(hole_reports_missing(&self.version(def).path));
        }
        let changes = if include.contains(Include::Changes) {
            Some(self.changes(def)?)
        } else {
            None
        };
        let has = |item| include.contains(item);
        let own = BlockFacets {
            signature: has(Include::Signature),
            contract: has(Include::Contract),
            effects: has(Include::Effects),
            docs: has(Include::Docs),
            implementation: has(Include::Implementation),
        };
        let dependency = BlockFacets {
            signature: true,
            contract: true,
            effects: has(Include::Effects),
            docs: has(Include::Docs),
            implementation: false,
        };
        let interface_dependencies = has(Include::InterfaceDependencies).then(|| {
            self.interface_dependencies(def)
                .into_iter()
                .map(|dep| SymbolBlock {
                    relation: Some(dep.kind.into()),
                    ..self.symbol_block(dep.def, dependency)
                })
                .collect()
        });
        let dependencies = has(Include::Dependencies).then(|| {
            self.dependencies(def)
                .into_iter()
                .map(|dep| SymbolBlock {
                    relation: Some(dep.kind.into()),
                    interface: Some(dep.interface),
                    ..self.symbol_block(dep.def, dependency)
                })
                .collect()
        });
        let callers = has(Include::Callers).then(|| {
            self.callers(def)
                .into_iter()
                .map(|caller| SymbolBlock {
                    call_sites: Some(self.call_sites(caller, def)),
                    ..self.symbol_block(caller, BlockFacets::default())
                })
                .collect()
        });
        let tests = has(Include::Tests).then(|| {
            self.tests_for(def)
                .into_iter()
                .map(|test| SymbolBlock {
                    relation: Some(self.test_relation(test, def)),
                    ..self.symbol_block(test, BlockFacets::default())
                })
                .collect()
        });
        Ok(InspectOutput {
            include: include.items.iter().copied().collect(),
            symbol: self.symbol_block(def, own),
            interface_dependencies,
            dependencies,
            callers,
            obligations: has(Include::Obligations).then(|| self.obligations(def)),
            tests,
            changes,
        })
    }

    /// The `changes` of a response: the statements of the semantic diff
    /// from the base to this revision whose `id` is the target's.
    fn changes(&self, def: DefId) -> Result<InspectChanges, Diagnostic> {
        let base = self
            .changes_base
            .as_ref()
            .ok_or_else(crate::history::no_repository)?;
        let id = &self.version(def).id;
        let statements = semantic_diff(base, &self.facets)
            .statements
            .into_iter()
            .filter(|s| &s.id == id)
            .collect();
        Ok(InspectChanges {
            since: base.revision.clone(),
            statements,
        })
    }

    /// A definition's symbol block (Section 6.4) with the facet texts
    /// `facets` selects: `signature`, `contract`, `effects`, and `docs` for
    /// every kind, `implementation` and its `holes` for the kinds with a
    /// body (module docs).
    pub(crate) fn symbol_block(&self, def: DefId, facets: BlockFacets) -> SymbolBlock {
        let program = self.program;
        let d = program.def(def);
        let decl = program.ast(def);
        let module = &program.module(d.module).path;
        let with_body = facets.implementation
            && crate::facet::applicable_facets(&decl.kind).contains(&Facet::Body);
        SymbolBlock {
            version: self.version(def).clone(),
            kind: d.kind.id_kind().to_string(),
            module: module.join("."),
            public: d.is_pub,
            location: program.source_map().location(d.span),
            verification_policy: matches!(d.kind, DefKind::Fn(_)).then_some(
                VerificationPolicyJson {
                    prove: d.policy.prove,
                },
            ),
            signature: facets.signature.then(|| signature_text(decl)),
            contract: facets.contract.then(|| contract_list(decl)),
            effects: facets.effects.then(|| effect_list(decl)),
            docs: facets
                .docs
                .then(|| facet_text(module, decl, Facet::Docs).filter(|text| !text.is_empty())),
            implementation: with_body
                .then(|| facet_text(module, decl, Facet::Body).unwrap_or_default()),
            holes: with_body.then(|| {
                self.holes()
                    .iter()
                    .filter(|h| h.def == def)
                    .map(|h| self.hole_report_json(h))
                    .collect()
            }),
            relation: None,
            interface: None,
            call_sites: None,
        }
    }

    /// Whether the body of a function, test, or property holds a hole
    /// (an `ExprKind::Hole` of the HIR).
    fn body_has_holes(&self, def: DefId) -> bool {
        let body: Option<&Block> = match &self.program.def(def).kind {
            DefKind::Fn(f) => match &f.body {
                FnBody::Block(b) => Some(b),
                FnBody::Builtin(_) => None,
            },
            DefKind::Test(t) => Some(&t.body),
            DefKind::Property(p) => Some(&p.body),
            _ => None,
        };
        let mut holes = Holes::default();
        if let Some(b) = body {
            walk::block(b, &mut holes);
        }
        holes.found
    }

    /// Display-path order, then id, then `DefId` (Section 6.5: "Lists are
    /// sorted by display path").
    pub(crate) fn sort_by_path(&self, defs: &mut [DefId]) {
        defs.sort_by(|&a, &b| self.path_order(a, b));
    }

    fn path_order(&self, a: DefId, b: DefId) -> std::cmp::Ordering {
        let (va, vb) = (self.version(a), self.version(b));
        va.path
            .cmp(&vb.path)
            .then_with(|| va.id.cmp(&vb.id))
            .then(a.cmp(&b))
    }
}

/// Which facet texts a symbol block carries.
#[derive(Copy, Clone, Debug, Default)]
pub(crate) struct BlockFacets {
    pub signature: bool,
    pub contract: bool,
    pub effects: bool,
    pub docs: bool,
    pub implementation: bool,
}

/// The `Call` and `CapabilityOp` edges of Section 6.5's call closure.
fn is_call_edge(kind: RefKind) -> bool {
    matches!(kind, RefKind::Call | RefKind::CapabilityOp)
}

/// `outer` covers `inner` (same file).
fn contains(outer: Span, inner: Span) -> bool {
    outer.file == inner.file && outer.start <= inner.start && inner.end <= outer.end
}

/// A definition's version (Section 5.4) under the `Program`'s identity
/// (Section 4.5; [`Index::build`]).
/// A definition's version from its declaration's: the later occurrence of a duplicated `@id`
/// carries the provisional identity the program gives it (Section 4.5).
fn adjust_version(mut version: SymbolVersion, def: &Def) -> SymbolVersion {
    if version.id != def.stable_id {
        version.id = def.stable_id.clone();
        version.provisional = def.stable_id.is_provisional();
        version
            .facets
            .insert(Facet::Identity, facet_hash(def.stable_id.as_str()));
        version.definition = definition_hash(&version.facets);
    }
    version
}

/// Whether a definition's name answers a bare name (6.1 row 4): an
/// identifier names a declaration of that name, a std member of that name
/// (`take` names `std.prelude.List.take`), and a test or property with
/// that name text; a `Receiver.member` pair names that member; a text names
/// a test or property.
fn bare_name_matches(def: &DeclName, name: &DeclName) -> bool {
    match (def, name) {
        (DeclName::Ident(a), DeclName::Ident(b)) | (DeclName::Text(a), DeclName::Ident(b)) => {
            a == b
        }
        (DeclName::Member { name: a, .. }, DeclName::Ident(b)) => a == b,
        (DeclName::Text(a), DeclName::Text(b)) => a == b,
        (
            DeclName::Member { receiver, name },
            DeclName::Member {
                receiver: r,
                name: n,
            },
        ) => receiver == r && name == n,
        _ => false,
    }
}

/// The span of a declaration's name token (the `TEXT` name of a test or
/// property), where declaration-level diagnostics point (semantics S63 (3)).
pub(crate) fn name_span(decl: &Declaration) -> Span {
    match &decl.kind {
        DeclKind::Test(t) => t.name.span,
        DeclKind::Property(p) => p.name.span,
        _ => decl.name().map_or(decl.span, |n| n.span),
    }
}

/// The `signature` of a symbol block: "the header with its name" (6.4),
/// the `signature` facet's text with the `_` of T24 replaced by the name
/// (`pub fn paginate<T>(items: List<T>, size: Positive) -> List<List<T>>`,
/// `pub type Positive = Int`, `pub struct Page {` ...). A test, which has
/// no signature facet (5.1), has the header `test "<name>"`; a property's
/// header is `property "<name>"` with its `for` clauses on the following
/// lines, indented as the formatter writes them.
fn signature_text(decl: &Declaration) -> String {
    let facet = format_fragment(Fragment::Signature(decl), FragmentMode::Canonical);
    let keyword = match &decl.kind {
        DeclKind::Property(p) => {
            let mut out = format!("property {}", quote_text(&p.name.value));
            for line in facet.lines() {
                out.push_str("\n    ");
                out.push_str(line);
            }
            return out;
        }
        DeclKind::Test(t) => return format!("test {}", quote_text(&t.name.value)),
        DeclKind::Fn(_) => "fn",
        DeclKind::Type(_) => "type",
        DeclKind::Struct(_) => "struct",
        DeclKind::Enum(_) => "enum",
        DeclKind::Capability(_) => "capability",
    };
    let Some(name) = decl.name() else {
        return facet;
    };
    let visibility = if decl.is_pub() { "pub " } else { "" };
    match facet.strip_prefix(&format!("{visibility}{keyword} _")) {
        Some(rest) => format!("{visibility}{keyword} {}{rest}", name.text),
        None => facet,
    }
}

/// The `contract` of a symbol block: "canonical clause texts in canonical
/// order" (6.4), each with its keyword: a function's `requires`, then
/// `ensures`, then `decreases` clauses (`ensures flatten(result) ==
/// items`); a refinement's `where` predicate; for a capability, each
/// operation's `requires` then `ensures` clauses prefixed with `fn
/// <operation>`, as its contract facet groups them (`fn get requires ...`).
/// Clause texts are one line (T56). A kind without clauses has `[]`.
fn contract_list(decl: &Declaration) -> Vec<String> {
    match &decl.kind {
        DeclKind::Fn(f) => {
            let mut out: Vec<String> = f
                .requires()
                .map(|c| format!("requires {}", clause_text(&c.expr)))
                .collect();
            out.extend(
                f.ensures()
                    .map(|c| format!("ensures {}", clause_text(&c.expr))),
            );
            if let Some(d) = f.decreases() {
                out.push(format!("decreases {}", clause_text(&d.expr)));
            }
            out
        }
        DeclKind::Type(t) => match &t.kind {
            TypeDeclKind::Refinement(r) => vec![format!("where {}", clause_text(&r.predicate))],
            TypeDeclKind::Wrapper(_) | TypeDeclKind::Opaque(_) => Vec::new(),
        },
        DeclKind::Capability(c) => {
            let mut out = Vec::new();
            for op in &c.operations {
                let requires = op
                    .clauses
                    .iter()
                    .filter(|c| c.kind == cpl_syntax::ast::ContractKind::Requires);
                let ensures = op
                    .clauses
                    .iter()
                    .filter(|c| c.kind == cpl_syntax::ast::ContractKind::Ensures);
                for clause in requires.chain(ensures) {
                    out.push(format!(
                        "fn {} {} {}",
                        op.name.text,
                        clause.kind.keyword(),
                        clause_text(&clause.expr)
                    ));
                }
            }
            out
        }
        DeclKind::Struct(_) | DeclKind::Enum(_) | DeclKind::Test(_) | DeclKind::Property(_) => {
            Vec::new()
        }
    }
}

/// The `effects` of a symbol block: the bound's references as sorted,
/// distinct `param.op` strings (6.4); empty without an `effects` clause,
/// and for every kind but a function.
fn effect_list(decl: &Declaration) -> Vec<String> {
    match &decl.kind {
        DeclKind::Fn(f) => f
            .effects()
            .map(|clause| {
                clause
                    .effects
                    .iter()
                    .map(|e| format!("{}.{}", e.param.text, e.operation.text))
                    .collect::<BTreeSet<String>>()
                    .into_iter()
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// The call expressions of a definition, with the callee (`Call`) or the
/// capability (`CapCall`) each calls.
#[derive(Default)]
struct CallExprs {
    sites: Vec<(DefId, Span)>,
}

impl<'h> Visitor<'h> for CallExprs {
    fn expr(&mut self, e: &'h Expr) {
        match &e.kind {
            ExprKind::Call { callee, .. } => self.sites.push((*callee, e.span)),
            ExprKind::CapCall { capability, .. } => self.sites.push((*capability, e.span)),
            _ => {}
        }
    }
}

/// Whether a walk met a hole.
#[derive(Default)]
struct Holes {
    found: bool,
}

impl<'h> Visitor<'h> for Holes {
    fn expr(&mut self, e: &'h Expr) {
        self.found |= matches!(e.kind, ExprKind::Hole { .. });
    }
}

/// The refusal of [`Index::inspect`] to list the holes of `path`'s body
/// without the check's hole reports.
fn hole_reports_missing(path: &str) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        Category::Internal,
        format!("the holes of `{path}` are unknown: the index has no hole reports"),
    )
    .with_data("kind", json!("hole_reports_missing"))
    .with_data(
        "detail",
        json!(
            "the hole reports are the check's (`CheckResult::holes`): build the index with \
             `Index::from_check`, or give them with `Index::with_holes`"
        ),
    )
}
