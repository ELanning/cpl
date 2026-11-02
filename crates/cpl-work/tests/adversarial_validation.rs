//! Adversarial tests of `cpl work check` (`validate_work_file`) and of
//! `resolve_patches`.
//!
//! The lens: `spec/toolchain.md` 7.2 (`cpl work check`) and 1.9 (the
//! `cpl_work` API), grammar 4 (`.work` and `.patch` files, D34-D40, D68,
//! D69), and design-doc.md "Changes and agent tasks": every `work_*` code
//! with the fields and span the specifications give it, task graphs,
//! selectors resolved through the work file's `use` lines, namespaces and
//! `add` permissions, file selectors, requirements, `expect base` and
//! provisional identities, no repository versus a temporary repository with
//! revisions, and `resolve_patches` on valid and invalid patch files. Every
//! expectation is the specification's answer; where it is silent, the test
//! says so and follows design-doc.md.
//!
//! The tests run on a project of their own, so that every declaration kind
//! of Section 5.1, nested, empty, and look-alike modules, and a module
//! spelled like a file selector are at hand (`@id` values in parentheses):
//!
//! | Module | Declarations |
//! | - | - |
//! | `app.core` | `double` (`fn_double`, with a contract), `helper` (calls `double`), the private `hidden`, the refinement `Small` (`ty_small`), the wrapper `Name` (`ty_name`), the struct `Pair` (`ty_pair`), the enum `Choice` (`ty_choice`), the capability `Store` (`cap_store`), the effectful `fetch` (`fn_fetch`) |
//! | `app.core.deep` | `inner` |
//! | `app.empty` | nothing: a module header alone |
//! | `apps.misc` | `other` (`app.*` must not cover it) |
//! | `cfg.project` | `lock` (`project.lock` through `use cfg.{project}`) |
//! | `tests.app` | two tests, one calling `double`, one calling `helper` |
//! | `tests.other` | a test and a property calling neither |

mod common;

use common::{checked, codes, load, map_of, temp_dir, text, tree, with_file, work_file};
use cpl_diag::{Category, Diagnostic, Facet, FacetSelector, FileId, Severity, StableId};
use cpl_project::{VirtualTree, WorkPolicySection};
use cpl_sema::SelectorScope;
use cpl_work::{
    apply_patch, parse_submitted_patch, resolve_patches, validate_patch, validate_work_file,
    validate_work_file_with_policy, FileSelector, GitRepo, Grant, RequirementSpec,
    ResolvedNamespaceOrSelector, ResolvedPatch, ResolvedSelector, WorkCheckResult, WorkModel,
};
use serde_json::json;
use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

// ===========================================================================
// The project
// ===========================================================================

const W: &str = "work/w.work";
const CORE_PATH: &str = "src/app/core.cpl";

const CORE: &str = r#"module app.core

@id("fn_double")
pub fn double(n: Int) -> Int
    ensures result == n * 2
{
    n * 2
}

pub fn helper(n: Int) -> Int {
    double(n) + 1
}

fn hidden() -> Int {
    1
}

@id("ty_small")
pub type Small = Int where self < 10

@id("ty_name")
pub type Name(Text)

@id("ty_pair")
pub struct Pair {
    left: Int
    right: Int
}

@id("ty_choice")
pub enum Choice {
    Yes
    No
}

@id("cap_store")
pub capability Store {
    fn get(key: Text) -> Int
}

@id("fn_fetch")
pub fn fetch(key: Text, using store: Store) -> Int
    effects { store.get }
{
    store.get(key)
}
"#;

const DEEP: &str = "module app.core.deep\n\npub fn inner() -> Int {\n    7\n}\n";
const MISC: &str = "module apps.misc\n\npub fn other() -> Int {\n    3\n}\n";
const CFG: &str = "module cfg.project\n\npub fn lock() -> Int {\n    0\n}\n";

const TESTS_APP: &str = r#"module tests.app

use app.core.{double, helper}

test "double doubles" {
    assert double(2) == 4
}

test "helper adds one" {
    assert helper(1) == 3
}
"#;

const TESTS_OTHER: &str = r#"module tests.other

test "arithmetic" {
    assert 1 + 1 == 2
}

property "addition commutes"
    for a in gen.int
    for b in gen.int
{
    assert a + b == b + a
}
"#;

fn files() -> Vec<(&'static str, &'static str)> {
    vec![
        (CORE_PATH, CORE),
        ("src/app/core/deep.cpl", DEEP),
        ("src/app/empty.cpl", "module app.empty\n"),
        ("src/apps/misc.cpl", MISC),
        ("src/cfg/project.cpl", CFG),
        ("src/tests/app.cpl", TESTS_APP),
        ("src/tests/other.cpl", TESTS_OTHER),
    ]
}

/// The project with `work` as `work/w.work`.
fn project(work: &str) -> VirtualTree {
    let mut all: Vec<(&str, &str)> = files();
    all.push((W, work));
    tree(&all)
}

fn id(text: &str) -> StableId {
    StableId::parse(text).expect("a stable id")
}

fn facet(f: Facet) -> FacetSelector {
    FacetSelector::Facet(f)
}

/// A `[work.policy]` whose `files` are these spellings.
fn policy(files: &[&str]) -> WorkPolicySection {
    WorkPolicySection {
        files: files.iter().map(|f| f.to_string()).collect(),
        ..WorkPolicySection::default()
    }
}

fn targets(selectors: &[ResolvedSelector]) -> Vec<Vec<(StableId, FacetSelector)>> {
    selectors.iter().map(|s| s.targets.clone()).collect()
}

/// `a.b.c` as `["a", "b", "c"]`.
fn path(dotted: &str) -> Vec<String> {
    dotted.split('.').map(str::to_string).collect()
}

// ---------------------------------------------------------------------------
// Work file text
// ---------------------------------------------------------------------------

/// A work file: the `use` lines, then the change `c` on `main`.
fn work(uses: &[&str], items: &[String]) -> String {
    let mut out = String::new();
    for line in uses {
        out.push_str(line);
        out.push('\n');
    }
    if !uses.is_empty() {
        out.push('\n');
    }
    out.push_str(&change("c", items));
    out
}

/// `change <name>` with `base git("main")` and `target git("main")`.
fn change(name: &str, items: &[String]) -> String {
    change_on(name, "main", "main", items)
}

/// `change <name>` with these refs.
fn change_on(name: &str, base: &str, target: &str, items: &[String]) -> String {
    let mut out =
        format!("change {name} {{\n    base git(\"{base}\")\n    target git(\"{target}\")\n");
    for item in items {
        out.push('\n');
        out.push_str(item);
    }
    out.push_str("}\n");
    out
}

/// `task <header> { <lines> }`; the header may carry an `after` clause.
fn task(header: &str, lines: &[&str]) -> String {
    let mut out = format!("    task {header} {{\n");
    for line in lines {
        out.push_str("        ");
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("    }\n");
    out
}

/// `integrate { require <requirement> ... }`.
fn integrate(requirements: &[&str]) -> String {
    let mut out = String::from("    integrate {\n");
    for requirement in requirements {
        out.push_str("        require ");
        out.push_str(requirement);
        out.push('\n');
    }
    out.push_str("    }\n");
    out
}

/// A task that writes `double`'s body and nothing else.
fn writer() -> String {
    task("t", &["write { app.core.double.body }"])
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// One work file validated against the checked project.
struct Run {
    tree: VirtualTree,
    result: WorkCheckResult,
}

/// `validate_work_file` without a repository (the 1.9 entry point, with the
/// default `[work.policy]`).
fn run(work: &str) -> Run {
    let tree = project(work);
    let (project, program) = checked(&tree);
    let result = validate_work_file(work_file(&project, W), &program, None);
    Run { tree, result }
}

/// `validate_work_file_with_policy` with a policy and a repository.
fn run_with(work: &str, policy: &WorkPolicySection, repo: Option<&GitRepo>) -> Run {
    let tree = project(work);
    let (project, program) = checked(&tree);
    let result = validate_work_file_with_policy(work_file(&project, W), &program, policy, repo);
    Run { tree, result }
}

impl Run {
    fn codes_of(&self, severity: Severity) -> Vec<&'static str> {
        self.result
            .diagnostics
            .iter()
            .filter(|d| d.severity == severity)
            .map(|d| d.code.0)
            .collect()
    }

    fn errors(&self) -> Vec<&'static str> {
        self.codes_of(Severity::Error)
    }

    /// The warnings, without the `git_unavailable` of a run without a
    /// repository.
    fn warnings(&self) -> Vec<&'static str> {
        self.codes_of(Severity::Warning)
            .into_iter()
            .filter(|c| *c != "git_unavailable")
            .collect()
    }

    fn all(&self, code: &str) -> Vec<&Diagnostic> {
        self.result
            .diagnostics
            .iter()
            .filter(|d| d.code == code)
            .collect()
    }

    #[track_caller]
    fn one(&self, code: &str) -> &Diagnostic {
        let found = self.all(code);
        assert_eq!(
            found.len(),
            1,
            "one {code} in {:#?}",
            self.result.diagnostics
        );
        found[0]
    }

    fn text(&self) -> String {
        text(&self.tree, W)
    }

    /// The work-file text a diagnostic's primary span covers.
    #[track_caller]
    fn spanned(&self, d: &Diagnostic) -> String {
        let span = d.span.unwrap_or_else(|| panic!("{} has no span", d.code.0));
        self.text()[span.start as usize..span.end as usize].to_string()
    }

    /// Whether the primary span lies inside the first occurrence of `part`
    /// in the work file.
    #[track_caller]
    fn inside(&self, d: &Diagnostic, part: &str) -> bool {
        let text = self.text();
        let start = text
            .find(part)
            .unwrap_or_else(|| panic!("no {part:?} in the work file"));
        let span = d.span.unwrap_or_else(|| panic!("{} has no span", d.code.0));
        start <= span.start as usize && span.end as usize <= start + part.len()
    }

    #[track_caller]
    fn model(&self) -> &WorkModel {
        self.result
            .model
            .as_ref()
            .unwrap_or_else(|| panic!("no model: {:#?}", self.result.diagnostics))
    }

    /// No errors, so a model.
    #[track_caller]
    fn valid(&self) {
        assert_eq!(
            self.errors(),
            Vec::<&str>::new(),
            "{}\n{:#?}",
            self.text(),
            self.result.diagnostics
        );
        assert!(!self.result.has_errors());
        assert!(self.result.model.is_some(), "a valid file has a model");
    }

    /// Exactly these errors, in diagnostic order, and no model.
    #[track_caller]
    fn refused(&self, expected: &[&str]) {
        assert_eq!(
            self.errors(),
            expected,
            "{}\n{:#?}",
            self.text(),
            self.result.diagnostics
        );
        assert!(self.result.has_errors());
        assert!(self.result.model.is_none(), "no model with errors");
    }
}

/// S49: `qualified_name` whenever `symbol` is present, and `facet` only
/// together with `symbol`.
#[track_caller]
fn assert_envelope(d: &Diagnostic) {
    assert_eq!(
        d.symbol.is_some(),
        d.qualified_name.is_some(),
        "{}: `qualified_name` accompanies `symbol` (S49): {d:#?}",
        d.code.0
    );
    assert!(
        d.facet.is_none() || d.symbol.is_some(),
        "{}: `facet` only together with `symbol` (S49): {d:#?}",
        d.code.0
    );
}

// ===========================================================================
// `use` lines: "every `use` resolves (as in a `.cpl` file, semantics 1.6)"
// ===========================================================================

/// Each import that fails is diagnosed exactly as the checker diagnoses the
/// same line in a `.cpl` module of the project: same code, category,
/// severity, `symbol` and `qualified_name`, every payload field the checker
/// gives (the toolchain may add fields, never rename or omit them, S60),
/// and the same text under the span. A work file belongs to no module, so
/// a private item is `private_item` there as from any other module.
#[test]
fn use_lines_are_diagnosed_exactly_as_in_a_source_file() {
    let lines = [
        "use app.core.{nope}",
        "use app.nowhere.{thing}",
        "use app.nowhere.{one, two}",
        "use app.{thing}",
        "use app.core.{hidden}",
        "use app.core.{Yes}",
        "use app.core.{get}",
        "use gen.{int}",
        "use std.text.{nope}",
        "use app.core.nope",
        "use nowhere.thing",
        // An imported name is never an item of the importing module
        // (semantics S3: no re-export).
        "use tests.app.{double}",
    ];
    for line in lines {
        let source = format!("module probe\n\n{line}\n");
        let mut all: Vec<(&str, &str)> = files();
        all.push(("src/probe.cpl", source.as_str()));
        let reference = cpl_sema::check(&load(&tree(&all)));
        let expected: Vec<&Diagnostic> = reference
            .diagnostics
            .iter()
            .filter(|d| d.is_error())
            .collect();
        assert_eq!(
            expected.len(),
            1,
            "{line} in a .cpl file: {:#?}",
            reference.diagnostics
        );
        let expected = expected[0];
        let span = expected.span.expect("the checker's span");
        let expected_text = &source[span.start as usize..span.end as usize];

        let r = run(&work(&[line], &[writer()]));
        let found: Vec<&Diagnostic> = r
            .result
            .diagnostics
            .iter()
            .filter(|d| d.is_error())
            .collect();
        assert_eq!(
            found.len(),
            1,
            "{line} in a .work file: {:#?}",
            r.result.diagnostics
        );
        let found = found[0];
        assert_eq!(found.code, expected.code, "{line}");
        assert_eq!(found.category, Category::Resolve, "{line}");
        assert_eq!(found.category, expected.category, "{line}");
        assert_eq!(found.severity, expected.severity, "{line}");
        assert_eq!(found.symbol, expected.symbol, "{line}");
        assert_eq!(found.qualified_name, expected.qualified_name, "{line}");
        for (key, value) in &expected.data {
            assert_eq!(
                found.data.get(key),
                Some(value),
                "{line}: the field `{key}`"
            );
        }
        assert_eq!(r.spanned(found), expected_text, "{line}: the span");
        assert!(r.result.model.is_none(), "{line}");
    }
}

/// The single form (D15), a submodule alias, and a standard-library import
/// all bring names into the selectors' scope (grammar 4.1: "it makes the
/// imported names available to selectors").
#[test]
fn every_use_form_brings_its_names_into_selector_scope() {
    let r = run(&work(
        &[
            "use app.core.double",
            "use app.{core}",
            "use std.text.{trim}",
        ],
        &[task(
            "t",
            &[
                "write { double.body, core.helper.body, core.deep.inner.body }",
                "depend { trim, std.prelude.flatten }",
            ],
        )],
    ));
    r.valid();
    let t = &r.model().changes[0].tasks[0];
    assert_eq!(
        targets(&t.write),
        [
            vec![(id("fn_double"), facet(Facet::Body))],
            vec![(id("app.core.helper"), facet(Facet::Body))],
            vec![(id("app.core.deep.inner"), facet(Facet::Body))],
        ]
    );
    assert_eq!(
        targets(&t.depend),
        [
            vec![(id("std.text.trim"), FacetSelector::Definition)],
            vec![(id("std.prelude.flatten"), FacetSelector::Definition)],
        ]
    );
    // `helper` and `inner` carry no `@id` (T47).
    let provisional: BTreeSet<StableId> = r
        .all("work_provisional_identity")
        .iter()
        .filter_map(|d| d.symbol.clone())
        .collect();
    assert!(
        provisional.contains(&id("app.core.helper")),
        "{provisional:?}"
    );
    assert!(
        provisional.contains(&id("app.core.deep.inner")),
        "{provisional:?}"
    );
}

/// Imports are scoped to the file that contains them (semantics S5): the
/// `use app.core.{double, helper}` of `tests.app` is nothing to a work
/// file, whose selectors resolve "against the use imports of the .work file
/// and otherwise fully qualified" (grammar 4.4).
#[test]
fn a_work_file_sees_only_its_own_imports() {
    let r = run(&work(&[], &[task("t", &["write { double.body }"])]));
    r.refused(&["work_selector_empty"]);
    assert_eq!(r.spanned(r.one("work_selector_empty")), "double.body");
}

/// The prelude is not among a work file's imports (grammar 4.4): a bare
/// `flatten` names nothing, `std.prelude.flatten` the prelude function.
#[test]
fn prelude_names_are_not_in_a_work_files_scope() {
    let r = run(&work(
        &[],
        &[task(
            "t",
            &["write { app.core.double.body }", "depend { flatten }"],
        )],
    ));
    r.refused(&["work_selector_empty"]);
    assert_eq!(r.spanned(r.one("work_selector_empty")), "flatten");
    let r = run(&work(
        &[],
        &[task(
            "t",
            &[
                "write { app.core.double.body }",
                "depend { std.prelude.flatten }",
            ],
        )],
    ));
    r.valid();
}

/// Visibility governs imports, not selectors: a task may be given a private
/// function's body, named fully qualified (design: `write` permits changes
/// to specified existing facets; nothing in 7.2 restricts it to public
/// declarations).
#[test]
fn a_private_declaration_is_selected_fully_qualified() {
    let r = run(&work(
        &[],
        &[task("t", &["write { app.core.hidden.body }"])],
    ));
    r.valid();
    assert_eq!(
        targets(&r.model().changes[0].tasks[0].write),
        [vec![(id("app.core.hidden"), facet(Facet::Body))]]
    );
}

// ===========================================================================
// base, target, workers (7.2 row 2, grammar 4.1)
// ===========================================================================

/// `workers` is a concurrency ceiling of at least 1 and defaults to 1; the
/// literal's separators are not part of its value (grammar 1.5), and the
/// specification sets no upper bound below the model's `u32`.
#[test]
fn workers_is_a_ceiling_of_at_least_one_defaulting_to_one() {
    let body = writer();
    for (line, expected) in [
        ("", 1u32),
        ("    workers 1\n", 1),
        ("    workers 1_000\n", 1000),
        ("    workers 4294967295\n", u32::MAX),
    ] {
        let text = format!(
            "change c {{\n    base git(\"main\")\n    target git(\"main\")\n{line}\n{body}}}\n"
        );
        let r = run(&text);
        r.valid();
        assert_eq!(r.model().changes[0].workers, expected, "{line:?}");
        assert_eq!(r.result.output.changes[0].workers, expected, "{line:?}");
    }
    let r = run(&format!(
        "change c {{\n    base git(\"main\")\n    target git(\"main\")\n    workers 0\n\n{body}}}\n"
    ));
    r.refused(&["work_bad_workers"]);
    let d = r.one("work_bad_workers");
    assert_eq!(d.category, Category::Work);
    assert!(r.inside(d, "workers 0"), "{:?}", r.spanned(d));
}

/// `base` and `target` are required per change: two changes in one file,
/// the second without either, report both errors at the second change only.
#[test]
fn missing_base_and_target_are_reported_at_their_own_change() {
    let first = change("first", &[writer()]);
    let second = "change second {\n    workers 2\n\n    task b {\n        write { app.core.fetch.body }\n    }\n}\n";
    let r = run(&format!("{first}\n{second}"));
    let mut errors = r.errors();
    errors.sort_unstable();
    assert_eq!(errors, ["work_missing_base", "work_missing_target"]);
    assert!(r.result.model.is_none());
    for code in ["work_missing_base", "work_missing_target"] {
        let d = r.one(code);
        assert_eq!((d.category, d.severity), (Category::Work, Severity::Error));
        assert!(r.inside(d, second), "{code} belongs to `second`: {d:#?}");
    }
    assert_eq!(r.result.output.changes.len(), 2);
}

/// Without a repository, resolution is skipped "with the warning
/// `git_unavailable`": one span-less warning of category `git` for the
/// whole file (it is about the repository, not a source location), listed
/// after every spanned diagnostic (T46), and no `base_commit` anywhere.
#[test]
fn no_repository_is_one_spanless_git_unavailable_warning() {
    let one = change("one", &[task("a", &["write { app.core.double.body }"])]);
    let two = change("two", &[task("b", &["write { app.core.double.contract }"])]);
    let r = run(&format!("{one}\n{two}"));
    r.valid();
    let d = r.one("git_unavailable");
    assert_eq!(d.category, Category::Git);
    assert_eq!(d.severity, Severity::Warning);
    assert_eq!(d.span, None);
    assert_eq!(r.result.diagnostics.last(), Some(d));
    assert!(r
        .result
        .output
        .changes
        .iter()
        .all(|c| c.base_commit.is_none()));
    let json = serde_json::to_value(&r.result.output).expect("json");
    assert!(
        json["changes"][0].get("base_commit").is_none(),
        "omitted, not null (Appendix C)"
    );
}

// ===========================================================================
// base and target in a temporary repository (D35, 10.7)
// ===========================================================================

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

/// Run git in `dir` for a test's setup: the identity comes from the child's
/// environment, and no hook or signing configuration is consulted.
#[track_caller]
fn git(dir: &Path, args: &[&str]) -> String {
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

/// A repository holding the project: commits `first` and `second` on
/// `main`, the lightweight tag `v1` and the annotated tag `v2` (tag object
/// `annotated`) on `first`, the branch `feature/x` and the remote-tracking
/// ref `refs/remotes/origin/main` on `second`.
struct History {
    repo: GitRepo,
    first: String,
    second: String,
    annotated: String,
}

fn history(name: &str) -> History {
    let dir = temp_dir(name).join("repo");
    for (path, bytes) in &tree(&files()).files {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().expect("a parent")).expect("create a directory");
        std::fs::write(&file, bytes).expect("write a file");
    }
    git(&dir, &["init", "-q", "-b", "main"]);
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-q", "-m", "first"]);
    let first = git(&dir, &["rev-parse", "HEAD"]);
    std::fs::write(dir.join("notes.txt"), "second\n").expect("write a file");
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-q", "-m", "second"]);
    let second = git(&dir, &["rev-parse", "HEAD"]);
    git(&dir, &["tag", "v1", &first]);
    git(&dir, &["tag", "-a", "v2", "-m", "annotated", &first]);
    let annotated = git(&dir, &["rev-parse", "v2"]);
    git(&dir, &["branch", "feature/x"]);
    git(&dir, &["update-ref", "refs/remotes/origin/main", &second]);
    History {
        repo: GitRepo {
            toplevel: dir,
            project_prefix: String::new(),
        },
        first,
        second,
        annotated,
    }
}

/// D35: `base` may be any ref (branch, tag, or commit id) and is resolved to
/// a full commit id, with `git rev-parse --verify <rev>^{commit}` (10.7):
/// an annotated tag gives the commit it tags, not the tag object, and an
/// abbreviated id the full one.
#[test]
fn base_resolves_any_ref_to_the_full_id_of_its_commit() {
    if !git_available() {
        return;
    }
    let h = history("base");
    assert_ne!(h.annotated, h.first, "v2 is a tag object");
    let abbreviated = h.first[..12].to_string();
    let cases: Vec<(&str, &str)> = vec![
        ("main", &h.second),
        ("refs/heads/main", &h.second),
        ("HEAD", &h.second),
        ("feature/x", &h.second),
        ("v1", &h.first),
        ("v2", &h.first),
        ("refs/tags/v2", &h.first),
        (&abbreviated, &h.first),
        (&h.first, &h.first),
    ];
    for (base, commit) in cases {
        let r = run_with(
            &change_on("c", base, "main", &[writer()]),
            &WorkPolicySection::default(),
            Some(&h.repo),
        );
        r.valid();
        assert!(
            r.all("git_unavailable").is_empty(),
            "{base}: a repository is given"
        );
        assert_eq!(
            r.result.output.changes[0].base_commit.as_deref(),
            Some(commit),
            "base git(\"{base}\")"
        );
    }
}

/// D35: the integrator advances `target`, so it must name a branch
/// (`refs/heads/...`); a tag (lightweight or annotated), a commit id,
/// `HEAD`, and a remote-tracking ref all resolve but are no branch.
#[test]
fn target_must_resolve_to_a_branch() {
    if !git_available() {
        return;
    }
    let h = history("target");
    for target in [
        "main",
        "refs/heads/main",
        "feature/x",
        "refs/heads/feature/x",
    ] {
        let r = run_with(
            &change_on("c", "main", target, &[writer()]),
            &WorkPolicySection::default(),
            Some(&h.repo),
        );
        r.valid();
    }
    let abbreviated = h.second[..12].to_string();
    for target in [
        "v1",
        "v2",
        "refs/tags/v1",
        "HEAD",
        "origin/main",
        "refs/remotes/origin/main",
        h.first.as_str(),
        abbreviated.as_str(),
    ] {
        let r = run_with(
            &change_on("c", "main", target, &[writer()]),
            &WorkPolicySection::default(),
            Some(&h.repo),
        );
        r.refused(&["work_target_not_branch"]);
        let d = r.one("work_target_not_branch");
        assert_eq!(d.category, Category::Work);
        assert!(
            r.inside(d, &format!("target git(\"{target}\")")),
            "{target}: {d:#?}"
        );
    }
}

/// A ref that names no commit is `work_unknown_ref`, whether it names
/// nothing, is empty, or is spelled like an option (1.9: an empty or
/// option-like revision never reaches git), or names an object that is no
/// commit (D35: base resolves to a commit id).
#[test]
fn refs_that_name_no_commit_are_unknown() {
    if !git_available() {
        return;
    }
    let h = history("unknown");
    for (base, target, line) in [
        ("nope", "main", "base git(\"nope\")"),
        ("main", "nope", "target git(\"nope\")"),
        ("", "main", "base git(\"\")"),
        ("main", "", "target git(\"\")"),
        ("--all", "main", "base git(\"--all\")"),
        ("main", "-h", "target git(\"-h\")"),
        ("main^{tree}", "main", "base git(\"main^{tree}\")"),
        (
            "0000000000000000000000000000000000000000",
            "main",
            "base git(\"0000000000000000000000000000000000000000\")",
        ),
    ] {
        let r = run_with(
            &change_on("c", base, target, &[writer()]),
            &WorkPolicySection::default(),
            Some(&h.repo),
        );
        r.refused(&["work_unknown_ref"]);
        let d = r.one("work_unknown_ref");
        assert_eq!(d.category, Category::Work);
        assert!(r.inside(d, line), "{line}: {d:#?}");
        assert!(r.all("git_unavailable").is_empty(), "{line}");
    }
}

/// A repository without commits is still a repository (no
/// `git_unavailable`), and neither `main` resolves: two `work_unknown_ref`.
#[test]
fn a_repository_without_commits_resolves_neither_ref() {
    if !git_available() {
        return;
    }
    let dir = temp_dir("no-commits").join("repo");
    std::fs::create_dir_all(&dir).expect("create the directory");
    git(&dir, &["init", "-q", "-b", "main"]);
    let repo = GitRepo {
        toplevel: dir,
        project_prefix: String::new(),
    };
    let r = run_with(
        &work(&[], &[writer()]),
        &WorkPolicySection::default(),
        Some(&repo),
    );
    r.refused(&["work_unknown_ref", "work_unknown_ref"]);
    assert!(r.all("git_unavailable").is_empty());
    assert_eq!(r.result.output.changes[0].base_commit, None);
}

/// Every change resolves its own refs: an unknown `base` in the second of
/// two changes is that change's error, and only the first has a
/// `base_commit`.
#[test]
fn each_change_resolves_its_own_refs() {
    if !git_available() {
        return;
    }
    let h = history("changes");
    let one = change_on(
        "one",
        "main",
        "main",
        &[task("a", &["write { app.core.double.body }"])],
    );
    let two = change_on(
        "two",
        "nope",
        "feature/x",
        &[task("b", &["write { app.core.fetch.body }"])],
    );
    let r = run_with(
        &format!("{one}\n{two}"),
        &WorkPolicySection::default(),
        Some(&h.repo),
    );
    r.refused(&["work_unknown_ref"]);
    assert!(r.inside(r.one("work_unknown_ref"), &two));
    assert_eq!(
        r.result.output.changes[0].base_commit.as_deref(),
        Some(h.second.as_str())
    );
    assert_eq!(r.result.output.changes[1].base_commit, None);
}

/// A repository changes nothing but ref resolution: the same file gives the
/// same model and diagnostics with and without one, except for the
/// `git_unavailable` warning and `base_commit`.
#[test]
fn a_repository_changes_nothing_but_ref_resolution() {
    if !git_available() {
        return;
    }
    let h = history("versus");
    let text = work(
        &["use app.core.{double}"],
        &[
            task(
                "t",
                &[
                    "write { double.body }",
                    "depend { app.core.helper.contract }",
                ],
            ),
            integrate(&["typecheck", "test(tests.*)"]),
        ],
    );
    let without = run(&text);
    let with = run_with(&text, &WorkPolicySection::default(), Some(&h.repo));
    without.valid();
    with.valid();
    assert_eq!(without.result.model, with.result.model);
    let rest = |r: &Run| -> Vec<Diagnostic> {
        r.result
            .diagnostics
            .iter()
            .filter(|d| d.code != "git_unavailable")
            .cloned()
            .collect()
    };
    assert_eq!(rest(&without), rest(&with));
    assert_eq!(
        without.result.diagnostics.len(),
        with.result.diagnostics.len() + 1
    );
    let mut output = with.result.output.clone();
    assert_eq!(
        output.changes[0].base_commit.as_deref(),
        Some(h.second.as_str())
    );
    output.changes[0].base_commit = None;
    assert_eq!(output, without.result.output);
}

// ===========================================================================
// write, depend, and preserve selectors (7.2, grammar 4.4, T43)
// ===========================================================================

/// "every write/depend/preserve selector resolves to at least one
/// declaration": a namespace whose only module declares nothing selects
/// nothing. An `add` namespace need not hold a declaration, and a module
/// exists under this one, so it is not even new.
#[test]
fn a_namespace_whose_modules_declare_nothing_selects_nothing() {
    for set in ["write", "depend", "preserve"] {
        let line = format!("{set} {{ app.empty.* }}");
        let r = run(&work(&[], &[task("t", &[line.as_str()])]));
        r.refused(&["work_selector_empty"]);
        assert_eq!(
            r.spanned(r.one("work_selector_empty")),
            "app.empty.*",
            "{set}"
        );
    }
    let r = run(&work(&[], &[task("t", &["add { app.empty.* }"])]));
    r.valid();
    assert_eq!(r.warnings(), Vec::<&str>::new());
}

/// Grammar D38: `*` is never a facet wildcard (`paginate.*` is an error if
/// `paginate` is a declaration), whether the declaration is named fully
/// qualified or through an import.
#[test]
fn a_wildcard_after_a_declaration_is_no_facet_wildcard() {
    let cases: [(&[&str], &str); 3] = [
        (&[], "app.core.double.*"),
        (&["use app.core.{double}"], "double.*"),
        (&[], "app.core.Pair.*"),
    ];
    for (uses, selector) in cases {
        let line = format!("write {{ {selector} }}");
        let r = run(&work(uses, &[task("t", &[line.as_str()])]));
        r.refused(&["work_selector_empty"]);
        assert_eq!(r.spanned(r.one("work_selector_empty")), selector);
    }
}

/// T43: `m.*` denotes module `m` and every module whose path has `m` as a
/// proper prefix, segment by segment: `app.*` covers `app.core` and
/// `app.core.deep` but not `apps.misc`, and `ap.*` covers nothing.
#[test]
fn a_namespace_covers_whole_segments_and_every_module_below() {
    let r = run(&work(
        &[],
        &[task(
            "t",
            &["write { app.core.double.body }", "depend { app.* }"],
        )],
    ));
    r.valid();
    let depend = &r.model().changes[0].tasks[0].depend[0];
    let ids: BTreeSet<&str> = depend.targets.iter().map(|(id, _)| id.as_str()).collect();
    for expected in [
        "fn_double",
        "app.core.helper",
        "cap_store",
        "app.core.deep.inner",
    ] {
        assert!(ids.contains(expected), "{expected} in {ids:?}");
    }
    assert!(
        !ids.contains("apps.misc.other"),
        "`app.*` does not cover `apps.misc`"
    );
    assert_eq!(
        ids.len(),
        10,
        "app.core's nine declarations and inner: {ids:?}"
    );
    assert!(depend
        .targets
        .iter()
        .all(|(_, f)| *f == FacetSelector::Definition));

    let r = run(&work(
        &[],
        &[task(
            "t",
            &["write { app.core.double.body }", "depend { ap.* }"],
        )],
    ));
    r.refused(&["work_selector_empty"]);
}

/// Members, modules, and tests are not declarations a selector can name:
/// an enum variant is a member of its type (semantics 1.4), a module path
/// is a namespace (semantics 1.2), and a test is reached only through its
/// namespace's wildcard (D69).
#[test]
fn members_modules_and_tests_are_not_named_by_declaration_selectors() {
    for selector in [
        "app.core.Choice.Yes",
        "app.core",
        "app.core.deep",
        "tests.app",
        "tests",
    ] {
        let line = format!("depend {{ {selector} }}");
        let r = run(&work(
            &[],
            &[task(
                "t",
                &["write { app.core.double.body }", line.as_str()],
            )],
        ));
        r.refused(&["work_selector_empty"]);
        assert_eq!(r.spanned(r.one("work_selector_empty")), selector);
    }
}

/// Section 5.1: every facet of every declaration kind, as facet selectors
/// in one `depend` set. Exactly the inapplicable ones are
/// `work_facet_not_applicable`, each spanning its selector and blaming its
/// declaration and facet (S49: `symbol`, `qualified_name`, `facet`).
#[test]
fn facet_selectors_follow_the_coverage_table_of_section_5_1() {
    use Facet::*;
    let kinds: [(&str, &str, &[Facet]); 6] = [
        ("app.core.double", "fn_double", &Facet::ALL),
        (
            "app.core.Name",
            "ty_name",
            &[Identity, Name, Signature, Docs],
        ),
        (
            "app.core.Small",
            "ty_small",
            &[Identity, Name, Signature, Contract, Docs],
        ),
        (
            "app.core.Pair",
            "ty_pair",
            &[Identity, Name, Signature, Docs],
        ),
        (
            "app.core.Choice",
            "ty_choice",
            &[Identity, Name, Signature, Docs],
        ),
        (
            "app.core.Store",
            "cap_store",
            &[Identity, Name, Signature, Contract, Docs],
        ),
    ];
    let mut selectors = Vec::new();
    let mut expected = BTreeSet::new();
    for (decl, symbol, applicable) in kinds {
        for f in Facet::ALL {
            let selector = format!("{decl}.{}", f.as_str());
            if !applicable.contains(&f) {
                expected.insert((selector.clone(), symbol.to_string(), decl.to_string(), f));
            }
            selectors.push(selector);
        }
    }
    let depend = format!("depend {{ {} }}", selectors.join(", "));
    let r = run(&work(
        &[],
        &[task(
            "t",
            &["write { app.core.double.body }", depend.as_str()],
        )],
    ));
    assert!(
        r.errors().iter().all(|c| *c == "work_facet_not_applicable"),
        "{:#?}",
        r.result.diagnostics
    );
    let found: BTreeSet<(String, String, String, Facet)> = r
        .all("work_facet_not_applicable")
        .iter()
        .map(|d| {
            (
                r.spanned(d),
                d.symbol
                    .as_ref()
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_default(),
                d.qualified_name.clone().unwrap_or_default(),
                d.facet.expect("the blamed facet"),
            )
        })
        .collect();
    assert_eq!(r.all("work_facet_not_applicable").len(), expected.len());
    assert_eq!(found, expected);
    assert!(r.result.model.is_none());
}

/// The facet rule holds in every set, not only `depend`.
#[test]
fn facet_applicability_is_checked_in_write_and_preserve_sets() {
    for (line, selector) in [
        ("write { app.core.Pair.body }", "app.core.Pair.body"),
        (
            "preserve { app.core.Name.contract }",
            "app.core.Name.contract",
        ),
    ] {
        let r = run(&work(&[], &[task("t", &[line])]));
        r.refused(&["work_facet_not_applicable"]);
        assert_eq!(r.spanned(r.one("work_facet_not_applicable")), selector);
    }
}

/// "`identity` is never in a write ... set" (7.2, 10.3): writing it is
/// `work_identity_not_writable` blaming the identity facet (it applies to
/// every kind, so it is not also `work_facet_not_applicable`); a whole
/// declaration, `.definition`, depends on it, and preserving it are fine.
#[test]
fn identity_is_never_writable_but_a_whole_declaration_is() {
    let r = run(&work(
        &[],
        &[task("t", &["write { app.core.Small.identity }"])],
    ));
    r.refused(&["work_identity_not_writable"]);
    let d = r.one("work_identity_not_writable");
    assert_eq!(d.category, Category::Work);
    assert_eq!(d.symbol, Some(id("ty_small")));
    assert_eq!(d.qualified_name.as_deref(), Some("app.core.Small"));
    assert_eq!(d.facet, Some(Facet::Identity));
    assert_eq!(r.spanned(d), "app.core.Small.identity");
    for selector in [
        "app.core.Small",
        "app.core.Small.definition",
        "app.core.Store",
    ] {
        let line = format!("write {{ {selector} }}");
        run(&work(&[], &[task("t", &[line.as_str()])])).valid();
    }
    run(&work(
        &[],
        &[task(
            "t",
            &[
                "write { app.core.double.body }",
                "depend { app.core.Small.identity }",
                "preserve { app.core.double.identity }",
            ],
        )],
    ))
    .valid();
}

/// `.definition` is the whole declaration (D38) and, in a write set, every
/// applicable facet but `identity` (7.2, 10.3).
#[test]
fn definition_writes_every_applicable_facet_but_identity() {
    let r = run(&work(
        &[],
        &[task(
            "t",
            &[
                "write { app.core.double.definition }",
                "preserve { app.core.double.docs }",
            ],
        )],
    ));
    r.refused(&["work_preserve_written"]);
    let d = r.one("work_preserve_written");
    assert_eq!(d.symbol, Some(id("fn_double")));
    assert_eq!(d.facet, Some(Facet::Docs));
    assert_envelope(d);

    let r = run(&work(
        &[],
        &[task(
            "t",
            &[
                "write { app.core.double.definition }",
                "preserve { app.core.double.identity }",
            ],
        )],
    ));
    r.valid();
    assert_eq!(
        targets(&r.model().changes[0].tasks[0].write),
        [vec![(id("fn_double"), FacetSelector::Definition)]]
    );
}

// ===========================================================================
// File selectors (T42)
// ===========================================================================

/// T42: file selectors "are written in a write set like any other selector
/// (grammar 4.4 parses them as declaration selectors)". A facet selector
/// (`work.body`: D37 makes a final facet spelling the facet) or a namespace
/// is never one and names nothing; the three spellings are file selectors
/// that the default policy, which lists no file, does not grant.
#[test]
fn only_a_declaration_selector_can_spell_a_file() {
    for (selector, code) in [
        ("work.body", "work_selector_empty"),
        ("work.definition", "work_selector_empty"),
        ("project.manifest.docs", "work_selector_empty"),
        ("project.*", "work_selector_empty"),
        ("project.manifest", "work_file_not_grantable"),
        ("project.lock", "work_file_not_grantable"),
        ("work.other", "work_file_not_grantable"),
    ] {
        let line = format!("write {{ {selector} }}");
        let r = run(&work(&[], &[task("t", &[line.as_str()])]));
        r.refused(&[code]);
        assert_eq!(r.spanned(r.one(code)), selector, "{selector}");
    }
}

/// Each file selector needs its own entry in `[work.policy] files`; a
/// granted one is in `write_files`, not in `write`, and is output as `{
/// "selector", "file" }` (7.2).
#[test]
fn a_file_selector_is_granted_by_its_own_policy_entry() {
    let lock_only = policy(&["project.lock"]);
    let r = run_with(
        &work(&[], &[task("t", &["write { project.lock }"])]),
        &lock_only,
        None,
    );
    r.valid();
    let t = &r.model().changes[0].tasks[0];
    assert_eq!(t.write_files, [FileSelector::Lock]);
    assert!(t.write.is_empty());
    assert_eq!(
        serde_json::to_value(&r.result.output.changes[0].tasks[0].write).expect("json"),
        json!([{ "selector": "project.lock", "file": "cpl.lock" }])
    );
    for selector in ["project.manifest", "work.other"] {
        let line = format!("write {{ {selector} }}");
        run_with(&work(&[], &[task("t", &[line.as_str()])]), &lock_only, None)
            .refused(&["work_file_not_grantable"]);
    }
    let r = run_with(
        &work(&[], &[task("t", &["write { work.other }"])]),
        &policy(&["work.other"]),
        None,
    );
    r.valid();
    assert_eq!(
        r.model().changes[0].tasks[0].write_files,
        [FileSelector::Work("other".to_string())]
    );
    assert_eq!(
        serde_json::to_value(&r.result.output.changes[0].tasks[0].write).expect("json"),
        json!([{ "selector": "work.other", "file": "work/other.work" }])
    );
}

/// "file selectors are not compared" for overlap (7.2), and a granted file
/// is write authority (no `work_task_without_authority`).
#[test]
fn file_selectors_are_never_compared_for_overlap() {
    let r = run_with(
        &work(
            &[],
            &[
                task("a", &["write { project.lock }"]),
                task("b", &["write { project.lock }"]),
            ],
        ),
        &policy(&["project.lock"]),
        None,
    );
    r.valid();
    assert_eq!(r.warnings(), Vec::<&str>::new());
}

/// T42: a file selector is resolved only after declaration resolution has
/// failed, and a declaration of that spelling wins with
/// `work_file_selector_shadowed`, also when the declaration is reached
/// through the work file's own imports (`use cfg.{project}`), in every set.
#[test]
fn a_declaration_reached_through_an_import_shadows_a_file_selector() {
    let r = run(&work(
        &["use cfg.{project}"],
        &[task("t", &["write { project.lock }"])],
    ));
    r.valid();
    let d = r.one("work_file_selector_shadowed");
    assert_eq!(
        (d.category, d.severity),
        (Category::Work, Severity::Warning)
    );
    assert_eq!(d.symbol, Some(id("cfg.project.lock")));
    assert_eq!(d.qualified_name.as_deref(), Some("cfg.project.lock"));
    assert_eq!(r.spanned(d), "project.lock");
    let t = &r.model().changes[0].tasks[0];
    assert!(t.write_files.is_empty());
    assert_eq!(
        targets(&t.write),
        [vec![(id("cfg.project.lock"), FacetSelector::Definition)]]
    );
    // `lock` has no `@id` and is named individually (T47).
    assert_eq!(
        r.one("work_provisional_identity").symbol,
        Some(id("cfg.project.lock"))
    );
    assert_eq!(
        serde_json::to_value(&r.result.output.changes[0].tasks[0].write).expect("json"),
        json!([{ "selector": "project.lock", "targets": [{ "id": "cfg.project.lock", "facet": "definition" }] }])
    );
    for line in ["depend { project.lock }", "preserve { project.lock }"] {
        let r = run(&work(
            &["use cfg.{project}"],
            &[task("t", &["write { app.core.double.body }", line])],
        ));
        r.valid();
        assert_eq!(
            r.one("work_file_selector_shadowed").symbol,
            Some(id("cfg.project.lock")),
            "{line}"
        );
    }
}

// ===========================================================================
// add namespaces (7.2, T43, design line 353)
// ===========================================================================

/// An `add` namespace is "a well-formed module path not under `std` or
/// `gen`" (7.2; semantics 1.1 reserves both): under is segment-wise (T43),
/// so `stdlib` and `generators` are ordinary new namespaces, and an alias
/// into the standard library is still the standard library.
#[test]
fn add_refuses_std_and_gen_segment_by_segment() {
    let refused: [(&[&str], &str); 7] = [
        (&[], "std.*"),
        (&[], "std.text.*"),
        (&[], "std.extra.*"),
        (&[], "gen.*"),
        (&[], "gen.extra.*"),
        (&["use std.{text}"], "text.*"),
        (&["use std.{text}"], "text.more.*"),
    ];
    for (uses, namespace) in refused {
        let line = format!("add {{ {namespace} }}");
        let r = run(&work(uses, &[task("t", &[line.as_str()])]));
        r.refused(&["work_selector_empty"]);
        assert_eq!(
            r.spanned(r.one("work_selector_empty")),
            namespace,
            "{uses:?}"
        );
    }
    for namespace in ["stdlib.*", "generators.*", "app.std.*", "app.gen.*"] {
        let line = format!("add {{ {namespace} }}");
        let r = run(&work(&[], &[task("t", &[line.as_str()])]));
        r.valid();
        assert_eq!(r.warnings(), ["work_add_namespace_new"], "{namespace}");
        assert_eq!(
            r.model().changes[0].tasks[0].add[0].module_prefix,
            path(namespace.trim_end_matches(".*"))
        );
    }
}

/// `work_add_namespace_new` warns exactly when no module exists under the
/// namespace, segment-wise (T43).
#[test]
fn an_add_namespace_is_new_only_when_no_module_lies_under_it() {
    for (namespace, new) in [
        ("app.*", false),
        ("app.core.*", false),
        ("app.core.deep.*", false),
        ("app.empty.*", false),
        ("tests.*", false),
        ("app.core.deeper.*", true),
        ("app.core.deep.er.*", true),
        ("ap.*", true),
        ("appl.*", true),
        ("tests.app.unit.*", true),
    ] {
        let line = format!("add {{ {namespace} }}");
        let r = run(&work(&[], &[task("t", &[line.as_str()])]));
        r.valid();
        if new {
            let d = r.one("work_add_namespace_new");
            assert_eq!(
                (d.category, d.severity),
                (Category::Work, Severity::Warning)
            );
            assert_eq!(r.spanned(d), namespace);
        } else {
            assert!(r.all("work_add_namespace_new").is_empty(), "{namespace}");
        }
    }
}

/// An `add` namespace through a submodule alias of the work file is the
/// module path the alias continues (grammar 4.1, 1.6 `resolve_selector`).
#[test]
fn an_add_namespace_expands_a_submodule_alias() {
    let r = run(&work(
        &["use app.{core}"],
        &[
            task("a", &["add { core.deep.* }"]),
            task("b", &["add { core.fresh.* }"]),
        ],
    ));
    r.valid();
    let tasks = &r.model().changes[0].tasks;
    assert_eq!(tasks[0].add[0].module_prefix, path("app.core.deep"));
    assert_eq!(tasks[1].add[0].module_prefix, path("app.core.fresh"));
    assert_eq!(r.warnings(), ["work_add_namespace_new"]);
    assert_eq!(r.spanned(r.one("work_add_namespace_new")), "core.fresh.*");
    assert_eq!(
        serde_json::to_value(&r.result.output.changes[0].tasks[0].add).expect("json"),
        json!([{ "selector": "core.deep.*", "namespace": "app.core.deep" }])
    );
}

/// Grammar D38: "`paginate.*` is an error if `paginate` is a declaration".
/// With `use app.core.{double}`, the file's scope binds `double` to the
/// function (the import hides any root module of that name, semantics
/// 2.1), so `add { double.* }` names no namespace a task may populate; the
/// one error code of the `add` row of 7.2 is `work_selector_empty`.
#[test]
fn an_add_namespace_through_an_imported_declaration_is_an_error() {
    let r = run(&work(
        &["use app.core.{double}"],
        &[task("t", &["add { double.* }"])],
    ));
    r.refused(&["work_selector_empty"]);
    assert_eq!(r.spanned(r.one("work_selector_empty")), "double.*");
}

/// The same rule (D38) for the declaration's fully qualified path.
#[test]
fn an_add_namespace_spelled_as_a_declaration_path_is_an_error() {
    let r = run(&work(&[], &[task("t", &["add { app.core.double.* }"])]));
    r.refused(&["work_selector_empty"]);
    assert_eq!(r.spanned(r.one("work_selector_empty")), "app.core.double.*");
}

/// T43: the `add` namespaces of unordered tasks must not overlap, one
/// covering the other segment by segment; ordered by `after`, they may.
#[test]
fn add_namespaces_of_unordered_tasks_overlap_segment_wise() {
    for (a, b, overlap) in [
        ("tests.*", "tests.app.*", true),
        ("tests.app.*", "tests.app.*", true),
        ("app.core.deep.*", "app.*", true),
        ("tests.app.*", "tests.other.*", false),
        ("app.*", "apps.*", false),
        ("app.core.*", "app.cor.*", false),
    ] {
        let la = format!("add {{ {a} }}");
        let lb = format!("add {{ {b} }}");
        let r = run(&work(
            &[],
            &[task("a", &[la.as_str()]), task("b", &[lb.as_str()])],
        ));
        if overlap {
            r.refused(&["work_overlapping_writes"]);
            assert_eq!(r.one("work_overlapping_writes").category, Category::Work);
        } else {
            r.valid();
        }
        let r = run(&work(
            &[],
            &[task("a", &[la.as_str()]), task("b after a", &[lb.as_str()])],
        ));
        r.valid();
    }
}

/// 7.2 compares write sets with write sets and `add` namespaces with `add`
/// namespaces: a write over a namespace and an `add` to it are not an
/// overlap (writing existing declarations and creating new ones).
#[test]
fn a_write_set_and_an_add_namespace_are_never_compared() {
    let r = run(&work(
        &[],
        &[
            task("a", &["write { tests.app.* }"]),
            task("b", &["add { tests.app.* }"]),
        ],
    ));
    r.valid();
}

// ===========================================================================
// Provisional identities (7.2, 4.3, T47)
// ===========================================================================

/// Requirements name declarations individually too: `prove(helper)` and
/// `test(helper)` each warn for `helper`, which has no `@id`, at the
/// selector, with `symbol` and `qualified_name` its provisional identity.
#[test]
fn integrate_requirements_name_declarations_individually() {
    for requirement in ["prove(app.core.helper)", "test(app.core.helper)"] {
        let r = run(&work(&[], &[writer(), integrate(&[requirement])]));
        r.valid();
        let d = r.one("work_provisional_identity");
        assert_eq!(
            (d.category, d.severity),
            (Category::Work, Severity::Warning)
        );
        assert_eq!(d.symbol, Some(id("app.core.helper")), "{requirement}");
        assert_eq!(d.qualified_name.as_deref(), Some("app.core.helper"));
        assert_eq!(r.spanned(d), "app.core.helper");
    }
}

/// 7.2: "every declaration named individually (by a declaration *or facet*
/// selector ...) ... in an integrate prove/test requirement carries an
/// `@id`". A facet selector in `test(...)` is `work_facet_not_applicable`,
/// and it still names `helper`, which has no `@id`.
#[test]
fn a_facet_selector_in_a_test_requirement_still_names_its_declaration() {
    let r = run(&work(
        &[],
        &[writer(), integrate(&["test(app.core.helper.body)"])],
    ));
    r.refused(&["work_facet_not_applicable"]);
    let d = r.one("work_facet_not_applicable");
    assert_eq!(d.symbol, Some(id("app.core.helper")));
    assert_eq!(d.facet, Some(Facet::Body));
    let warned: Vec<Option<StableId>> = r
        .all("work_provisional_identity")
        .iter()
        .map(|d| d.symbol.clone())
        .collect();
    assert_eq!(
        warned,
        [Some(id("app.core.helper"))],
        "{:#?}",
        r.result.diagnostics
    );
}

/// "a namespace wildcard names a namespace, not its declarations": no
/// provisional warning for the tests and the `inner` below namespaces, nor
/// for declarations with an `@id`.
#[test]
fn namespaces_and_annotated_declarations_raise_no_provisional_warning() {
    let r = run(&work(
        &[],
        &[
            task(
                "t",
                &[
                    "write { app.core.double.body, app.core.deep.* }",
                    "depend { tests.*, app.core.Pair }",
                    "preserve { app.core.fetch.signature }",
                ],
            ),
            integrate(&[
                "prove(app.core.double)",
                "test(tests.app.*)",
                "test(app.core.double)",
            ]),
        ],
    ));
    r.valid();
    assert!(
        r.all("work_provisional_identity").is_empty(),
        "{:#?}",
        r.result.diagnostics
    );
}

// ===========================================================================
// `after` and the task graph (7.2, grammar 4.1)
// ===========================================================================

/// "`after` names tasks of the same change": a task of another change in
/// the same file is unknown, for `after` and for `review` alike.
#[test]
fn after_and_review_name_tasks_of_their_own_change() {
    let one = change("one", &[task("a", &["write { app.core.double.body }"])]);
    let two = change(
        "two",
        &[
            task("b after a", &["write { app.core.double.contract }"]),
            integrate(&["review(a)"]),
        ],
    );
    let r = run(&format!("{one}\n{two}"));
    r.refused(&["work_unknown_task", "work_unknown_task"]);
    for d in r.all("work_unknown_task") {
        assert_eq!(d.category, Category::Work);
        assert!(r.inside(d, &two), "{d:#?}");
    }
}

/// Items may appear in any order (grammar 4.1): `after` may name a task
/// declared later, which orders the two.
#[test]
fn after_may_name_a_task_declared_later() {
    let r = run(&work(
        &[],
        &[
            task("first after second", &["write { app.core.double.body }"]),
            task("second", &["write { app.core.double.body }"]),
        ],
    ));
    r.valid();
    assert_eq!(r.model().changes[0].tasks[0].after, ["second"]);
}

/// A cycle through three tasks is a `work_cycle` (and no task of it is
/// unknown).
#[test]
fn a_cycle_through_three_tasks_is_a_work_cycle() {
    let r = run(&work(
        &[],
        &[
            task("a after c", &["write { app.core.double.body }"]),
            task("b after a", &["write { app.core.helper.body }"]),
            task("c after b", &["write { app.core.fetch.body }"]),
        ],
    ));
    r.refused(&["work_cycle"]);
    assert_eq!(r.one("work_cycle").category, Category::Work);
}

/// Two independent cycles are two mistakes, so two diagnostics (7.2 is
/// silent on the count; semantics S63 keeps "counts equal to mistakes").
#[test]
fn independent_cycles_are_reported_separately() {
    let r = run(&work(
        &[],
        &[
            task("a after b", &["write { app.core.double.body }"]),
            task("b after a", &["write { app.core.helper.body }"]),
            task("c after d", &["write { app.core.fetch.body }"]),
            task("d after c", &["write { app.core.hidden.body }"]),
        ],
    ));
    r.refused(&["work_cycle", "work_cycle"]);
}

/// An unknown predecessor and a cycle in one change are both reported.
#[test]
fn an_unknown_predecessor_and_a_cycle_are_both_reported() {
    let r = run(&work(
        &[],
        &[
            task("a after a", &["write { app.core.double.body }"]),
            task("b after phantom", &["write { app.core.fetch.body }"]),
        ],
    ));
    let mut errors = r.errors();
    errors.sort_unstable();
    assert_eq!(errors, ["work_cycle", "work_unknown_task"]);
    assert!(r.result.model.is_none());
    assert!(r.inside(r.one("work_unknown_task"), "after phantom"));
}

// ===========================================================================
// Overlapping writes at facet level (7.2, design "Changes and agent tasks")
// ===========================================================================

/// The overlap rule is at facet level: unordered tasks may write different
/// facets of one declaration.
#[test]
fn unordered_tasks_may_write_different_facets_of_one_declaration() {
    let r = run(&work(
        &[],
        &[
            task("a", &["write { app.core.double.body }"]),
            task("b", &["write { app.core.double.contract }"]),
            task(
                "c",
                &["write { app.core.double.docs, app.core.double.name }"],
            ),
        ],
    ));
    r.valid();
}

/// A namespace write writes every applicable facet of every declaration of
/// the modules under it (T43), so it overlaps a facet another unordered
/// task writes, in a nested module too; the diagnostic blames the facet.
#[test]
fn a_namespace_write_overlaps_the_facets_of_its_declarations() {
    for (other, symbol, qualified, f) in [
        (
            "app.core.double.body",
            "fn_double",
            "app.core.double",
            Facet::Body,
        ),
        (
            "app.core.deep.inner.name",
            "app.core.deep.inner",
            "app.core.deep.inner",
            Facet::Name,
        ),
    ] {
        let line = format!("write {{ {other} }}");
        let r = run(&work(
            &[],
            &[
                task("a", &["write { app.core.* }"]),
                task("b", &[line.as_str()]),
            ],
        ));
        r.refused(&["work_overlapping_writes"]);
        let d = r.one("work_overlapping_writes");
        assert_eq!(d.category, Category::Work);
        assert_eq!(d.symbol, Some(id(symbol)), "{other}");
        assert_eq!(d.qualified_name.as_deref(), Some(qualified));
        assert_eq!(d.facet, Some(f));
    }
}

/// "neither reaches the other through `after`": order is transitive.
#[test]
fn order_through_after_is_transitive() {
    let r = run(&work(
        &[],
        &[
            task("a", &["write { app.core.double.body }"]),
            task("b after a", &["write { app.core.helper.body }"]),
            task("c after b", &["write { app.core.double.body }"]),
        ],
    ));
    r.valid();
}

/// Two tasks after one common predecessor are unordered between them: one
/// overlap, between the siblings only.
#[test]
fn siblings_after_one_task_are_unordered() {
    let b = task("b after a", &["write { app.core.double.body }"]);
    let c = task("c after a", &["write { app.core.double.body }"]);
    let r = run(&work(
        &[],
        &[
            task("a", &["write { app.core.double.body }"]),
            b.clone(),
            c.clone(),
        ],
    ));
    r.refused(&["work_overlapping_writes"]);
    let d = r.one("work_overlapping_writes");
    assert!(r.inside(d, &b) || r.inside(d, &c), "{d:#?}");
}

/// Only write sets are compared: unordered tasks may depend on and
/// preserve the facets another writes.
#[test]
fn depend_and_preserve_sets_are_never_compared_for_overlap() {
    let r = run(&work(
        &[],
        &[
            task(
                "a",
                &[
                    "write { app.core.helper.body }",
                    "depend { app.core.double.body }",
                    "preserve { app.core.double.contract }",
                ],
            ),
            task(
                "b",
                &[
                    "write { app.core.fetch.body }",
                    "depend { app.core.double.body }",
                    "preserve { app.core.double.contract }",
                ],
            ),
            task(
                "c",
                &["write { app.core.double.body, app.core.double.contract }"],
            ),
        ],
    ));
    r.valid();
}

/// "`.definition` counts as every applicable facet but `identity`": a
/// refinement has no `body`, so `Small` and `Small.body` share no facet
/// (the latter is only `work_facet_not_applicable`), and `Pair.identity`
/// is only `work_identity_not_writable`.
#[test]
fn a_whole_declaration_overlaps_only_on_its_applicable_writable_facets() {
    let r = run(&work(
        &[],
        &[
            task("a", &["write { app.core.Small }"]),
            task("b", &["write { app.core.Small.body }"]),
        ],
    ));
    r.refused(&["work_facet_not_applicable"]);
    let r = run(&work(
        &[],
        &[
            task("a", &["write { app.core.Pair }"]),
            task("b", &["write { app.core.Pair.identity }"]),
        ],
    ));
    r.refused(&["work_identity_not_writable"]);
}

/// Two whole-declaration writes overlap, and the diagnostic blames a facet
/// both write: one of the struct's writable facets, never `identity`.
#[test]
fn two_whole_declaration_writes_overlap_on_a_writable_facet() {
    let r = run(&work(
        &[],
        &[
            task("a", &["write { app.core.Pair }"]),
            task("b", &["write { app.core.Pair.definition }"]),
        ],
    ));
    r.refused(&["work_overlapping_writes"]);
    let d = r.one("work_overlapping_writes");
    assert_eq!(d.symbol, Some(id("ty_pair")));
    let blamed = d.facet.expect("the overlapping facet");
    assert!(
        [Facet::Name, Facet::Signature, Facet::Docs].contains(&blamed),
        "{blamed:?}"
    );
}

/// The overlap rule compares the tasks of one change: "Competing
/// implementations use separate candidate changes rather than silently
/// merging two bodies" (design, "Changes and agent tasks"), so two changes
/// may write the same body.
#[test]
fn competing_changes_never_overlap() {
    let one = change("one", &[task("a", &["write { app.core.double.body }"])]);
    let two = change("two", &[task("a", &["write { app.core.double.body }"])]);
    let r = run(&format!("{one}\n{two}"));
    r.valid();
    assert_eq!(r.model().changes.len(), 2);
}

// ===========================================================================
// preserve (7.2)
// ===========================================================================

/// "no facet is in both the write set and the preserve set of one task",
/// compared as resolved facets: a namespace on either side, two spellings
/// of one facet, and `.definition` against one facet.
#[test]
fn a_task_never_preserves_what_it_writes_whatever_the_selectors() {
    for (write, preserve, f) in [
        ("app.core.double.body", "app.core.*", Facet::Body),
        ("app.core.*", "app.core.double.contract", Facet::Contract),
        ("double.body", "app.core.double.body", Facet::Body),
        (
            "app.core.double",
            "double.verification_policy",
            Facet::VerificationPolicy,
        ),
    ] {
        let lw = format!("write {{ {write} }}");
        let lp = format!("preserve {{ {preserve} }}");
        let r = run(&work(
            &["use app.core.{double}"],
            &[task("t", &[lw.as_str(), lp.as_str()])],
        ));
        r.refused(&["work_preserve_written"]);
        let d = r.one("work_preserve_written");
        assert_eq!(d.category, Category::Work);
        assert_eq!(d.symbol, Some(id("fn_double")), "{write} / {preserve}");
        assert_eq!(d.qualified_name.as_deref(), Some("app.core.double"));
        assert_eq!(d.facet, Some(f), "{write} / {preserve}");
    }
}

/// The design's implementation task: writing the body while preserving the
/// rest, identity included, is valid.
#[test]
fn writing_the_body_while_preserving_the_other_facets_is_valid() {
    let r = run(&work(
        &[],
        &[task(
            "t",
            &[
                "write { app.core.double.body }",
                "preserve { app.core.double.signature, app.core.double.contract, app.core.double.effects, app.core.double.verification_policy, app.core.double.identity }",
            ],
        )],
    ));
    r.valid();
}

// ===========================================================================
// Authority (7.2)
// ===========================================================================

/// "a task has neither write nor add and is not named by a review
/// requirement": empty sets are no authority, an `add` is, and a review
/// excuses a task without authority.
#[test]
fn only_tasks_with_neither_write_nor_add_need_a_review() {
    let a = task(
        "a",
        &[
            "depend { app.core.double.contract }",
            "goal \"Look around.\"",
        ],
    );
    let b = task("b", &["write {}", "add {}"]);
    let r = run(&work(
        &[],
        &[
            a.clone(),
            b.clone(),
            task("c", &["add { tests.fresh.* }"]),
            task("d", &["write { app.core.double.body }"]),
            task("e", &["write {}"]),
            integrate(&["review(e)"]),
        ],
    ));
    r.valid();
    let warned = r.all("work_task_without_authority");
    assert_eq!(warned.len(), 2, "{:#?}", r.result.diagnostics);
    assert_eq!(warned[0].severity, Severity::Warning);
    assert!(r.inside(warned[0], &a), "{:#?}", warned[0]);
    assert!(r.inside(warned[1], &b), "{:#?}", warned[1]);
}

/// A `review` in another change names no task of this one: it is unknown
/// there and excuses nothing here.
#[test]
fn a_review_in_another_change_reviews_nothing_here() {
    let one = change("one", &[task("audit", &["write {}"])]);
    let two = change("two", &[writer(), integrate(&["review(audit)"])]);
    let r = run(&format!("{one}\n{two}"));
    r.refused(&["work_unknown_task"]);
    assert!(r.inside(r.one("work_unknown_task"), &two));
    assert!(r.inside(r.one("work_task_without_authority"), &one));
}

// ===========================================================================
// integrate (7.2, grammar 4.3)
// ===========================================================================

/// `no_unresolved_assumptions` is a requirement of its own (D36), in the
/// model and as `{ "require": "no_unresolved_assumptions" }` (7.2).
#[test]
fn no_unresolved_assumptions_is_a_requirement_of_its_own() {
    let r = run(&work(
        &[],
        &[
            writer(),
            integrate(&["no_unresolved_assumptions", "typecheck", "no_holes"]),
        ],
    ));
    r.valid();
    assert_eq!(
        r.model().changes[0].integrate,
        [
            RequirementSpec::NoUnresolvedAssumptions,
            RequirementSpec::Typecheck,
            RequirementSpec::NoHoles
        ]
    );
    assert_eq!(
        serde_json::to_value(&r.result.output.changes[0].integrate).expect("json"),
        json!([{ "require": "no_unresolved_assumptions" }, { "require": "typecheck" }, { "require": "no_holes" }])
    );
}

/// "a namespace selects the tests in it (T43)": tests and properties of the
/// module and of every module below it; a namespace without one is
/// `work_test_selector_empty` ("an empty selection is a configuration
/// failure", design "Integration semantics").
#[test]
fn a_test_namespace_selects_every_test_and_property_below_it() {
    for (namespace, module, matched) in [
        ("tests.*", "tests", 4),
        ("tests.other.*", "tests.other", 2),
        ("tests.app.*", "tests.app", 2),
    ] {
        let line = format!("test({namespace})");
        let r = run(&work(&[], &[writer(), integrate(&[line.as_str()])]));
        r.valid();
        assert_eq!(
            serde_json::to_value(&r.result.output.changes[0].integrate[0]).expect("json"),
            json!({ "require": "test", "namespace": module, "matched": matched })
        );
        match &r.model().changes[0].integrate[0] {
            RequirementSpec::Test(ResolvedNamespaceOrSelector::Namespace(ns)) => {
                assert_eq!(ns.module_prefix, path(module));
                assert_eq!(ns.text, namespace);
            }
            other => panic!("test({namespace}): {other:?}"),
        }
    }
    for namespace in ["app.*", "app.core.*", "apps.*", "nowhere.*"] {
        let line = format!("test({namespace})");
        let r = run(&work(&[], &[writer(), integrate(&[line.as_str()])]));
        r.refused(&["work_test_selector_empty"]);
        let d = r.one("work_test_selector_empty");
        assert_eq!(d.category, Category::Work);
        assert_eq!(r.spanned(d), namespace);
        assert_eq!(r.result.output.changes[0].integrate[0].matched, Some(0));
    }
}

/// A test namespace through a submodule alias is the aliased module path:
/// `use tests.{app}` binds `app`, so `test(app.*)` is `tests.app`, not the
/// project's `app` namespace.
#[test]
fn a_test_namespace_through_an_alias_is_the_aliased_module() {
    let r = run(&work(
        &["use tests.{app}"],
        &[writer(), integrate(&["test(app.*)"])],
    ));
    r.valid();
    assert_eq!(
        serde_json::to_value(&r.result.output.changes[0].integrate[0]).expect("json"),
        json!({ "require": "test", "namespace": "tests.app", "matched": 2 })
    );
}

/// "a declaration selector selects `Index::tests_for` of that declaration"
/// (6.5: every test whose transitive closure of `Call` and `CapabilityOp`
/// edges reaches it): `double` is called by one test directly and by
/// another through `helper`; a function no test calls, and a type, which
/// no call reaches, match none.
#[test]
fn a_declaration_test_selector_selects_the_tests_that_call_it() {
    for (selector, symbol, matched) in [
        ("app.core.double", "fn_double", 2),
        ("app.core.helper", "app.core.helper", 1),
    ] {
        let line = format!("test({selector})");
        let r = run(&work(&[], &[writer(), integrate(&[line.as_str()])]));
        r.valid();
        let output = &r.result.output.changes[0].integrate[0];
        assert_eq!(output.matched, Some(matched), "{selector}");
        assert_eq!(
            serde_json::to_value(&output.targets).expect("json"),
            json!([{ "id": symbol, "facet": "definition" }])
        );
    }
    for (selector, symbol) in [
        ("app.core.deep.inner", "app.core.deep.inner"),
        ("app.core.hidden", "app.core.hidden"),
        ("app.core.Small", "ty_small"),
        ("app.core.Pair", "ty_pair"),
    ] {
        let line = format!("test({selector})");
        let r = run(&work(&[], &[writer(), integrate(&[line.as_str()])]));
        r.refused(&["work_test_selector_empty"]);
        let d = r.one("work_test_selector_empty");
        assert_eq!(d.symbol, Some(id(symbol)), "{selector}");
        assert_eq!(d.qualified_name.as_deref(), Some(selector));
        assert_eq!(r.spanned(d), selector);
    }
}

/// "`require prove(sel)` names functions": a struct, an enum, a wrapper, a
/// refinement, and a capability are `work_prove_not_function` blaming the
/// declaration; every function is provable, an effectful one and a
/// private one too.
#[test]
fn prove_names_functions() {
    for (selector, symbol) in [
        ("app.core.Pair", "ty_pair"),
        ("app.core.Choice", "ty_choice"),
        ("app.core.Name", "ty_name"),
        ("app.core.Small", "ty_small"),
        ("app.core.Store", "cap_store"),
    ] {
        let line = format!("prove({selector})");
        let r = run(&work(&[], &[writer(), integrate(&[line.as_str()])]));
        r.refused(&["work_prove_not_function"]);
        let d = r.one("work_prove_not_function");
        assert_eq!(d.category, Category::Work);
        assert_eq!(d.symbol, Some(id(symbol)), "{selector}");
        assert_eq!(d.qualified_name.as_deref(), Some(selector));
        assert_eq!(r.spanned(d), selector);
    }
    for (selector, symbol) in [
        ("app.core.fetch", "fn_fetch"),
        ("app.core.double", "fn_double"),
        ("app.core.hidden", "app.core.hidden"),
    ] {
        let line = format!("prove({selector})");
        let r = run(&work(&[], &[writer(), integrate(&[line.as_str()])]));
        r.valid();
        assert_eq!(
            serde_json::to_value(&r.result.output.changes[0].integrate[0]).expect("json"),
            json!({ "require": "prove", "targets": [{ "id": symbol, "facet": "definition" }] })
        );
    }
}

/// A `prove` selector that names nothing does not name functions: the 7.2
/// row of `require prove(sel)` gives `work_prove_not_function`, and
/// `work_selector_empty` is the code of `write`/`depend`/`preserve`
/// selectors only (7.2 and Appendix B: "a write/depend/preserve selector
/// matches no declaration").
#[test]
fn prove_of_a_selector_that_names_nothing_is_work_prove_not_function() {
    let r = run(&work(
        &[],
        &[writer(), integrate(&["prove(app.core.nothing)"])],
    ));
    r.refused(&["work_prove_not_function"]);
}

// ===========================================================================
// The model (1.9)
// ===========================================================================

/// The model keeps changes, tasks, `after` names, goals, and requirements
/// in source order, and finds changes and tasks by name; the output omits
/// an absent goal (Appendix C).
#[test]
fn the_model_keeps_changes_tasks_and_requirements_in_source_order() {
    let one = change(
        "one",
        &[
            task("b", &["write { app.core.double.body }"]),
            task(
                "a after b",
                &["write { app.core.double.body }", "goal \"Polish it.\""],
            ),
            integrate(&["review(a)", "typecheck"]),
        ],
    );
    let two = change("two", &[task("solo", &["add { tests.fresh.* }"])]);
    let r = run(&format!("{one}\n{two}"));
    r.valid();
    let model = r.model();
    let names: Vec<&str> = model.changes.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["one", "two"]);
    assert!(model.change("three").is_none());
    let first = model.change("one").expect("one");
    let tasks: Vec<&str> = first.tasks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(tasks, ["b", "a"]);
    let a = first.task("a").expect("a");
    assert_eq!(a.after, ["b"]);
    assert_eq!(a.goal.as_deref(), Some("Polish it."));
    assert_eq!(first.task("b").expect("b").goal, None);
    assert_eq!(
        first.integrate,
        [
            RequirementSpec::Review("a".to_string()),
            RequirementSpec::Typecheck
        ]
    );
    assert_eq!(
        model.change("two").expect("two").tasks[0].add[0].module_prefix,
        path("tests.fresh")
    );
    let json = serde_json::to_value(&r.result.output).expect("json");
    assert_eq!(json["file"], json!(W));
    assert!(json["changes"][0]["tasks"][0].get("goal").is_none());
    assert_eq!(json["changes"][0]["tasks"][1]["goal"], json!("Polish it."));
    assert_eq!(json["changes"][0]["tasks"][1]["after"], json!(["b"]));
}

// ===========================================================================
// Envelope invariants over every code
// ===========================================================================

/// Two work files that between them raise every code `cpl work check`
/// produces without a repository.
fn assorted() -> Vec<String> {
    vec![
        work(
            &["use app.core.{nope, hidden}", "use app.nowhere.{x}", "use cfg.{project}"],
            &[
                task(
                    "a",
                    &[
                        "write { app.nothing.body, app.core.Pair.body, app.core.Small.identity, project.manifest, project.lock }",
                        "add { fresh.* }",
                    ],
                ),
                task(
                    "b after phantom",
                    &["write { app.core.helper.body }", "preserve { app.core.helper.body }"],
                ),
                task("c", &["write { app.core.helper.body }"]),
                task("d after d", &[]),
                task("e", &["depend { app.core.double }"]),
                integrate(&["test(app.*)", "prove(app.core.Pair)", "review(nobody)"]),
            ],
        ),
        "change bare {\n    workers 0\n}\n".to_string(),
    ]
}

/// Every diagnostic has the category and severity of Appendix B (the
/// `use` codes are semantics 9.4's `resolve` errors), the S49 envelope
/// rules (`qualified_name` with `symbol`, `facet` only with `symbol`), no
/// payload key among the reserved envelope keys (T5), and a span in the
/// work file unless it is `git_unavailable`.
#[test]
fn every_diagnostic_has_the_category_severity_and_envelope_of_its_code() {
    let warnings = [
        "work_add_namespace_new",
        "work_file_selector_shadowed",
        "work_provisional_identity",
        "work_task_without_authority",
        "git_unavailable",
    ];
    let resolve = ["unresolved_module", "unresolved_name", "private_item"];
    let mut seen = BTreeSet::new();
    for text in assorted() {
        let r = run(&text);
        for d in &r.result.diagnostics {
            let code = d.code.0;
            seen.insert(code);
            let category = if code.starts_with("work_") {
                Category::Work
            } else if code.starts_with("git_") {
                Category::Git
            } else {
                assert!(
                    resolve.contains(&code),
                    "{code} is no code of cpl work check"
                );
                Category::Resolve
            };
            assert_eq!(d.category, category, "{code}");
            let severity = if warnings.contains(&code) {
                Severity::Warning
            } else {
                Severity::Error
            };
            assert_eq!(d.severity, severity, "{code}");
            assert_envelope(d);
            for key in [
                "code",
                "category",
                "severity",
                "message",
                "span",
                "symbol",
                "qualified_name",
                "facet",
                "notes",
                "backtrace",
            ] {
                assert!(
                    !d.data.contains_key(key),
                    "{code}: the reserved key `{key}`"
                );
            }
            assert!(!d.message.is_empty(), "{code}");
            if code != "git_unavailable" {
                assert!(d.span.is_some(), "{code} has a location in the work file");
            }
        }
    }
    let expected: BTreeSet<&str> = [
        "unresolved_module",
        "unresolved_name",
        "private_item",
        "work_missing_base",
        "work_missing_target",
        "work_bad_workers",
        "work_selector_empty",
        "work_add_namespace_new",
        "work_file_selector_shadowed",
        "work_file_not_grantable",
        "work_provisional_identity",
        "work_facet_not_applicable",
        "work_identity_not_writable",
        "work_unknown_task",
        "work_cycle",
        "work_overlapping_writes",
        "work_test_selector_empty",
        "work_prove_not_function",
        "work_preserve_written",
        "work_task_without_authority",
        "git_unavailable",
    ]
    .into_iter()
    .collect();
    let missing: Vec<&&str> = expected.difference(&seen).collect();
    assert!(
        missing.is_empty(),
        "not raised by the assorted files: {missing:?}"
    );
}

/// T46: diagnostics come by span start, then severity, then code (all in
/// the one work file), the span-less ones last.
#[test]
fn diagnostics_come_in_sorted_order() {
    for text in assorted() {
        let r = run(&text);
        let diagnostics = &r.result.diagnostics;
        let spanned = diagnostics
            .iter()
            .position(|d| d.span.is_none())
            .unwrap_or(diagnostics.len());
        assert!(diagnostics[spanned..].iter().all(|d| d.span.is_none()));
        for pair in diagnostics[..spanned].windows(2) {
            let key = |d: &Diagnostic| (d.span.expect("a span").start, d.severity, d.code.0);
            assert!(key(&pair[0]) <= key(&pair[1]), "{:#?}", pair);
        }
    }
}

/// Determinism (7.1): validating the same file twice gives the same result.
#[test]
fn validation_is_deterministic() {
    for text in assorted() {
        assert_eq!(run(&text).result, run(&text).result);
    }
}

// ===========================================================================
// resolve_patches (1.9, 5.5, 10.5, grammar 4.5)
// ===========================================================================

/// `patch <target>` / `expect base(<expect>)` / the block.
fn patch_form(target: &str, expect: &str, block: &str) -> String {
    format!("patch {target}\n    expect base({expect})\n{block}\n")
}

/// The patch file id: a file of no project (the forms' spans are offsets in
/// the patch text).
const PATCH_FILE: FileId = FileId(4242);

/// The project with a work file of these `use` lines, its patch-file
/// `text` resolved in that file's scope (grammar 4.5).
fn resolve(
    uses: &[&str],
    text: &str,
) -> (VirtualTree, Result<Vec<ResolvedPatch>, Vec<Diagnostic>>) {
    let tree = project(&work(uses, &[writer()]));
    let (project, program) = checked(&tree);
    let file = parse_submitted_patch(text, PATCH_FILE).expect("the patch parses");
    let scope = SelectorScope {
        uses: &work_file(&project, W).uses,
        module: None,
    };
    let resolved = resolve_patches(&file, &program, &scope);
    (tree, resolved)
}

/// Section 5.5 step 1 compares the resolved `(id, facet)` pairs, not the
/// spellings: `double.body` through the work file's import and
/// `app.core.double.body` are one facet, and the patch validates.
#[test]
fn one_facet_spelled_two_ways_is_one_patch_target() {
    let (tree, resolved) = resolve(
        &["use app.core.{double}"],
        &patch_form("double.body", "app.core.double.body", "{\n    n + n\n}"),
    );
    let patches = resolved.expect("both selectors resolve");
    assert_eq!(patches.len(), 1);
    assert_eq!(patches[0].target, (id("fn_double"), Facet::Body));
    assert_eq!(patches[0].expect, patches[0].target);
    let map = map_of(&tree);
    let grant = Grant {
        write: [(id("fn_double"), Facet::Body)].into_iter().collect(),
        ..Grant::default()
    };
    assert_eq!(
        validate_patch(&patches[0], &map, &map, Some(&grant)),
        vec![]
    );
}

/// The standard library is never part of a revision (Section 5.4), so a
/// patch of a std body names an id absent from every baseline:
/// `patch_unknown_symbol` (5.5 step 2), blaming the target facet.
#[test]
fn a_standard_library_body_is_in_no_baseline() {
    let (tree, resolved) = resolve(
        &["use std.text.{trim}"],
        &patch_form("trim.body", "std.text.trim.body", "{\n    text\n}"),
    );
    let patches = resolved.expect("the selectors resolve to the std stub");
    assert_eq!(patches[0].target, (id("std.text.trim"), Facet::Body));
    let map = map_of(&tree);
    let diagnostics = validate_patch(&patches[0], &map, &map, None);
    assert_eq!(codes(&diagnostics), ["patch_unknown_symbol"]);
    assert_eq!(diagnostics[0].category, Category::Patch);
    assert_eq!(diagnostics[0].span, Some(patches[0].span));
    assert_envelope(&diagnostics[0]);
    // No file of the tree declares it: nothing to replace (1.9 apply_patch:
    // "an undeclared target ... is patch_target_missing").
    let error = apply_patch(&tree, &load(&tree), &patches[0]).expect_err("a std stub");
    assert_eq!(error.code, "patch_target_missing");
}

/// A patch of a declaration without `@id` resolves, through the work file's
/// submodule alias, to its provisional identity, which keys the facet map
/// (4.3, 5.4): it validates and applies. After a rename the provisional
/// identity names nothing in the tree being patched (4.3: "a patch's
/// `expect base(...)` on a provisional id fails after a rename").
#[test]
fn a_provisional_target_is_its_qualified_name_until_a_rename() {
    let (tree, resolved) = resolve(
        &["use app.{core}"],
        &patch_form(
            "core.helper.body",
            "core.helper.body",
            "{\n    double(n) + 2\n}",
        ),
    );
    let patches = resolved.expect("resolves through the alias");
    let patch = &patches[0];
    assert_eq!(patch.target, (id("app.core.helper"), Facet::Body));
    let baseline = map_of(&tree);
    assert!(baseline.get(&id("app.core.helper")).is_some());
    assert_eq!(validate_patch(patch, &baseline, &baseline, None), vec![]);
    let patched = apply_patch(&tree, &load(&tree), patch).expect("applies");
    assert!(
        text(&patched, CORE_PATH).contains("pub fn helper(n: Int) -> Int {\n    double(n) + 2\n}")
    );

    let renamed = with_file(
        &tree,
        CORE_PATH,
        &CORE.replace("pub fn helper(", "pub fn helper2("),
    );
    let current = map_of(&renamed);
    assert_eq!(
        codes(&validate_patch(patch, &baseline, &current, None)),
        ["patch_target_missing"]
    );
    let error = apply_patch(&renamed, &load(&renamed), patch).expect_err("no such declaration");
    assert_eq!(error.code, "patch_target_missing");
}

/// The grant of a task validated from its work file decides a patch's
/// scope (10.5, design "Semantic patch syntax": a patch is rejected if it
/// escapes the write set or changes a preserved facet): the written body is
/// in scope; a preserved one is out of scope and, changed, preserved; a
/// patch that only adds a comment changes no preserved hash (5.2).
#[test]
fn the_grant_of_a_validated_task_decides_scope_and_preservation() {
    let w = work(
        &["use app.core.{double, helper}"],
        &[task(
            "impl",
            &["write { double.body }", "preserve { helper.body }"],
        )],
    );
    let r = run(&w);
    r.valid();
    let spec = &r.model().changes[0].tasks[0];
    let pairs = |selectors: &[ResolvedSelector]| -> BTreeSet<(StableId, Facet)> {
        selectors
            .iter()
            .flat_map(|s| s.targets.iter())
            .map(|(id, f)| match f {
                FacetSelector::Facet(f) => (id.clone(), *f),
                FacetSelector::Definition => panic!("a facet selector"),
            })
            .collect()
    };
    let grant = Grant {
        write: pairs(&spec.write),
        preserve: pairs(&spec.preserve),
        ..Grant::default()
    };
    let tree = project(&w);
    let (project, program) = checked(&tree);
    let text = [
        patch_form("double.body", "double.body", "{\n    n + n\n}"),
        patch_form("helper.body", "helper.body", "{\n    double(n) + 2\n}"),
        patch_form(
            "helper.body",
            "helper.body",
            "{\n    // the same body, commented\n    double(n) + 1\n}",
        ),
    ]
    .join("\n");
    let file = parse_submitted_patch(&text, PATCH_FILE).expect("the patch parses");
    let scope = SelectorScope {
        uses: &work_file(&project, W).uses,
        module: None,
    };
    let patches = resolve_patches(&file, &program, &scope).expect("the patch resolves");
    let map = map_of(&tree);
    let verdicts: Vec<Vec<&str>> = patches
        .iter()
        .map(|p| codes(&validate_patch(p, &map, &map, Some(&grant))))
        .collect();
    assert_eq!(
        verdicts,
        [
            vec![],
            vec!["patch_out_of_scope", "patch_preserved_facet"],
            vec!["patch_out_of_scope"],
        ]
    );
}

/// `resolve_patches` refuses the whole file when any form fails, with the
/// diagnostics of every failing form in order: a selector that names no
/// declaration is `patch_unknown_symbol`, a whole declaration or a
/// namespace `patch_facet_unsupported` (D39: a patch replaces one body).
#[test]
fn every_failing_patch_form_is_reported_in_order() {
    let text = [
        patch_form("double.body", "double.body", "{\n    n\n}"),
        patch_form("nope.body", "nope.body", "{\n    n\n}"),
        patch_form("double", "double.body", "{\n    n\n}"),
        patch_form("app.core.*", "double.body", "{\n    n\n}"),
    ]
    .join("\n");
    let (_, resolved) = resolve(&["use app.core.{double}"], &text);
    let errors = resolved.expect_err("three forms fail");
    assert_eq!(
        codes(&errors),
        [
            "patch_unknown_symbol",
            "patch_unknown_symbol",
            "patch_facet_unsupported",
            "patch_facet_unsupported"
        ]
    );
    let spanned: Vec<&str> = errors
        .iter()
        .map(|d| {
            let span = d.span.expect("a span");
            &text[span.start as usize..span.end as usize]
        })
        .collect();
    assert_eq!(spanned, ["nope.body", "nope.body", "double", "app.core.*"]);
    for d in &errors {
        assert_eq!((d.category, d.severity), (Category::Patch, Severity::Error));
        assert_envelope(d);
    }
}

/// `validate_patch` "blames the target facet" (1.9) and S49 allows a facet
/// only together with its symbol (and the symbol only with its qualified
/// name): a target in neither facet map (the program the patch was
/// resolved against declares it, the baseline and the tree being patched
/// do not) is `patch_unknown_symbol` with a consistent envelope.
#[test]
fn a_patch_target_in_no_facet_map_is_blamed_consistently() {
    let base = project(&work(&[], &[writer()]));
    let extra = with_file(
        &base,
        "src/app/extra.cpl",
        "module app.extra\n\n@id(\"fn_spare\")\npub fn spare() -> Int {\n    0\n}\n",
    );
    let (extra_project, program) = checked(&extra);
    let file = parse_submitted_patch(
        &patch_form(
            "app.extra.spare.body",
            "app.extra.spare.body",
            "{\n    1\n}",
        ),
        PATCH_FILE,
    )
    .expect("the patch parses");
    let scope = SelectorScope {
        uses: &work_file(&extra_project, W).uses,
        module: None,
    };
    let patches = resolve_patches(&file, &program, &scope).expect("resolves against that program");
    assert_eq!(patches[0].target, (id("fn_spare"), Facet::Body));
    let map = map_of(&base);
    let diagnostics = validate_patch(&patches[0], &map, &map, None);
    assert_eq!(codes(&diagnostics), ["patch_unknown_symbol"]);
    assert_eq!(diagnostics[0].span, Some(patches[0].span));
    assert_envelope(&diagnostics[0]);
}

/// `apply_patch` of a facet other than `body` is `patch_facet_unsupported`
/// (1.9); S49 allows its `facet` only together with the target's `symbol`
/// and `qualified_name`.
#[test]
fn apply_patch_blames_an_unsupported_facet_consistently() {
    let (tree, resolved) = resolve(
        &["use app.core.{double}"],
        &patch_form("double.contract", "double.contract", "{\n    n\n}"),
    );
    let patch = resolved.expect("a contract selector resolves").remove(0);
    assert_eq!(patch.target, (id("fn_double"), Facet::Contract));
    let error = apply_patch(&tree, &load(&tree), &patch).expect_err("not a body");
    assert_eq!(error.code, "patch_facet_unsupported");
    assert_eq!(error.span, Some(patch.span));
    assert_envelope(&error);
}

// ===========================================================================
// Further cases
// ===========================================================================

/// T47 warns "for every declaration" a task or requirement names without an
/// `@id`: one warning per declaration, however often and wherever it is
/// named (two sets, a requirement, a second change).
#[test]
fn each_provisional_declaration_warns_once() {
    let one = change(
        "one",
        &[
            task(
                "a",
                &[
                    "write { app.core.helper.body }",
                    "depend { app.core.helper.contract, app.core.hidden }",
                ],
            ),
            integrate(&["prove(app.core.helper)"]),
        ],
    );
    let two = change("two", &[task("b", &["write { app.core.helper.docs }"])]);
    let r = run(&format!("{one}\n{two}"));
    r.valid();
    let warned: Vec<Option<StableId>> = r
        .all("work_provisional_identity")
        .iter()
        .map(|d| d.symbol.clone())
        .collect();
    assert_eq!(
        warned,
        [Some(id("app.core.helper")), Some(id("app.core.hidden"))],
        "{:#?}",
        r.result.diagnostics
    );
}

/// `cpl work check` consults the policy's `files` only (7.2,
/// `work_file_not_grantable`); `[work.policy] write` and `add` narrow the
/// grant when a change opens (10.3) and refuse nothing here.
#[test]
fn only_the_policys_files_matter_to_work_check() {
    let narrow = WorkPolicySection {
        write: vec!["app.core.helper.body".to_string()],
        add: vec!["tests.app.*".to_string()],
        files: Vec::new(),
        ..WorkPolicySection::default()
    };
    let r = run_with(
        &work(
            &[],
            &[
                task("a", &["write { app.core.double.body }"]),
                task("b", &["add { tests.fresh.* }"]),
            ],
        ),
        &narrow,
        None,
    );
    r.valid();
}

/// The overlap rule compares resolved facets and namespaces, not their
/// spellings: an imported and a fully qualified name of one body, and an
/// alias of `app.core` against a module below it.
#[test]
fn overlap_compares_resolved_facets_and_namespaces_not_spellings() {
    let r = run(&work(
        &["use app.core.{double}"],
        &[
            task("a", &["write { double.body }"]),
            task("b", &["write { app.core.double.body }"]),
        ],
    ));
    r.refused(&["work_overlapping_writes"]);
    assert_eq!(
        r.one("work_overlapping_writes").symbol,
        Some(id("fn_double"))
    );
    let r = run(&work(
        &["use app.{core}"],
        &[
            task("a", &["add { core.* }"]),
            task("b", &["add { app.core.deep.* }"]),
        ],
    ));
    r.refused(&["work_overlapping_writes"]);
}

/// With a repository, a missing `base` or `target` does not stop the other
/// ref's resolution.
#[test]
fn a_missing_item_does_not_stop_the_other_refs_resolution() {
    if !git_available() {
        return;
    }
    let h = history("missing");
    let no_base = format!("change c {{\n    target git(\"v1\")\n\n{}}}\n", writer());
    let r = run_with(&no_base, &WorkPolicySection::default(), Some(&h.repo));
    r.refused(&["work_missing_base", "work_target_not_branch"]);
    let no_target = format!("change c {{\n    base git(\"v2\")\n\n{}}}\n", writer());
    let r = run_with(&no_target, &WorkPolicySection::default(), Some(&h.repo));
    r.refused(&["work_missing_target"]);
    assert_eq!(
        r.result.output.changes[0].base_commit.as_deref(),
        Some(h.first.as_str())
    );
}
