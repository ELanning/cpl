//! The state one `check_with` run threads through its stages
//! (`spec/toolchain.md` 3.1): the program under construction, the
//! collected files and declarations, the scopes, the refinement and member
//! tables, the resolution side tables of bodies, and the diagnostics of
//! each stage.
//!
//! Every stage is a function `fn(&mut Checker)`; the body typer, the
//! effect checker, the contract checker, and the policy stage receive the
//! same [`Checker`] (see the hooks in `crate::check`).

use crate::check::{CheckConfig, HoleReport, VerificationStatus};
use crate::codes;
use crate::members::{MemberKind, MemberTable};
use crate::project::Project;
use crate::refine::Refinements;
use crate::resolve::{CtorIndex, Resolutions, Resolver};
use crate::scope::{FileScope, ModuleScope, Universe};
use cpl_diag::{DiagCode, Diagnostic, Facet, FileId, Severity, Span};
use cpl_hir::{DefId, DefStatus, ModuleId, Program};
use cpl_syntax::ast;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// The stages of `spec/toolchain.md` 3.1 in their order; diagnostics are
/// collected per stage and concatenated in this order before the final
/// `Diagnostics::sorted` (which keeps the stage order among span-less ones).
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Stage {
    Project = 0,
    Parse = 1,
    Resolve = 2,
    Type = 3,
    Effect = 4,
    Contract = 5,
    Hole = 6,
    ReleasePolicy = 7,
}

/// One collected `.cpl` file (std or project) with its scope.
#[derive(Debug)]
pub(crate) struct FileInfo {
    pub id: FileId,
    pub ast: Arc<ast::Module>,
    pub module: ModuleId,
    pub is_std: bool,
    /// The file's imports (S5), filled by import resolution.
    pub scope: FileScope,
}

/// The annotation facts of one declaration (1.8), after validation: a
/// misplaced or invalid annotation contributes nothing.
#[derive(Clone, Debug, Default)]
pub(crate) struct Annotations {
    /// A valid `@id` argument and the annotation's span.
    pub id: Option<(String, Span)>,
    /// `@prove` on a `fn`.
    pub prove: bool,
    /// A valid `@builtin("<id>")` in the standard library.
    pub builtin: Option<(String, Span)>,
    /// `@method`/`@property` with a receiver: a member of the table keyed
    /// by the receiver head (stdlib S37).
    pub member: Option<MemberKind>,
    /// A zero-parameter `@property`: a module-level property such as
    /// `std.gen.int`, read without parentheses (stdlib S40).
    pub module_property: bool,
}

/// One collected declaration; index = `DefId.0`.
#[derive(Clone, Debug)]
pub(crate) struct DeclInfo {
    /// Index into [`Checker::files`].
    pub file: usize,
    pub decl_index: usize,
    pub annots: Annotations,
    /// The name token (the `TEXT` name of a test or property): the span
    /// of declaration-level diagnostics (semantics S63 (3)).
    pub name_span: Span,
}

/// The checker state.
pub(crate) struct Checker<'a> {
    pub project: &'a Project,
    pub config: &'a CheckConfig,
    /// The program under construction (Section 3.3).
    pub program: Program,
    /// The collected `.cpl` files in `FileId` order.
    pub files: Vec<FileInfo>,
    /// One per definition, index = `DefId.0`.
    pub decls: Vec<DeclInfo>,
    /// One per module, index = `ModuleId.0`.
    pub modules: Vec<ModuleScope>,
    pub universe: Universe,
    /// Every constructor of the program by name (S10 deferral, see
    /// [`CtorIndex`]).
    pub ctors: CtorIndex,
    pub members: MemberTable,
    pub refinements: Refinements,
    /// The resolution side tables of every body, clause, and predicate
    /// (`crate::names`), for the body typer.
    pub names: Resolutions,
    /// The call graph: every function each definition calls by name (its
    /// body and its clauses; method calls, constructors, and capability
    /// operations are not calls of a definition), from the resolution walk.
    /// The contract stage's `unused_decreases` and `missing_decreases`
    /// (5.4) read it.
    pub calls: BTreeMap<DefId, BTreeSet<DefId>>,
    /// `CheckResult::holes`, filled by the typer's hole stage
    /// (`crate::types`, S47, S48, S64).
    pub holes: Vec<HoleReport>,
    /// `CheckResult::verification`, filled by the contract stage
    /// (`crate::contracts`).
    pub verification: Vec<VerificationStatus>,
    stages: Vec<Vec<Diagnostic>>,
}

impl<'a> Checker<'a> {
    pub fn new(project: &'a Project, config: &'a CheckConfig) -> Checker<'a> {
        Checker {
            project,
            config,
            program: Program::empty(project.source_map.clone()),
            files: Vec::new(),
            decls: Vec::new(),
            modules: Vec::new(),
            universe: Universe::default(),
            ctors: CtorIndex::default(),
            members: MemberTable::default(),
            refinements: Refinements::default(),
            names: Resolutions::default(),
            calls: BTreeMap::new(),
            holes: Vec::new(),
            verification: Vec::new(),
            stages: vec![Vec::new(); 8],
        }
    }

    /// Report a diagnostic in `stage`.
    pub fn emit(&mut self, stage: Stage, diagnostic: Diagnostic) {
        self.stages[stage as usize].push(diagnostic);
    }

    /// Report several diagnostics in `stage`.
    pub fn emit_all(&mut self, stage: Stage, diagnostics: impl IntoIterator<Item = Diagnostic>) {
        self.stages[stage as usize].extend(diagnostics);
    }

    /// The diagnostics of every stage in stage order.
    pub fn take_diagnostics(&mut self) -> Vec<Diagnostic> {
        self.stages.iter_mut().flat_map(std::mem::take).collect()
    }

    /// Mark a definition `HasErrors` (Section 3.1: later stages skip it
    /// where continuing would only cascade).
    pub fn fail(&mut self, def: DefId) {
        if let Some(d) = self.program.defs.get_mut(def.0 as usize) {
            d.status = DefStatus::HasErrors;
        }
    }

    /// The AST of a definition's file and the declaration's index in it.
    /// The `Arc` is cloned so that the caller may keep the declaration
    /// while mutating the checker.
    pub fn decl_ast(&self, def: DefId) -> (Arc<ast::Module>, usize) {
        let info = &self.decls[def.0 as usize];
        (self.files[info.file].ast.clone(), info.decl_index)
    }

    /// A resolver in the scope of `def`'s file, with `generics` in scope 3.
    pub fn resolver<'c>(&'c self, def: DefId, generics: &'c [String]) -> Resolver<'c> {
        let file = &self.files[self.decls[def.0 as usize].file];
        Resolver::new(self, file, generics)
    }

    /// The (`symbol`, `qualified_name`) pair of a definition (S2, S49).
    pub fn symbol_of(&self, def: DefId) -> (cpl_diag::StableId, String) {
        (
            self.program.def(def).stable_id.clone(),
            self.program.display_path(def),
        )
    }
}

/// A diagnostic with the category and default severity of `code`.
pub(crate) fn diag(code: DiagCode, span: Option<Span>, message: impl Into<String>) -> Diagnostic {
    diag_with(code, codes::default_severity(code), span, message)
}

/// A diagnostic with an explicit severity.
pub(crate) fn diag_with(
    code: DiagCode,
    severity: Severity,
    span: Option<Span>,
    message: impl Into<String>,
) -> Diagnostic {
    let mut d = Diagnostic::new(code, codes::category(code), severity, message);
    d.span = span;
    d
}

/// Attach the blamed declaration (`symbol`, `qualified_name`, and `facet`).
pub(crate) fn blame(
    d: Diagnostic,
    cx: &Checker<'_>,
    def: DefId,
    facet: Option<Facet>,
) -> Diagnostic {
    let (id, name) = cx.symbol_of(def);
    d.with_symbol(id, name, facet)
}
