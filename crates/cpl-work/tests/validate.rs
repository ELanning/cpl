//! `validate_work_file` (`spec/toolchain.md` 7.2, and the "work and patch"
//! row of Section 12): `examples/reader/work/pagination.work` validates
//! against the checked reader without a repository with no errors, only
//! the warnings `git_unavailable` and `work_provisional_identity` for
//! `Positive`, into the model of 1.9 and the `WorkCheckOutput` of 7.2;
//! mutated copies trigger every rule of the 7.2 table; and against a
//! temporary repository (`git init -b main`) `base` and `target` resolve,
//! `target` only to a branch.

mod common;

use common::*;
use cpl_diag::{Category, Diagnostic, Facet, FacetSelector, Severity, StableId};
use cpl_project::{VirtualTree, WorkPolicySection};
use cpl_work::{
    validate_work_file, validate_work_file_with_policy, FileSelector, GitRepo, RequirementSpec,
    ResolvedNamespaceOrSelector, ResolvedSelector, WorkCheckResult,
};
use serde_json::json;
use std::path::Path;
use std::process::Command;

const WORK: &str = "work/pagination.work";

fn id(text: &str) -> StableId {
    StableId::parse(text).expect("a stable id")
}

fn pgn() -> StableId {
    id("pgn_7c2d")
}

fn positive() -> StableId {
    id("reader.pages.Positive")
}

/// `examples/reader` with `work/pagination.work` edited: the first `from`
/// (which must occur) replaced by `to`.
#[track_caller]
fn edited(from: &str, to: &str) -> VirtualTree {
    let work = text(&reader_tree(), WORK);
    assert!(work.contains(from), "the corpus work file has no {from:?}");
    with_file(&reader_tree(), WORK, &work.replacen(from, to, 1))
}

/// Validate the tree's `work/pagination.work` against its checked program,
/// with the project's policy when given (else `validate_work_file`).
fn validate(
    tree: &VirtualTree,
    policy: Option<&WorkPolicySection>,
    repo: Option<&GitRepo>,
) -> WorkCheckResult {
    let (project, program) = checked(tree);
    let work = work_file(&project, WORK);
    match policy {
        Some(policy) => validate_work_file_with_policy(work, &program, policy, repo),
        None => validate_work_file(work, &program, repo),
    }
}

/// Without a repository, with the default `[work.policy]`.
fn plain(tree: &VirtualTree) -> WorkCheckResult {
    validate(tree, None, None)
}

/// The codes of the diagnostics of one severity, in order.
fn of(result: &WorkCheckResult, severity: Severity) -> Vec<&'static str> {
    result
        .diagnostics
        .iter()
        .filter(|d| d.severity == severity)
        .map(|d| d.code.0)
        .collect()
}

fn errors(result: &WorkCheckResult) -> Vec<&'static str> {
    of(result, Severity::Error)
}

fn warnings(result: &WorkCheckResult) -> Vec<&'static str> {
    of(result, Severity::Warning)
}

/// The warnings of the corpus file without a repository, in sorted order
/// (`git_unavailable` has no span, so it comes last).
const BASELINE: [&str; 2] = ["work_provisional_identity", "git_unavailable"];

/// The one diagnostic with this code.
#[track_caller]
fn one<'r>(result: &'r WorkCheckResult, code: &str) -> &'r Diagnostic {
    let found: Vec<&Diagnostic> = result
        .diagnostics
        .iter()
        .filter(|d| d.code == code)
        .collect();
    assert_eq!(found.len(), 1, "one {code} in {:#?}", result.diagnostics);
    found[0]
}

/// The text of the work file a diagnostic spans.
#[track_caller]
fn spanned(tree: &VirtualTree, d: &Diagnostic) -> String {
    let span = d.span.expect("a span");
    text(tree, WORK)[span.start as usize..span.end as usize].to_string()
}

/// The 1-based line of the work file a span starts on.
fn line_of(tree: &VirtualTree, offset: u32) -> usize {
    text(tree, WORK)[..offset as usize].matches('\n').count() + 1
}

/// Assert that `result` has exactly these errors (and the baseline
/// warnings), and no model.
#[track_caller]
fn assert_errors(result: &WorkCheckResult, expected: &[&str]) {
    assert_eq!(errors(result), expected, "{:#?}", result.diagnostics);
    assert!(result.has_errors());
    assert!(result.model.is_none(), "no model with errors");
}

fn targets(selectors: &[ResolvedSelector]) -> Vec<Vec<(StableId, FacetSelector)>> {
    selectors.iter().map(|s| s.targets.clone()).collect()
}

fn facet(f: Facet) -> FacetSelector {
    FacetSelector::Facet(f)
}

// ---------------------------------------------------------------------------
// The corpus file
// ---------------------------------------------------------------------------

#[test]
fn pagination_work_validates_with_only_the_two_warnings() {
    let tree = reader_tree();
    let result = plain(&tree);
    assert_eq!(
        errors(&result),
        Vec::<&str>::new(),
        "{:#?}",
        result.diagnostics
    );
    assert_eq!(warnings(&result), BASELINE);
    assert_eq!(result.diagnostics.len(), 2);
    assert!(!result.has_errors());
    assert!(result.model.is_some());

    let provisional = one(&result, "work_provisional_identity");
    assert_eq!(provisional.category, Category::Work);
    assert_eq!(provisional.symbol, Some(positive()));
    assert_eq!(
        provisional.qualified_name.as_deref(),
        Some("reader.pages.Positive")
    );
    assert_eq!(spanned(&tree, provisional), "Positive.definition");
    assert!(provisional.notes.is_empty());

    let git = one(&result, "git_unavailable");
    assert_eq!(git.category, Category::Git);
    assert_eq!(git.span, None);
    assert_eq!(git.data["reason"], json!("no_repository"));
}

#[test]
fn pagination_work_model() {
    let model = plain(&reader_tree()).model.expect("a model");
    assert_eq!(model.changes.len(), 1);
    let change = model.change("pagination_update").expect("the change");
    assert_eq!(
        (change.base.as_str(), change.target.as_str(), change.workers),
        ("main", "main", 3)
    );
    let names: Vec<&str> = change.tasks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["implementation", "examples", "audit"]);

    let implementation = change.task("implementation").expect("implementation");
    assert!(implementation.after.is_empty());
    assert_eq!(
        targets(&implementation.write),
        [vec![(pgn(), facet(Facet::Body))]]
    );
    assert_eq!(implementation.write[0].text, "paginate.body");
    assert!(implementation.add.is_empty());
    assert!(implementation.write_files.is_empty());
    assert_eq!(
        targets(&implementation.depend),
        [
            vec![(pgn(), facet(Facet::Contract))],
            vec![(positive(), FacetSelector::Definition)],
        ]
    );
    assert_eq!(implementation.depend[1].text, "Positive.definition");
    assert_eq!(
        targets(&implementation.preserve),
        [
            vec![(pgn(), facet(Facet::Signature))],
            vec![(pgn(), facet(Facet::Contract))],
            vec![(pgn(), facet(Facet::Effects))],
            vec![(pgn(), facet(Facet::VerificationPolicy))],
        ]
    );
    assert_eq!(
        implementation.goal.as_deref(),
        Some("Implement pagination and discharge its proof obligations.")
    );

    let examples = change.task("examples").expect("examples");
    assert!(examples.write.is_empty());
    assert_eq!(examples.add.len(), 1);
    assert_eq!(examples.add[0].text, "tests.pagination.*");
    assert_eq!(examples.add[0].module_prefix, ["tests", "pagination"]);
    assert!(examples.add[0].covers(&["tests".into(), "pagination".into(), "edge".into()]));

    let audit = change.task("audit").expect("audit");
    assert_eq!(audit.after, ["implementation", "examples"]);
    assert!(audit.write.is_empty() && audit.add.is_empty());

    assert_eq!(change.integrate.len(), 5);
    assert_eq!(change.integrate[0], RequirementSpec::Typecheck);
    match &change.integrate[1] {
        RequirementSpec::Prove(sel) => {
            assert_eq!(sel.text, "paginate");
            assert_eq!(sel.targets, [(pgn(), FacetSelector::Definition)]);
        }
        other => panic!("prove(paginate), found {other:?}"),
    }
    match &change.integrate[2] {
        RequirementSpec::Test(ResolvedNamespaceOrSelector::Namespace(ns)) => {
            assert_eq!(ns.text, "tests.pagination.*");
            assert_eq!(ns.module_prefix, ["tests", "pagination"]);
        }
        other => panic!("test(tests.pagination.*), found {other:?}"),
    }
    assert_eq!(change.integrate[3], RequirementSpec::Review("audit".into()));
    assert_eq!(change.integrate[4], RequirementSpec::NoHoles);
}

#[test]
fn pagination_work_output_is_the_json_of_section_7_2() {
    let output = serde_json::to_value(plain(&reader_tree()).output).expect("json");
    let target = |id: &str, facet: &str| json!([{ "id": id, "facet": facet }]);
    assert_eq!(
        output,
        json!({
            "file": "work/pagination.work",
            "changes": [{
                // No `base_commit`: there is no repository.
                "name": "pagination_update", "base": "main", "target": "main", "workers": 3,
                "tasks": [
                    {
                        "name": "implementation", "after": [],
                        "write": [{ "selector": "paginate.body", "targets": target("pgn_7c2d", "body") }],
                        "add": [],
                        "depend": [
                            { "selector": "paginate.contract", "targets": target("pgn_7c2d", "contract") },
                            { "selector": "Positive.definition", "targets": target("reader.pages.Positive", "definition") },
                        ],
                        "preserve": [
                            { "selector": "paginate.signature", "targets": target("pgn_7c2d", "signature") },
                            { "selector": "paginate.contract", "targets": target("pgn_7c2d", "contract") },
                            { "selector": "paginate.effects", "targets": target("pgn_7c2d", "effects") },
                            { "selector": "paginate.verification_policy", "targets": target("pgn_7c2d", "verification_policy") },
                        ],
                        "goal": "Implement pagination and discharge its proof obligations.",
                    },
                    {
                        "name": "examples", "after": [], "write": [],
                        "add": [{ "selector": "tests.pagination.*", "namespace": "tests.pagination" }],
                        "depend": [], "preserve": [],
                        "goal": "Add boundary cases for empty input and partial final pages.",
                    },
                    {
                        "name": "audit", "after": ["implementation", "examples"],
                        "write": [], "add": [], "depend": [], "preserve": [],
                        "goal": "Review both patches against the frozen pagination contract.",
                    },
                ],
                "integrate": [
                    { "require": "typecheck" },
                    { "require": "prove", "targets": target("pgn_7c2d", "definition") },
                    { "require": "test", "namespace": "tests.pagination", "matched": 3 },
                    { "require": "review", "task": "audit" },
                    { "require": "no_holes" },
                ],
            }],
        })
    );
}

#[test]
fn diagnostics_are_sorted_and_render_with_their_work_file_location() {
    let tree = edited("workers 3", "workers 0");
    let (project, program) = checked(&tree);
    let result = validate_work_file(work_file(&project, WORK), &program, None);
    let codes: Vec<&str> = result.diagnostics.iter().map(|d| d.code.0).collect();
    // By position (workers on line 6, Positive.definition on line 10), the
    // span-less warning last (T46).
    assert_eq!(
        codes,
        [
            "work_bad_workers",
            "work_provisional_identity",
            "git_unavailable"
        ]
    );
    let json = result.diagnostics[0].to_json(&project.source_map);
    let location = json.span.expect("a location");
    assert_eq!(location.file, "work/pagination.work");
    assert_eq!((location.line, location.col), (6, 13));
    assert!(result.diagnostics[0]
        .render(&project.source_map)
        .starts_with("error[work_bad_workers]: "));
}

// ---------------------------------------------------------------------------
// `use` lines (semantics 1.6)
// ---------------------------------------------------------------------------

#[test]
fn every_use_resolves_as_in_a_source_file() {
    let uses = "use reader.pages.{paginate, Positive}";

    let tree = edited(uses, "use reader.pages.{paginate, Positive, nope}");
    let result = plain(&tree);
    assert_errors(&result, &["unresolved_name"]);
    let d = one(&result, "unresolved_name");
    assert_eq!(d.category, Category::Resolve);
    assert_eq!(spanned(&tree, d), "nope");
    assert_eq!(d.data["name"], json!("nope"));
    assert_eq!(d.data["module"], json!("reader.pages"));

    let tree = edited(uses, &format!("{uses}\nuse reader.nowhere.{{thing}}"));
    let result = plain(&tree);
    assert_errors(&result, &["unresolved_module"]);
    let d = one(&result, "unresolved_module");
    assert_eq!(spanned(&tree, d), "reader.nowhere");
    assert_eq!(d.data["module"], json!("reader.nowhere"));

    // `reader` is only a prefix of module paths: it admits submodules only.
    let tree = edited(uses, &format!("{uses}\nuse reader.{{nothing}}"));
    let result = plain(&tree);
    assert_errors(&result, &["unresolved_module"]);
    let d = one(&result, "unresolved_module");
    assert_eq!(spanned(&tree, d), "nothing");
    assert_eq!(d.data["module"], json!("reader"));

    let tree = edited(uses, "use reader.pages.{paginate, Positive, clean}");
    let result = plain(&tree);
    assert_errors(&result, &["private_item"]);
    let d = one(&result, "private_item");
    assert_eq!(d.category, Category::Resolve);
    assert_eq!(spanned(&tree, d), "clean");
    assert_eq!(d.symbol, Some(id("reader.pages.clean")));
}

#[test]
fn a_submodule_alias_resolves_selectors_and_a_failed_import_does_not_cascade() {
    let uses = "use reader.pages.{paginate, Positive}";
    let depend = "depend { paginate.contract, Positive.definition }";
    // `use reader.{pages}` imports the submodule alias `pages`.
    let tree = edited(uses, &format!("{uses}\nuse reader.{{pages}}"));
    let work = text(&tree, WORK).replacen(
        depend,
        "depend { paginate.contract, Positive.definition, pages.clean.body }",
        1,
    );
    let tree = with_file(&tree, WORK, &work);
    let result = plain(&tree);
    assert_eq!(
        errors(&result),
        Vec::<&str>::new(),
        "{:#?}",
        result.diagnostics
    );
    let model = result.model.as_ref().expect("a model");
    assert_eq!(
        model.changes[0].tasks[0].depend[2].targets,
        [(id("reader.pages.clean"), facet(Facet::Body))]
    );
    // `clean` has no @id either.
    assert_eq!(
        warnings(&result),
        [
            "work_provisional_identity",
            "work_provisional_identity",
            "git_unavailable"
        ]
    );

    // A selector through an import that failed is not reported again.
    let tree = edited(uses, "use reader.pages.{paginate, Positive, nope}");
    let work = text(&tree, WORK).replacen(
        depend,
        "depend { paginate.contract, Positive.definition, nope.contract }",
        1,
    );
    let result = plain(&with_file(&tree, WORK, &work));
    assert_errors(&result, &["unresolved_name"]);
}

// ---------------------------------------------------------------------------
// base, target, workers
// ---------------------------------------------------------------------------

#[test]
fn base_and_target_are_required_and_workers_at_least_one() {
    let tree = edited("    base git(\"main\")\n", "");
    let result = plain(&tree);
    assert_errors(&result, &["work_missing_base"]);
    let d = one(&result, "work_missing_base");
    assert_eq!(d.category, Category::Work);
    assert_eq!(spanned(&tree, d), "pagination_update");
    assert_eq!(d.data["change"], json!("pagination_update"));
    assert_eq!(result.output.changes[0].base, "");

    let tree = edited("    target git(\"main\")\n", "");
    let result = plain(&tree);
    assert_errors(&result, &["work_missing_target"]);
    assert_eq!(
        spanned(&tree, one(&result, "work_missing_target")),
        "pagination_update"
    );

    let tree = edited("workers 3", "workers 0");
    let result = plain(&tree);
    assert_errors(&result, &["work_bad_workers"]);
    let d = one(&result, "work_bad_workers");
    assert_eq!(spanned(&tree, d), "0");
    assert_eq!(d.data["found"], json!(0));
    assert_eq!(result.output.changes[0].workers, 0);

    // Beyond the u32 ceiling of the model.
    let tree = edited("workers 3", "workers 4294967296");
    let result = plain(&tree);
    assert_errors(&result, &["work_bad_workers"]);
    assert_eq!(
        one(&result, "work_bad_workers").data["found"].to_string(),
        "4294967296"
    );

    // `workers` defaults to 1.
    let result = plain(&edited("    workers 3\n", ""));
    assert_eq!(errors(&result), Vec::<&str>::new());
    assert_eq!(result.model.expect("a model").changes[0].workers, 1);
}

// ---------------------------------------------------------------------------
// Selectors (T42, T43)
// ---------------------------------------------------------------------------

#[test]
fn write_depend_and_preserve_selectors_name_declarations() {
    for (from, to, set, selector) in [
        (
            "depend { paginate.contract, Positive.definition }",
            "depend { paginate.contract, Positive.definition, reader.pages.nope }",
            "depend",
            "reader.pages.nope",
        ),
        (
            "write { paginate.body }",
            "write { paginate.body, reader.nothing.* }",
            "write",
            "reader.nothing.*",
        ),
        (
            "            paginate.verification_policy,\n",
            "            paginate.verification_policy,\n            nope.body,\n",
            "preserve",
            "nope.body",
        ),
    ] {
        let tree = edited(from, to);
        let result = plain(&tree);
        assert_errors(&result, &["work_selector_empty"]);
        let d = one(&result, "work_selector_empty");
        assert_eq!(spanned(&tree, d), selector);
        assert_eq!(d.data["set"], json!(set));
        assert_eq!(d.data["selector"], json!(selector));
        // The output keeps the selector, with no targets.
        let task = &result.output.changes[0].tasks[0];
        let listed = match set {
            "write" => &task.write,
            "depend" => &task.depend,
            _ => &task.preserve,
        };
        let last = listed.last().expect("the selector is listed");
        assert_eq!(last.selector, selector);
        assert_eq!(last.targets, Some(Vec::new()));
    }
    // A declaration selector that does not resolve carries the resolver's
    // explanation as a note.
    let result = plain(&edited(
        "depend { paginate.contract, Positive.definition }",
        "depend { paginate.contract, Positive.definition, reader.pages.nope }",
    ));
    let d = one(&result, "work_selector_empty");
    assert_eq!(d.notes.len(), 1);
    assert!(d.notes[0].message.contains("nope"), "{:?}", d.notes);
}

#[test]
fn add_namespaces_may_be_new_but_never_std() {
    let tree = edited("add { tests.pagination.* }", "add { tests.edge.* }");
    let result = plain(&tree);
    assert_eq!(errors(&result), Vec::<&str>::new());
    assert_eq!(
        warnings(&result),
        [
            "work_provisional_identity",
            "work_add_namespace_new",
            "git_unavailable"
        ]
    );
    let d = one(&result, "work_add_namespace_new");
    assert_eq!(spanned(&tree, d), "tests.edge.*");
    assert_eq!(d.data["namespace"], json!("tests.edge"));
    let model = result.model.expect("a warning keeps the model");
    assert_eq!(
        model.changes[0].tasks[1].add[0].module_prefix,
        ["tests", "edge"]
    );

    // A namespace above existing modules is not new.
    let result = plain(&edited("add { tests.pagination.* }", "add { tests.* }"));
    assert_eq!(warnings(&result), BASELINE);

    for namespace in ["std.text.*", "gen.*", "tests.Pagination.*"] {
        let tree = edited(
            "add { tests.pagination.* }",
            &format!("add {{ {namespace} }}"),
        );
        let result = plain(&tree);
        assert_errors(&result, &["work_selector_empty"]);
        let d = one(&result, "work_selector_empty");
        assert_eq!(spanned(&tree, d), namespace);
        assert_eq!(d.data["set"], json!("add"));
    }
}

#[test]
fn file_selectors_are_granted_only_through_the_policy() {
    let tree = edited(
        "write { paginate.body }",
        "write { paginate.body, project.manifest }",
    );
    // The default policy lists no file.
    let result = plain(&tree);
    assert_errors(&result, &["work_file_not_grantable"]);
    let d = one(&result, "work_file_not_grantable");
    assert_eq!(spanned(&tree, d), "project.manifest");
    assert_eq!(d.data["file"], json!("cpl.toml"));

    let policy = WorkPolicySection {
        files: vec!["project.manifest".to_string()],
        ..WorkPolicySection::default()
    };
    let result = validate(&tree, Some(&policy), None);
    assert_eq!(errors(&result), Vec::<&str>::new());
    assert_eq!(warnings(&result), BASELINE);
    let model = result.model.expect("a model");
    assert_eq!(
        model.changes[0].tasks[0].write_files,
        [FileSelector::Manifest]
    );
    assert_eq!(model.changes[0].tasks[0].write.len(), 1);
    assert_eq!(
        serde_json::to_value(&result.output.changes[0].tasks[0].write[1]).expect("json"),
        json!({ "selector": "project.manifest", "file": "cpl.toml" })
    );

    // `depend` and `preserve` may name files without any grant.
    let tree = edited(
        "depend { paginate.contract, Positive.definition }",
        "depend { paginate.contract, Positive.definition, project.lock, work.pagination }",
    );
    let result = plain(&tree);
    assert_eq!(errors(&result), Vec::<&str>::new());
    let task = &result.output.changes[0].tasks[0];
    assert_eq!(task.depend[2].file.as_deref(), Some("cpl.lock"));
    assert_eq!(task.depend[3].file.as_deref(), Some("work/pagination.work"));
    assert_eq!(
        result.model.expect("a model").changes[0].tasks[0]
            .depend
            .len(),
        2,
        "the model records declarations only"
    );
}

#[test]
fn a_declaration_spelled_like_a_file_selector_wins() {
    let tree = with_file(
        &edited(
            "write { paginate.body }",
            "write { paginate.body, project.manifest }",
        ),
        "src/project.cpl",
        "module project\n\npub fn manifest() -> Int {\n    1\n}\n",
    );
    let result = plain(&tree);
    assert_eq!(
        errors(&result),
        Vec::<&str>::new(),
        "{:#?}",
        result.diagnostics
    );
    assert_eq!(
        warnings(&result),
        [
            "work_file_selector_shadowed",
            "work_provisional_identity",
            "work_provisional_identity",
            "git_unavailable"
        ]
    );
    let d = one(&result, "work_file_selector_shadowed");
    assert_eq!(spanned(&tree, d), "project.manifest");
    assert_eq!(d.symbol, Some(id("project.manifest")));
    assert_eq!(d.data["file"], json!("cpl.toml"));
    let task = &result.model.expect("a model").changes[0].tasks[0];
    assert!(task.write_files.is_empty());
    assert_eq!(
        task.write[1].targets,
        [(id("project.manifest"), FacetSelector::Definition)]
    );
}

#[test]
fn provisional_identities_warn_once_per_declaration() {
    // `Positive` named twice: one warning, with a note at the second name.
    let tree = edited(
        "        write {}\n",
        "        write {}\n        depend { Positive }\n",
    );
    let result = plain(&tree);
    assert_eq!(errors(&result), Vec::<&str>::new());
    assert_eq!(warnings(&result), BASELINE);
    let d = one(&result, "work_provisional_identity");
    assert_eq!(spanned(&tree, d), "Positive.definition");
    assert_eq!(d.notes.len(), 1);
    let note = d.notes[0].span.expect("the note's span");
    assert_eq!(
        line_of(&tree, note.start),
        27,
        "the `depend` line added to `audit`"
    );

    // Namespaces name no declaration individually, and standard-library
    // declarations carry no `@id`: neither warns.
    let tree = edited(
        "depend { paginate.contract, Positive.definition }",
        "depend { paginate.contract, Positive.definition, std.prelude.flatten, reader.images.* }",
    );
    let result = plain(&tree);
    assert_eq!(errors(&result), Vec::<&str>::new());
    assert_eq!(warnings(&result), BASELINE);
}

#[test]
fn facets_must_apply_and_identity_is_never_written() {
    let tree = edited("Positive.definition }", "Positive.body }");
    let result = plain(&tree);
    assert_errors(&result, &["work_facet_not_applicable"]);
    let d = one(&result, "work_facet_not_applicable");
    assert_eq!(spanned(&tree, d), "Positive.body");
    assert_eq!(d.symbol, Some(positive()));
    assert_eq!(d.facet, Some(Facet::Body));
    assert_eq!(d.data["kind"], json!("ty"));
    assert_eq!(warnings(&result), BASELINE);

    let tree = edited(
        "write { paginate.body }",
        "write { paginate.body, paginate.identity }",
    );
    let result = plain(&tree);
    assert_errors(&result, &["work_identity_not_writable"]);
    let d = one(&result, "work_identity_not_writable");
    assert_eq!(spanned(&tree, d), "paginate.identity");
    assert_eq!(d.symbol, Some(pgn()));
    assert_eq!(d.facet, Some(Facet::Identity));

    // A whole declaration in a write set writes every facet but identity:
    // here the four facets `implementation` preserves, and not the
    // identity it now preserves as well.
    let tree = edited("write { paginate.body }", "write { paginate }");
    let tree = with_file(
        &tree,
        WORK,
        &text(&tree, WORK).replacen(
            "            paginate.verification_policy,\n",
            "            paginate.verification_policy,\n            paginate.identity,\n",
            1,
        ),
    );
    let result = plain(&tree);
    assert_errors(
        &result,
        &[
            "work_preserve_written",
            "work_preserve_written",
            "work_preserve_written",
            "work_preserve_written",
        ],
    );
    let preserved: Vec<String> = result
        .diagnostics
        .iter()
        .filter(|d| d.code == "work_preserve_written")
        .map(|d| spanned(&tree, d))
        .collect();
    assert_eq!(
        preserved,
        [
            "paginate.signature",
            "paginate.contract",
            "paginate.effects",
            "paginate.verification_policy"
        ]
    );
    // Without a preserve set, a whole-declaration write is valid.
    let result = plain(&edited("write {}", "write { paginate }"));
    assert_eq!(errors(&result), Vec::<&str>::new());
    // Identity may be depended on and preserved.
    let result = plain(&edited(
        "depend { paginate.contract, Positive.definition }",
        "depend { paginate.contract, Positive.definition, paginate.identity }",
    ));
    assert_eq!(errors(&result), Vec::<&str>::new());
}

// ---------------------------------------------------------------------------
// Tasks: `after`, overlaps, preserve, authority
// ---------------------------------------------------------------------------

#[test]
fn after_names_tasks_of_the_change_and_forms_no_cycle() {
    let tree = edited(
        "task audit after implementation, examples",
        "task audit after implementation, phantom",
    );
    let result = plain(&tree);
    assert_errors(&result, &["work_unknown_task"]);
    let d = one(&result, "work_unknown_task");
    assert_eq!(spanned(&tree, d), "phantom");
    assert_eq!(d.data["task"], json!("phantom"));
    assert_eq!(d.data["change"], json!("pagination_update"));

    let tree = edited("task implementation {", "task implementation after audit {");
    let result = plain(&tree);
    assert_errors(&result, &["work_cycle"]);
    let d = one(&result, "work_cycle");
    assert_eq!(spanned(&tree, d), "after audit");
    assert_eq!(
        d.data["tasks"],
        json!(["implementation", "audit", "implementation"])
    );

    let tree = edited("task examples {", "task examples after examples {");
    let result = plain(&tree);
    assert_errors(&result, &["work_cycle"]);
    assert_eq!(
        one(&result, "work_cycle").data["tasks"],
        json!(["examples", "examples"])
    );
}

#[test]
fn unordered_tasks_never_overlap() {
    // `examples` also writes `paginate.body`; neither task runs after the other.
    let tree = edited(
        "        add { tests.pagination.* }\n",
        "        write { paginate.body }\n        add { tests.pagination.* }\n",
    );
    let result = plain(&tree);
    assert_errors(&result, &["work_overlapping_writes"]);
    let d = one(&result, "work_overlapping_writes");
    assert_eq!(spanned(&tree, d), "paginate.body");
    assert_eq!(
        line_of(&tree, d.span.expect("a span").start),
        21,
        "the later task's selector"
    );
    assert_eq!(d.notes.len(), 1);
    assert_eq!(
        line_of(&tree, d.notes[0].span.expect("a note span").start),
        9
    );
    assert_eq!(d.data["tasks"], json!(["implementation", "examples"]));
    assert_eq!(
        d.data["overlap"],
        json!([{ "id": "pgn_7c2d", "facet": "body" }])
    );
    assert_eq!(d.symbol, Some(pgn()));
    assert_eq!(d.facet, Some(Facet::Body));

    // A whole declaration overlaps the one facet the other task writes.
    let result = plain(&edited(
        "        add { tests.pagination.* }\n",
        "        write { paginate }\n        add { tests.pagination.* }\n",
    ));
    assert_errors(&result, &["work_overlapping_writes"]);
    assert_eq!(
        one(&result, "work_overlapping_writes").data["overlap"],
        json!([{ "id": "pgn_7c2d", "facet": "body" }])
    );

    // A later task may edit the output of an explicit predecessor.
    let result = plain(&edited("write {}", "write { paginate.body }"));
    assert_eq!(errors(&result), Vec::<&str>::new());

    // Overlapping `add` namespaces (T43, segment-wise).
    let tree = edited(
        "        write { paginate.body }\n",
        "        write { paginate.body }\n        add { tests.* }\n",
    );
    let result = plain(&tree);
    assert_errors(&result, &["work_overlapping_writes"]);
    let d = one(&result, "work_overlapping_writes");
    assert_eq!(spanned(&tree, d), "tests.pagination.*");
    assert_eq!(d.data["namespaces"], json!(["tests", "tests.pagination"]));
    // Disjoint namespaces do not overlap.
    let result = plain(&edited(
        "        write { paginate.body }\n",
        "        write { paginate.body }\n        add { testsuite.* }\n",
    ));
    assert_eq!(errors(&result), Vec::<&str>::new());
}

#[test]
fn a_task_never_preserves_what_it_writes() {
    let tree = edited(
        "            paginate.verification_policy,\n",
        "            paginate.verification_policy,\n            paginate.body,\n",
    );
    let result = plain(&tree);
    assert_errors(&result, &["work_preserve_written"]);
    let d = one(&result, "work_preserve_written");
    assert_eq!(spanned(&tree, d), "paginate.body");
    assert_eq!(line_of(&tree, d.span.expect("a span").start), 16);
    assert_eq!(d.symbol, Some(pgn()));
    assert_eq!(d.facet, Some(Facet::Body));
    assert_eq!(d.data["task"], json!("implementation"));
    assert_eq!(
        d.data["overlap"],
        json!([{ "id": "pgn_7c2d", "facet": "body" }])
    );
    assert_eq!(
        line_of(&tree, d.notes[0].span.expect("a note span").start),
        9
    );

    // Preserving the whole declaration includes the written body.
    let result = plain(&edited(
        "            paginate.verification_policy,\n",
        "            paginate.verification_policy,\n            paginate,\n",
    ));
    assert_errors(&result, &["work_preserve_written"]);
}

#[test]
fn a_task_without_authority_must_be_reviewed() {
    let tree = edited("        require review(audit)\n", "");
    let result = plain(&tree);
    assert_eq!(errors(&result), Vec::<&str>::new());
    assert_eq!(
        warnings(&result),
        [
            "work_provisional_identity",
            "work_task_without_authority",
            "git_unavailable"
        ]
    );
    let d = one(&result, "work_task_without_authority");
    assert_eq!(spanned(&tree, d), "audit");
    assert_eq!(d.data["task"], json!("audit"));
}

// ---------------------------------------------------------------------------
// integrate
// ---------------------------------------------------------------------------

#[test]
fn test_requirements_match_at_least_one_test() {
    let requirement = "require test(tests.pagination.*)";

    let tree = edited(requirement, "require test(reader.images.*)");
    let result = plain(&tree);
    assert_errors(&result, &["work_test_selector_empty"]);
    let d = one(&result, "work_test_selector_empty");
    assert_eq!(spanned(&tree, d), "reader.images.*");
    assert_eq!(d.data["namespace"], json!("reader.images"));
    assert_eq!(result.output.changes[0].integrate[2].matched, Some(0));

    // A declaration selector selects the tests that reach it (6.5).
    let result = plain(&edited(requirement, "require test(paginate)"));
    assert_eq!(errors(&result), Vec::<&str>::new());
    assert_eq!(
        serde_json::to_value(&result.output.changes[0].integrate[2]).expect("json"),
        json!({ "require": "test", "targets": [{ "id": "pgn_7c2d", "facet": "definition" }], "matched": 3 })
    );
    match &result.model.expect("a model").changes[0].integrate[2] {
        RequirementSpec::Test(ResolvedNamespaceOrSelector::Selector(sel)) => {
            assert_eq!(sel.targets, [(pgn(), FacetSelector::Definition)]);
        }
        other => panic!("test(paginate), found {other:?}"),
    }

    // No test calls `paginate_ints`, which has no @id either.
    let tree = edited(requirement, "require test(reader.pages.paginate_ints)");
    let result = plain(&tree);
    assert_errors(&result, &["work_test_selector_empty"]);
    assert_eq!(
        one(&result, "work_test_selector_empty").symbol,
        Some(id("reader.pages.paginate_ints"))
    );
    assert_eq!(
        warnings(&result),
        [
            "work_provisional_identity",
            "work_provisional_identity",
            "git_unavailable"
        ]
    );

    // An unresolvable declaration selector matches no test either.
    let result = plain(&edited(requirement, "require test(nothing)"));
    assert_errors(&result, &["work_test_selector_empty"]);

    // A facet selector selects no tests.
    let tree = edited(requirement, "require test(paginate.body)");
    let result = plain(&tree);
    assert_errors(&result, &["work_facet_not_applicable"]);
    let d = one(&result, "work_facet_not_applicable");
    assert_eq!(spanned(&tree, d), "paginate.body");
    assert_eq!(d.facet, Some(Facet::Body));
}

#[test]
fn prove_requirements_name_functions() {
    let requirement = "require prove(paginate)";

    let tree = edited(requirement, "require prove(Positive)");
    let result = plain(&tree);
    assert_errors(&result, &["work_prove_not_function"]);
    let d = one(&result, "work_prove_not_function");
    assert_eq!(spanned(&tree, d), "Positive");
    assert_eq!(d.symbol, Some(positive()));
    assert_eq!(d.data["kind"], json!("ty"));
    // `Positive` is now named twice: still one provisional warning.
    assert_eq!(
        one(&result, "work_provisional_identity").notes.len(),
        1,
        "a note at the prove requirement"
    );

    let result = plain(&edited(requirement, "require prove(tests.pagination.*)"));
    assert_errors(&result, &["work_prove_not_function"]);

    // A namespace proves the functions in it.
    let result = plain(&edited(requirement, "require prove(reader.pages.*)"));
    assert_eq!(errors(&result), Vec::<&str>::new());
    assert_eq!(warnings(&result), BASELINE);
    let model = result.model.expect("a model");
    let RequirementSpec::Prove(sel) = &model.changes[0].integrate[1] else {
        panic!("a prove requirement");
    };
    assert_eq!(sel.targets.len(), 9, "{:?}", sel.targets);
    assert!(sel.targets.contains(&(pgn(), FacetSelector::Definition)));
    assert!(!sel.targets.iter().any(|(id, _)| *id == positive()));

    // A selector that names nothing names no function: the code of the
    // `prove` rule of 7.2, since 7.2 and Appendix B give
    // `work_selector_empty` to write/depend/preserve selectors only.
    let tree = edited(requirement, "require prove(nothing)");
    let result = plain(&tree);
    assert_errors(&result, &["work_prove_not_function"]);
    let d = one(&result, "work_prove_not_function");
    assert_eq!(spanned(&tree, d), "nothing");
    assert_eq!(d.data["selector"], json!("nothing"));
    assert!(d.symbol.is_none(), "no declaration to blame");
}

#[test]
fn review_requirements_name_tasks() {
    let tree = edited("require review(audit)", "require review(auditor)");
    let result = plain(&tree);
    assert_errors(&result, &["work_unknown_task"]);
    let d = one(&result, "work_unknown_task");
    assert_eq!(spanned(&tree, d), "auditor");
    assert_eq!(d.data["task"], json!("auditor"));
    // `audit` is reviewed by no requirement now.
    assert_eq!(
        warnings(&result),
        [
            "work_provisional_identity",
            "work_task_without_authority",
            "git_unavailable"
        ]
    );
}

// ---------------------------------------------------------------------------
// base and target in a repository (D35)
// ---------------------------------------------------------------------------

fn git_available() -> bool {
    let found = Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !found {
        eprintln!("skipped: git is not installed");
    }
    found
}

/// Run git in `dir` for the test's setup, with a fixed identity in the
/// environment and no hooks or signing.
fn run(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "tag.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .env("LC_ALL", "C")
        .env("GIT_AUTHOR_NAME", "cpl test")
        .env("GIT_AUTHOR_EMAIL", "cpl@example.invalid")
        .env("GIT_COMMITTER_NAME", "cpl test")
        .env("GIT_COMMITTER_EMAIL", "cpl@example.invalid")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("utf-8")
        .trim()
        .to_string()
}

/// A fresh repository holding `examples/reader`: one commit on `main`, a
/// lightweight tag `v1`, and a branch `feature`.
fn repository(name: &str) -> GitRepo {
    let dir = temp_dir(name).join("repo");
    for (path, bytes) in &reader_tree().files {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().expect("a parent")).expect("create a directory");
        std::fs::write(&file, bytes).expect("write a file");
    }
    run(&dir, &["init", "-q", "-b", "main"]);
    run(&dir, &["add", "-A"]);
    run(&dir, &["commit", "-q", "-m", "reader"]);
    run(&dir, &["tag", "v1"]);
    run(&dir, &["branch", "feature"]);
    GitRepo {
        toplevel: dir,
        project_prefix: String::new(),
    }
}

#[test]
fn base_and_target_resolve_in_the_repository() {
    if !git_available() {
        return;
    }
    let repo = repository("refs");
    let head = run(&repo.toplevel, &["rev-parse", "HEAD"]);
    let result = validate(&reader_tree(), None, Some(&repo));
    assert_eq!(
        errors(&result),
        Vec::<&str>::new(),
        "{:#?}",
        result.diagnostics
    );
    assert_eq!(warnings(&result), ["work_provisional_identity"]);
    assert_eq!(
        result.output.changes[0].base_commit.as_deref(),
        Some(head.as_str())
    );
    assert_eq!(
        serde_json::to_value(&result.output).expect("json")["changes"][0]["base_commit"],
        json!(head)
    );

    let base = "base git(\"main\")";
    let target = "target git(\"main\")";
    let accepted = [
        (base, "base git(\"v1\")".to_string()),
        (base, format!("base git(\"{head}\")")),
        (base, "base git(\"feature\")".to_string()),
        (target, "target git(\"feature\")".to_string()),
        (target, "target git(\"refs/heads/main\")".to_string()),
    ];
    for (from, to) in accepted {
        let result = validate(&edited(from, &to), None, Some(&repo));
        assert_eq!(
            errors(&result),
            Vec::<&str>::new(),
            "{to}: {:#?}",
            result.diagnostics
        );
    }

    let refused = [
        (base, "base git(\"nope\")".to_string(), "work_unknown_ref"),
        (
            target,
            "target git(\"nope\")".to_string(),
            "work_unknown_ref",
        ),
        (
            target,
            "target git(\"v1\")".to_string(),
            "work_target_not_branch",
        ),
        (
            target,
            "target git(\"refs/tags/v1\")".to_string(),
            "work_target_not_branch",
        ),
        (
            target,
            format!("target git(\"{head}\")"),
            "work_target_not_branch",
        ),
        (
            target,
            "target git(\"main~0\")".to_string(),
            "work_target_not_branch",
        ),
        (
            target,
            "target git(\"HEAD\")".to_string(),
            "work_target_not_branch",
        ),
    ];
    for (from, to, code) in refused {
        let tree = edited(from, &to);
        let result = validate(&tree, None, Some(&repo));
        assert_errors(&result, &[code]);
        let d = one(&result, code);
        assert_eq!(d.category, Category::Work);
        assert!(spanned(&tree, d).starts_with("git(\""), "{to}");
        assert_eq!(d.data["change"], json!("pagination_update"));
        if code == "work_target_not_branch" {
            assert_eq!(d.data["commit"], json!(head), "{to}");
        } else {
            assert_eq!(d.data["ref"], json!("nope"));
        }
    }
}

#[test]
fn a_directory_outside_any_repository_skips_resolution() {
    if !git_available() {
        return;
    }
    let dir = temp_dir("no-repository");
    let inside = Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .is_ok_and(|o| o.status.success());
    if inside {
        eprintln!("skipped: the temporary directory is inside a git repository");
        return;
    }
    let repo = GitRepo {
        toplevel: dir,
        project_prefix: String::new(),
    };
    let result = validate(&reader_tree(), None, Some(&repo));
    assert_eq!(
        errors(&result),
        Vec::<&str>::new(),
        "{:#?}",
        result.diagnostics
    );
    assert_eq!(warnings(&result), BASELINE);
    assert_eq!(
        one(&result, "git_unavailable").data["reason"],
        json!("not_a_repository")
    );
    assert_eq!(result.output.changes[0].base_commit, None);
}

#[test]
fn a_repository_whose_directory_is_gone_skips_resolution() {
    let repo = GitRepo {
        toplevel: temp_dir("gone").join("removed"),
        project_prefix: String::new(),
    };
    let result = validate(&reader_tree(), None, Some(&repo));
    assert_eq!(
        errors(&result),
        Vec::<&str>::new(),
        "{:#?}",
        result.diagnostics
    );
    assert_eq!(warnings(&result), BASELINE);
    assert_eq!(
        one(&result, "git_unavailable").data["reason"],
        json!("not_a_repository")
    );
}

#[test]
fn pagination_work_validates_from_the_examples_directory() {
    // The same file loaded from disk (`FileOrigin::Project`), as the CLI
    // loads it, gives the same result as from its tree.
    let project = cpl_project::load_project(
        &workspace_path("examples/reader"),
        &cpl_project::LoadOptions {
            include_work_files: true,
            ..cpl_project::LoadOptions::default()
        },
    )
    .expect("examples/reader loads");
    let checked = cpl_sema::check(&project);
    assert!(!checked.has_errors());
    let result = validate_work_file(work_file(&project, WORK), &checked.program, None);
    assert_eq!(codes(&result.diagnostics), BASELINE);
    assert_eq!(result.output.file, "work/pagination.work");
    assert_eq!(result, plain(&reader_tree()), "disk and tree agree");
}

/// A mutation of the corpus work file: the code it triggers, the text to
/// replace and its replacement, and a source file to add (path, text).
type Mutation = (
    &'static str,
    &'static str,
    &'static str,
    Option<(&'static str, &'static str)>,
);

/// One mutation of the corpus work file per code of Section 7.2 that
/// `cpl work check` owns (the Milestone 6 codes are the scheduler's): the
/// code is among the result's diagnostics.
#[test]
fn mutated_copies_trigger_every_work_check_code() {
    let declaration = (
        "src/project.cpl",
        "module project\n\npub fn manifest() -> Int {\n    1\n}\n",
    );
    let cases: Vec<Mutation> = vec![
        ("work_missing_base", "    base git(\"main\")\n", "", None),
        (
            "work_missing_target",
            "    target git(\"main\")\n",
            "",
            None,
        ),
        ("work_bad_workers", "workers 3", "workers 0", None),
        (
            "work_selector_empty",
            "write { paginate.body }",
            "write { paginate.body, nothing.here }",
            None,
        ),
        (
            "work_add_namespace_new",
            "add { tests.pagination.* }",
            "add { tests.edge.* }",
            None,
        ),
        (
            "work_file_selector_shadowed",
            "write { paginate.body }",
            "write { paginate.body, project.manifest }",
            Some(declaration),
        ),
        (
            "work_file_not_grantable",
            "write { paginate.body }",
            "write { paginate.body, project.manifest }",
            None,
        ),
        // The corpus itself names `Positive.definition`.
        ("work_provisional_identity", "", "", None),
        (
            "work_facet_not_applicable",
            "Positive.definition }",
            "Positive.body }",
            None,
        ),
        (
            "work_identity_not_writable",
            "write { paginate.body }",
            "write { paginate.identity }",
            None,
        ),
        (
            "work_unknown_task",
            "require review(audit)",
            "require review(nobody)",
            None,
        ),
        (
            "work_cycle",
            "task implementation {",
            "task implementation after audit {",
            None,
        ),
        (
            "work_overlapping_writes",
            "        add { tests.pagination.* }\n",
            "        write { paginate.body }\n        add { tests.pagination.* }\n",
            None,
        ),
        (
            "work_test_selector_empty",
            "require test(tests.pagination.*)",
            "require test(reader.images.*)",
            None,
        ),
        (
            "work_prove_not_function",
            "require prove(paginate)",
            "require prove(Positive)",
            None,
        ),
        (
            "work_preserve_written",
            "            paginate.verification_policy,\n",
            "            paginate.verification_policy,\n            paginate.body,\n",
            None,
        ),
        (
            "work_task_without_authority",
            "        require review(audit)\n",
            "",
            None,
        ),
    ];
    let mut covered = std::collections::BTreeSet::new();
    for (code, from, to, extra) in &cases {
        let mut tree = edited(from, to);
        if let Some((path, source)) = extra {
            tree = with_file(&tree, path, source);
        }
        let result = plain(&tree);
        assert!(
            result.diagnostics.iter().any(|d| d.code == *code),
            "{code}: {:#?}",
            result.diagnostics
        );
        covered.insert(*code);
    }
    // The two codes that need a repository.
    if git_available() {
        let repo = repository("coverage");
        for (code, from, to) in [
            (
                "work_unknown_ref",
                "base git(\"main\")",
                "base git(\"nope\")",
            ),
            (
                "work_target_not_branch",
                "target git(\"main\")",
                "target git(\"v1\")",
            ),
        ] {
            let result = validate(&edited(from, to), None, Some(&repo));
            assert_eq!(errors(&result), [code], "{:#?}", result.diagnostics);
            covered.insert(code);
        }
    } else {
        covered.extend(["work_unknown_ref", "work_target_not_branch"]);
    }
    let scheduler_only = [
        "work_stale_submission",
        "work_conflict",
        "work_stale_against_target",
        "work_review_stale",
        "work_not_authorized",
        "work_boundary_unenforceable",
    ];
    let owned: std::collections::BTreeSet<&str> = cpl_work::codes::WORK_CODES
        .iter()
        .map(|c| c.0)
        .filter(|c| !scheduler_only.contains(c))
        .collect();
    assert_eq!(covered, owned);
}
