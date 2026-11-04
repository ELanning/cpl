//! `cpl_sema::check`: checking a loaded project (`spec/toolchain.md` 1.6
//! and 3.1-3.7; the rules are `spec/semantics.md` Sections 1-9), hole
//! reports (semantics S48), verification statuses (S36), and selector
//! resolution.
//!
//! # Stages
//!
//! [`check_with`] runs the stages of toolchain 3.1 in order:
//!
//! | Stage | Module | Status |
//! | - | - | - |
//! | project (loading's diagnostics, the release lock check of 11.2/T37 and T75, the binding checks of 11.1; from Milestone 2 the C binding checks and the export plan, T74, T80) | here, `crate::interop` | done |
//! | parse (loading's diagnostics; S52 stops here when a compilation unit failed) | here | done |
//! | collect: module graph, definitions, identities, annotations, names | `collect` | done |
//! | resolve: imports, refinement bases and classes, signatures, containment, the resolution walk of bodies | `imports`, `refine`, `sig`, `wf`, `names` | done |
//! | type: bodies, clauses, predicates, ranges into the typed HIR; references; the hole reports and `hole`/`hole_type_unknown`/`hole_in_release`; then `unused_import` (a resolve-category code, reported once the typer has completed the constructor resolutions of S10 that wait for an expected type) | `types::type_bodies`, `imports::report_unused` | done |
//! | effect: operation calls and forwarding against the declared bounds, handle positions, `unused_capability`, `EffectBound::inferred` | `effects::check_effects` | done |
//! | contract: holes, `?`, statements, operation calls, and self-calls in contract expressions; `unused_decreases`, `missing_decreases`; `verification_result` and `CheckResult::verification` | `contracts::check_contracts` | done |
//! | release_policy: `verification_unsatisfied` (`hole_in_release` is the typer's hole stage) | `policy::apply_policy` | done |
//!
//! Every stage runs on every definition even when an earlier one reported
//! errors; a definition with an error is marked `DefStatus::HasErrors`,
//! and error-typed names (`Ty::Error`, `Res::Error`) keep later stages from
//! cascading (S62). The diagnostics are concatenated in stage order and
//! returned in `Diagnostics::sorted` order (T46), which keeps the stage
//! order among span-less diagnostics.
//!
//! The C target (Milestone 2, toolchain 1.6 and 11.1) is planned last, once
//! every stage has run, so that the plan sees the complete reference graph
//! (the note of `project_export_generic` names a generic function's
//! callers) and leaves out every definition marked `HasErrors`; its
//! diagnostics, and those of `cpl_project::check_c_bindings`, go to the
//! project stage after the binding checks, where toolchain 1.6 places them.
//! Neither runs when a file failed to parse (S52), as the binding checks do
//! not.

use crate::checker::{diag, Checker, Stage};
use crate::codes;
use crate::hir::{BindingKind, DefId, DefKind, ModuleId, Program, Ty};
use crate::interop::InteropPlan;
use crate::project::{ParsedFile, Project};
use crate::{collect, contracts, effects, imports, names, policy, refine, sig, types, wf};
use cpl_diag::json::Digest;
use cpl_diag::{
    Category, Diagnostic, Diagnostics, FacetSelector, FileOrigin, Severity, Span, SymbolPath,
};
use cpl_syntax::ast;
use cpl_syntax::FileKind;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

pub use cpl_diag::json::ProofStatus;

/// What `check_with` checks beyond development mode.
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct CheckConfig {
    /// Section 7.2: `hole_in_release` and `verification_unsatisfied` (semantics S47).
    pub release: bool,
    /// `--deny-warnings`: [`CheckResult::has_errors`] also counts warnings.
    pub warnings_as_errors: bool,
    /// Enabled off-by-default lints (Section 7.2 `--lint`): `id_missing`.
    pub lints: BTreeSet<String>,
}

/// `check_with(project, &CheckConfig::default())`.
pub fn check(project: &Project) -> CheckResult {
    check_with(project, &CheckConfig::default())
}

/// Check a loaded project (Section 3.1; see the module docs for the stages).
pub fn check_with(project: &Project, config: &CheckConfig) -> CheckResult {
    let mut cx = Checker::new(project, config);
    for d in &project.diagnostics {
        let stage = match d.category {
            Category::Project => Stage::Project,
            Category::Parse => Stage::Parse,
            _ => Stage::Resolve,
        };
        cx.emit(stage, d.clone());
    }
    if config.release {
        lock_check(&mut cx);
    }
    // S52: a compilation unit that failed to parse stops the check before
    // resolution (a .work or .patch file does not: semantics S53).
    let parse_failed = project
        .files
        .iter()
        .zip(&project.parsed)
        .any(|(entry, parsed)| entry.kind == FileKind::Cpl && matches!(parsed, ParsedFile::Failed));
    if parse_failed {
        return finish(cx, None);
    }
    resolve(&mut cx);
    types::type_bodies(&mut cx);
    // After the typer: it finishes the constructor resolutions of S10 that
    // need an expected type, and the imports they go through are used (1.6).
    imports::report_unused(&mut cx);
    effects::check_effects(&mut cx);
    // Every reference is known now; the later stages read the call graph through
    // `Program::references_from`, which needs the (from, span) order.
    cx.program.sort_references();
    crate::machines::check(&mut cx);
    crate::concurrency::check(&mut cx);
    crate::evolution::check(&mut cx);
    crate::linear::check(&mut cx);
    contracts::check_contracts(&mut cx);
    policy::apply_policy(&mut cx);
    let interop = c_target(&mut cx);
    finish(cx, interop)
}

/// The C target (toolchain 11.1, T74, T80): `cpl_project::check_c_bindings`
/// on the manifest and the project root (its file checks are skipped for a
/// `VirtualTree`) and, from Milestone 3, `cpl_project::check_python`; then,
/// when `"c"` or `"python"` is in `[interop] targets`, the export plan of
/// `crate::interop::plan`, which [`CheckResult::interop`] carries.
fn c_target(cx: &mut Checker<'_>) -> Option<InteropPlan> {
    let project = cx.project;
    let manifest = &project.manifest;
    let checked = crate::project::check_c_bindings(manifest, project.root.as_deref());
    cx.emit_all(Stage::Project, checked.diagnostics);
    // Milestone 3 (spec/python.md): the Python target's manifest rules and the fixtures
    let python = crate::project::check_python(manifest, project.root.as_deref());
    cx.emit_all(Stage::Project, python);
    // the Python package contains the C package and exports what it exports (spec/python.md 1)
    if !manifest
        .interop
        .targets
        .iter()
        .any(|t| t == "c" || t == "python")
    {
        return None;
    }
    let (plan, diagnostics) = crate::interop::plan(&cx.program, manifest, &checked.cbind);
    cx.emit_all(Stage::Project, diagnostics);
    Some(plan)
}

/// The declaration half: collection, imports, refinements, signatures,
/// containment, the resolution walk, and the binding checks
/// (`unused_import` follows the body typer, which completes resolution).
fn resolve(cx: &mut Checker<'_>) {
    collect::collect(cx);
    imports::resolve_imports(cx);
    refine::resolve_refinements(cx);
    sig::resolve_signatures(cx);
    wf::check_containment(cx);
    names::resolve_bodies(cx);
    check_bindings(cx);
}

/// Run the declaration half on a project that parsed and hand the checker
/// state to `inspect` (unit tests of the internal tables).
#[cfg(test)]
pub(crate) fn with_resolved<R>(project: &Project, inspect: impl FnOnce(&Checker<'_>) -> R) -> R {
    let config = CheckConfig::default();
    let mut cx = Checker::new(project, &config);
    resolve(&mut cx);
    inspect(&cx)
}

fn finish(mut cx: Checker<'_>, interop: Option<InteropPlan>) -> CheckResult {
    let mut program = std::mem::replace(
        &mut cx.program,
        Program::empty(cx.project.source_map.clone()),
    );
    program.sort_references();
    let diagnostics = Diagnostics::from(cx.take_diagnostics()).sorted(&cx.project.source_map);
    CheckResult {
        program,
        diagnostics,
        holes: std::mem::take(&mut cx.holes),
        verification: std::mem::take(&mut cx.verification),
        interop,
        warnings_as_errors: cx.config.warnings_as_errors,
    }
}

/// The binding checks of toolchain 11.1 (`project_binding_*`), which need
/// name resolution: a binding's `capability` must name a `pub capability`.
fn check_bindings(cx: &mut Checker<'_>) {
    let program = &cx.program;
    let diags = crate::project::check_bindings(&cx.project.manifest, |path| {
        SymbolPath::parse(path)
            .ok()
            .and_then(|p| program.lookup(&p))
            .map(|d| program.def(d))
            .is_some_and(|d| d.is_pub && matches!(d.kind, DefKind::Capability(_)))
    });
    cx.emit_all(Stage::Project, diags);
}

/// The release lock check (toolchain 11.2, T37): a missing `cpl.lock` is
/// the warning `project_lock_missing`; a present one whose `[sources]`,
/// `[verification]`, or `toolchain.std` differ from the tree and the
/// manifest is the error `project_lock_stale`. Both are span-less (S63 (6)).
///
/// From Milestone 2 (T75), when the lock has a C entry and the project a
/// root, the comparison also covers the digests of the C build inputs,
/// computed from the files (`cpl_project::c_input_digests`, no build step):
/// `"adapters"` (each backend-`c` binding's adapter file against
/// `adapters.<b>.files`) and `"foreign.c"` (its vendored sources against
/// `foreign.c.<b>.sources`), each differing file in `paths` after those of
/// `[sources]`, a binding present on one side only contributing all of its
/// files. A stale C input is resolved by `cpl build --lock`, which the
/// message names (`cpl check --lock` only carries the C entries through).
fn lock_check(cx: &mut Checker<'_>) {
    let project = cx.project;
    let Some(lock) = &project.lock else {
        let d = diag(
            codes::PROJECT_LOCK_MISSING,
            None,
            "cpl.lock is missing: a release check needs the locked build inputs; run `cpl check --lock`",
        );
        cx.emit(Stage::Project, d);
        return;
    };
    let mut current = BTreeMap::new();
    current.insert("cpl.toml".to_string(), project.manifest_digest.clone());
    for entry in &project.files {
        if entry.origin != FileOrigin::Std {
            current.insert(
                project.source_map.file(entry.file).path.clone(),
                entry.digest.clone(),
            );
        }
    }
    let mut differences = Vec::new();
    let mut paths: Vec<String> = Vec::new();
    if lock.sources != current {
        differences.push("sources");
        let keys: BTreeSet<&String> = lock.sources.keys().chain(current.keys()).collect();
        paths = keys
            .into_iter()
            .filter(|k| lock.sources.get(*k) != current.get(*k))
            .cloned()
            .collect();
    }
    if lock.verification != project.manifest.verification {
        differences.push("verification");
    }
    if lock.toolchain.std != project.std_digest() {
        differences.push("toolchain.std");
    }
    let mut c_stale = false;
    if let (true, Some(root)) = (lock.has_c_entry(), project.root.as_deref()) {
        let current = crate::project::c_input_digests(&project.manifest, root);
        let adapters: BTreeMap<String, BTreeMap<String, Digest>> = lock
            .c_adapters()
            .into_iter()
            .map(|(binding, adapter)| (binding, adapter.files))
            .collect();
        let sources: BTreeMap<String, BTreeMap<String, Digest>> = lock
            .foreign_c()
            .into_iter()
            .map(|(binding, foreign)| (binding, foreign.sources))
            .collect();
        for (name, locked, now) in [
            ("adapters", &adapters, &current.adapters),
            ("foreign.c", &sources, &current.sources),
        ] {
            if locked != now {
                differences.push(name);
                c_stale = true;
                for path in differing_inputs(locked, now) {
                    if !paths.contains(&path) {
                        paths.push(path);
                    }
                }
            }
        }
    }
    if !differences.is_empty() {
        let relock = if c_stale {
            "cpl build --lock"
        } else {
            "cpl check --lock"
        };
        let d = diag(
            codes::PROJECT_LOCK_STALE,
            None,
            format!(
                "cpl.lock is stale ({} changed); run `{relock}`",
                differences.join(", ")
            ),
        )
        .with_data("differences", json!(differences))
        .with_data("paths", json!(paths));
        cx.emit(Stage::Project, d);
    }
}

/// The files whose digests differ between the locked and the current build
/// inputs (by binding, then path), in path order: every file of a binding
/// present on one side only, and every file one side lacks or digests
/// differently.
fn differing_inputs(
    locked: &BTreeMap<String, BTreeMap<String, Digest>>,
    now: &BTreeMap<String, BTreeMap<String, Digest>>,
) -> Vec<String> {
    let empty = BTreeMap::new();
    let bindings: BTreeSet<&String> = locked.keys().chain(now.keys()).collect();
    let mut out = BTreeSet::new();
    for binding in bindings {
        let a = locked.get(binding).unwrap_or(&empty);
        let b = now.get(binding).unwrap_or(&empty);
        for path in a.keys().chain(b.keys()) {
            if a.get(path) != b.get(path) {
                out.insert(path.clone());
            }
        }
    }
    out.into_iter().collect()
}

/// The result of checking a project.
#[derive(Clone, Debug)]
pub struct CheckResult {
    /// Always present; it has no definitions when some file failed to parse
    /// (semantics S52). Definitions whose checking failed have
    /// `Def::status == DefStatus::HasErrors`.
    pub program: Program,
    /// Emitted in stage order (project, parse, resolve, type, effect,
    /// contract, hole, release_policy) and within a stage by file path then
    /// item order; returned in `Diagnostics::sorted` order (Section 7.1,
    /// T46), which every output uses.
    pub diagnostics: Vec<Diagnostic>,
    /// One per hole with an expected type; each is also the `hole`
    /// diagnostic (info) in `diagnostics`, or `hole_in_release` under
    /// release (semantics S64).
    pub holes: Vec<HoleReport>,
    /// One per `@prove` declaration; each is also a `verification_result` diagnostic.
    pub verification: Vec<VerificationStatus>,
    /// Milestone 2 (T80): the C surface of the program
    /// ([`crate::interop::plan`]) when `"c"` is in `[interop] targets` and
    /// every file parsed; `None` otherwise.
    pub interop: Option<InteropPlan>,
    warnings_as_errors: bool,
}

impl CheckResult {
    /// True when the check failed: some diagnostic is an error, or, with
    /// `CheckConfig::warnings_as_errors` (`--deny-warnings`), a warning.
    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(|d| {
            d.severity == Severity::Error
                || (self.warnings_as_errors && d.severity == Severity::Warning)
        })
    }
}

/// The verification status of one `@prove` declaration: the verifier's
/// (`spec/verification.md`), with its reason when `Unknown`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerificationStatus {
    pub def: DefId,
    pub status: ProofStatus,
    pub reason: Option<String>,
}

/// The hole report of spec/semantics.md S48, with resolved definitions
/// instead of strings where the index needs them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoleReport {
    /// `"build_pages"`.
    pub name: String,
    pub span: Span,
    /// The enclosing declaration.
    pub def: DefId,
    /// The hole's expected type. It is (or contains) `Ty::Error` when the
    /// position's type comes from a declaration or a form that failed
    /// (`takes(?h)` with `fn takes(x: Nope)`): such a hole is still
    /// reported (semantics S64), its `expected_type` rendering `?` there.
    pub expected: Ty,
    /// Visible bindings, innermost first.
    pub bindings: Vec<HoleBinding>,
    /// One per `using` parameter in scope.
    pub capabilities: Vec<HoleCapability>,
    /// The enclosing fn's clauses, canonical text, in order (empty for tests).
    pub requires: Vec<String>,
    pub ensures: Vec<String>,
    /// Enclosing `while` invariants, innermost first.
    pub invariants: Vec<String>,
    /// Every definition the enclosing declaration references, itself
    /// included when it is recursive (as `Index::dependencies`, toolchain
    /// 6.5), sorted by qualified name; cpl-cli adds versions via cpl-index.
    pub dependencies: Vec<DefId>,
}

/// A binding visible at a hole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoleBinding {
    pub name: String,
    pub ty: Ty,
    pub kind: BindingKind,
    pub mutable: bool,
}

/// A `using` parameter in scope at a hole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoleCapability {
    pub parameter: String,
    pub capability: DefId,
    pub operations: Vec<HoleOperation>,
}

/// One operation of a [`HoleCapability`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoleOperation {
    pub name: String,
    pub signature: String,
    /// `parameter.name` is in the enclosing function's effect bound.
    pub in_bound: bool,
}

/// The scope a selector is resolved in: a `.work` file's `use` list, a
/// module, or fully qualified only.
#[derive(Copy, Clone, Debug)]
pub struct SelectorScope<'a> {
    pub uses: &'a [ast::UseDecl],
    pub module: Option<ModuleId>,
}

/// One declaration and facet a selector selects; a namespace wildcard
/// expands to one entry per declaration.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Selected {
    pub def: DefId,
    pub facet: FacetSelector,
}

/// Resolve a selector in a scope (the `Program::resolve_selector` of
/// Section 1.6, as a free function).
pub fn resolve_selector(
    program: &Program,
    sel: &ast::Selector,
    scope: &SelectorScope,
) -> Result<Vec<Selected>, Diagnostic> {
    crate::selector::resolve_selector(program, sel, scope)
}
