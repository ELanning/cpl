//! Adversarial tests of semantic patches and scope enforcement
//! (`spec/toolchain.md` 5.5 and 10.5; design-doc.md "Semantic patch
//! syntax" and "Changes and agent tasks"):
//!
//! - `expect base` staleness (5.5): what must and must not count as a stale
//!   body, at task-level validation and against a moved target head;
//! - write/add/preserve enforcement over facet diffs (10.5 rules 1-5): add
//!   namespaces, file selectors, modules spanning files, identity
//!   integrity, non-facet text;
//! - `apply_patch` byte-exactness (10.5): indentation, line terminators,
//!   comments around and inside the body, modules spanning files, generic
//!   targets;
//! - the `patch_*` codes (Appendix B), including `patch_syntax`.
//!
//! Every expectation is the specification's answer; where the
//! specification is silent the test says so and follows design-doc.md.
//! The helpers are local, so this file depends only on the public APIs of
//! cpl-work, cpl-index, cpl-project, cpl-syntax, and cpl-diag.

use cpl_diag::json::{DiffStatementKind, FacetHash, Revision, RevisionState};
use cpl_diag::{Category, Diagnostic, Facet, FileId, FileOrigin, Severity, StableId};
use cpl_index::{facet_map, source_digest, FacetMap};
use cpl_project::{load_project_from_tree, LoadOptions, Project, VirtualTree};
use cpl_syntax::ast::PatchForm;
use cpl_work::{
    apply_patch, enforce, parse_submitted_patch, tree_diff, validate_patch, Enforcement,
    FileSelector, Grant, ResolvedPatch,
};
use serde_json::json;
use std::collections::BTreeSet;
use std::path::Path;

// ===========================================================================
// Helpers
// ===========================================================================

const MANIFEST: &str = "[project]\nname = \"t\"\nedition = \"0.1\"\n";

/// A tree of `(path, text)` pairs plus a minimal `cpl.toml`.
fn tree(files: &[(&str, &str)]) -> VirtualTree {
    let mut tree = VirtualTree::default();
    tree.files
        .insert("cpl.toml".to_string(), MANIFEST.as_bytes().to_vec());
    for (path, text) in files {
        tree.files
            .insert((*path).to_string(), text.as_bytes().to_vec());
    }
    tree
}

fn with(tree: &VirtualTree, path: &str, text: &str) -> VirtualTree {
    let mut tree = tree.clone();
    tree.files
        .insert(path.to_string(), text.as_bytes().to_vec());
    tree
}

#[track_caller]
fn without(tree: &VirtualTree, path: &str) -> VirtualTree {
    let mut tree = tree.clone();
    assert!(tree.files.remove(path).is_some(), "no {path}");
    tree
}

#[track_caller]
fn text(tree: &VirtualTree, path: &str) -> String {
    let bytes = tree.files.get(path).unwrap_or_else(|| panic!("no {path}"));
    String::from_utf8(bytes.clone()).expect("utf-8")
}

/// `tree` with the one occurrence of `from` in `path` replaced by `to`.
#[track_caller]
fn edit(tree: &VirtualTree, path: &str, from: &str, to: &str) -> VirtualTree {
    let old = text(tree, path);
    assert_eq!(
        old.matches(from).count(),
        1,
        "{path} contains {from:?} exactly once"
    );
    with(tree, path, &old.replacen(from, to, 1))
}

/// Load a tree, `work/` files included, whatever its parse diagnostics.
fn load_lenient(tree: &VirtualTree) -> Project {
    load_project_from_tree(
        tree,
        FileOrigin::Virtual,
        &LoadOptions {
            include_work_files: true,
            ..LoadOptions::default()
        },
    )
    .unwrap_or_else(|e| panic!("load: {e}"))
}

/// Load a tree whose every file parses.
#[track_caller]
fn load(tree: &VirtualTree) -> Project {
    let project = load_lenient(tree);
    let parse: Vec<&Diagnostic> = project
        .diagnostics
        .iter()
        .filter(|d| d.category == Category::Parse)
        .collect();
    assert!(parse.is_empty(), "parse diagnostics: {parse:?}");
    project
}

fn map_of_project(project: &Project) -> FacetMap {
    let revision = Revision {
        commit: None,
        state: RevisionState::Worktree,
        digest: source_digest(project),
    };
    facet_map(project, revision)
}

#[track_caller]
fn map_of(tree: &VirtualTree) -> FacetMap {
    map_of_project(&load(tree))
}

#[track_caller]
fn id(text: &str) -> StableId {
    StableId::parse(text).unwrap_or_else(|e| panic!("{text} is not a stable id: {e:?}"))
}

#[track_caller]
fn hash(map: &FacetMap, target: &StableId, facet: Facet) -> FacetHash {
    map.get(target)
        .and_then(|e| e.facet(facet))
        .unwrap_or_else(|| panic!("{target} has no {facet} facet"))
        .clone()
}

#[track_caller]
fn forms(text: &str) -> Vec<PatchForm> {
    parse_submitted_patch(text, FileId(0))
        .unwrap_or_else(|d| panic!("the patch parses: {d:?}"))
        .forms
}

/// The first patch form of `patch_text`, both selectors resolved to the
/// body of `target` (selector resolution needs the checker; the tests
/// resolve by hand, as `tests/patch.rs` does).
#[track_caller]
fn body_patch(target: &str, patch_text: &str) -> ResolvedPatch {
    let form = forms(patch_text).remove(0);
    ResolvedPatch {
        target: (id(target), Facet::Body),
        expect: (id(target), Facet::Body),
        body: form.body,
        span: form.span,
    }
}

/// `patch <sel>.body` / `    expect base(<sel>.body)` / `<block>`.
fn patch_text(selector: &str, block: &str) -> String {
    format!("patch {selector}.body\n    expect base({selector}.body)\n{block}\n")
}

#[track_caller]
fn apply(tree: &VirtualTree, patch: &ResolvedPatch) -> VirtualTree {
    apply_patch(tree, &load(tree), patch).unwrap_or_else(|d| panic!("the patch applies: {d:?}"))
}

fn codes(diagnostics: &[Diagnostic]) -> Vec<&'static str> {
    diagnostics.iter().map(|d| d.code.0).collect()
}

fn write(pairs: &[(StableId, Facet)]) -> Grant {
    Grant {
        write: pairs.iter().cloned().collect(),
        ..Grant::default()
    }
}

fn write_facets(target: &StableId, facets: &[Facet]) -> Grant {
    Grant {
        write: facets.iter().map(|f| (target.clone(), *f)).collect(),
        ..Grant::default()
    }
}

/// `tests.app` -> `["tests", "app"]`.
fn namespaces(spellings: &[&str]) -> Vec<Vec<String>> {
    spellings
        .iter()
        .map(|s| s.split('.').map(str::to_string).collect())
        .collect()
}

fn adding(spellings: &[&str]) -> Grant {
    Grant {
        add: namespaces(spellings),
        ..Grant::default()
    }
}

fn files(selectors: &[FileSelector]) -> Grant {
    Grant {
        files: selectors.iter().cloned().collect(),
        ..Grant::default()
    }
}

fn union(grants: impl IntoIterator<Item = Grant>) -> Grant {
    let mut all = Grant::default();
    for grant in grants {
        all.write.extend(grant.write);
        all.add.extend(grant.add);
        all.preserve.extend(grant.preserve);
        all.files.extend(grant.files);
    }
    all
}

/// Every facet of a `fn` except `identity`, which no grant holds (10.3).
const FN_FACETS: [Facet; 7] = [
    Facet::Name,
    Facet::Signature,
    Facet::Contract,
    Facet::Effects,
    Facet::Body,
    Facet::Docs,
    Facet::VerificationPolicy,
];
/// A struct's, enum's, or wrapper type's facets except `identity` (5.1).
const DATA_FACETS: [Facet; 3] = [Facet::Name, Facet::Signature, Facet::Docs];
/// A test's facets except `identity` (5.1).
const TEST_FACETS: [Facet; 3] = [Facet::Name, Facet::Body, Facet::Docs];

/// `enforce` with the default sources `["src"]`.
#[track_caller]
fn check(baseline: &VirtualTree, candidate: &VirtualTree, grant: &Grant) -> Enforcement {
    check_in(&["src"], baseline, candidate, grant)
}

#[track_caller]
fn check_in(
    sources: &[&str],
    baseline: &VirtualTree,
    candidate: &VirtualTree,
    grant: &Grant,
) -> Enforcement {
    let sources: Vec<String> = sources.iter().map(|s| s.to_string()).collect();
    let diff = tree_diff(baseline, candidate, &sources);
    enforce(&map_of(baseline), &map_of(candidate), &diff, grant)
}

/// `enforce` over file-level changes only: both facet maps are those of
/// `map_tree` (the changed files are not sources, so the maps are equal).
#[track_caller]
fn check_files(
    map_tree: &VirtualTree,
    baseline: &VirtualTree,
    candidate: &VirtualTree,
    grant: &Grant,
) -> Enforcement {
    let map = map_of(map_tree);
    let diff = tree_diff(baseline, candidate, &["src".to_string()]);
    enforce(&map, &map, &diff, grant)
}

fn violation_codes(e: &Enforcement) -> Vec<&'static str> {
    codes(&e.violations)
}

/// The `path` field of the file-level violations, sorted. Rule 1 names the
/// field (`violation(patch_out_of_scope, path)`); rule 5 judges files too,
/// so its violations are read the same way.
fn violation_paths(e: &Enforcement) -> Vec<String> {
    let mut paths: Vec<String> = e
        .violations
        .iter()
        .filter_map(|d| d.data.get("path").and_then(|p| p.as_str()))
        .map(str::to_string)
        .collect();
    paths.sort();
    paths
}

// ===========================================================================
// Fixtures
// ===========================================================================

// ---- a project for the staleness tests ------------------------------------

const CALC: &str = "src/calc.cpl";
const TOTAL_ID: &str = "fn_kkkkkkkkkkkkkkkk";
const TOTAL_BODY: &str = "{\n    var sum = 0\n    var rest = xs\n    while rest.len > 0\n        invariant sum >= 0\n    {\n        sum = (sum + 1)\n        rest = rest.drop(1)\n    }\n    sum\n}";

fn total_decl(name: &str) -> String {
    format!(
        "/// Totals.\n@id(\"{TOTAL_ID}\")\npub fn {name}(xs: List<Int>) -> Int\n    ensures result >= 0\n{TOTAL_BODY}\n"
    )
}

fn calc_text() -> String {
    format!(
        "module calc\n\n{}\nfn count(n: Int) -> Int\n    requires n >= 0\n    decreases n\n{{\n    if n == 0 {{\n        0\n    }} else {{\n        count(n - 1)\n    }}\n}}\n\nfn other() -> Int {{\n    1\n}}\n",
        total_decl("total")
    )
}

fn calc_tree() -> VirtualTree {
    tree(&[(CALC, &calc_text())])
}

fn total() -> StableId {
    id(TOTAL_ID)
}

fn total_patch() -> ResolvedPatch {
    body_patch(TOTAL_ID, &patch_text("total", "{\n    xs.len\n}"))
}

fn total_writer() -> Grant {
    write(&[(total(), Facet::Body)])
}

// ---- a project for the enforcement tests ----------------------------------

const PAGES: &str = "src/app/pages.cpl";
const UTIL: &str = "src/app/util.cpl";
/// A second file of module `app.util` (a module spanning files, 3.7).
const UTIL_EXTRA: &str = "src/app/util/extra.cpl";
const TESTS: &str = "src/tests/app.cpl";
const WORK: &str = "work/app.work";

const PAGES_TEXT: &str = r#"module app.pages

use app.util.{helper}

pub capability Store {
    fn get(k: Int) -> Int
    fn put(k: Int) -> Int
    fn del(k: Int) -> Int
}

@id("ty_bbbbbbbbbbbbbbbb")
pub type Size(Int)

/// An item.
@id("ty_aaaaaaaaaaaaaaaa")
pub struct Item {
    /// The key.
    key: Int
}

/// Doubles.
@id("fn_aaaaaaaaaaaaaaaa")
pub fn double(x: Int) -> Int
    requires x > 0
    ensures result == x * 2
{
    // the work
    helper(x) * 2
}

@id("fn_bbbbbbbbbbbbbbbb")
fn noop() -> Int {
    0
}

@id("fn_cccccccccccccccc")
pub fn fetch(k: Int, using s: Store) -> Int
    effects { s.get, s.put }
{
    s.get(k)
}
"#;

const UTIL_TEXT: &str = r#"module app.util

@id("fn_ffffffffffffffff")
pub fn helper(x: Int) -> Int {
    x
}

fn plain() -> Int {
    1
}
"#;

const UTIL_EXTRA_TEXT: &str = r#"module app.util

@id("fn_eeeeeeeeeeeeeeee")
pub fn twice(x: Int) -> Int {
    x + x
}

fn spare() -> Int {
    3
}
"#;

const TESTS_TEXT: &str = r#"module tests.app

use app.pages.{double}

test "doubles" {
    assert double(2) == 4
}

@id("test_aaaaaaaaaaaaaaaa")
test "doubles one" {
    assert double(1) == 2
}
"#;

const WORK_TEXT: &str = "change c {\n    base git(\"main\")\n    target git(\"main\")\n\n    task t {\n        write { double.body }\n    }\n}\n";

fn base() -> VirtualTree {
    tree(&[
        (PAGES, PAGES_TEXT),
        (UTIL, UTIL_TEXT),
        (UTIL_EXTRA, UTIL_EXTRA_TEXT),
        (TESTS, TESTS_TEXT),
        (WORK, WORK_TEXT),
    ])
}

fn double() -> StableId {
    id("fn_aaaaaaaaaaaaaaaa")
}
fn noop() -> StableId {
    id("fn_bbbbbbbbbbbbbbbb")
}
fn fetch() -> StableId {
    id("fn_cccccccccccccccc")
}
fn twice() -> StableId {
    id("fn_eeeeeeeeeeeeeeee")
}
fn helper() -> StableId {
    id("fn_ffffffffffffffff")
}
fn item() -> StableId {
    id("ty_aaaaaaaaaaaaaaaa")
}
fn size() -> StableId {
    id("ty_bbbbbbbbbbbbbbbb")
}
fn plain() -> StableId {
    id("app.util.plain")
}
fn spare() -> StableId {
    id("app.util.spare")
}
fn doubles_test() -> StableId {
    id("tests.app.\"doubles\"")
}
fn doubles_one() -> StableId {
    id("test_aaaaaaaaaaaaaaaa")
}

// ---- the acceptance corpus -------------------------------------------------

const READER_PAGES: &str = "src/reader/pages.cpl";

/// `examples/reader`, every file, keyed by `/` paths.
fn reader_tree() -> VirtualTree {
    fn walk(root: &Path, dir: &Path, tree: &mut VirtualTree) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .expect("read dir")
            .map(|e| e.expect("dir entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(root, &path, tree);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .expect("under the root")
                    .to_string_lossy()
                    .replace('\\', "/");
                tree.files
                    .insert(relative, std::fs::read(&path).expect("read file"));
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/reader");
    let mut tree = VirtualTree::default();
    walk(&root, &root, &mut tree);
    tree
}

/// The reader with `paginate`'s body replaced by `{ ?build_pages }`
/// (Section 12, "work and patch").
fn reader_hole_tree() -> VirtualTree {
    let complete = reader_tree();
    let pages = text(&complete, READER_PAGES);
    let last_clause = "    ensures forall page in result.drop_last(): page.len == size\n";
    let start = pages.find(last_clause).expect("paginate's last clause") + last_clause.len();
    let tail = "\n    pages\n}";
    let end = start + pages[start..].find(tail).expect("paginate's tail") + tail.len();
    assert!(pages[start..end].starts_with("{\n    var pages"));
    with(
        &complete,
        READER_PAGES,
        &format!("{}{{ ?build_pages }}{}", &pages[..start], &pages[end..]),
    )
}

fn pgn() -> StableId {
    id("pgn_7c2d")
}

/// `work/pagination.patch`, both selectors resolved to `pgn_7c2d.body`.
fn corpus_patch(tree: &VirtualTree) -> ResolvedPatch {
    let form = forms(&text(tree, "work/pagination.patch")).remove(0);
    ResolvedPatch {
        target: (pgn(), Facet::Body),
        expect: (pgn(), Facet::Body),
        body: form.body,
        span: form.span,
    }
}

/// The grant of `pagination.work`'s `implementation` task.
fn implementation_grant() -> Grant {
    Grant {
        write: [(pgn(), Facet::Body)].into(),
        preserve: [
            (pgn(), Facet::Signature),
            (pgn(), Facet::Contract),
            (pgn(), Facet::Effects),
            (pgn(), Facet::VerificationPolicy),
        ]
        .into(),
        ..Grant::default()
    }
}

// ===========================================================================
// 5.5: `expect base` staleness
// ===========================================================================

#[test]
fn stale_check_ignores_comments_added_to_the_current_body() {
    let baseline = calc_tree();
    let current = edit(
        &edit(
            &baseline,
            CALC,
            "    var sum = 0\n",
            "    // start at zero\n    var sum = 0 // running count\n",
        ),
        CALC,
        "    sum\n}\n\nfn count",
        "    sum\n    // done\n}\n\nfn count",
    );
    let (before, now) = (map_of(&baseline), map_of(&current));
    // 5.2: comments are excluded from every facet text, so the body facet
    // version is unchanged and the patch is not stale (design: a patch
    // tolerates unrelated movement).
    assert_eq!(
        hash(&before, &total(), Facet::Body),
        hash(&now, &total(), Facet::Body)
    );
    assert_eq!(
        validate_patch(&total_patch(), &before, &now, Some(&total_writer())),
        vec![]
    );
}

#[test]
fn stale_check_ignores_the_layout_of_the_current_body() {
    let baseline = calc_tree();
    // A blank line, tab indentation, a source line break inside
    // parentheses (break_before_op is ignored by canonical text, 5.2), and
    // over-indentation.
    let current = edit(
        &edit(
            &edit(
                &baseline,
                CALC,
                "    var rest = xs\n",
                "\n\tvar rest = xs\n",
            ),
            CALC,
            "        sum = (sum + 1)\n",
            "  sum = (sum\n            + 1)\n",
        ),
        CALC,
        "    sum\n}\n\nfn count",
        "            sum\n}\n\nfn count",
    );
    let (before, now) = (map_of(&baseline), map_of(&current));
    assert_eq!(
        hash(&before, &total(), Facet::Body),
        hash(&now, &total(), Facet::Body)
    );
    assert_eq!(
        validate_patch(&total_patch(), &before, &now, Some(&total_writer())),
        vec![]
    );
}

#[test]
fn renamed_and_moved_target_is_not_stale_and_is_patched_where_it_lives() {
    // Design "Semantic patch syntax": a patch tolerates unrelated file
    // movement; renames and moves preserve an `@id` identity (4.6).
    let baseline = calc_tree();
    let moved_file = "src/stats/more.cpl";
    let moved_text = format!("module stats\n\n// moved here\n{}", total_decl("sum_all"));
    let current = with(
        &edit(
            &edit(&baseline, CALC, &format!("{}\n", total_decl("total")), ""),
            CALC,
            "fn other() -> Int {\n    1\n}",
            "fn other() -> Int {\n    2\n}",
        ),
        moved_file,
        &moved_text,
    );
    let (before, head) = (map_of(&baseline), map_of(&current));
    assert_eq!(
        head.get(&total()).map(|e| e.version.path.as_str()),
        Some("stats.sum_all")
    );
    assert_eq!(
        validate_patch(&total_patch(), &before, &head, Some(&total_writer())),
        vec![]
    );
    let patched = apply(&current, &total_patch());
    assert_eq!(
        text(&patched, moved_file),
        moved_text.replace(TOTAL_BODY, "{\n    xs.len\n}")
    );
    assert_eq!(
        without(&patched, moved_file),
        without(&current, moved_file),
        "every other file is byte-identical"
    );
}

#[test]
fn stale_check_compares_the_body_facet_only() {
    // 5.5 steps 2-4 compare `facets[body]`; a changed signature, contract,
    // docs, or policy is not a stale body.
    let baseline = calc_tree();
    let current = edit(
        &edit(
            &baseline,
            CALC,
            "/// Totals.\n@id(\"fn_kkkkkkkkkkkkkkkk\")\npub fn total",
            "/// Totals a list.\n@id(\"fn_kkkkkkkkkkkkkkkk\")\n@prove\nfn total",
        ),
        CALC,
        "    ensures result >= 0\n",
        "    requires xs.len >= 0\n    ensures result >= 0\n",
    );
    let (before, now) = (map_of(&baseline), map_of(&current));
    for facet in [
        Facet::Signature,
        Facet::Contract,
        Facet::Docs,
        Facet::VerificationPolicy,
    ] {
        assert_ne!(
            hash(&before, &total(), facet),
            hash(&now, &total(), facet),
            "{facet}"
        );
    }
    assert_eq!(
        validate_patch(&total_patch(), &before, &now, Some(&total_writer())),
        vec![]
    );
}

#[test]
fn a_changed_loop_invariant_makes_a_body_patch_stale() {
    // D-159: loop invariants belong to the body facet.
    let baseline = calc_tree();
    let current = edit(
        &baseline,
        CALC,
        "        invariant sum >= 0\n",
        "        invariant sum >= 0\n        invariant rest.len >= 0\n",
    );
    let (before, now) = (map_of(&baseline), map_of(&current));
    let d = validate_patch(&total_patch(), &before, &now, Some(&total_writer()));
    assert_eq!(codes(&d), ["patch_stale"]);
    // "patch_stale with both hashes" (5.5 step 4), in the expected/actual
    // vocabulary of semantics 9.2.
    assert_eq!(
        d[0].data["expected"],
        json!(hash(&before, &total(), Facet::Body))
    );
    assert_eq!(
        d[0].data["actual"],
        json!(hash(&now, &total(), Facet::Body))
    );
    assert_eq!(
        (d[0].category, d[0].severity),
        (Category::Patch, Severity::Error)
    );
}

#[test]
fn a_changed_function_decreases_clause_is_contract_not_body() {
    // D-159: a function's `decreases` clause belongs to the contract.
    let baseline = calc_tree();
    let current = edit(
        &baseline,
        CALC,
        "    decreases n\n",
        "    decreases n + 1\n",
    );
    let count = id("calc.count");
    let patch = body_patch("calc.count", &patch_text("count", "{\n    n\n}"));
    let (before, now) = (map_of(&baseline), map_of(&current));
    assert_ne!(
        hash(&before, &count, Facet::Contract),
        hash(&now, &count, Facet::Contract)
    );
    assert_eq!(
        validate_patch(&patch, &before, &now, Some(&write(&[(count, Facet::Body)]))),
        vec![]
    );
}

#[test]
fn a_provisional_target_renamed_in_the_head_is_target_missing() {
    // 4.3: "a patch's expect base(...) on a provisional id fails after a
    // rename"; 5.5 step 3: absent from the tree being patched.
    let baseline = calc_tree();
    let current = edit(
        &baseline,
        CALC,
        "fn other() -> Int {",
        "fn another() -> Int {",
    );
    let patch = body_patch("calc.other", &patch_text("other", "{\n    2\n}"));
    let d = validate_patch(
        &patch,
        &map_of(&baseline),
        &map_of(&current),
        Some(&write(&[(id("calc.other"), Facet::Body)])),
    );
    assert_eq!(codes(&d), ["patch_target_missing"]);
    let error = apply_patch(&current, &load(&current), &patch).expect_err("no calc.other");
    assert_eq!(error.code, "patch_target_missing");
}

#[test]
fn a_provisional_target_moved_to_another_module_is_target_missing() {
    let baseline = calc_tree();
    let current = with(
        &edit(&baseline, CALC, "\nfn other() -> Int {\n    1\n}\n", ""),
        "src/misc.cpl",
        "module misc\n\nfn other() -> Int {\n    1\n}\n",
    );
    let patch = body_patch("calc.other", &patch_text("other", "{\n    2\n}"));
    let d = validate_patch(&patch, &map_of(&baseline), &map_of(&current), None);
    assert_eq!(codes(&d), ["patch_target_missing"]);
}

#[test]
fn a_provisional_target_moved_between_files_of_its_module_still_matches() {
    // 3.7 and 4.3: the provisional identity comes from the module header,
    // not the file, so a move to another file of the same module is
    // "unrelated file movement" the patch tolerates.
    let baseline = calc_tree();
    let more = "src/calc/more.cpl";
    let current = with(
        &edit(&baseline, CALC, "\nfn other() -> Int {\n    1\n}\n", ""),
        more,
        "module calc\n\nfn other() -> Int {\n    1\n}\n",
    );
    let patch = body_patch("calc.other", &patch_text("other", "{\n    2\n}"));
    assert_eq!(
        validate_patch(&patch, &map_of(&baseline), &map_of(&current), None),
        vec![]
    );
    let patched = apply(&current, &patch);
    assert_eq!(
        text(&patched, more),
        "module calc\n\nfn other() -> Int {\n    2\n}\n"
    );
    assert_eq!(text(&patched, CALC), text(&current, CALC));
}

#[test]
fn a_head_whose_target_has_no_body_facet_is_target_missing() {
    // The same @id now names a struct: 5.5 step 3, `facets[body]` absent.
    let baseline = calc_tree();
    let current = edit(
        &baseline,
        CALC,
        &total_decl("total"),
        "@id(\"fn_kkkkkkkkkkkkkkkk\")\npub struct Total {\n    value: Int\n}\n",
    );
    let d = validate_patch(
        &total_patch(),
        &map_of(&baseline),
        &map_of(&current),
        Some(&total_writer()),
    );
    assert_eq!(codes(&d), ["patch_target_missing"]);
}

#[test]
fn a_head_whose_target_file_does_not_parse_is_target_missing() {
    // 5.4: a file that failed to parse contributes no declarations, so the
    // target is absent from the tree being patched (not stale).
    let baseline = calc_tree();
    let current = edit(
        &baseline,
        CALC,
        "fn other() -> Int {\n    1\n}",
        "fn other( -> Int {\n    1\n}",
    );
    let now = map_of_project(&load_lenient(&current));
    assert!(now.get(&total()).is_none());
    let d = validate_patch(
        &total_patch(),
        &map_of(&baseline),
        &now,
        Some(&total_writer()),
    );
    assert_eq!(codes(&d), ["patch_target_missing"]);
}

#[test]
fn a_mismatched_expect_is_refused_even_when_both_bodies_hash_alike() {
    // D39: the patch selector and the expected selector must denote the
    // same facet; equal hashes do not make two declarations one.
    let baseline = edit(
        &calc_tree(),
        CALC,
        "fn other() -> Int {\n    1\n}\n",
        "fn other() -> Int {\n    1\n}\n\nfn one() -> Int {\n    1\n}\n",
    );
    let map = map_of(&baseline);
    let (other, one) = (id("calc.other"), id("calc.one"));
    assert_eq!(
        hash(&map, &other, Facet::Body),
        hash(&map, &one, Facet::Body)
    );
    let mut patch = body_patch("calc.other", &patch_text("other", "{\n    2\n}"));
    patch.expect = (one.clone(), Facet::Body);
    let grant = write(&[(other, Facet::Body), (one, Facet::Body)]);
    assert_eq!(
        codes(&validate_patch(&patch, &map, &map, Some(&grant))),
        ["patch_facet_unsupported"]
    );
}

#[test]
fn a_stale_patch_outside_the_write_set_reports_both_facts() {
    // Design: a patch is rejected if it escapes the write set *or* is
    // stale; validate_patch (1.9) reports each.
    let baseline = calc_tree();
    let current = edit(
        &baseline,
        CALC,
        "        invariant sum >= 0\n",
        "        invariant sum > -1\n",
    );
    let d = validate_patch(
        &total_patch(),
        &map_of(&baseline),
        &map_of(&current),
        Some(&write(&[(id("calc.other"), Facet::Body)])),
    );
    let mut seen = codes(&d);
    seen.sort_unstable();
    assert_eq!(seen, ["patch_out_of_scope", "patch_stale"]);
}

#[test]
fn a_patch_checked_against_its_own_result_is_stale() {
    // Applying a patch twice is refused: after the first application the
    // body is the patch's, not the baseline's.
    let baseline = calc_tree();
    let patched = apply(&baseline, &total_patch());
    let d = validate_patch(
        &total_patch(),
        &map_of(&baseline),
        &map_of(&patched),
        Some(&total_writer()),
    );
    assert_eq!(codes(&d), ["patch_stale"]);
    assert_eq!(d[0].data["actual"], json!(total_patch().body_hash()));
}

#[test]
fn a_dependent_baseline_includes_the_predecessor_output() {
    // Design line 413: "base refers to the immutable baseline assigned to
    // the submitting task, including predecessor outputs for dependent
    // tasks". The predecessor replaced the body; the dependent task's
    // patch expects the predecessor's body.
    let base = calc_tree();
    let predecessor = edit(&base, CALC, TOTAL_BODY, "{\n    0\n}");
    let dependent = map_of(&predecessor);
    let patch = total_patch();
    // Task-level validation: the tree being patched is the baseline (5.5).
    assert_eq!(
        validate_patch(&patch, &dependent, &dependent, Some(&total_writer())),
        vec![]
    );
    // Integration on a head that carries the predecessor's output and an
    // unrelated change.
    let head = edit(
        &predecessor,
        CALC,
        "fn other() -> Int {\n    1\n}",
        "fn other() -> Int {\n    3\n}",
    );
    assert_eq!(
        validate_patch(&patch, &dependent, &map_of(&head), Some(&total_writer())),
        vec![]
    );
    // ... and on one that lacks it.
    let without_predecessor = map_of(&base);
    let d = validate_patch(
        &patch,
        &dependent,
        &without_predecessor,
        Some(&total_writer()),
    );
    assert_eq!(codes(&d), ["patch_stale"]);
    assert_eq!(
        d[0].data["expected"],
        json!(hash(&dependent, &total(), Facet::Body))
    );
    assert_eq!(
        d[0].data["actual"],
        json!(hash(&without_predecessor, &total(), Facet::Body))
    );
}

#[test]
fn the_corpus_patch_is_a_byte_exact_no_op_on_the_complete_reader() {
    // The corpus body is already the patch's, in canonical layout: the
    // patch validates and applying it changes no byte of any file.
    let complete = reader_tree();
    let map = map_of(&complete);
    let patch = corpus_patch(&complete);
    assert_eq!(
        validate_patch(&patch, &map, &map, Some(&implementation_grant())),
        vec![]
    );
    assert_eq!(apply(&complete, &patch), complete);
}

#[test]
fn every_patch_validation_diagnostic_is_an_error_of_the_patch_category() {
    let tree = calc_tree();
    let map = map_of(&tree);
    let patch = total_patch();
    let mut all = Vec::new();
    let mut contract = patch.clone();
    contract.target.1 = Facet::Contract;
    contract.expect.1 = Facet::Contract;
    all.extend(validate_patch(&contract, &map, &map, None));
    let mut unknown = patch.clone();
    unknown.target.0 = id("fn_zzzzzzzzzzzzzzzz");
    unknown.expect.0 = id("fn_zzzzzzzzzzzzzzzz");
    all.extend(validate_patch(&unknown, &map, &map, None));
    let renumbered = map_of(&edit(
        &tree,
        CALC,
        "@id(\"fn_kkkkkkkkkkkkkkkk\")",
        "@id(\"fn_jjjjjjjjjjjjjjjj\")",
    ));
    all.extend(validate_patch(&patch, &map, &renumbered, None));
    let stale = map_of(&edit(
        &tree,
        CALC,
        "        invariant sum >= 0\n",
        "        invariant sum > -1\n",
    ));
    let preserve_only = Grant {
        preserve: [(total(), Facet::Body)].into(),
        ..Grant::default()
    };
    all.extend(validate_patch(&patch, &map, &stale, Some(&preserve_only)));
    let mut seen = codes(&all);
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(
        seen,
        [
            "patch_facet_unsupported",
            "patch_out_of_scope",
            "patch_preserved_facet",
            "patch_stale",
            "patch_target_missing",
            "patch_unknown_symbol",
        ]
    );
    for d in &all {
        assert_eq!(
            (d.category, d.severity),
            (Category::Patch, Severity::Error),
            "{}",
            d.code
        );
        // Semantics 9.2: `qualified_name` whenever `symbol` is present.
        assert_eq!(d.symbol.is_some(), d.qualified_name.is_some(), "{}", d.code);
    }
}

// ===========================================================================
// 10.5 rule 1: files other than <sources>/**/*.cpl
// ===========================================================================

#[test]
fn rule1_adding_changing_or_removing_the_lock_needs_project_lock() {
    let base = base();
    let locked = with(&base, "cpl.lock", "# lock\n");
    let relocked = with(&base, "cpl.lock", "# another lock\n");
    // (The lock is read, never parsed here: the facet maps are the base's.)
    for (before, after) in [(&base, &locked), (&locked, &relocked), (&locked, &base)] {
        for refused in [
            Grant::default(),
            files(&[FileSelector::Manifest, FileSelector::Work("app".into())]),
        ] {
            let e = check_files(&base, before, after, &refused);
            assert_eq!(violation_codes(&e), ["patch_out_of_scope"]);
            assert_eq!(violation_paths(&e), ["cpl.lock"]);
            let d = &e.violations[0];
            assert_eq!((d.category, d.severity), (Category::Patch, Severity::Error));
        }
        assert!(check_files(&base, before, after, &files(&[FileSelector::Lock])).accepted());
    }
}

#[test]
fn rule1_a_work_file_selector_matches_its_stem_exactly() {
    let base = base();
    let changed = edit(&base, WORK, "double.body", "noop.body");
    let removed = without(&base, WORK);
    let others = files(&[
        FileSelector::Work("other".into()),
        FileSelector::Manifest,
        FileSelector::Lock,
    ]);
    for candidate in [&changed, &removed] {
        let e = check_files(&base, &base, candidate, &others);
        assert_eq!(violation_paths(&e), [WORK]);
        assert!(check_files(
            &base,
            &base,
            candidate,
            &files(&[FileSelector::Work("app".into())])
        )
        .accepted());
    }
    let upper = with(&base, "work/App.work", WORK_TEXT);
    let e = check_files(
        &base,
        &base,
        &upper,
        &files(&[FileSelector::Work("app".into())]),
    );
    assert_eq!(violation_paths(&e), ["work/App.work"]);
    assert!(check_files(
        &base,
        &base,
        &upper,
        &files(&[FileSelector::Work("App".into())])
    )
    .accepted());
}

#[test]
fn rule1_a_work_file_in_a_subdirectory_is_any_other_path() {
    // Section 0: the source set holds the `*.work` files directly under
    // `work/`; T42's `work.<stem>` is `work/<stem>.work`.
    let base = base();
    let candidate = with(&base, "work/sub/app.work", WORK_TEXT);
    let everything = files(&[
        FileSelector::Manifest,
        FileSelector::Lock,
        FileSelector::Work("app".into()),
        FileSelector::Work("sub".into()),
    ]);
    let e = check_files(&base, &base, &candidate, &everything);
    assert_eq!(violation_codes(&e), ["patch_out_of_scope"]);
    assert_eq!(violation_paths(&e), ["work/sub/app.work"]);
}

#[test]
fn rule1_only_patch_files_directly_under_work_are_consumed() {
    let base = base();
    let patch = patch_text("double", "{\n    x\n}");
    let mut candidate = base.clone();
    for path in [
        "work/fix.patch",
        "work/a.patch",
        "work/fix.patch.orig",
        "src/fix.patch",
        "fix.patch",
    ] {
        candidate = with(&candidate, path, &patch);
    }
    candidate = with(&candidate, "work/fix.txt", "notes\n");
    let everything = files(&[
        FileSelector::Manifest,
        FileSelector::Lock,
        FileSelector::Work("app".into()),
        FileSelector::Work("fix".into()),
    ]);
    let e = check_files(&base, &base, &candidate, &everything);
    assert_eq!(
        violation_paths(&e),
        [
            "fix.patch",
            "src/fix.patch",
            "work/fix.patch.orig",
            "work/fix.txt"
        ]
    );
    assert_eq!(violation_codes(&e), ["patch_out_of_scope"; 4]);
    let consumed: BTreeSet<&str> = e.consumed.iter().map(String::as_str).collect();
    assert_eq!(consumed, BTreeSet::from(["work/a.patch", "work/fix.patch"]));
}

#[test]
fn rule1_cpl_files_outside_the_configured_sources_are_any_other_path() {
    let manifest = "[project]\nname = \"t\"\nedition = \"0.1\"\n\n[build]\nsources = [\"lib\"]\n";
    let base = with(
        &tree(&[(
            "lib/app/pages.cpl",
            "module app.pages\n\nfn noop() -> Int {\n    0\n}\n",
        )]),
        "cpl.toml",
        manifest,
    );
    let candidate = with(
        &with(
            &base,
            "src/app/stray.cpl",
            "module app.stray\n\nfn stray() -> Int {\n    1\n}\n",
        ),
        "lib/app/more.cpl",
        "module app.more\n\nfn more() -> Int {\n    2\n}\n",
    );
    // The new file under `lib` is a source the add namespace covers; the
    // one under `src` is outside the source set and no grant covers it.
    let e = check_in(&["lib"], &base, &candidate, &adding(&["app"]));
    assert_eq!(violation_codes(&e), ["patch_out_of_scope"]);
    assert_eq!(violation_paths(&e), ["src/app/stray.cpl"]);
}

#[test]
fn rule1_a_source_directory_matches_whole_path_segments() {
    let base = base();
    let candidate = with(
        &with(&base, "srcs/app/x.cpl", "module app.x\n"),
        "src2/x.cpl",
        "module x\n",
    );
    let e = check(&base, &candidate, &adding(&["app", "x"]));
    assert_eq!(violation_paths(&e), ["src2/x.cpl", "srcs/app/x.cpl"]);
    assert_eq!(violation_codes(&e), ["patch_out_of_scope"; 2]);
}

#[test]
fn rule1_manifest_and_lock_names_elsewhere_are_any_other_path() {
    let base = base();
    let candidate = with(
        &with(
            &with(&base, "src/cpl.toml", MANIFEST),
            "work/cpl.lock",
            "# lock\n",
        ),
        "docs/cpl.toml",
        MANIFEST,
    );
    let e = check_files(
        &base,
        &base,
        &candidate,
        &files(&[FileSelector::Manifest, FileSelector::Lock]),
    );
    assert_eq!(
        violation_paths(&e),
        ["docs/cpl.toml", "src/cpl.toml", "work/cpl.lock"]
    );
}

#[test]
fn rule1_a_consumed_patch_never_excuses_the_change_it_made() {
    // A `.patch` file is never a violation, but the body change it carries
    // into the candidate is judged by rules 2 and 5 like any other.
    let base = base();
    let candidate = with(
        &edit(
            &base,
            PAGES,
            "    // the work\n    helper(x) * 2\n",
            "    helper(x) + helper(x)\n",
        ),
        "work/double.patch",
        &patch_text("double", "{\n    helper(x) + helper(x)\n}"),
    );
    let granted = check(&base, &candidate, &write(&[(double(), Facet::Body)]));
    assert!(granted.accepted(), "{:?}", granted.violations);
    assert_eq!(granted.consumed, ["work/double.patch"]);
    let refused = check(&base, &candidate, &write(&[(noop(), Facet::Body)]));
    assert_eq!(refused.consumed, ["work/double.patch"]);
    assert_eq!(
        violation_codes(&refused),
        ["patch_out_of_scope", "patch_out_of_scope"]
    );
    assert_eq!(refused.violations[0].symbol, Some(double()));
    assert_eq!(refused.violations[0].facet, Some(Facet::Body));
    assert_eq!(violation_paths(&refused), [PAGES]);
}

// ===========================================================================
// 10.5 rule 2: facet-level statements
// ===========================================================================

#[test]
fn rule2_an_add_namespace_is_a_prefix_of_whole_segments() {
    // T43: `tests.app.*` covers `tests.app` and modules below it, never
    // `tests.application` ("prefix match on module path", by segment).
    let base = base();
    let new_file = "src/tests/application.cpl";
    let candidate = with(
        &base,
        new_file,
        "module tests.application\n\ntest \"applies\" {\n    assert true\n}\n",
    );
    let e = check(&base, &candidate, &adding(&["tests.app"]));
    assert_eq!(
        violation_codes(&e),
        ["patch_out_of_scope", "patch_out_of_scope"]
    );
    assert_eq!(
        e.violations[0].symbol,
        Some(id("tests.application.\"applies\""))
    );
    assert_eq!(violation_paths(&e), [new_file]);
    for covering in [
        &["tests"][..],
        &["tests.application"][..],
        &["tests.app", "tests.application"][..],
    ] {
        assert!(
            check(&base, &candidate, &adding(covering)).accepted(),
            "{covering:?}"
        );
    }
}

#[test]
fn rule2_add_does_not_authorize_editing_an_existing_test() {
    // Design line 353 and 10.3: "a task with add { tests.pagination.* }
    // cannot edit an existing acceptance test".
    let base = base();
    let candidate = edit(
        &edit(
            &base,
            TESTS,
            "    assert double(2) == 4\n",
            "    assert double(2) == 5\n",
        ),
        TESTS,
        "    assert double(1) == 2\n}\n",
        "    assert double(1) == 2\n}\n\ntest \"doubles three\" {\n    assert double(3) == 6\n}\n",
    );
    let e = check(&base, &candidate, &adding(&["tests.app"]));
    // The new test is covered and the file holds it (rule 5); the edited
    // test is not writable.
    assert_eq!(violation_codes(&e), ["patch_out_of_scope"]);
    let d = &e.violations[0];
    assert_eq!(d.symbol, Some(doubles_test()));
    assert_eq!(d.qualified_name.as_deref(), Some("tests.app.\"doubles\""));
    assert_eq!(d.facet, Some(Facet::Body));
}

#[test]
fn rule2_add_does_not_authorize_removing_an_existing_test() {
    let base = base();
    let candidate = edit(
        &base,
        TESTS,
        "test \"doubles\" {\n    assert double(2) == 4\n}\n\n",
        "",
    );
    let e = check(&base, &candidate, &adding(&["tests"]));
    // The removal (rule 2) and its file, which holds no granted statement
    // (rule 5).
    assert_eq!(
        violation_codes(&e),
        ["patch_out_of_scope", "patch_out_of_scope"]
    );
    assert_eq!(e.violations[0].symbol, Some(doubles_test()));
    assert_eq!(violation_paths(&e), [TESTS]);
    // Removing a test needs its name, body, and docs (5.1; no signature).
    assert!(check(
        &base,
        &candidate,
        &write_facets(&doubles_test(), &TEST_FACETS)
    )
    .accepted());
}

#[test]
fn rule2_renaming_a_test_without_an_id_removes_one_identity_and_adds_another() {
    // 4.3: provisional identities do not survive a rename, so the add
    // namespace covers the new name while the old test's removal needs
    // write authority over all of it.
    let base = base();
    let candidate = edit(
        &base,
        TESTS,
        "test \"doubles\" {",
        "test \"doubles numbers\" {",
    );
    let e = check(&base, &candidate, &adding(&["tests"]));
    assert_eq!(violation_codes(&e), ["patch_out_of_scope"]);
    assert_eq!(e.violations[0].symbol, Some(doubles_test()));
    let full = Grant {
        add: namespaces(&["tests"]),
        ..write_facets(&doubles_test(), &TEST_FACETS)
    };
    assert!(check(&base, &candidate, &full).accepted());
}

#[test]
fn rule2_renaming_a_test_with_an_id_needs_its_name() {
    let base = base();
    let candidate = edit(
        &base,
        TESTS,
        "test \"doubles one\" {",
        "test \"doubles the one\" {",
    );
    let e = check(&base, &candidate, &adding(&["tests"]));
    assert_eq!(
        violation_codes(&e),
        ["patch_out_of_scope", "patch_out_of_scope"]
    );
    assert_eq!(e.violations[0].symbol, Some(doubles_one()));
    assert_eq!(e.violations[0].facet, Some(Facet::Name));
    assert!(check(&base, &candidate, &write(&[(doubles_one(), Facet::Name)])).accepted());
}

#[test]
fn rule2_a_removal_needs_exactly_the_facets_its_kind_has() {
    // 5.1: a struct has identity, name, signature, and docs; identity is
    // never granted (10.3), so the other three suffice and none is spare.
    let base = base();
    let candidate = edit(
        &base,
        PAGES,
        "/// An item.\n@id(\"ty_aaaaaaaaaaaaaaaa\")\npub struct Item {\n    /// The key.\n    key: Int\n}\n\n",
        "",
    );
    let e = check(&base, &candidate, &write_facets(&item(), &DATA_FACETS));
    assert!(e.accepted(), "{:?}", e.violations);
    for missing in DATA_FACETS {
        let partial: Vec<Facet> = DATA_FACETS.into_iter().filter(|f| *f != missing).collect();
        let e = check(&base, &candidate, &write_facets(&item(), &partial));
        assert!(!e.accepted(), "without {missing}");
        assert_eq!(e.violations[0].symbol, Some(item()));
    }
}

#[test]
fn rule2_moving_a_struct_needs_its_writable_facets_only() {
    let item_text = "/// An item.\n@id(\"ty_aaaaaaaaaaaaaaaa\")\npub struct Item {\n    /// The key.\n    key: Int\n}\n\n";
    let base = base();
    let candidate = edit(
        &edit(&base, PAGES, item_text, ""),
        UTIL,
        "fn plain() -> Int {",
        &format!("{item_text}fn plain() -> Int {{"),
    );
    let e = check(&base, &candidate, &write_facets(&item(), &DATA_FACETS));
    assert!(e.accepted(), "{:?}", e.violations);
    assert_eq!(e.statements.len(), 1);
    assert_eq!(e.statements[0].statement, DiffStatementKind::Moved);
    let e = check(
        &base,
        &candidate,
        &write_facets(&item(), &[Facet::Signature, Facet::Docs]),
    );
    assert!(!e.accepted());
}

#[test]
fn rule2_a_kind_change_needs_removal_and_addition_authority() {
    // 9.1: a wrapper type that became a refinement changes its facet set,
    // so the diff is identity_removed plus identity_added (kind_changed),
    // and rule 2 judges each statement.
    let base = base();
    let candidate = edit(
        &base,
        PAGES,
        "pub type Size(Int)",
        "pub type Size = Int where self > 0",
    );
    let e = check(&base, &candidate, &Grant::default());
    let kinds: Vec<DiffStatementKind> = e.statements.iter().map(|s| s.statement).collect();
    assert_eq!(kinds.len(), 2);
    assert!(kinds.contains(&DiffStatementKind::IdentityRemoved));
    assert!(kinds.contains(&DiffStatementKind::IdentityAdded));
    let removal = write_facets(&size(), &DATA_FACETS);
    let addition = adding(&["app.pages"]);
    assert!(!check(&base, &candidate, &removal).accepted());
    assert!(!check(&base, &candidate, &addition).accepted());
    assert!(check(&base, &candidate, &union([removal, addition])).accepted());
}

#[test]
fn rule2_a_rename_that_rewrites_callers_needs_each_callers_body() {
    // 10.5: "Renames that rewrite callers produce body_changed statements
    // on each caller"; design: identity tracking never grants anything.
    let base = base();
    let candidate = edit(
        &edit(
            &edit(&base, UTIL, "pub fn helper(", "pub fn assist("),
            PAGES,
            "use app.util.{helper}",
            "use app.util.{assist}",
        ),
        PAGES,
        "    helper(x) * 2\n",
        "    assist(x) * 2\n",
    );
    let e = check(&base, &candidate, &write(&[(helper(), Facet::Name)]));
    // The caller's body (rule 2) and the caller's file, which holds no
    // granted statement (rule 5).
    assert_eq!(
        violation_codes(&e),
        ["patch_out_of_scope", "patch_out_of_scope"]
    );
    assert_eq!(e.violations[0].symbol, Some(double()));
    assert_eq!(e.violations[0].facet, Some(Facet::Body));
    assert_eq!(violation_paths(&e), [PAGES]);
    let both = write(&[(helper(), Facet::Name), (double(), Facet::Body)]);
    assert!(check(&base, &candidate, &both).accepted());
}

#[test]
fn rule2_reordering_an_effects_set_is_no_statement_but_still_a_text_change() {
    // D-162: the effects text is sorted, so a reordered set is no
    // statement; the file changed anyway and rule 5 needs a granted
    // statement in it, which write authority over `effects` alone cannot
    // supply.
    let base = base();
    let candidate = edit(
        &base,
        PAGES,
        "effects { s.get, s.put }",
        "effects { s.put, s.get }",
    );
    let e = check(&base, &candidate, &write(&[(fetch(), Facet::Effects)]));
    assert!(e.statements.is_empty());
    assert_eq!(violation_codes(&e), ["patch_out_of_scope"]);
    assert_eq!(violation_paths(&e), [PAGES]);
    let with_body = edit(&candidate, PAGES, "    s.get(k)\n", "    s.put(k)\n");
    assert!(check(&base, &with_body, &write(&[(fetch(), Facet::Body)])).accepted());
}

#[test]
fn rule2_incomparable_effect_sets_are_two_statements_on_effects() {
    // 9.1: both effect_bound_widened and effect_bound_narrowed appear when
    // the sets are incomparable; each needs (id, effects).
    let base = base();
    let candidate = edit(
        &base,
        PAGES,
        "effects { s.get, s.put }",
        "effects { s.get, s.del }",
    );
    let e = check(&base, &candidate, &write(&[(fetch(), Facet::Body)]));
    let on_effects = e
        .violations
        .iter()
        .filter(|d| d.symbol == Some(fetch()) && d.facet == Some(Facet::Effects))
        .count();
    assert_eq!(on_effects, 2);
    assert!(check(&base, &candidate, &write(&[(fetch(), Facet::Effects)])).accepted());
}

#[test]
fn rule2_a_field_doc_comment_is_docs_and_a_field_type_is_signature() {
    // D-161: member doc comments belong to `docs`, not `signature`.
    let base = base();
    let redocumented = edit(
        &base,
        PAGES,
        "    /// The key.\n",
        "    /// The lookup key.\n",
    );
    let e = check(&base, &redocumented, &write(&[(item(), Facet::Signature)]));
    assert!(!e.accepted());
    assert_eq!(e.violations[0].facet, Some(Facet::Docs));
    assert!(check(&base, &redocumented, &write(&[(item(), Facet::Docs)])).accepted());
    let retyped = edit(&base, PAGES, "    key: Int\n", "    key: Text\n");
    assert!(check(&base, &retyped, &write(&[(item(), Facet::Signature)])).accepted());
    assert!(!check(&base, &retyped, &write(&[(item(), Facet::Docs)])).accepted());
}

// ===========================================================================
// 10.5 rule 3: preserve
// ===========================================================================

#[test]
fn rule3_an_unwritable_preserved_change_is_reported_by_rules_2_3_and_5_in_order() {
    let base = base();
    let candidate = edit(
        &base,
        PAGES,
        "    requires x > 0\n",
        "    requires x >= 0\n",
    );
    let grant = Grant {
        preserve: [(double(), Facet::Contract)].into(),
        ..write(&[(double(), Facet::Body)])
    };
    let e = check(&base, &candidate, &grant);
    assert_eq!(
        violation_codes(&e),
        [
            "patch_out_of_scope",
            "patch_preserved_facet",
            "patch_out_of_scope"
        ]
    );
    let preserved = &e.violations[1];
    assert_eq!(preserved.symbol, Some(double()));
    assert_eq!(preserved.facet, Some(Facet::Contract));
    assert_eq!(
        (preserved.category, preserved.severity),
        (Category::Patch, Severity::Error)
    );
}

#[test]
fn rule3_preserve_holds_even_where_write_is_granted() {
    // Rule 3 runs over every preserved pair, whatever rule 2 granted.
    let base = base();
    let candidate = edit(
        &base,
        PAGES,
        "    requires x > 0\n",
        "    requires x >= 0\n",
    );
    let grant = Grant {
        preserve: [(double(), Facet::Contract)].into(),
        ..write(&[(double(), Facet::Contract)])
    };
    assert_eq!(
        violation_codes(&check(&base, &candidate, &grant)),
        ["patch_preserved_facet"]
    );
}

#[test]
fn rule3_comments_and_reordering_keep_preserved_hashes() {
    // 5.2 and D-162: comments are not facet text and the effects text is
    // sorted, so neither changes a preserved facet; the granted body
    // change carries the file (rule 5).
    let base = base();
    let candidate = edit(
        &edit(
            &edit(
                &base,
                PAGES,
                "    requires x > 0\n",
                "    requires x > 0 // positive only\n",
            ),
            PAGES,
            "effects { s.get, s.put }",
            "effects { s.put, s.get } // io",
        ),
        PAGES,
        "    helper(x) * 2\n",
        "    helper(x) * 3\n",
    );
    let grant = Grant {
        preserve: [
            (double(), Facet::Signature),
            (double(), Facet::Contract),
            (fetch(), Facet::Effects),
        ]
        .into(),
        ..write(&[(double(), Facet::Body)])
    };
    let e = check(&base, &candidate, &grant);
    assert!(e.accepted(), "{:?}", e.violations);
}

#[test]
fn rule3_the_corpus_implementation_task_accepts_the_corpus_body_and_nothing_more() {
    let hole = reader_hole_tree();
    let complete = reader_tree();
    let e = check(&hole, &complete, &implementation_grant());
    assert!(e.accepted(), "{:?}", e.violations);
    assert_eq!(e.statements.len(), 1);
    assert_eq!(e.statements[0].statement, DiffStatementKind::BodyChanged);
    assert_eq!(e.statements[0].id, pgn());
    // `work/pagination.patch` is in both trees, unchanged: not consumed.
    assert!(e.consumed.is_empty());
    // Also dropping `@prove`: a verification_policy change outside the
    // write set (rule 2) that the task must preserve (rule 3); the file
    // holds the granted body change (rule 5).
    let unproved = edit(
        &complete,
        READER_PAGES,
        "@id(\"pgn_7c2d\")\n@prove\n",
        "@id(\"pgn_7c2d\")\n",
    );
    let e = check(&hole, &unproved, &implementation_grant());
    assert_eq!(
        violation_codes(&e),
        ["patch_out_of_scope", "patch_preserved_facet"]
    );
    assert!(e
        .violations
        .iter()
        .all(|d| d.facet == Some(Facet::VerificationPolicy) && d.symbol == Some(pgn())));
}

#[test]
fn a_comment_only_patch_changes_no_facet_but_its_file_change_needs_a_granted_statement() {
    // 5.5 and rule 3 compare hashes, which exclude comments, so a patch
    // that only rewrites comments passes validate_patch even where the
    // body is preserved; the candidate it produces differs in non-facet
    // text only, which rule 5 refuses without a granted statement.
    let tree = calc_tree();
    let baseline = map_of(&tree);
    let commented = body_patch(
        TOTAL_ID,
        &patch_text(
            "total",
            &TOTAL_BODY.replace("    sum\n}", "    // the count\n    sum\n}"),
        ),
    );
    assert_eq!(
        commented.body_hash(),
        hash(&baseline, &total(), Facet::Body)
    );
    let preserving = Grant {
        preserve: [(total(), Facet::Body)].into(),
        ..total_writer()
    };
    assert_eq!(
        validate_patch(&commented, &baseline, &baseline, Some(&preserving)),
        vec![]
    );
    let patched = apply(&tree, &commented);
    let e = check(&tree, &patched, &preserving);
    assert!(e.statements.is_empty());
    assert_eq!(violation_codes(&e), ["patch_out_of_scope"]);
    assert_eq!(violation_paths(&e), [CALC]);
}

// ===========================================================================
// 10.5 rule 4: identity integrity
// ===========================================================================

#[test]
fn rule4_an_add_task_cannot_take_over_an_existing_test_by_giving_it_an_id() {
    // An existing test re-identified: its provisional identity is removed
    // (rule 2) and the change provisional -> @id lacks write authority over
    // its name (rule 4).
    let base = base();
    let candidate = edit(
        &base,
        TESTS,
        "test \"doubles\" {",
        "@id(\"test_bbbbbbbbbbbbbbbb\")\ntest \"doubles\" {",
    );
    let e = check(&base, &candidate, &adding(&["tests"]));
    assert_eq!(
        violation_codes(&e),
        ["patch_out_of_scope", "patch_identity_changed"]
    );
    let changed = &e.violations[1];
    assert_eq!(
        changed.qualified_name.as_deref(),
        Some("tests.app.\"doubles\"")
    );
    assert_eq!(
        (changed.category, changed.severity),
        (Category::Patch, Severity::Error)
    );
}

#[test]
fn rule4_a_new_file_of_an_existing_module_can_duplicate_a_provisional_identity() {
    // 3.7: a module's files share one namespace, so `fn plain` in a second
    // file of `app.util` has the provisional id `app.util.plain` again.
    let base = base();
    let candidate = with(
        &base,
        "src/app/util/more.cpl",
        "module app.util\n\nfn plain() -> Int {\n    2\n}\n",
    );
    let e = check(&base, &candidate, &adding(&["app"]));
    assert_eq!(violation_codes(&e), ["duplicate_identity"]);
    let d = &e.violations[0];
    assert_eq!(d.symbol, Some(plain()));
    // Semantics 9.4: duplicate_identity is a resolve error.
    assert_eq!(
        (d.category, d.severity),
        (Category::Resolve, Severity::Error)
    );
}

#[test]
fn rule4_a_copied_id_is_a_duplicate_whichever_occurrence_comes_first() {
    // 4.6: a copy keeps the copied @id line and is a duplicate; here the
    // copy precedes the original, so the facet map keeps the copy (5.4).
    let base = base();
    let candidate = edit(
        &base,
        PAGES,
        "@id(\"fn_bbbbbbbbbbbbbbbb\")\nfn noop() -> Int {",
        "@id(\"fn_bbbbbbbbbbbbbbbb\")\nfn noop_copy() -> Int {\n    0\n}\n\n@id(\"fn_bbbbbbbbbbbbbbbb\")\nfn noop() -> Int {",
    );
    let full = Grant {
        add: namespaces(&["app"]),
        ..write_facets(&noop(), &FN_FACETS)
    };
    let e = check(&base, &candidate, &full);
    assert_eq!(violation_codes(&e), ["duplicate_identity"]);
    assert_eq!(e.violations[0].symbol, Some(noop()));
}

#[test]
fn rule4_duplicates_already_in_the_baseline_still_reject_the_candidate() {
    // 10.5 step 4: "duplicates in candidate map -> violation", whoever
    // introduced them.
    let baseline = with(
        &base(),
        "src/app/dup.cpl",
        "module app.dup\n\n@id(\"fn_bbbbbbbbbbbbbbbb\")\nfn noop_again() -> Int {\n    0\n}\n",
    );
    let candidate = edit(
        &baseline,
        PAGES,
        "    helper(x) * 2\n",
        "    helper(x) * 3\n",
    );
    let e = check(&baseline, &candidate, &write(&[(double(), Facet::Body)]));
    assert_eq!(violation_codes(&e), ["duplicate_identity"]);
    assert_eq!(e.violations[0].symbol, Some(noop()));
}

// ===========================================================================
// 10.5 rule 5: non-facet text in .cpl files
// ===========================================================================

#[test]
fn rule5_converting_line_endings_alone_is_out_of_scope() {
    let base = base();
    let candidate = with(&base, UTIL, &UTIL_TEXT.replace('\n', "\r\n"));
    let everything = union([
        write_facets(&helper(), &FN_FACETS),
        write_facets(&plain(), &FN_FACETS),
    ]);
    let e = check(&base, &candidate, &everything);
    assert!(e.statements.is_empty());
    assert_eq!(violation_codes(&e), ["patch_out_of_scope"]);
    assert_eq!(violation_paths(&e), [UTIL]);
}

#[test]
fn rule5_module_doc_comments_and_header_comments_ride_on_granted_statements() {
    // Grammar 3.1: module doc comments precede the `module` header and are
    // not part of it, and a trailing comment does not change it, so the
    // header is unchanged.
    let base = base();
    let docs = edit(
        &base,
        UTIL,
        "module app.util\n",
        "/// Utilities.\nmodule app.util // shared\n",
    );
    let body_writer = write(&[(helper(), Facet::Body)]);
    let e = check(&base, &docs, &body_writer);
    assert_eq!(violation_paths(&e), [UTIL]);
    let with_body = edit(
        &docs,
        UTIL,
        "    x\n}\n\nfn plain",
        "    x + 0\n}\n\nfn plain",
    );
    let e = check(&base, &with_body, &body_writer);
    assert!(e.accepted(), "{:?}", e.violations);
}

#[test]
fn rule5_moving_a_declaration_between_files_of_one_module_is_out_of_scope() {
    // Within one module a move is no statement (9.1 `moved` needs another
    // module), so neither file holds a granted statement, whatever the
    // grant over the declaration.
    let twice_text =
        "@id(\"fn_eeeeeeeeeeeeeeee\")\npub fn twice(x: Int) -> Int {\n    x + x\n}\n\n";
    let base = base();
    let candidate = edit(
        &edit(&base, UTIL_EXTRA, twice_text, ""),
        UTIL,
        "fn plain() -> Int {",
        &format!("{twice_text}fn plain() -> Int {{"),
    );
    let e = check(&base, &candidate, &write_facets(&twice(), &FN_FACETS));
    assert!(e.statements.is_empty());
    assert_eq!(
        violation_codes(&e),
        ["patch_out_of_scope", "patch_out_of_scope"]
    );
    assert_eq!(violation_paths(&e), [UTIL, UTIL_EXTRA]);
}

#[test]
fn rule5_a_new_header_only_file_needs_an_add_namespace_over_its_module() {
    let base = base();
    let new_file = "src/tests/extra.cpl";
    let candidate = with(&base, new_file, "module tests.extra\n");
    assert!(check(&base, &candidate, &adding(&["tests"])).accepted());
    let e = check(&base, &candidate, &adding(&["app"]));
    assert_eq!(violation_codes(&e), ["patch_out_of_scope"]);
    assert_eq!(violation_paths(&e), [new_file]);
}

#[test]
fn rule5_a_new_file_of_an_existing_module_is_covered_by_the_add_namespace() {
    let base = base();
    let more = "src/tests/app/more.cpl";
    let candidate = with(
        &base,
        more,
        "module tests.app\n\ntest \"more\" {\n    assert true\n}\n",
    );
    assert!(check(&base, &candidate, &adding(&["tests.app"])).accepted());
    let e = check(&base, &candidate, &Grant::default());
    assert_eq!(
        violation_codes(&e),
        ["patch_out_of_scope", "patch_out_of_scope"]
    );
    assert_eq!(violation_paths(&e), [more]);
}

#[test]
fn rule5_a_granted_statement_covers_only_its_own_files() {
    let base = base();
    let candidate = edit(
        &edit(&base, PAGES, "    helper(x) * 2\n", "    helper(x) * 3\n"),
        UTIL,
        "fn plain() -> Int {",
        "// plain\nfn plain() -> Int {",
    );
    let e = check(&base, &candidate, &write(&[(double(), Facet::Body)]));
    assert_eq!(violation_codes(&e), ["patch_out_of_scope"]);
    assert_eq!(violation_paths(&e), [UTIL]);
}

#[test]
fn rule5_a_new_file_needs_an_add_namespace_even_for_a_granted_move() {
    // Reading of rule 5: a new file has no module header to keep
    // unchanged, so it passes only by the parenthesised alternative (its
    // module under an add namespace); creating a module is creation
    // (design: add permits creation in a namespace).
    let noop_text = "@id(\"fn_bbbbbbbbbbbbbbbb\")\nfn noop() -> Int {\n    0\n}\n\n";
    let misc = "src/app/misc.cpl";
    let base = base();
    let candidate = with(
        &edit(&base, PAGES, noop_text, ""),
        misc,
        "module app.misc\n\n@id(\"fn_bbbbbbbbbbbbbbbb\")\nfn noop() -> Int {\n    0\n}\n",
    );
    let full = write_facets(&noop(), &FN_FACETS);
    let e = check(&base, &candidate, &full);
    assert_eq!(violation_codes(&e), ["patch_out_of_scope"]);
    assert_eq!(violation_paths(&e), [misc]);
    let with_add = Grant {
        add: namespaces(&["app.misc"]),
        ..full
    };
    assert!(check(&base, &candidate, &with_add).accepted());
}

#[test]
fn rule5_removing_one_file_of_a_multi_file_module_needs_every_removal_granted() {
    let base = base();
    let candidate = without(&base, UTIL_EXTRA);
    let both = union([
        write_facets(&twice(), &FN_FACETS),
        write_facets(&spare(), &FN_FACETS),
    ]);
    assert!(check(&base, &candidate, &both).accepted());
    let e = check(&base, &candidate, &write_facets(&twice(), &FN_FACETS));
    assert_eq!(violation_codes(&e), ["patch_out_of_scope"]);
    assert_eq!(e.violations[0].symbol, Some(spare()));
}

#[test]
fn every_enforcement_violation_is_an_error() {
    let base = base();
    let candidate = with(
        &with(
            &edit(
                &edit(
                    &base,
                    PAGES,
                    "    requires x > 0\n",
                    "    requires x >= 0\n",
                ),
                UTIL,
                "module app.util\n",
                "module app.utils\n",
            ),
            "notes.txt",
            "notes\n",
        ),
        "src/app/util/more.cpl",
        "module app.util\n\nfn plain() -> Int {\n    2\n}\n",
    );
    let grant = Grant {
        preserve: [(double(), Facet::Contract)].into(),
        ..Grant::default()
    };
    let e = check(&base, &candidate, &grant);
    assert!(e.violations.len() >= 4, "{:?}", codes(&e.violations));
    for d in &e.violations {
        assert_eq!(d.severity, Severity::Error, "{}", d.code);
        if d.code.0.starts_with("patch_") {
            assert_eq!(d.category, Category::Patch, "{}", d.code);
        }
        assert_eq!(d.symbol.is_some(), d.qualified_name.is_some(), "{}", d.code);
    }
}

// ===========================================================================
// 10.5 patch application: byte-exactness
// ===========================================================================

#[test]
fn apply_formats_the_patch_block_whatever_its_indentation_in_the_patch_file() {
    // 10.5: the block is replaced with format_fragment(Block, Source), so
    // the patch file's own indentation and blank-line runs do not leak.
    let tree = tree(&[("src/m.cpl", "module m\n\nfn f(x: Int) -> Int {\n    x\n}\n")]);
    let patch = body_patch(
        "m.f",
        "patch f.body\n    expect base(f.body)\n{\n  let y = x + 1\n\n\n\tif y > 2 {\n\t\ty\n\t} else {\n      0\n  }\n}\n",
    );
    assert_eq!(
        text(&apply(&tree, &patch), "src/m.cpl"),
        "module m\n\nfn f(x: Int) -> Int {\n    let y = x + 1\n\n    if y > 2 {\n        y\n    } else {\n        0\n    }\n}\n"
    );
}

#[test]
fn apply_keeps_every_comment_inside_the_patch_block() {
    // 2.3 rules B, D, E and 2.4 rule 11: a comment after the patch's `{`
    // leads the first statement, a same-line comment trails its statement,
    // a dangling comment stays before `}`.
    let tree = tree(&[(
        "src/m.cpl",
        "module m\n\nfn f(x: Int) -> Int {\n    // old comment\n    x\n}\n",
    )]);
    let patch = body_patch(
        "m.f",
        "patch f.body\n    expect base(f.body)\n{ // on the brace line\n    // leading\n    let y = x + 1 // trailing\n\n    // before the tail\n    y\n    // dangling\n}\n",
    );
    let patched = apply(&tree, &patch);
    assert_eq!(
        text(&patched, "src/m.cpl"),
        "module m\n\nfn f(x: Int) -> Int {\n    // on the brace line\n    // leading\n    let y = x + 1 // trailing\n\n    // before the tail\n    y\n    // dangling\n}\n"
    );
    assert_eq!(
        hash(&map_of(&patched), &id("m.f"), Facet::Body),
        patch.body_hash()
    );
}

#[test]
fn apply_moves_a_comment_between_expect_and_the_brace_into_the_block() {
    // 2.3 rule C: a comment between a header's last clause and a `{` that
    // begins a new line is leading trivia of the first node inside the
    // braces, so it is part of the block that is applied.
    let tree = tree(&[("src/m.cpl", "module m\n\nfn f(x: Int) -> Int {\n    x\n}\n")]);
    let patch = body_patch(
        "m.f",
        "patch f.body\n    expect base(f.body)\n// moved inside\n{\n    x + 1\n}\n",
    );
    assert_eq!(
        text(&apply(&tree, &patch), "src/m.cpl"),
        "module m\n\nfn f(x: Int) -> Int {\n    // moved inside\n    x + 1\n}\n"
    );
}

#[test]
fn apply_leaves_out_comments_outside_the_patch_block() {
    // 2.3: a comment above the form leads the form (rule D), one after
    // the block's `}` trails the form (rule B; a block is not attachable),
    // one after the last form belongs to the file (rule F).
    let tree = tree(&[("src/m.cpl", "module m\n\nfn f(x: Int) -> Int {\n    x\n}\n")]);
    let patch = body_patch(
        "m.f",
        "// above the form\npatch f.body\n    expect base(f.body)\n{\n    x + 1\n} // after the block\n// at the end\n",
    );
    assert_eq!(
        text(&apply(&tree, &patch), "src/m.cpl"),
        "module m\n\nfn f(x: Int) -> Int {\n    x + 1\n}\n"
    );
}

#[test]
fn apply_keeps_the_comments_around_the_target_block_byte_for_byte() {
    // Only the bytes of the block, `{` to `}`, are replaced; a comment
    // between the clauses and `{` (attached inside the block by 2.3 rule
    // C) and one after `}` are other bytes and stay.
    let source = "module m\n\n// before the declaration\nfn f(x: Int) -> Int\n    requires x > 0 // positive\n// between the clauses and the body\n{\n    // replaced with the body\n    x\n} // after the body\n// after the declaration\n";
    let tree = tree(&[("src/m.cpl", source)]);
    let patch = body_patch("m.f", &patch_text("f", "{\n    x + 1\n}"));
    let patched = apply(&tree, &patch);
    assert_eq!(
        text(&patched, "src/m.cpl"),
        "module m\n\n// before the declaration\nfn f(x: Int) -> Int\n    requires x > 0 // positive\n// between the clauses and the body\n{\n    x + 1\n} // after the body\n// after the declaration\n"
    );
    assert_eq!(
        hash(&map_of(&patched), &id("m.f"), Facet::Body),
        patch.body_hash()
    );
}

#[test]
fn apply_keeps_an_lf_file_lf_whatever_the_patch_files_line_ends() {
    // 2.4 rule 1: the formatter's text has LF line ends, and an LF file
    // keeps them.
    let tree = tree(&[("src/m.cpl", "module m\n\nfn f(x: Int) -> Int {\n    x\n}\n")]);
    let crlf = "patch f.body\n    expect base(f.body)\n{\n    // leading\n    let y = x // copy\n\n    y\n    // dangling\n}\n".replace('\n', "\r\n");
    let patch = body_patch("m.f", &crlf);
    let patched = text(&apply(&tree, &patch), "src/m.cpl");
    // Comment text is kept "byte-for-byte from //" with trailing whitespace
    // dropped (2.4 rule 11), so no CR of the patch file survives.
    assert_eq!(
        patched,
        "module m\n\nfn f(x: Int) -> Int {\n    // leading\n    let y = x // copy\n\n    y\n    // dangling\n}\n"
    );
    // A CRLF patch file parses to the same body as its LF spelling.
    assert_eq!(
        patch.body_hash(),
        body_patch("m.f", &crlf.replace("\r\n", "\n")).body_hash()
    );
}

#[test]
fn apply_keeps_every_byte_of_a_crlf_file_outside_the_block() {
    // 10.5 says nothing about the terminators of the spliced block in a
    // CRLF file; whatever they are, every other byte is unchanged and the
    // block is the formatted patch body.
    let source = "module m\r\n\r\nfn f(x: Int) -> Int\r\n    requires x > 0\r\n{\r\n    x\r\n}\r\n\r\nfn g() -> Int {\r\n    0\r\n}\r\n";
    let tree = tree(&[("src/m.cpl", source)]);
    let patch = body_patch("m.f", &patch_text("f", "{\n    let y = x\n\n    y\n}"));
    let patched = apply(&tree, &patch);
    let after = text(&patched, "src/m.cpl");
    let prefix = "module m\r\n\r\nfn f(x: Int) -> Int\r\n    requires x > 0\r\n";
    let suffix = "\r\n\r\nfn g() -> Int {\r\n    0\r\n}\r\n";
    assert!(after.starts_with(prefix), "{after:?}");
    assert!(after.ends_with(suffix), "{after:?}");
    let block = &after[prefix.len()..after.len() - suffix.len()];
    assert_eq!(block.replace("\r\n", "\n"), "{\n    let y = x\n\n    y\n}");
    assert!(
        !block.replace("\r\n", "").contains('\r'),
        "no lone CR: {block:?}"
    );
    load(&patched);
}

#[test]
fn apply_patches_the_file_of_a_multi_file_module_that_declares_the_target() {
    // 3.7: `app.util` spans `util.cpl` and `util/extra.cpl`; the patch
    // touches only the file that declares the target, for an @id and for a
    // provisional identity alike.
    let base = base();
    for (target, name, block, expected) in [
        (
            "fn_eeeeeeeeeeeeeeee",
            "twice",
            "{\n    2 * x\n}",
            UTIL_EXTRA_TEXT.replace("    x + x\n", "    2 * x\n"),
        ),
        (
            "app.util.spare",
            "spare",
            "{\n    4\n}",
            UTIL_EXTRA_TEXT.replace("    3\n", "    4\n"),
        ),
    ] {
        let patched = apply(&base, &body_patch(target, &patch_text(name, block)));
        assert_eq!(text(&patched, UTIL_EXTRA), expected, "{target}");
        assert_eq!(
            without(&patched, UTIL_EXTRA),
            without(&base, UTIL_EXTRA),
            "{target}: every other file is byte-identical"
        );
    }
}

#[test]
fn apply_patches_the_first_occurrence_of_a_duplicated_identity() {
    // 10.5 is silent on duplicates; 5.4 and 4.5 make the first occurrence
    // in (file path, declaration order) the one that holds the identity,
    // and it is the body validate_patch compared.
    let a = "module a\n\n@id(\"fn_xxxxxxxxxxxxxxxx\")\nfn first() -> Int {\n    1\n}\n";
    let b = "module b\n\n@id(\"fn_xxxxxxxxxxxxxxxx\")\nfn second() -> Int {\n    1\n}\n";
    let two_files = tree(&[("src/b.cpl", b), ("src/a.cpl", a)]);
    let patched = apply(
        &two_files,
        &body_patch("fn_xxxxxxxxxxxxxxxx", &patch_text("first", "{\n    2\n}")),
    );
    assert_eq!(text(&patched, "src/a.cpl"), a.replace("    1\n", "    2\n"));
    assert_eq!(text(&patched, "src/b.cpl"), b);

    let one = "module c\n\n@id(\"fn_yyyyyyyyyyyyyyyy\")\nfn early() -> Int {\n    1\n}\n\n@id(\"fn_yyyyyyyyyyyyyyyy\")\nfn late() -> Int {\n    1\n}\n";
    let one_file = tree(&[("src/c.cpl", one)]);
    let patched = apply(
        &one_file,
        &body_patch("fn_yyyyyyyyyyyyyyyy", &patch_text("early", "{\n    2\n}")),
    );
    assert_eq!(
        text(&patched, "src/c.cpl"),
        one.replacen("    1\n", "    2\n", 1)
    );
}

#[test]
fn apply_replaces_the_body_of_a_generic_function_and_nothing_of_its_header() {
    // Design: the patch body is parsed in the target's generic scope; the
    // header (generics, parameters, clauses, docs, annotations) is other
    // bytes.
    let source = "module g\n\n/// Picks.\n@id(\"fn_gggggggggggggggg\")\n@prove\npub fn pick<T, U>(xs: List<T>, fallback: T, tag: U) -> T\n    ensures true\n{\n    fallback\n}\n";
    let tree = tree(&[("src/g.cpl", source)]);
    let patch = body_patch(
        "fn_gggggggggggggggg",
        "patch pick.body\n    expect base(pick.body)\n{\n    let kept: List<T> = xs.take(1)\n    let tags: List<U> = [tag]\n    fallback\n}\n",
    );
    let patched = apply(&tree, &patch);
    assert_eq!(
        text(&patched, "src/g.cpl"),
        "module g\n\n/// Picks.\n@id(\"fn_gggggggggggggggg\")\n@prove\npub fn pick<T, U>(xs: List<T>, fallback: T, tag: U) -> T\n    ensures true\n{\n    let kept: List<T> = xs.take(1)\n    let tags: List<U> = [tag]\n    fallback\n}\n"
    );
    let pick = id("fn_gggggggggggggggg");
    let (before, after) = (map_of(&tree), map_of(&patched));
    for facet in [
        Facet::Name,
        Facet::Signature,
        Facet::Contract,
        Facet::Effects,
        Facet::Docs,
        Facet::VerificationPolicy,
    ] {
        assert_eq!(
            hash(&before, &pick, facet),
            hash(&after, &pick, facet),
            "{facet}"
        );
    }
    assert_eq!(hash(&after, &pick, Facet::Body), patch.body_hash());
}

#[test]
fn apply_changes_only_the_target_among_identical_bodies() {
    let source = "module m\n\nfn a() -> Int {\n    1\n}\n\nfn b() -> Int {\n    1\n}\n";
    let tree = tree(&[("src/m.cpl", source)]);
    let patched = apply(&tree, &body_patch("m.b", &patch_text("b", "{\n    2\n}")));
    assert_eq!(
        text(&patched, "src/m.cpl"),
        "module m\n\nfn a() -> Int {\n    1\n}\n\nfn b() -> Int {\n    2\n}\n"
    );
}

#[test]
fn apply_replaces_an_oddly_formatted_body_entirely() {
    let source = "module m\n\nfn f(x: Int) -> Int\n    requires x > 0\n{\n\t\tx   +   1 // old\n\n\n        }\n\nfn g() -> Int {\n    0\n}\n";
    let tree = tree(&[("src/m.cpl", source)]);
    let patched = apply(&tree, &body_patch("m.f", &patch_text("f", "{\n    x\n}")));
    assert_eq!(
        text(&patched, "src/m.cpl"),
        "module m\n\nfn f(x: Int) -> Int\n    requires x > 0\n{\n    x\n}\n\nfn g() -> Int {\n    0\n}\n"
    );
}

#[test]
fn apply_reindents_without_trailing_whitespace_on_blank_lines() {
    // "Re-indented to the declaration's level" (10.5) with 2.4 rule 1:
    // lines have no trailing whitespace, so a blank line stays empty.
    let source = "module m\n\n    fn f() -> Int {\n        1\n    }\n";
    let tree = tree(&[("src/m.cpl", source)]);
    let patched = apply(
        &tree,
        &body_patch("m.f", &patch_text("f", "{\n    let a = 1\n\n    a\n}")),
    );
    assert_eq!(
        text(&patched, "src/m.cpl"),
        "module m\n\n    fn f() -> Int {\n        let a = 1\n\n        a\n    }\n"
    );
}

#[test]
fn apply_splices_at_byte_offsets_after_non_ascii_text() {
    let source = "module m\n\n// naïve café ✓\nfn f() -> Text {\n    \"héllo wörld\"\n}\n\nfn g() -> Text {\n    \"ünïcode\"\n}\n";
    let tree = tree(&[("src/m.cpl", source)]);
    let patched = apply(
        &tree,
        &body_patch("m.g", &patch_text("g", "{\n    \"ok ✓\"\n}")),
    );
    assert_eq!(
        text(&patched, "src/m.cpl"),
        source.replace("    \"ünïcode\"\n", "    \"ok ✓\"\n")
    );
}

#[test]
fn apply_keeps_a_leading_byte_order_mark() {
    // Grammar 1.1: a leading byte-order mark is skipped by the lexer; it
    // is not part of the block, so patch application leaves it in place.
    let source = "\u{feff}module m\n\nfn f() -> Int {\n    1\n}\n";
    let tree = tree(&[("src/m.cpl", source)]);
    let patched = apply(&tree, &body_patch("m.f", &patch_text("f", "{\n    2\n}")));
    assert_eq!(
        text(&patched, "src/m.cpl"),
        "\u{feff}module m\n\nfn f() -> Int {\n    2\n}\n"
    );
}

#[test]
fn apply_handles_empty_and_one_line_blocks_on_either_side() {
    // 2.4 rule 8: blocks are multi-line except the empty block `{}`.
    let empty = tree(&[("src/m.cpl", "module m\n\nfn f() -> Unit {}\n")]);
    let patched = apply(
        &empty,
        &body_patch("m.f", &patch_text("f", "{\n    assert true\n}")),
    );
    assert_eq!(
        text(&patched, "src/m.cpl"),
        "module m\n\nfn f() -> Unit {\n    assert true\n}\n"
    );
    let full = tree(&[(
        "src/m.cpl",
        "module m\n\nfn f() -> Unit {\n    assert true\n}\n",
    )]);
    let patched = apply(&full, &body_patch("m.f", &patch_text("f", "{}")));
    assert_eq!(
        text(&patched, "src/m.cpl"),
        "module m\n\nfn f() -> Unit {}\n"
    );
    let one_line = tree(&[("src/m.cpl", "module m\n\nfn f(x: Int) -> Int { x }\n")]);
    let patched = apply(
        &one_line,
        &body_patch("m.f", &patch_text("f", "{\n    x + 1\n}")),
    );
    assert_eq!(
        text(&patched, "src/m.cpl"),
        "module m\n\nfn f(x: Int) -> Int {\n    x + 1\n}\n"
    );
}

#[test]
fn apply_writes_text_literals_with_canonical_escapes() {
    // 10.5 applies format_fragment's output, which re-escapes text
    // literals canonically (2.4 rule 10): U+0009 as `\t`, `A` literally.
    let tree = tree(&[("src/m.cpl", "module m\n\nfn f() -> Text {\n    \"\"\n}\n")]);
    let patch = body_patch(
        "m.f",
        &patch_text("f", "{\n    \"tab\\u{9}end \\u{41}\"\n}"),
    );
    assert_eq!(
        text(&apply(&tree, &patch), "src/m.cpl"),
        "module m\n\nfn f() -> Text {\n    \"tab\\tend A\"\n}\n"
    );
}

#[test]
fn apply_two_forms_of_one_patch_file_in_sequence() {
    let source = "module m\n\nfn f() -> Int {\n    1\n}\n\nfn g() -> Int {\n    2\n}\n";
    let tree = tree(&[("src/m.cpl", source)]);
    let both = forms(
        "patch f.body\n    expect base(f.body)\n{\n    10\n}\n\npatch g.body\n    expect base(g.body)\n{\n    20\n}\n",
    );
    assert_eq!(both.len(), 2);
    let mut patched = tree.clone();
    for (form, target) in both.into_iter().zip(["m.f", "m.g"]) {
        let patch = ResolvedPatch {
            target: (id(target), Facet::Body),
            expect: (id(target), Facet::Body),
            body: form.body,
            span: form.span,
        };
        patched = apply(&patched, &patch);
    }
    assert_eq!(
        text(&patched, "src/m.cpl"),
        "module m\n\nfn f() -> Int {\n    10\n}\n\nfn g() -> Int {\n    20\n}\n"
    );
}

#[test]
fn apply_ignores_an_unrelated_file_that_does_not_parse() {
    let broken = "module broken\n\nfn x( {\n";
    let tree = tree(&[
        ("src/m.cpl", "module m\n\nfn f() -> Int {\n    1\n}\n"),
        ("src/broken.cpl", broken),
    ]);
    let patched = apply_patch(
        &tree,
        &load_lenient(&tree),
        &body_patch("m.f", &patch_text("f", "{\n    2\n}")),
    )
    .expect("the target's file parses");
    assert_eq!(
        text(&patched, "src/m.cpl"),
        "module m\n\nfn f() -> Int {\n    2\n}\n"
    );
    assert_eq!(text(&patched, "src/broken.cpl"), broken);
}

#[test]
fn patch_summary_records_the_baseline_hash_and_the_canonical_patch_hash() {
    // 10.1 PatchSummary { target, expected, applied }: the baseline's body
    // hash and the hash the patched body has, comments excluded (5.2).
    let tree = calc_tree();
    let baseline = map_of(&tree);
    let commented = body_patch(
        TOTAL_ID,
        &patch_text("total", "{\n    // just the length\n    xs.len // fast\n}"),
    );
    assert_eq!(commented.body_hash(), total_patch().body_hash());
    let summary = commented
        .summary(&baseline)
        .expect("the baseline has the body");
    assert_eq!(summary.target, (total(), Facet::Body));
    assert_eq!(summary.expected, hash(&baseline, &total(), Facet::Body));
    assert_eq!(summary.applied, commented.body_hash());
    let patched = apply(&tree, &commented);
    assert_eq!(
        hash(&map_of(&patched), &total(), Facet::Body),
        summary.applied
    );
    assert!(text(&patched, CALC).contains("{\n    // just the length\n    xs.len // fast\n}\n"));
}

// ===========================================================================
// 10.5: `patch_syntax`
// ===========================================================================

/// `parse_submitted_patch` on a file that does not parse: `patch_syntax`
/// (patch, error) whose one note carries the parse diagnostic, the first
/// and only one the parser reports (T6): its message and its span.
#[track_caller]
fn syntax_error(text: &str) -> Diagnostic {
    let file = FileId(3);
    let error = parse_submitted_patch(text, file).expect_err("does not parse");
    assert_eq!(error.code, "patch_syntax");
    assert_eq!(
        (error.category, error.severity),
        (Category::Patch, Severity::Error)
    );
    let parse = cpl_syntax::parse_patch(text, file);
    assert!(parse.value.is_none(), "cpl-syntax rejects it too");
    let first = parse.diagnostics.first().expect("a parse diagnostic");
    assert_eq!(error.notes.len(), 1, "{:?}", error.notes);
    assert!(
        error.notes[0].message.contains(&first.message),
        "{:?} carries {:?}",
        error.notes[0].message,
        first.message
    );
    assert_eq!(error.notes[0].span, first.span);
    error
}

#[test]
fn syntax_a_brace_on_the_expect_line_is_patch_syntax() {
    // Grammar 4.5: `expect` is a clause, so `{` must start the next line.
    syntax_error("patch f.body\n    expect base(f.body) {\n    1\n}\n");
}

#[test]
fn syntax_a_missing_expect_clause_is_patch_syntax() {
    // D39: `expect base(...)` is mandatory; grammar 2.4 rule 3 names the
    // message.
    let error = syntax_error("patch f.body\n{\n    1\n}\n");
    assert!(
        error.notes[0]
            .message
            .contains("expected `expect` on its own line"),
        "{}",
        error.notes[0].message
    );
}

#[test]
fn syntax_a_patch_file_holds_only_patch_forms() {
    // D40: no `module`, `use`, or declarations in a `.patch` file.
    for text in [
        "module m\n\npatch f.body\n    expect base(f.body)\n{\n    1\n}\n",
        "use m.{f}\n\npatch f.body\n    expect base(f.body)\n{\n    1\n}\n",
        "fn f() -> Int {\n    1\n}\n",
        "patch f.body\n    expect base(f.body)\n{\n    1\n}\n\nfn g() -> Int {\n    2\n}\n",
    ] {
        syntax_error(text);
    }
}

#[test]
fn syntax_an_empty_patch_file_is_patch_syntax() {
    // Grammar 4.5: `patch_file = patch_form ( NEWLINE patch_form )* ...`
    // requires at least one form.
    for text in ["", "\n", "// nothing to apply\n"] {
        syntax_error(text);
    }
}

#[test]
fn syntax_a_lexical_error_in_the_block_is_patch_syntax() {
    syntax_error("patch f.body\n    expect base(f.body)\n{\n    \"unterminated\n}\n");
    syntax_error("patch f.body\n    expect base(f.body)\n{\n    x; y\n}\n");
}

#[test]
fn syntax_only_the_first_parse_error_is_carried() {
    let text = "patch f.body\n{\n    1\n}\n\npatch g.body\n{\n    2\n}\n";
    let error = syntax_error(text);
    let span = error.notes[0].span.expect("the parse error's span");
    assert!(
        (span.start as usize) < text.find("patch g").expect("the second form"),
        "the first error, in the first form"
    );
}

#[test]
fn syntax_a_broken_later_form_rejects_the_whole_file() {
    // T6 and 10.5: the file yields no PatchFile, so nothing from it is
    // applied, not even its valid first form.
    syntax_error(
        "patch f.body\n    expect base(f.body)\n{\n    1\n}\n\npatch g.body\n    expect base(g.body) {\n    2\n}\n",
    );
}

#[test]
fn syntax_a_doc_comment_before_expect_is_patch_syntax() {
    // Grammar 1.2: a doc comment "before the expect line of a patch form"
    // is doc_comment_position.
    syntax_error("patch f.body\n    /// why\n    expect base(f.body)\n{\n    1\n}\n");
}

#[test]
fn syntax_doc_comments_before_patch_forms_are_accepted() {
    let both = forms(
        "/// Why f.\npatch f.body\n    expect base(f.body)\n{\n    1\n}\n\n/// Why g.\npatch g.body\n    expect base(g.body)\n{\n    2\n}\n",
    );
    assert_eq!(both.len(), 2);
    assert!(both.iter().all(|form| form.docs.is_some()));
}

#[test]
fn the_patch_codes_are_the_eight_of_appendix_b() {
    let spelled: Vec<&str> = cpl_work::codes::PATCH_CODES.iter().map(|c| c.0).collect();
    assert_eq!(
        spelled,
        [
            "patch_facet_unsupported",
            "patch_unknown_symbol",
            "patch_target_missing",
            "patch_stale",
            "patch_out_of_scope",
            "patch_preserved_facet",
            "patch_identity_changed",
            "patch_syntax",
        ]
    );
    assert_eq!(cpl_work::codes::PATCH, Category::Patch);
}

// ===========================================================================
// Further boundaries: the corpus tasks, identity assignment, odd bytes
// ===========================================================================

const READER_TESTS: &str = "src/tests/pagination.cpl";

/// The grant of `pagination.work`'s `examples` task: `add { tests.pagination.* }`.
fn examples_grant() -> Grant {
    adding(&["tests.pagination"])
}

#[test]
fn rule2_the_corpus_examples_task_may_add_tests_and_nothing_else() {
    // Design lines 330 and 353: `add { tests.pagination.* }` creates
    // boundary cases; existing acceptance tests stay read-only.
    let base = reader_tree();
    let added = edit(
        &base,
        READER_TESTS,
        "test \"pagination handles empty input\" {",
        "test \"pagination keeps a full final page\" {\n    assert paginate([1, 2], 2) == [[1, 2]]\n}\n\ntest \"pagination handles empty input\" {",
    );
    let e = check(&base, &added, &examples_grant());
    assert!(e.accepted(), "{:?}", e.violations);
    // A new module below the namespace (T43).
    let edge = with(
        &base,
        "src/tests/pagination/edge.cpl",
        "module tests.pagination.edge\n\nuse reader.pages.{paginate}\n\ntest \"pagination of one item\" {\n    assert paginate([1], 3) == [[1]]\n}\n",
    );
    assert!(check(&base, &edge, &examples_grant()).accepted());
    // An existing test's body, and the property's generators (its
    // `signature`, 5.1), even next to a granted addition.
    for (from, to) in [
        (
            "    assert paginate([1, 2, 3], 2) == [[1, 2], [3]]\n",
            "    assert true\n",
        ),
        (
            "    for size in gen.positive_int\n",
            "    for size in gen.int\n",
        ),
    ] {
        let edited = edit(&added, READER_TESTS, from, to);
        let e = check(&base, &edited, &examples_grant());
        assert_eq!(violation_codes(&e), ["patch_out_of_scope"], "{to}");
    }
}

#[test]
fn rule5_an_add_task_cannot_repoint_the_import_existing_tests_run_against() {
    // Design line 285: "A task allowed to add its own tests cannot change
    // ... unrelated acceptance tests"; line 353: add "does not permit
    // editing existing acceptance tests. Everything outside the granted
    // write and add sets is read-only." The task creates a look-alike
    // `paginate` in a module its namespace covers and repoints the existing
    // tests' `use` line at it: no facet of an existing test changes, yet
    // every one of them now runs against the look-alike (and `require
    // test(tests.pagination.*)` would pass on it). Toolchain 10.5 rule 5
    // accepts the `use` line because the file also holds a granted
    // statement (the added test); the README's precedence puts
    // design-doc.md first, so the candidate must be refused.
    let base = reader_tree();
    let candidate = edit(
        &edit(
            &with(
                &base,
                "src/tests/pagination/fake.cpl",
                "module tests.pagination.fake\n\npub fn paginate<T>(items: List<T>, size: Int) -> List<List<T>> {\n    [items]\n}\n",
            ),
            READER_TESTS,
            "use reader.pages.{paginate}",
            "use tests.pagination.fake.{paginate}",
        ),
        READER_TESTS,
        "property \"pagination preserves input\"",
        "test \"pagination of nothing\" {\n    assert true\n}\n\nproperty \"pagination preserves input\"",
    );
    let (before, after) = (map_of(&base), map_of(&candidate));
    for name in [
        "pagination preserves a short tail",
        "pagination handles empty input",
        "pagination preserves input",
    ] {
        let existing = id(&format!("tests.pagination.\"{name}\""));
        assert_eq!(
            before.get(&existing).map(|e| &e.version.definition),
            after.get(&existing).map(|e| &e.version.definition),
            "{name}: no facet of the existing test changed"
        );
    }
    let e = check(&base, &candidate, &examples_grant());
    assert!(
        !e.accepted(),
        "the existing acceptance tests now call tests.pagination.fake.paginate"
    );
}

#[test]
fn a_provisional_target_that_gained_an_id_in_the_head_is_target_missing() {
    // 5.5 step 3 with 4.3: the head no longer has the provisional identity
    // the patch names. T47 makes this the normal case after any other
    // integration: the integrator gives every declaration still lacking an
    // @id one.
    let baseline = calc_tree();
    let head = edit(
        &baseline,
        CALC,
        "fn other() -> Int {",
        "@id(\"fn_oooooooooooooooo\")\nfn other() -> Int {",
    );
    let patch = body_patch("calc.other", &patch_text("other", "{\n    2\n}"));
    assert_eq!(
        codes(&validate_patch(
            &patch,
            &map_of(&baseline),
            &map_of(&head),
            None
        )),
        ["patch_target_missing"]
    );
}

#[test]
fn an_unchanged_candidate_is_accepted_with_no_statement_and_nothing_consumed() {
    let base = base();
    let e = check(&base, &base, &Grant::default());
    assert!(e.accepted(), "{:?}", e.violations);
    assert!(e.statements.is_empty());
    assert!(e.consumed.is_empty());
}

#[test]
fn rule1_an_cpl_file_under_work_is_any_other_path() {
    // Section 0: only `*.work` and `*.patch` files directly under `work/`
    // and `.cpl` files under the source directories are in the source set.
    let base = base();
    let candidate = with(&base, "work/notes.cpl", "module notes\n");
    let everything = union([
        adding(&["notes"]),
        files(&[FileSelector::Work("notes".into())]),
    ]);
    let e = check(&base, &candidate, &everything);
    assert_eq!(violation_codes(&e), ["patch_out_of_scope"]);
    assert_eq!(violation_paths(&e), ["work/notes.cpl"]);
}

#[test]
fn rule5_a_byte_order_mark_or_crlf_line_ends_keep_the_module_header() {
    // Grammar 1.1: a leading byte-order mark is skipped and CRLF is one
    // terminator, so the header reads the same and a granted change
    // carries the file.
    for (name, util) in [
        ("bom", format!("\u{feff}{UTIL_TEXT}")),
        ("crlf", UTIL_TEXT.replace('\n', "\r\n")),
    ] {
        let baseline = with(&base(), UTIL, &util);
        let candidate = with(&baseline, UTIL, &util.replacen("    x", "    x + 0", 1));
        let e = check(&baseline, &candidate, &write(&[(helper(), Facet::Body)]));
        assert!(e.accepted(), "{name}: {:?}", e.violations);
        assert_eq!(e.statements.len(), 1, "{name}");
    }
}

#[test]
fn rule2_renaming_a_struct_is_a_name_change_only() {
    // D-160: the signature's canonical text replaces the name with `_`.
    let base = base();
    let candidate = edit(&base, PAGES, "pub struct Item {", "pub struct Entry {");
    let e = check(&base, &candidate, &write(&[(item(), Facet::Name)]));
    assert!(e.accepted(), "{:?}", e.violations);
    assert_eq!(e.statements.len(), 1);
    assert_eq!(
        e.statements[0].statement,
        DiffStatementKind::IdentityRenamed
    );
    assert!(!check(&base, &candidate, &write(&[(item(), Facet::Signature)])).accepted());
}

#[test]
fn apply_is_not_fooled_by_braces_in_text_literals_and_comments() {
    let source = "module m\n\nfn f() -> Text {\n    let s = \"}{\" // }\n    // {\n    s\n}\n\nfn g() -> Text {\n    \"}\"\n}\n";
    let tree = tree(&[("src/m.cpl", source)]);
    let patched = apply(
        &tree,
        &body_patch("m.f", &patch_text("f", "{\n    \"{\" // {\n}")),
    );
    assert_eq!(
        text(&patched, "src/m.cpl"),
        "module m\n\nfn f() -> Text {\n    \"{\" // {\n}\n\nfn g() -> Text {\n    \"}\"\n}\n"
    );
}

#[test]
fn apply_replaces_a_comment_after_the_targets_opening_brace() {
    // 2.3 rule D: a comment after `{` on a header's line leads the first
    // statement; its bytes lie inside the block, which the patch replaces.
    let tree = tree(&[(
        "src/m.cpl",
        "module m\n\nfn f() -> Int { // starts here\n    1\n}\n",
    )]);
    let patched = apply(&tree, &body_patch("m.f", &patch_text("f", "{\n    2\n}")));
    assert_eq!(
        text(&patched, "src/m.cpl"),
        "module m\n\nfn f() -> Int {\n    2\n}\n"
    );
}

#[test]
fn apply_replaces_exactly_the_block_under_every_corpus_header_shape() {
    // Broken parameter groups with `{` alone on its line, `using`
    // parameters, `effects` clauses, contract clauses, and a `match` body:
    // only the bytes of the block change.
    let reader = reader_tree();
    for (path, target, name, body) in [
        (
            READER_PAGES,
            "reader.pages.paginate_ints",
            "paginate_ints",
            "{\n    let checked_size = check<Positive>(size)?\n    Ok(paginate(items, checked_size))\n}",
        ),
        (
            READER_PAGES,
            "reader.pages.load_page",
            "load_page",
            "{\n    db.get(id)\n}",
        ),
        (
            READER_PAGES,
            "reader.pages.divide_exactly",
            "divide_exactly",
            "{\n    a / b\n}",
        ),
        (
            READER_PAGES,
            "reader.pages.label",
            "label",
            "{\n    match result {\n        Ok(page) => page.text\n        Err(Missing(_)) => \"Missing page\"\n        Err(Unavailable) => \"Storage unavailable\"\n    }\n}",
        ),
        (
            "src/reader/images.cpl",
            "reader.images.image_width",
            "image_width",
            "{\n    let dimensions = codec.dimensions(image)?\n    Ok(dimensions.width)\n}",
        ),
    ] {
        let original = text(&reader, path);
        assert_eq!(original.matches(body).count(), 1, "{target}");
        let patched = apply(
            &reader,
            &body_patch(target, &patch_text(name, "{\n    ?todo\n}")),
        );
        assert_eq!(
            text(&patched, path),
            original.replacen(body, "{\n    ?todo\n}", 1),
            "{target}"
        );
        assert_eq!(
            without(&patched, path),
            without(&reader, path),
            "{target}: every other file is byte-identical"
        );
    }
}

#[test]
fn apply_adds_no_final_line_end_to_a_file_that_has_none() {
    let tree = tree(&[("src/m.cpl", "module m\n\nfn f() -> Int {\n    1\n}")]);
    let patched = apply(&tree, &body_patch("m.f", &patch_text("f", "{\n    2\n}")));
    assert_eq!(
        text(&patched, "src/m.cpl"),
        "module m\n\nfn f() -> Int {\n    2\n}"
    );
}

#[test]
fn a_patch_on_a_standard_library_declaration_is_refused() {
    // 10.3 fixed exclusions: std. Std is never in the source set (Section
    // 0), so it is not in the baseline (patch_unknown_symbol) nor in the
    // tree being patched (patch_target_missing).
    let tree = calc_tree();
    let map = map_of(&tree);
    let flatten = "std.prelude.flatten";
    let patch = body_patch(flatten, &patch_text("flatten", "{\n    []\n}"));
    let grant = write(&[(id(flatten), Facet::Body)]);
    assert_eq!(
        codes(&validate_patch(&patch, &map, &map, Some(&grant))),
        ["patch_unknown_symbol"]
    );
    let error = apply_patch(&tree, &load(&tree), &patch).expect_err("std is not patched");
    assert_eq!(error.code, "patch_target_missing");
    assert_eq!(error.category, Category::Patch);
}

#[test]
fn the_corpus_audit_and_examples_tasks_cannot_patch_paginate() {
    // Design: "An audit observes exact predecessor outputs and emits a
    // review record, not a source edit" (`write {}`); `add` grants no
    // existing facet (10.3).
    let hole = reader_hole_tree();
    let map = map_of(&hole);
    let patch = corpus_patch(&hole);
    for grant in [Grant::default(), examples_grant()] {
        let d = validate_patch(&patch, &map, &map, Some(&grant));
        assert_eq!(codes(&d), ["patch_out_of_scope"]);
        assert_eq!(d[0].symbol, Some(pgn()));
        assert_eq!(
            d[0].qualified_name.as_deref(),
            Some("reader.pages.paginate")
        );
        assert_eq!(d[0].facet, Some(Facet::Body));
    }
    // The candidate the patch produces is refused by enforcement as well.
    let complete = reader_tree();
    for grant in [Grant::default(), examples_grant()] {
        let e = check(&hole, &complete, &grant);
        assert_eq!(
            violation_codes(&e),
            ["patch_out_of_scope", "patch_out_of_scope"]
        );
        assert_eq!(violation_paths(&e), [READER_PAGES]);
    }
}

#[test]
fn rule3_a_preserved_provisional_identity_cannot_be_escaped_by_renaming_or_re_identifying() {
    // Rule 3: "if hash differs between baseline and candidate"; a
    // preserved (id, facet) whose provisional id no longer exists has no
    // hash in the candidate, which differs. Full write and add authority
    // satisfy rules 2 and 4, not rule 3.
    let base = base();
    let full = Grant {
        add: namespaces(&["app"]),
        preserve: [(plain(), Facet::Contract)].into(),
        ..write_facets(&plain(), &FN_FACETS)
    };
    let renamed = edit(&base, UTIL, "fn plain() -> Int {", "fn plain2() -> Int {");
    let re_identified = edit(
        &base,
        UTIL,
        "fn plain() -> Int {",
        "@id(\"fn_hhhhhhhhhhhhhhhh\")\nfn plain() -> Int {",
    );
    for candidate in [&renamed, &re_identified] {
        let e = check(&base, candidate, &full);
        assert_eq!(violation_codes(&e), ["patch_preserved_facet"]);
        assert_eq!(e.violations[0].symbol, Some(plain()));
        assert_eq!(e.violations[0].facet, Some(Facet::Contract));
    }
}

#[test]
fn rule4_a_duplicate_that_sorts_first_cannot_take_over_an_identity() {
    // A new file whose path sorts before the original's declares the same
    // @id: the facet map keeps the newcomer (5.4), so the diff describes
    // it, and the duplicate itself is refused (step 4), whatever the add
    // namespace allows.
    let base = base();
    let candidate = with(
        &base,
        "src/app/aaa.cpl",
        "module app.aaa\n\n@id(\"fn_aaaaaaaaaaaaaaaa\")\nfn evil() -> Int {\n    0\n}\n",
    );
    let e = check(&base, &candidate, &adding(&["app"]));
    assert!(!e.accepted());
    assert!(violation_codes(&e).contains(&"duplicate_identity"));
    assert_eq!(
        map_of(&candidate)
            .get(&double())
            .map(|entry| entry.file.as_str()),
        Some("src/app/aaa.cpl")
    );
}
