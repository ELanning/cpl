//! `cpl work check` (`spec/toolchain.md` 7.2): validating a `.work` file
//! against a checked program ([`validate_work_file`],
//! [`validate_work_file_with_policy`]).
//!
//! Selectors are resolved with `cpl_sema::resolve_selector` in the scope of
//! the file's `use` lines (and otherwise fully qualified, grammar 4.4); the
//! stable identity of a target is the `Program`'s (`Def::stable_id`), the
//! key of the facet maps of Section 5.4 (except that the program gives the
//! later occurrence of a duplicated `@id` its provisional identity, 4.5).
//! The rules and their codes, in the order of the 7.2 table:
//!
//! | Rule | Code | Severity |
//! | - | - | - |
//! | every `use` resolves (semantics 1.6) | `unresolved_module`, `unresolved_name`, `private_item` | error |
//! | `base` and `target` present; `workers >= 1` | `work_missing_base`, `work_missing_target`, `work_bad_workers` | error |
//! | `base`/`target` resolve; `target` names a branch (D35); skipped without a repository or `git` | `work_unknown_ref`, `work_target_not_branch`; `git_unavailable` (warning), `git_command_failed` | error |
//! | `write`/`depend`/`preserve` selectors name a declaration or a file (T42); `add` namespaces are module paths outside `std` and `gen`, not declarations (D38) | `work_selector_empty`; `work_add_namespace_new`, `work_file_selector_shadowed` (warnings) | error / warning |
//! | a `write` file selector is in `[work.policy] files` | `work_file_not_grantable` | error |
//! | declarations named individually carry an `@id` (T47) | `work_provisional_identity` | warning |
//! | facet selectors name an applicable facet (5.1) | `work_facet_not_applicable` | error |
//! | `identity` is not written (10.3) | `work_identity_not_writable` | error |
//! | `after` names tasks of the change; no cycle | `work_unknown_task`, `work_cycle` | error |
//! | unordered tasks write disjoint facets and add to disjoint namespaces (T43) | `work_overlapping_writes` | error |
//! | `test(sel)` matches a test; `prove(sel)` names functions; `review(t)` names a task | `work_test_selector_empty`, `work_prove_not_function`, `work_unknown_task` | error |
//! | no facet is both written and preserved by one task | `work_preserve_written` | error |
//! | a task without `write` or `add` is reviewed | `work_task_without_authority` | warning |
//!
//! Readings of the specification (each is cited where it is implemented):
//!
//! - [`validate_work_file`] has no manifest, so it judges file selectors
//!   against the default `[work.policy]` (no grantable files);
//!   [`validate_work_file_with_policy`] takes the project's policy, which
//!   `work_file_not_grantable` needs. [`WorkCheckResult::output`] is the
//!   `WorkCheckOutput` wire type of 7.2, produced here (T3).
//! - A selector that names no declaration has the code of the row whose
//!   rule it breaks: `work_selector_empty` in a `write`, `depend`, or
//!   `preserve` set, `work_test_selector_empty` in `test(...)` (it matches
//!   no test), and `work_prove_not_function` in `prove(...)` (it names no
//!   function), since 7.2 and Appendix B give `work_selector_empty` to
//!   write/depend/preserve selectors only. `prove` of a namespace selects
//!   the functions in it, as `test` of a namespace selects its tests.
//! - A facet selector in `test(...)` is `work_facet_not_applicable` and
//!   still names its declaration individually, so it counts for
//!   `work_provisional_identity` (the 7.2 row: "by a declaration or facet
//!   selector").
//! - A use item whose import failed binds nothing, and a selector that
//!   fails through it is not reported again (semantics S62's no-cascade
//!   rule for `.cpl` files).
//! - `work_provisional_identity` is reported once per declaration (its first
//!   naming selector, with a note at each further one), and not for
//!   standard-library declarations, which carry no `@id` and are never in a
//!   tree the integrator assigns ids in (T47).
//! - `target` names a branch when `refs/heads/<target>` (or the target
//!   itself, spelled `refs/heads/...`) is a well-formed ref name that
//!   resolves; the command is `rev-parse --verify` of Section 10.7.
//! - An `add` namespace under `std` or `gen`, or one that is not a module
//!   path, is `work_selector_empty`, the error code of the 7.2 row. So is a
//!   namespace under which no module exists and whose path names a
//!   declaration in the file's scope (`double.*` through `use
//!   app.core.{double}`, or `app.core.double.*`): `*` is never a facet
//!   wildcard and `paginate.*` is an error if `paginate` is a declaration
//!   (grammar D38), so the "need not contain a module yet" exemption does
//!   not reach it. Where a module exists, the module wins, as a chain
//!   resolves to the longest module path (semantics 2.4), and the namespace
//!   is that module's, as in a `write` set.
//! - `workers` beyond `u32` is `work_bad_workers` too.

use crate::codes;
use crate::model::{
    namespace_covers, ChangeSpec, FileSelector, RequirementSpec, ResolvedNamespace,
    ResolvedNamespaceOrSelector, ResolvedSelector, TaskSpec, WorkModel,
};
use cpl_diag::json::{
    RequireKind, WorkAddJson, WorkChangeJson, WorkCheckOutput, WorkRequirementJson,
    WorkSelectorJson, WorkTargetJson, WorkTaskJson,
};
use cpl_diag::{
    Category, DeclName, DiagCode, Diagnostic, Diagnostics, Facet, FacetSelector, FileOrigin, Span,
    StableId, SymbolPath,
};
use cpl_index::{applicable_facets, resolve_rev, GitError, GitRepo, IdKind};
use cpl_project::WorkPolicySection;
use cpl_sema::codes as sema_codes;
use cpl_sema::hir::{DefId, DefKind, Program, RefKind};
use cpl_sema::{resolve_selector, Selected, SelectorScope};
use cpl_syntax::ast::{self, DeclKind, SelectorKind};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

/// The outcome of validating a `.work` file (Section 7.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkCheckResult {
    /// The model, when the file validated without errors (warnings allowed).
    pub model: Option<WorkModel>,
    /// Every diagnostic of the rules of 7.2, in `Diagnostics::sorted` order
    /// (T46) over the program's source map.
    pub diagnostics: Vec<Diagnostic>,
    /// The `WorkCheckOutput` of `cpl work check --json` (7.2), produced here
    /// rather than at the CLI (T3). It lists every change, also when the
    /// file has errors, with what resolved: a selector that names nothing
    /// has empty `targets`, and `base_commit` is present only when `base`
    /// resolved. `file` is the path the program's source map registers for
    /// the work file (`work/pagination.work`), or empty when the file is not
    /// registered there, in which case the caller sets it.
    pub output: WorkCheckOutput,
}

impl WorkCheckResult {
    /// Whether some diagnostic is an error (`cpl work check` exits 1).
    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(Diagnostic::is_error)
    }
}

/// Validate a `.work` file against a checked program (Section 7.2) with the
/// default `[work.policy]` of Section 11.1, under which no file selector is
/// grantable: [`validate_work_file_with_policy`] with
/// `WorkPolicySection::default()`. `cpl work check` passes the project's
/// policy to [`validate_work_file_with_policy`] instead.
pub fn validate_work_file(
    file: &ast::WorkFile,
    program: &Program,
    repo: Option<&GitRepo>,
) -> WorkCheckResult {
    validate_work_file_with_policy(file, program, &WorkPolicySection::default(), repo)
}

/// Validate a `.work` file against a checked program (Section 7.2), with the
/// project's `[work.policy]` (its `files` decide `work_file_not_grantable`)
/// and, when `repo` is given, the repository its `base` and `target` refs
/// are resolved in (without one, or without `git`, resolution is skipped
/// with the warning `git_unavailable`).
///
/// `program` is the checked project (`cpl_sema::check`); a program without
/// definitions (some file failed to parse, semantics S52) resolves no
/// selector, so the caller reports the check's errors first. `file` should
/// be the parsed work file of the same project, so that its spans are in the
/// program's source map.
pub fn validate_work_file_with_policy(
    file: &ast::WorkFile,
    program: &Program,
    policy: &WorkPolicySection,
    repo: Option<&GitRepo>,
) -> WorkCheckResult {
    let mut validator = Validator::new(file, program, policy, repo);
    validator.check_uses();
    let mut changes = Vec::new();
    let mut outputs = Vec::new();
    for change in &file.changes {
        let (spec, output) = validator.change(change);
        changes.push(spec);
        outputs.push(output);
    }
    validator.finish();
    let diagnostics = Diagnostics::from(validator.diagnostics).sorted(program.source_map());
    let valid = !diagnostics.iter().any(Diagnostic::is_error);
    WorkCheckResult {
        model: valid.then_some(WorkModel { changes }),
        diagnostics,
        output: WorkCheckOutput {
            file: program
                .source_map()
                .get(file.span.file)
                .map(|f| f.path.clone())
                .unwrap_or_default(),
            changes: outputs,
        },
    }
}

/// The selector as written, in canonical spelling: `paginate.body`,
/// `tests.pagination.*`, `Positive`.
pub(crate) fn selector_text(sel: &ast::Selector) -> String {
    let path = sel.path.dotted();
    match &sel.kind {
        SelectorKind::Namespace => format!("{path}.*"),
        SelectorKind::Facet(f) => format!("{path}.{}", f.kind.as_str()),
        SelectorKind::Declaration => path,
    }
}

// ---------------------------------------------------------------------------
// The validator
// ---------------------------------------------------------------------------

/// The three selector sets of a task that name declarations (grammar 4.2).
#[derive(Copy, Clone, PartialEq, Eq)]
enum Set {
    Write,
    Depend,
    Preserve,
}

impl Set {
    fn keyword(self) -> &'static str {
        match self {
            Set::Write => "write",
            Set::Depend => "depend",
            Set::Preserve => "preserve",
        }
    }
}

/// What a `write`/`depend`/`preserve` selector resolved to.
enum Entry {
    /// Declarations (one per declaration of a namespace wildcard).
    Declarations(ResolvedSelector, Vec<Selected>),
    /// A file selector (T42), with the selector's text.
    File(FileSelector, String),
    /// Nothing (already diagnosed), with the selector's text.
    Unresolved(String),
}

/// What the overlap and authority rules need of a task.
#[derive(Default)]
struct TaskFacts {
    /// Every facet the `write` set requests (`.definition` as every
    /// applicable facet but `identity`, Section 10.3), with the span of the
    /// first selector requesting it.
    writes: BTreeMap<(DefId, Facet), Span>,
    /// The `add` namespaces with their selectors' spans.
    adds: Vec<(Vec<String>, Span)>,
    /// The task has a `write` or an `add` selector.
    authority: bool,
}

struct Validator<'a> {
    file: &'a ast::WorkFile,
    program: &'a Program,
    scope: SelectorScope<'a>,
    /// `[work.policy] files`.
    grantable: BTreeSet<FileSelector>,
    repo: Option<&'a GitRepo>,
    /// Why `base`/`target` resolution is skipped (the `reason` of
    /// `git_unavailable`): no repository was given, or git turned out to be
    /// missing or the directory no repository.
    git_skipped: Option<&'static str>,
    diagnostics: Vec<Diagnostic>,
    /// The names of use items whose import failed: they bind nothing, and a
    /// selector that fails through one is not reported again.
    poisoned: BTreeSet<String>,
    /// Project declarations named individually without an `@id`, with the
    /// span of every selector naming them, in source order.
    provisional: BTreeMap<DefId, Vec<Span>>,
}

impl<'a> Validator<'a> {
    fn new(
        file: &'a ast::WorkFile,
        program: &'a Program,
        policy: &WorkPolicySection,
        repo: Option<&'a GitRepo>,
    ) -> Validator<'a> {
        Validator {
            file,
            program,
            scope: SelectorScope {
                uses: &file.uses,
                module: None,
            },
            grantable: policy
                .files
                .iter()
                .filter_map(|spelling| FileSelector::parse(spelling))
                .collect(),
            repo,
            git_skipped: repo.is_none().then_some("no_repository"),
            diagnostics: Vec::new(),
            poisoned: BTreeSet::new(),
            provisional: BTreeMap::new(),
        }
    }

    fn push(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    /// The diagnostics collected over the whole file: one
    /// `work_provisional_identity` per declaration, and `git_unavailable`.
    fn finish(&mut self) {
        let program = self.program;
        for (def, spans) in std::mem::take(&mut self.provisional) {
            let path = program.display_path(def);
            let mut diagnostic = warning(
                codes::WORK_PROVISIONAL_IDENTITY,
                spans[0],
                format!(
                    "`{path}` has no `@id`: the work file names it by its provisional identity, which does not survive a rename or move, and the integrator gives it a generated `@id` (T47); assign one with `cpl id assign`"
                ),
            );
            for span in &spans[1..] {
                diagnostic = diagnostic.with_note("also named here", Some(*span));
            }
            self.push(blame(diagnostic, program, def, None));
        }
        if let Some(reason) = self.git_skipped {
            let why = match reason {
                "not_installed" => "git is not installed or not on PATH",
                "not_a_repository" => "the project is not in a git repository",
                _ => "no git repository is available (no repository, or no git)",
            };
            self.push(
                Diagnostic::warning(
                    codes::GIT_UNAVAILABLE,
                    Category::Git,
                    format!("{why}: the `base` and `target` refs were not resolved"),
                )
                .with_data("reason", json!(reason)),
            );
        }
    }

    // -----------------------------------------------------------------------
    // `use` lines (semantics 1.6)
    // -----------------------------------------------------------------------

    /// Every `use` resolves as in a `.cpl` file: the module exists (or, for
    /// a submodule item, is a prefix of module paths), the item exists in
    /// it, and it is public (a work file belongs to no module).
    fn check_uses(&mut self) {
        let program = self.program;
        let file = self.file;
        let module_paths: Vec<&[String]> = program.modules().map(|m| m.path.as_slice()).collect();
        let is_prefix = |path: &[String]| {
            module_paths
                .iter()
                .any(|m| m.len() > path.len() && m.starts_with(path))
        };
        let names_module =
            |path: &[String]| program.module_by_path(path).is_some() || is_prefix(path);
        for use_decl in &file.uses {
            let path: Vec<String> = use_decl
                .path
                .segments
                .iter()
                .map(|s| s.text.clone())
                .collect();
            let dotted = path.join(".");
            let module = program.module_by_path(&path);
            if module.is_none() && !is_prefix(&path) {
                self.push(
                    resolve_error(
                        sema_codes::UNRESOLVED_MODULE,
                        use_decl.path.span,
                        format!("no module `{dotted}` exists in the program"),
                    )
                    .with_data("module", json!(dotted)),
                );
                self.poisoned
                    .extend(use_decl.items.iter().map(|i| i.name.text.clone()));
                continue;
            }
            for item in &use_decl.items {
                let name = &item.name.text;
                let mut sub = path.clone();
                sub.push(name.clone());
                if module.is_some() {
                    let def = program.lookup(&SymbolPath::new(
                        path.clone(),
                        DeclName::Ident(name.clone()),
                    ));
                    match def {
                        Some(def) if !program.def(def).is_pub => {
                            let diagnostic = resolve_error(
                                sema_codes::PRIVATE_ITEM,
                                item.span,
                                format!(
                                    "`{}` is private to module `{dotted}`",
                                    program.display_path(def)
                                ),
                            );
                            self.push(blame(diagnostic, program, def, None));
                        }
                        Some(_) => {}
                        None if names_module(&sub) => {}
                        None => {
                            self.push(
                                resolve_error(
                                    sema_codes::UNRESOLVED_NAME,
                                    item.span,
                                    format!("module `{dotted}` has no item `{name}`"),
                                )
                                .with_data("name", json!(name))
                                .with_data("module", json!(dotted)),
                            );
                            self.poisoned.insert(name.clone());
                        }
                    }
                } else if !names_module(&sub) {
                    self.push(
                        resolve_error(
                            sema_codes::UNRESOLVED_MODULE,
                            item.span,
                            format!(
                                "no module `{dotted}` exists in the program (it is only a prefix of module paths), so `{name}` cannot be imported from it"
                            ),
                        )
                        .with_data("module", json!(dotted)),
                    );
                    self.poisoned.insert(name.clone());
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Changes
    // -----------------------------------------------------------------------

    fn change(&mut self, change: &ast::ChangeDecl) -> (ChangeSpec, WorkChangeJson) {
        let name = change.name.text.clone();
        let base = change.base();
        let target = change.target();
        if base.is_none() {
            self.push(
                error(
                    codes::WORK_MISSING_BASE,
                    change.name.span,
                    format!(
                        "change `{name}` has no `base git(\"...\")`: the snapshot its tasks start from"
                    ),
                )
                .with_data("change", json!(name)),
            );
        }
        if target.is_none() {
            self.push(
                error(
                    codes::WORK_MISSING_TARGET,
                    change.name.span,
                    format!(
                        "change `{name}` has no `target git(\"...\")`: the branch its integration advances"
                    ),
                )
                .with_data("change", json!(name)),
            );
        }
        let workers = self.workers(&name, change.workers());
        let base_commit = self.refs(&name, base, target);

        let tasks: Vec<&ast::TaskDecl> = change.tasks().collect();
        let names: BTreeSet<&str> = tasks.iter().map(|t| t.name.text.as_str()).collect();
        let mut specs = Vec::new();
        let mut task_outputs = Vec::new();
        let mut facts = Vec::new();
        for task in &tasks {
            let (spec, output, fact) = self.task(task);
            specs.push(spec);
            task_outputs.push(output);
            facts.push(fact);
        }
        if tasks.len() > crate::reach::MAX_TASKS_PER_CHANGE {
            // the order and overlap rules compare every pair of tasks: refused, not run
            self.push(
                error(
                    codes::WORK_TOO_MANY_TASKS,
                    change.name.span,
                    format!(
                        "change `{name}` declares {} tasks; a change may declare at most {}",
                        tasks.len(),
                        crate::reach::MAX_TASKS_PER_CHANGE
                    ),
                )
                .with_data("change", json!(name))
                .with_data("tasks", json!(tasks.len()))
                .with_data("limit", json!(crate::reach::MAX_TASKS_PER_CHANGE)),
            );
        } else {
            let graph = Graph::new(&tasks);
            self.after(&name, &tasks, &names, &graph);
            self.overlaps(&tasks, &facts, &graph);
        }
        let (integrate, integrate_outputs, reviewed) =
            self.integrate(&name, change.integrate(), &names);
        for (task, fact) in tasks.iter().zip(&facts) {
            let task_name = &task.name.text;
            if !fact.authority && !reviewed.contains(task_name) {
                self.push(
                    warning(
                        codes::WORK_TASK_WITHOUT_AUTHORITY,
                        task.name.span,
                        format!(
                            "task `{task_name}` has neither `write` nor `add` authority and no `require review({task_name})` names it, so nothing it produces is used"
                        ),
                    )
                    .with_data("task", json!(task_name)),
                );
            }
        }

        let base_text = base.map(|g| g.reference.value.clone()).unwrap_or_default();
        let target_text = target
            .map(|g| g.reference.value.clone())
            .unwrap_or_default();
        let spec = ChangeSpec {
            name: name.clone(),
            span: change.span,
            base: base_text.clone(),
            target: target_text.clone(),
            workers,
            tasks: specs,
            integrate,
        };
        let output = WorkChangeJson {
            name,
            base: base_text,
            base_commit,
            target: target_text,
            workers,
            tasks: task_outputs,
            integrate: integrate_outputs,
        };
        (spec, output)
    }

    /// `workers` (default 1) must be at least 1; a value beyond `u32` is
    /// refused as well (`work_bad_workers` with the value as `found`).
    fn workers(&mut self, change: &str, literal: Option<&ast::IntLiteral>) -> u32 {
        let Some(literal) = literal else {
            return 1;
        };
        let parsed = literal.digits.parse::<u32>();
        if let Ok(workers) = parsed {
            if workers >= 1 {
                return workers;
            }
        }
        let digits = literal.digits.trim_start_matches('0');
        let digits = if digits.is_empty() { "0" } else { digits };
        let found: serde_json::Value =
            serde_json::from_str(digits).unwrap_or_else(|_| json!(literal.digits));
        self.push(
            error(
                codes::WORK_BAD_WORKERS,
                literal.span,
                format!(
                    "change `{change}` has `workers {}`; the concurrency ceiling must be at least 1 and at most {}",
                    literal.digits,
                    u32::MAX
                ),
            )
            .with_data("change", json!(change))
            .with_data("found", found),
        );
        parsed.unwrap_or(u32::MAX)
    }

    // -----------------------------------------------------------------------
    // base and target (D35)
    // -----------------------------------------------------------------------

    /// Resolve `base` (any ref; its commit is returned) and `target` (which
    /// must name a branch) in the repository, unless resolution is skipped.
    fn refs(
        &mut self,
        change: &str,
        base: Option<&ast::GitRef>,
        target: Option<&ast::GitRef>,
    ) -> Option<String> {
        let repo = self.repo?;
        // A repository whose directory is gone is no repository: git would
        // fail to enter it, which `resolve_rev` reports as an unknown
        // revision, so every ref would look unknown.
        if !repo.toplevel.is_dir() {
            self.git_skipped = Some("not_a_repository");
        }
        let mut base_commit = None;
        if let Some(base) = base {
            if self.git_skipped.is_none() {
                match resolve_rev(repo, &base.reference.value) {
                    Ok(commit) => base_commit = Some(commit),
                    Err(failure) => self.ref_failure(change, base, "base", failure),
                }
            }
        }
        if let Some(target) = target {
            if self.git_skipped.is_none() {
                match resolve_rev(repo, &target.reference.value) {
                    Ok(commit) => self.branch(change, repo, target, &commit),
                    Err(failure) => self.ref_failure(change, target, "target", failure),
                }
            }
        }
        base_commit
    }

    /// D35: the integrator advances `target`, so it must name a branch:
    /// `refs/heads/<target>` (or the target itself when spelled
    /// `refs/heads/...`) is a ref name ([`branch_ref`]) that resolves.
    fn branch(&mut self, change: &str, repo: &GitRepo, target: &ast::GitRef, commit: &str) {
        let reference = &target.reference.value;
        let verdict = match branch_ref(reference) {
            None => Ok(false),
            Some(full) => match resolve_rev(repo, &full) {
                Ok(_) => Ok(true),
                Err(GitError::UnknownRevision(_)) => Ok(false),
                Err(failure) => Err(failure),
            },
        };
        match verdict {
            Ok(true) => {}
            Ok(false) => self.push(
                error(
                    codes::WORK_TARGET_NOT_BRANCH,
                    target.span,
                    format!(
                        "`target git(\"{reference}\")` of change `{change}` resolves to commit {commit} but names no branch; integration advances the target, so it must be a branch (`refs/heads/...`, grammar D35)"
                    ),
                )
                .with_data("change", json!(change))
                .with_data("ref", json!(reference))
                .with_data("commit", json!(commit)),
            ),
            Err(failure) => self.ref_failure(change, target, "target", failure),
        }
    }

    /// A ref that did not resolve: `work_unknown_ref` for an unknown
    /// revision; no git or no repository skips the remaining resolution
    /// (`git_unavailable`); any other failure is `git_command_failed`.
    fn ref_failure(&mut self, change: &str, at: &ast::GitRef, item: &str, failure: GitError) {
        let reference = &at.reference.value;
        match failure {
            GitError::UnknownRevision(_) => self.push(
                error(
                    codes::WORK_UNKNOWN_REF,
                    at.span,
                    format!(
                        "`{item} git(\"{reference}\")` of change `{change}` does not resolve to a commit in the repository"
                    ),
                )
                .with_data("change", json!(change))
                .with_data("item", json!(item))
                .with_data("ref", json!(reference)),
            ),
            GitError::NotInstalled => self.git_skipped = Some("not_installed"),
            GitError::NotARepository(_) => self.git_skipped = Some("not_a_repository"),
            other => self.push(
                Diagnostic::error(
                    codes::GIT_COMMAND_FAILED,
                    Category::Git,
                    format!("resolving `{item} git(\"{reference}\")` of change `{change}` failed: {other}"),
                )
                .with_span(at.span)
                .with_data("change", json!(change))
                .with_data("item", json!(item))
                .with_data("ref", json!(reference)),
            ),
        }
    }

    // -----------------------------------------------------------------------
    // Tasks
    // -----------------------------------------------------------------------

    fn task(&mut self, task: &ast::TaskDecl) -> (TaskSpec, WorkTaskJson, TaskFacts) {
        let name = task.name.text.clone();
        let after: Vec<String> = task
            .after
            .iter()
            .flat_map(|a| a.tasks.iter().map(|n| n.text.clone()))
            .collect();
        let mut spec = TaskSpec {
            name: name.clone(),
            span: task.span,
            after: after.clone(),
            write: Vec::new(),
            add: Vec::new(),
            write_files: Vec::new(),
            depend: Vec::new(),
            preserve: Vec::new(),
            goal: None,
        };
        let mut output = WorkTaskJson {
            name: name.clone(),
            after,
            write: Vec::new(),
            add: Vec::new(),
            depend: Vec::new(),
            preserve: Vec::new(),
            goal: None,
        };
        let mut facts = TaskFacts::default();
        // The facets of each `preserve` selector, for `work_preserve_written`.
        let mut preserved: Vec<(&ast::Selector, Vec<(DefId, Facet)>)> = Vec::new();
        for item in &task.items {
            match &item.kind {
                ast::TaskItemKind::Write(set) => {
                    for sel in &set.selectors {
                        facts.authority = true;
                        match self.set_selector(sel, Set::Write) {
                            Entry::Declarations(resolved, selected) => {
                                for s in &selected {
                                    for facet in self.facets(s, false) {
                                        facts.writes.entry((s.def, facet)).or_insert(sel.span);
                                    }
                                }
                                output.write.push(selector_json(&resolved));
                                spec.write.push(resolved);
                            }
                            Entry::File(file, text) => {
                                output.write.push(file_json(text, &file));
                                spec.write_files.push(file);
                            }
                            Entry::Unresolved(text) => output.write.push(empty_json(text)),
                        }
                    }
                }
                ast::TaskItemKind::Add(set) => {
                    for sel in &set.selectors {
                        facts.authority = true;
                        let namespace = self.add_namespace(sel);
                        facts
                            .adds
                            .push((namespace.module_prefix.clone(), namespace.span));
                        output.add.push(WorkAddJson {
                            selector: namespace.text.clone(),
                            namespace: namespace.module_prefix.join("."),
                        });
                        spec.add.push(namespace);
                    }
                }
                ast::TaskItemKind::Depend(set) => {
                    for sel in &set.selectors {
                        match self.set_selector(sel, Set::Depend) {
                            Entry::Declarations(resolved, _) => {
                                output.depend.push(selector_json(&resolved));
                                spec.depend.push(resolved);
                            }
                            // A file selector in `depend` or `preserve` is
                            // valid (7.2) but grants and preserves nothing
                            // (10.3), so the model does not record it.
                            Entry::File(file, text) => output.depend.push(file_json(text, &file)),
                            Entry::Unresolved(text) => output.depend.push(empty_json(text)),
                        }
                    }
                }
                ast::TaskItemKind::Preserve(set) => {
                    for sel in &set.selectors {
                        match self.set_selector(sel, Set::Preserve) {
                            Entry::Declarations(resolved, selected) => {
                                let facets = selected
                                    .iter()
                                    .flat_map(|s| {
                                        self.facets(s, true).into_iter().map(|f| (s.def, f))
                                    })
                                    .collect();
                                preserved.push((sel, facets));
                                output.preserve.push(selector_json(&resolved));
                                spec.preserve.push(resolved);
                            }
                            Entry::File(file, text) => output.preserve.push(file_json(text, &file)),
                            Entry::Unresolved(text) => output.preserve.push(empty_json(text)),
                        }
                    }
                }
                ast::TaskItemKind::Goal(goal) => {
                    spec.goal = Some(goal.value.clone());
                    output.goal = Some(goal.value.clone());
                }
            }
        }
        for (sel, facets) in preserved {
            let mut written: Vec<(DefId, Facet)> = facets
                .into_iter()
                .filter(|key| facts.writes.contains_key(key))
                .collect();
            written.sort();
            written.dedup();
            let Some(&(def, facet)) = written.first() else {
                continue;
            };
            let text = selector_text(sel);
            let overlap: Vec<serde_json::Value> = written
                .iter()
                .map(|(d, f)| json!({ "id": self.program.def(*d).stable_id, "facet": f }))
                .collect();
            let diagnostic = error(
                codes::WORK_PRESERVE_WRITTEN,
                sel.span,
                format!(
                    "task `{name}` both writes and preserves `{}.{facet}`: `preserve {{ {text} }}` requires the facet to keep its baseline version, which its `write` set allows it to change",
                    self.program.display_path(def)
                ),
            )
            .with_note("the write set names it here", Some(facts.writes[&(def, facet)]))
            .with_data("task", json!(name))
            .with_data("overlap", json!(overlap));
            self.push(blame(diagnostic, self.program, def, Some(facet)));
        }
        (spec, output, facts)
    }

    /// Resolve a `write`/`depend`/`preserve` selector (7.2, T42): to the
    /// declarations it names (a file selector spelling that also names a
    /// declaration is that declaration, with `work_file_selector_shadowed`),
    /// else to a file selector, else `work_selector_empty`.
    fn set_selector(&mut self, sel: &ast::Selector, set: Set) -> Entry {
        let text = selector_text(sel);
        let file = match sel.kind {
            SelectorKind::Declaration => FileSelector::parse(&sel.path.dotted()),
            _ => None,
        };
        let resolved = resolve_selector(self.program, sel, &self.scope);
        if let Ok(selected) = &resolved {
            if let Some(first) = selected.first() {
                if let Some(file) = &file {
                    let diagnostic = warning(
                        codes::WORK_FILE_SELECTOR_SHADOWED,
                        sel.span,
                        format!(
                            "`{text}` names the declaration `{}`, which wins over the file selector for `{}` (T42)",
                            self.program.display_path(first.def),
                            file.path()
                        ),
                    )
                    .with_data("selector", json!(text))
                    .with_data("file", json!(file.path()));
                    self.push(blame(diagnostic, self.program, first.def, None));
                }
                self.check_selected(sel, selected, set == Set::Write);
                let resolved = self.resolved(sel, text, selected);
                return Entry::Declarations(resolved, selected.clone());
            }
        }
        if let Some(file) = file {
            if set == Set::Write && !self.grantable.contains(&file) {
                self.push(
                    error(
                        codes::WORK_FILE_NOT_GRANTABLE,
                        sel.span,
                        format!(
                            "`{text}` names `{}`, which `[work.policy] files` of cpl.toml does not list, so no task can be granted it (T42)",
                            file.path()
                        ),
                    )
                    .with_data("selector", json!(text))
                    .with_data("file", json!(file.path())),
                );
            }
            return Entry::File(file, text);
        }
        self.unresolved(sel, &text, set.keyword(), resolved.err());
        Entry::Unresolved(text)
    }

    /// `work_selector_empty` for a `write`/`depend`/`preserve` selector that
    /// names no declaration (7.2, Appendix B), unless it fails through an
    /// import that already failed.
    fn unresolved(
        &mut self,
        sel: &ast::Selector,
        text: &str,
        set: &str,
        cause: Option<Diagnostic>,
    ) {
        if self.poisoned(sel) {
            return;
        }
        let message = match sel.kind {
            SelectorKind::Namespace => format!(
                "`{text}` names no declaration: no module under `{}` declares anything",
                self.namespace_path(sel).join(".")
            ),
            _ => format!(
                "`{text}` names no declaration and is not a file selector (`project.manifest`, `project.lock`, `work.<stem>`)"
            ),
        };
        let mut diagnostic = error(codes::WORK_SELECTOR_EMPTY, sel.span, message)
            .with_data("selector", json!(text))
            .with_data("set", json!(set));
        if let Some(cause) = cause {
            diagnostic = diagnostic.with_note(cause.message, cause.span);
        }
        self.push(diagnostic);
    }

    /// The per-declaration rules of a resolved selector: facet selectors
    /// name applicable facets (5.1), `identity` is not written (10.3), and
    /// declarations named individually carry an `@id` (T47).
    fn check_selected(&mut self, sel: &ast::Selector, selected: &[Selected], write: bool) {
        let program = self.program;
        let text = selector_text(sel);
        for s in selected {
            if let FacetSelector::Facet(facet) = s.facet {
                let decl = program.ast(s.def);
                if !applicable_facets(&decl.kind).contains(&facet) {
                    let diagnostic = error(
                        codes::WORK_FACET_NOT_APPLICABLE,
                        sel.span,
                        format!(
                            "`{text}`: `{}` is a {}, which has no `{facet}` facet (Section 5.1)",
                            program.display_path(s.def),
                            kind_word(&decl.kind)
                        ),
                    )
                    .with_data("kind", json!(IdKind::for_decl(&decl.kind).prefix()));
                    self.push(blame(diagnostic, program, s.def, Some(facet)));
                } else if write && facet == Facet::Identity {
                    let diagnostic = error(
                        codes::WORK_IDENTITY_NOT_WRITABLE,
                        sel.span,
                        format!(
                            "`{text}`: no task can write the identity of `{}` (Section 10.3); renames and moves keep it",
                            program.display_path(s.def)
                        ),
                    );
                    self.push(blame(diagnostic, program, s.def, Some(Facet::Identity)));
                }
            }
        }
        self.named_individually(sel, selected);
    }

    /// T47 (7.2): remember every project declaration without an `@id` that a
    /// declaration or facet selector names individually, for one
    /// `work_provisional_identity` per declaration. A namespace wildcard
    /// names a namespace, not its declarations, and a standard-library
    /// declaration never carries an `@id`.
    fn named_individually(&mut self, sel: &ast::Selector, selected: &[Selected]) {
        if matches!(sel.kind, SelectorKind::Namespace) {
            return;
        }
        let program = self.program;
        for s in selected {
            let def = program.def(s.def);
            let in_std = program.module(def.module).origin == FileOrigin::Std;
            if def.provisional && !in_std {
                self.provisional.entry(s.def).or_default().push(sel.span);
            }
        }
    }

    /// The facets a selected entry covers that the facet rules compare:
    /// `.definition` is every applicable facet, without `identity` for a
    /// write set (Section 10.3); a facet that does not apply covers nothing.
    fn facets(&self, s: &Selected, with_identity: bool) -> Vec<Facet> {
        let applicable = applicable_facets(&self.program.ast(s.def).kind);
        let keep = |f: Facet| with_identity || f != Facet::Identity;
        match s.facet {
            FacetSelector::Definition => applicable.iter().copied().filter(|&f| keep(f)).collect(),
            FacetSelector::Facet(f) if applicable.contains(&f) && keep(f) => vec![f],
            FacetSelector::Facet(_) => Vec::new(),
        }
    }

    /// An `add` namespace (7.2, T43): a module path outside `std` and `gen`
    /// that need not contain a module yet (`work_add_namespace_new`), unless
    /// its path names a declaration (grammar D38: `paginate.*` is an error
    /// if `paginate` is a declaration). Where a module exists under the
    /// path, the module wins (semantics 2.4) and the namespace is the
    /// existing one.
    fn add_namespace(&mut self, sel: &ast::Selector) -> ResolvedNamespace {
        let program = self.program;
        let text = selector_text(sel);
        let prefix = self.namespace_path(sel);
        let exists = program
            .modules()
            .any(|m| namespace_covers(&prefix, &m.path));
        let named = if exists {
            None
        } else {
            self.declaration_named(sel)
        };
        // The declaration the namespace's path names, when that is the problem.
        let mut blamed = None;
        let problem = if !matches!(sel.kind, SelectorKind::Namespace) {
            Some(format!(
                "`{text}`: an `add` set names namespaces (`<module path>.*`, grammar 4.2)"
            ))
        } else if matches!(prefix.first().map(String::as_str), Some("std" | "gen")) {
            Some(format!(
                "`{text}` is a namespace of the standard library; a task can add declarations only in project modules"
            ))
        } else if let Some(s) = prefix.iter().find(|s| !is_module_segment(s)) {
            Some(format!(
                "`{text}` is not a module path: `{s}` is not a lower-case identifier"
            ))
        } else if let Some(def) = named {
            blamed = Some(def);
            Some(format!(
                "`{text}`: `{}` names the {} `{}`, not a namespace; `*` is never a facet wildcard (grammar D38), and an `add` set names module paths",
                sel.path.dotted(),
                kind_word(&program.ast(def).kind),
                program.display_path(def)
            ))
        } else {
            None
        };
        match problem {
            Some(message) => {
                let diagnostic = error(codes::WORK_SELECTOR_EMPTY, sel.span, message)
                    .with_data("selector", json!(text))
                    .with_data("set", json!("add"));
                self.push(match blamed {
                    Some(def) => blame(diagnostic, program, def, None),
                    None => diagnostic,
                });
            }
            None => {
                if !exists && !self.poisoned(sel) {
                    let namespace = prefix.join(".");
                    self.push(
                        warning(
                            codes::WORK_ADD_NAMESPACE_NEW,
                            sel.span,
                            format!(
                                "no module exists under `{namespace}` yet; the task may create the first one (design line 353)"
                            ),
                        )
                        .with_data("namespace", json!(namespace)),
                    );
                }
            }
        }
        ResolvedNamespace {
            text,
            span: sel.span,
            module_prefix: prefix,
        }
    }

    // -----------------------------------------------------------------------
    // `after` and the overlap rule
    // -----------------------------------------------------------------------

    /// `after` names tasks of the same change (`work_unknown_task`), and the
    /// graph is acyclic (`work_cycle`, one per strongly connected component
    /// that has a cycle).
    fn after(
        &mut self,
        change: &str,
        tasks: &[&ast::TaskDecl],
        names: &BTreeSet<&str>,
        graph: &Graph,
    ) {
        for task in tasks {
            for name in task.after.iter().flat_map(|a| &a.tasks) {
                if !names.contains(name.text.as_str()) {
                    self.push(
                        error(
                            codes::WORK_UNKNOWN_TASK,
                            name.span,
                            format!(
                                "task `{}` runs after `{}`, which is not a task of change `{change}`",
                                task.name.text, name.text
                            ),
                        )
                        .with_data("task", json!(name.text))
                        .with_data("change", json!(change)),
                    );
                }
            }
        }
        for cycle in graph.cycles() {
            let first = tasks[cycle[0]];
            let span = first.after.as_ref().map_or(first.name.span, |a| a.span);
            let path: Vec<&str> = cycle.iter().map(|&i| tasks[i].name.text.as_str()).collect();
            self.push(
                error(
                    codes::WORK_CYCLE,
                    span,
                    format!(
                        "the `after` edges of change `{change}` form a cycle: {}",
                        path.join(" -> ")
                    ),
                )
                .with_data("change", json!(change))
                .with_data("tasks", json!(path)),
            );
        }
    }

    /// Unordered tasks (neither reaches the other through `after`) write
    /// disjoint facets (`.definition` as every applicable facet but
    /// `identity`; file selectors are not compared) and add to disjoint
    /// namespaces (T43). One diagnostic per pair of tasks and kind of
    /// overlap, at the later task's selector with a note at the earlier's.
    fn overlaps(&mut self, tasks: &[&ast::TaskDecl], facts: &[TaskFacts], graph: &Graph) {
        let program = self.program;
        for j in 0..tasks.len() {
            for i in 0..j {
                if graph.ordered(i, j) {
                    continue;
                }
                let (first, second) = (&tasks[i].name.text, &tasks[j].name.text);
                let shared: Vec<(DefId, Facet)> = facts[j]
                    .writes
                    .keys()
                    .filter(|key| facts[i].writes.contains_key(key))
                    .copied()
                    .collect();
                if let Some(&(def, facet)) = shared.first() {
                    let overlap: Vec<serde_json::Value> = shared
                        .iter()
                        .map(|(d, f)| json!({ "id": program.def(*d).stable_id, "facet": f }))
                        .collect();
                    let diagnostic = error(
                        codes::WORK_OVERLAPPING_WRITES,
                        facts[j].writes[&(def, facet)],
                        format!(
                            "tasks `{first}` and `{second}` are unordered (neither runs after the other), but both write `{}.{facet}`",
                            program.display_path(def)
                        ),
                    )
                    .with_note(
                        format!("task `{first}` writes it here"),
                        Some(facts[i].writes[&(def, facet)]),
                    )
                    .with_data("tasks", json!([first, second]))
                    .with_data("overlap", json!(overlap));
                    self.push(blame(diagnostic, program, def, Some(facet)));
                }
                let pair = facts[j].adds.iter().find_map(|(b, b_span)| {
                    facts[i]
                        .adds
                        .iter()
                        .find(|(a, _)| namespace_covers(a, b) || namespace_covers(b, a))
                        .map(|(a, a_span)| (a.join("."), *a_span, b.join("."), *b_span))
                });
                if let Some((a, a_span, b, b_span)) = pair {
                    self.push(
                        error(
                            codes::WORK_OVERLAPPING_WRITES,
                            b_span,
                            format!(
                                "tasks `{first}` and `{second}` are unordered (neither runs after the other), but both add to the overlapping namespaces `{a}.*` and `{b}.*` (T43)"
                            ),
                        )
                        .with_note(format!("task `{first}` adds here"), Some(a_span))
                        .with_data("tasks", json!([first, second]))
                        .with_data("namespaces", json!([a, b])),
                    );
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // integrate (grammar 4.3)
    // -----------------------------------------------------------------------

    /// The requirements, their JSON, and the tasks `review(...)` names.
    fn integrate(
        &mut self,
        change: &str,
        decl: Option<&ast::IntegrateDecl>,
        names: &BTreeSet<&str>,
    ) -> (
        Vec<RequirementSpec>,
        Vec<WorkRequirementJson>,
        BTreeSet<String>,
    ) {
        let mut specs = Vec::new();
        let mut outputs = Vec::new();
        let mut reviewed = BTreeSet::new();
        for item in decl.into_iter().flat_map(|d| &d.requirements) {
            let (spec, output) = match &item.requirement.kind {
                ast::RequirementKind::Typecheck => (
                    RequirementSpec::Typecheck,
                    WorkRequirementJson::new(RequireKind::Typecheck),
                ),
                ast::RequirementKind::NoHoles => (
                    RequirementSpec::NoHoles,
                    WorkRequirementJson::new(RequireKind::NoHoles),
                ),
                ast::RequirementKind::NoUnresolvedAssumptions => (
                    RequirementSpec::NoUnresolvedAssumptions,
                    WorkRequirementJson::new(RequireKind::NoUnresolvedAssumptions),
                ),
                ast::RequirementKind::Prove(sel) => self.prove(sel),
                ast::RequirementKind::Test(sel) => self.test(sel),
                ast::RequirementKind::Review(task) => {
                    if !names.contains(task.text.as_str()) {
                        self.push(
                            error(
                                codes::WORK_UNKNOWN_TASK,
                                task.span,
                                format!(
                                    "`require review({})` names no task of change `{change}`",
                                    task.text
                                ),
                            )
                            .with_data("task", json!(task.text))
                            .with_data("change", json!(change)),
                        );
                    }
                    reviewed.insert(task.text.clone());
                    let mut output = WorkRequirementJson::new(RequireKind::Review);
                    output.task = Some(task.text.clone());
                    (RequirementSpec::Review(task.text.clone()), output)
                }
            };
            specs.push(spec);
            outputs.push(output);
        }
        (specs, outputs, reviewed)
    }

    /// `require prove(sel)` names functions: a declaration or facet selector
    /// a function, a namespace at least one (the functions in it are the
    /// targets, as `test` of a namespace selects the tests in it). A
    /// selector that names nothing names no function either, so it is
    /// `work_prove_not_function` too, the code of this rule of 7.2 (7.2 and
    /// Appendix B give `work_selector_empty` to write/depend/preserve
    /// selectors only), unless it fails through an import that already
    /// failed.
    fn prove(&mut self, sel: &ast::Selector) -> (RequirementSpec, WorkRequirementJson) {
        let program = self.program;
        let text = selector_text(sel);
        let mut output = WorkRequirementJson::new(RequireKind::Prove);
        let selected = match resolve_selector(program, sel, &self.scope) {
            Ok(selected) => selected,
            Err(cause) => {
                if !self.poisoned(sel) {
                    self.push(
                        error(
                            codes::WORK_PROVE_NOT_FUNCTION,
                            sel.span,
                            format!(
                                "`require prove({text})`: `{text}` names no declaration, so no function; `prove` names functions, whose obligations a proof discharges"
                            ),
                        )
                        .with_note(cause.message, cause.span)
                        .with_data("selector", json!(text)),
                    );
                }
                output.targets = Some(Vec::new());
                let resolved = self.resolved(sel, text, &[]);
                return (RequirementSpec::Prove(resolved), output);
            }
        };
        self.check_selected(sel, &selected, false);
        let functions: Vec<Selected> = selected
            .iter()
            .copied()
            .filter(|s| matches!(program.def(s.def).kind, DefKind::Fn(_)))
            .collect();
        if functions.is_empty() {
            match selected.first() {
                Some(first) if !matches!(sel.kind, SelectorKind::Namespace) => {
                    let def = first.def;
                    let kind = &program.ast(def).kind;
                    let diagnostic = error(
                        codes::WORK_PROVE_NOT_FUNCTION,
                        sel.span,
                        format!(
                            "`require prove({text})` names the {} `{}`; `prove` names functions, whose obligations a proof discharges",
                            kind_word(kind),
                            program.display_path(def)
                        ),
                    )
                    .with_data("selector", json!(text))
                    .with_data("kind", json!(IdKind::for_decl(kind).prefix()));
                    self.push(blame(diagnostic, program, def, None));
                }
                // A namespace (one that selects nothing through a failed
                // import is not reported again).
                _ if selected.is_empty() && self.poisoned(sel) => {}
                _ => self.push(
                    error(
                        codes::WORK_PROVE_NOT_FUNCTION,
                        sel.span,
                        format!(
                            "`require prove({text})`: no module under `{}` declares a function; `prove` names functions",
                            self.namespace_path(sel).join(".")
                        ),
                    )
                    .with_data("selector", json!(text)),
                ),
            }
        }
        let resolved = self.resolved(sel, text, &functions);
        output.targets = Some(target_list(&resolved.targets));
        (RequirementSpec::Prove(resolved), output)
    }

    /// `require test(sel)` matches at least one test: a namespace selects
    /// the tests and properties in it (T43), a declaration selector those
    /// whose static call closure reaches it (`Index::tests_for`, Section
    /// 6.5), and a facet selector is `work_facet_not_applicable` (it still
    /// names its declaration for `work_provisional_identity`).
    fn test(&mut self, sel: &ast::Selector) -> (RequirementSpec, WorkRequirementJson) {
        let program = self.program;
        let text = selector_text(sel);
        let mut output = WorkRequirementJson::new(RequireKind::Test);
        let resolved = resolve_selector(program, sel, &self.scope);
        match &sel.kind {
            SelectorKind::Namespace => {
                let prefix = self.namespace_path(sel);
                let namespace = prefix.join(".");
                let matched = resolved
                    .unwrap_or_default()
                    .iter()
                    .filter(|s| is_test(program, s.def))
                    .count();
                if matched == 0 && !self.poisoned(sel) {
                    self.push(
                        error(
                            codes::WORK_TEST_SELECTOR_EMPTY,
                            sel.span,
                            format!(
                                "`require test({text})` matches no test: no module under `{namespace}` declares a test or property (an empty selection is a configuration failure)"
                            ),
                        )
                        .with_data("selector", json!(text))
                        .with_data("namespace", json!(namespace)),
                    );
                }
                output.namespace = Some(namespace);
                output.matched = Some(count(matched));
                let namespace = ResolvedNamespace {
                    text,
                    span: sel.span,
                    module_prefix: prefix,
                };
                (
                    RequirementSpec::Test(ResolvedNamespaceOrSelector::Namespace(namespace)),
                    output,
                )
            }
            SelectorKind::Facet(_) => {
                let selected = resolved.unwrap_or_default();
                let mut diagnostic = error(
                    codes::WORK_FACET_NOT_APPLICABLE,
                    sel.span,
                    format!(
                        "`require test({text})`: a test requirement selects tests by a namespace (`<module path>.*`) or by the declaration they exercise, not by a facet"
                    ),
                )
                .with_data("selector", json!(text));
                if let Some(Selected {
                    def,
                    facet: FacetSelector::Facet(facet),
                }) = selected.first().copied()
                {
                    diagnostic = blame(diagnostic, program, def, Some(facet));
                }
                self.push(diagnostic);
                // It still names its declaration individually (the 7.2 row
                // of `work_provisional_identity`: "by a declaration or facet
                // selector ... in an integrate prove/test requirement").
                self.named_individually(sel, &selected);
                let resolved = self.resolved(sel, text, &selected);
                output.targets = Some(target_list(&resolved.targets));
                (
                    RequirementSpec::Test(ResolvedNamespaceOrSelector::Selector(resolved)),
                    output,
                )
            }
            SelectorKind::Declaration => {
                let selected = match resolved {
                    Ok(selected) if !selected.is_empty() => selected,
                    resolved => {
                        if !self.poisoned(sel) {
                            let mut diagnostic = error(
                                codes::WORK_TEST_SELECTOR_EMPTY,
                                sel.span,
                                format!("`require test({text})` matches no test: `{text}` names no declaration"),
                            )
                            .with_data("selector", json!(text));
                            if let Err(cause) = resolved {
                                diagnostic = diagnostic.with_note(cause.message, cause.span);
                            }
                            self.push(diagnostic);
                        }
                        output.targets = Some(Vec::new());
                        output.matched = Some(0);
                        let resolved = self.resolved(sel, text, &[]);
                        return (
                            RequirementSpec::Test(ResolvedNamespaceOrSelector::Selector(resolved)),
                            output,
                        );
                    }
                };
                self.check_selected(sel, &selected, false);
                let def = selected[0].def;
                let matched = tests_for(program, def).len();
                if matched == 0 {
                    let diagnostic = error(
                        codes::WORK_TEST_SELECTOR_EMPTY,
                        sel.span,
                        format!(
                            "`require test({text})` matches no test: no test or property calls `{}`, directly or through other functions (Section 6.5)",
                            program.display_path(def)
                        ),
                    )
                    .with_data("selector", json!(text));
                    self.push(blame(diagnostic, program, def, None));
                }
                let resolved = self.resolved(sel, text, &selected);
                output.targets = Some(target_list(&resolved.targets));
                output.matched = Some(count(matched));
                (
                    RequirementSpec::Test(ResolvedNamespaceOrSelector::Selector(resolved)),
                    output,
                )
            }
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// A selected list as the model's resolved selector.
    fn resolved(
        &self,
        sel: &ast::Selector,
        text: String,
        selected: &[Selected],
    ) -> ResolvedSelector {
        ResolvedSelector {
            text,
            span: sel.span,
            targets: selected
                .iter()
                .map(|s| (self.program.def(s.def).stable_id.clone(), s.facet))
                .collect(),
        }
    }

    /// The declaration that a namespace selector's path names when it is
    /// read as a declaration selector in the file's scope (`double` through
    /// `use app.core.{double}`, `app.core.double`), resolved as every other
    /// selector is (`cpl_sema::resolve_selector`).
    fn declaration_named(&self, sel: &ast::Selector) -> Option<DefId> {
        let as_declaration = ast::Selector {
            span: sel.path.span,
            kind: SelectorKind::Declaration,
            ..sel.clone()
        };
        let selected = resolve_selector(self.program, &as_declaration, &self.scope).ok()?;
        selected.first().map(|s| s.def)
    }

    /// Whether the selector starts with the name of a failed import.
    fn poisoned(&self, sel: &ast::Selector) -> bool {
        sel.path
            .segments
            .first()
            .is_some_and(|s| self.poisoned.contains(&s.text))
    }

    /// The module path a namespace selector denotes: its segments, with a
    /// leading submodule alias of the file's imports expanded (`use
    /// reader.{pages}` makes `pages.*` the namespace `reader.pages`), as
    /// `cpl_sema::resolve_selector` expands it.
    fn namespace_path(&self, sel: &ast::Selector) -> Vec<String> {
        let segments: Vec<String> = sel.path.segments.iter().map(|s| s.text.clone()).collect();
        if let Some(target) = segments
            .first()
            .and_then(|first| use_target(&self.file.uses, first))
        {
            if self
                .program
                .modules()
                .any(|m| namespace_covers(&target, &m.path))
            {
                return target
                    .into_iter()
                    .chain(segments[1..].iter().cloned())
                    .collect();
            }
        }
        segments
    }
}

/// The `after` graph of one change: each task's known predecessors, and
/// every task each reaches through them.
struct Graph {
    /// Indices of the tasks a task's `after` names, in order (unknown names
    /// dropped).
    edges: Vec<Vec<usize>>,
    /// The tasks reachable from each task through `after` (itself only on a
    /// cycle).
    reach: crate::reach::Reach,
}

impl Graph {
    fn new(tasks: &[&ast::TaskDecl]) -> Graph {
        let mut index: BTreeMap<&str, usize> = BTreeMap::new();
        for (i, task) in tasks.iter().enumerate() {
            index.entry(task.name.text.as_str()).or_insert(i);
        }
        let edges: Vec<Vec<usize>> = tasks
            .iter()
            .map(|task| {
                task.after
                    .iter()
                    .flat_map(|a| &a.tasks)
                    .filter_map(|name| index.get(name.text.as_str()).copied())
                    .collect()
            })
            .collect();
        let reach = crate::reach::Reach::new(&edges);
        Graph { edges, reach }
    }

    /// Whether one of the two tasks runs after the other.
    fn ordered(&self, i: usize, j: usize) -> bool {
        self.reach.reaches(i, j) || self.reach.reaches(j, i)
    }

    /// One cycle per strongly connected component that has one, starting
    /// and ending at its first task in source order and following `after`
    /// edges in their order (`[a, c, b, a]` for `a after c`, `b after a`,
    /// `c after b`, the order `ScheduleError::Cycle` reports).
    fn cycles(&self) -> Vec<Vec<usize>> {
        self.reach
            .cycles()
            .into_iter()
            .map(|members| {
                let component: BTreeSet<usize> = members.iter().copied().collect();
                self.cycle_through(members[0], &component)
            })
            .collect()
    }

    fn cycle_through(&self, start: usize, component: &BTreeSet<usize>) -> Vec<usize> {
        fn walk(
            graph: &Graph,
            at: usize,
            start: usize,
            component: &BTreeSet<usize>,
            visited: &mut BTreeSet<usize>,
            path: &mut Vec<usize>,
        ) -> bool {
            for &next in &graph.edges[at] {
                if next == start {
                    path.push(start);
                    return true;
                }
                if component.contains(&next) && visited.insert(next) {
                    path.push(next);
                    if walk(graph, next, start, component, visited, path) {
                        return true;
                    }
                    path.pop();
                }
            }
            false
        }
        let mut path = vec![start];
        let mut visited = BTreeSet::from([start]);
        walk(self, start, start, component, &mut visited, &mut path);
        path
    }
}

/// `Index::tests_for` of Section 6.5 over the program's references: every
/// test and property from which `def` is reachable through `Call` and
/// `CapabilityOp` edges, in `DefId` order. (`cpl-index` has no `Index` yet;
/// this is the same rule.)
fn tests_for(program: &Program, def: DefId) -> Vec<DefId> {
    program
        .defs()
        .filter(|t| is_test(program, t.id) && reaches(program, t.id, def))
        .map(|t| t.id)
        .collect()
}

fn reaches(program: &Program, from: DefId, target: DefId) -> bool {
    let mut seen = BTreeSet::from([from]);
    let mut stack = vec![from];
    while let Some(at) = stack.pop() {
        for reference in program.references_from(at) {
            if !matches!(reference.kind, RefKind::Call | RefKind::CapabilityOp) {
                continue;
            }
            if reference.to == target {
                return true;
            }
            if seen.insert(reference.to) {
                stack.push(reference.to);
            }
        }
    }
    false
}

fn is_test(program: &Program, def: DefId) -> bool {
    matches!(
        program.def(def).kind,
        DefKind::Test(_) | DefKind::Property(_)
    )
}

/// The module path of the first use item named `name`: `use a.b.{name}` is
/// `a.b.name` (the lookup `cpl_sema::resolve_selector` makes).
fn use_target(uses: &[ast::UseDecl], name: &str) -> Option<Vec<String>> {
    uses.iter().find_map(|u| {
        u.items.iter().find(|i| i.name.text == name).map(|i| {
            u.path
                .segments
                .iter()
                .map(|s| s.text.clone())
                .chain([i.name.text.clone()])
                .collect()
        })
    })
}

/// A module path segment is a `LOWER_IDENT` (grammar 1.3, 3.1).
fn is_module_segment(segment: &str) -> bool {
    segment
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
}

/// D35: the branch ref a `target` names: `refs/heads/<target>`, or the
/// target itself when it is spelled `refs/heads/...`; `None` for any other
/// `refs/` path and for a text that is no ref name (a revision expression
/// such as `main~1` or `main@{1}`, which `rev-parse` would resolve).
fn branch_ref(target: &str) -> Option<String> {
    let full = if target.starts_with("refs/") {
        target.to_string()
    } else {
        format!("refs/heads/{target}")
    };
    (full.starts_with("refs/heads/") && is_ref_name(&full)).then_some(full)
}

/// The ref-name rules of `git check-ref-format`: no component is empty,
/// starts with `.`, or ends with `.lock`; no `..`, `@{`, control character,
/// space, or any of `~^:?*[\`; no trailing `/` or `.`.
fn is_ref_name(name: &str) -> bool {
    !name.ends_with('/')
        && !name.ends_with('.')
        && !name.contains("..")
        && !name.contains("@{")
        && !name
            .chars()
            .any(|c| c.is_ascii_control() || " ~^:?*[\\".contains(c))
        && name
            .split('/')
            .all(|c| !c.is_empty() && !c.starts_with('.') && !c.ends_with(".lock"))
}

/// A word for a declaration kind in messages.
fn kind_word(kind: &DeclKind) -> &'static str {
    match kind {
        DeclKind::Fn(_) => "function",
        DeclKind::Type(_) => "type",
        DeclKind::Struct(_) => "struct",
        DeclKind::Enum(_) => "enum",
        DeclKind::Capability(_) => "capability",
        DeclKind::Test(_) => "test",
        DeclKind::Property(_) => "property",
    }
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

fn error(code: DiagCode, span: Span, message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(code, codes::WORK, message).with_span(span)
}

fn warning(code: DiagCode, span: Span, message: impl Into<String>) -> Diagnostic {
    Diagnostic::warning(code, codes::WORK, message).with_span(span)
}

/// A `resolve` diagnostic of semantics 9.4 (the `use` rules).
fn resolve_error(code: DiagCode, span: Span, message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(code, sema_codes::category(code), message).with_span(span)
}

/// Blame a declaration: `symbol`, `qualified_name`, and the facet.
fn blame(
    diagnostic: Diagnostic,
    program: &Program,
    def: DefId,
    facet: Option<Facet>,
) -> Diagnostic {
    diagnostic.with_symbol(
        program.def(def).stable_id.clone(),
        program.display_path(def),
        facet,
    )
}

fn target_list(targets: &[(StableId, FacetSelector)]) -> Vec<WorkTargetJson> {
    targets
        .iter()
        .map(|(id, facet)| WorkTargetJson {
            id: id.clone(),
            facet: *facet,
        })
        .collect()
}

fn selector_json(resolved: &ResolvedSelector) -> WorkSelectorJson {
    WorkSelectorJson {
        selector: resolved.text.clone(),
        targets: Some(target_list(&resolved.targets)),
        file: None,
    }
}

fn file_json(text: String, file: &FileSelector) -> WorkSelectorJson {
    WorkSelectorJson {
        selector: text,
        targets: None,
        file: Some(file.path()),
    }
}

fn empty_json(text: String) -> WorkSelectorJson {
    WorkSelectorJson {
        selector: text,
        targets: Some(Vec::new()),
        file: None,
    }
}
