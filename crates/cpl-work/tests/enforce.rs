//! Write/add/preserve enforcement over facet diffs (`spec/toolchain.md`
//! 10.5): at least one case per rule of the algorithm, over a small
//! project whose baseline and candidate trees differ in one way each.

mod common;

use common::*;
use cpl_diag::json::DiffStatementKind;
use cpl_diag::{Category, Facet, StableId};
use cpl_project::VirtualTree;
use cpl_work::{enforce, module_header, tree_diff, Enforcement, FileSelector, FileStatus, Grant};
use serde_json::json;

const PAGES: &str = "src/app/pages.cpl";
const UTIL: &str = "src/app/util.cpl";
const TESTS: &str = "src/tests/app.cpl";
const WORK: &str = "work/app.work";

const PAGES_TEXT: &str = r#"module app.pages

use app.util.{helper}

pub capability Store {
    fn get(k: Int) -> Int
    fn put(k: Int) -> Int
}

/// Doubles.
@id("fn_aaaaaaaaaaaaaaaa")
pub fn double(x: Int) -> Int
    requires x > 0
{
    // the work
    helper(x) * 2
}

@id("fn_bbbbbbbbbbbbbbbb")
fn noop() -> Int {
    0
}

@id("fn_dddddddddddddddd")
pub fn fetch(k: Int, using s: Store) -> Int
    effects { s.get }
{
    s.get(k)
}
"#;

const UTIL_TEXT: &str = r#"module app.util

pub fn helper(x: Int) -> Int {
    x
}
"#;

const TESTS_TEXT: &str = r#"module tests.app

use app.pages.{double}

test "doubles" {
    assert double(2) == 4
}
"#;

const WORK_TEXT: &str = r#"change c {
    base git("main")
    target git("main")

    task t {
        write { double.body }
    }
}
"#;

const NOOP: &str = "@id(\"fn_bbbbbbbbbbbbbbbb\")\nfn noop() -> Int {\n    0\n}\n\n";

fn base() -> VirtualTree {
    tree(&[
        (PAGES, PAGES_TEXT),
        (UTIL, UTIL_TEXT),
        (TESTS, TESTS_TEXT),
        (WORK, WORK_TEXT),
    ])
}

/// `tree` with `from` replaced by `to` in the file at `path`.
fn edit(tree: &VirtualTree, path: &str, from: &str, to: &str) -> VirtualTree {
    let text = text(tree, path);
    assert!(text.contains(from), "{path} contains {from:?}");
    with_file(tree, path, &text.replacen(from, to, 1))
}

fn id(text: &str) -> StableId {
    StableId::parse(text).expect("a stable id")
}

fn double() -> StableId {
    id("fn_aaaaaaaaaaaaaaaa")
}

fn noop() -> StableId {
    id("fn_bbbbbbbbbbbbbbbb")
}

fn fetch() -> StableId {
    id("fn_dddddddddddddddd")
}

/// Every facet of a `fn` but `identity`.
const FN_FACETS: [Facet; 7] = [
    Facet::Name,
    Facet::Signature,
    Facet::Contract,
    Facet::Effects,
    Facet::Body,
    Facet::Docs,
    Facet::VerificationPolicy,
];

fn write(pairs: &[(StableId, Facet)]) -> Grant {
    Grant {
        write: pairs.iter().cloned().collect(),
        ..Grant::default()
    }
}

fn write_all(target: &StableId) -> Grant {
    write(&FN_FACETS.map(|f| (target.clone(), f)))
}

fn add(namespaces: &[&[&str]]) -> Vec<Vec<String>> {
    namespaces
        .iter()
        .map(|ns| ns.iter().map(|s| s.to_string()).collect())
        .collect()
}

/// Enforce `candidate` against `baseline` with the default `src` sources.
fn check(baseline: &VirtualTree, candidate: &VirtualTree, grant: &Grant) -> Enforcement {
    let diff = tree_diff(baseline, candidate, &["src".to_string()]);
    enforce(&map_of(baseline), &map_of(candidate), &diff, grant)
}

fn violation_codes(e: &Enforcement) -> Vec<&'static str> {
    codes(&e.violations)
}

fn statements(e: &Enforcement) -> Vec<String> {
    e.violations
        .iter()
        .filter_map(|d| {
            d.data
                .get("statement")
                .and_then(|s| s.as_str())
                .map(str::to_string)
        })
        .collect()
}

fn paths(e: &Enforcement) -> Vec<String> {
    e.violations
        .iter()
        .filter_map(|d| {
            d.data
                .get("path")
                .and_then(|s| s.as_str())
                .map(str::to_string)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The tree diff
// ---------------------------------------------------------------------------

#[test]
fn tree_diff_lists_changed_paths_with_module_headers() {
    let baseline = base();
    let candidate = without_file(
        &with_file(
            &edit(&baseline, UTIL, "    x\n", "    x + 0\n"),
            "src/new.cpl",
            "module app.new\n",
        ),
        WORK,
    );
    let diff = tree_diff(&baseline, &candidate, &["src".to_string()]);
    assert_eq!(diff.sources, ["src"]);
    let summary: Vec<(&str, FileStatus, Option<String>, Option<String>)> = diff
        .changes
        .iter()
        .map(|c| {
            (
                c.path.as_str(),
                c.status,
                c.old_module.as_ref().map(|m| m.join(".")),
                c.new_module.as_ref().map(|m| m.join(".")),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (
                "src/app/util.cpl",
                FileStatus::Modified,
                Some("app.util".into()),
                Some("app.util".into())
            ),
            (
                "src/new.cpl",
                FileStatus::Added,
                None,
                Some("app.new".into())
            ),
            ("work/app.work", FileStatus::Removed, None, None),
        ]
    );
    assert!(tree_diff(&baseline, &baseline, &[]).changes.is_empty());
}

#[test]
fn module_headers_are_read_with_the_lexer() {
    let header = |text: &str| module_header(text.as_bytes()).map(|m| m.join("."));
    assert_eq!(header("module a.b.c\n").as_deref(), Some("a.b.c"));
    assert_eq!(
        header("/// Docs.\n/// More.\nmodule a\n\nfn f(").as_deref(),
        Some("a")
    );
    assert_eq!(
        header("module a.b\n\nfn f( {{{ broken").as_deref(),
        Some("a.b")
    );
    assert_eq!(header("fn f() -> Int {\n    1\n}\n"), None);
    assert_eq!(header("module\n"), None);
    assert_eq!(header("module A.b\n"), None);
    assert_eq!(module_header(&[0xff, 0xfe]), None);
}

// ---------------------------------------------------------------------------
// Rule 1: files other than <sources>/**/*.cpl
// ---------------------------------------------------------------------------

const PATCH_TEXT: &str = "patch double.body\n    expect base(double.body)\n{\n    x\n}\n";

#[test]
fn rule1_patch_files_are_consumed_never_violations() {
    let baseline = base();
    let added = with_file(&baseline, "work/fix.patch", PATCH_TEXT);
    let e = check(&baseline, &added, &Grant::default());
    assert!(e.accepted(), "{:?}", e.violations);
    assert_eq!(e.consumed, ["work/fix.patch"]);

    let with_patch = with_file(&baseline, "work/old.patch", PATCH_TEXT);
    let changed = edit(&with_patch, "work/old.patch", "    x\n", "    x + x\n");
    let e = check(&with_patch, &changed, &Grant::default());
    assert!(e.accepted());
    assert_eq!(e.consumed, ["work/old.patch"]);

    let removed = without_file(&with_patch, "work/old.patch");
    let e = check(&with_patch, &removed, &Grant::default());
    assert!(e.accepted());
    assert!(e.consumed.is_empty());
}

#[test]
fn rule1_project_files_need_their_file_selector() {
    let baseline = base();
    let manifest = with_file(
        &baseline,
        "cpl.toml",
        "[project]\nname = \"t\"\nedition = \"0.1\"\ndescription = \"changed\"\n",
    );
    let e = check(&baseline, &manifest, &Grant::default());
    assert_eq!(violation_codes(&e), ["patch_out_of_scope"]);
    assert_eq!(paths(&e), ["cpl.toml"]);
    let granted = Grant {
        files: [FileSelector::Manifest].into(),
        ..Grant::default()
    };
    assert!(check(&baseline, &manifest, &granted).accepted());

    // `cpl.lock` and a `.work` file; the facet maps do not change.
    let map = map_of(&baseline);
    let lock = with_file(&baseline, "cpl.lock", "# a lock\n");
    let work = edit(&baseline, WORK, "double.body", "noop.body");
    let new_work = with_file(&baseline, "work/other.work", WORK_TEXT);
    let sources = ["src".to_string()];
    for (candidate, selector) in [
        (&lock, FileSelector::Lock),
        (&work, FileSelector::Work("app".into())),
        (&new_work, FileSelector::Work("other".into())),
    ] {
        let diff = tree_diff(&baseline, candidate, &sources);
        let refused = enforce(&map, &map, &diff, &Grant::default());
        assert_eq!(
            violation_codes(&refused),
            ["patch_out_of_scope"],
            "{selector}"
        );
        assert_eq!(paths(&refused), [selector.path()]);
        let granted = Grant {
            files: [selector.clone()].into(),
            ..Grant::default()
        };
        assert!(
            enforce(&map, &map, &diff, &granted).accepted(),
            "{selector}"
        );
    }
}

#[test]
fn rule1_other_paths_are_out_of_scope() {
    let baseline = base();
    let candidate = with_file(
        &with_file(
            &with_file(&baseline, "src/notes.txt", "notes\n"),
            "docs/guide.cpl",
            "module docs.guide\n",
        ),
        "work/nested/x.patch",
        PATCH_TEXT,
    );
    let everything = Grant {
        files: [FileSelector::Manifest, FileSelector::Lock].into(),
        add: add(&[&[]]),
        ..Grant::default()
    };
    let e = check(&baseline, &candidate, &everything);
    assert_eq!(violation_codes(&e), ["patch_out_of_scope"; 3]);
    assert_eq!(
        paths(&e),
        ["docs/guide.cpl", "src/notes.txt", "work/nested/x.patch"]
    );
    assert!(e.consumed.is_empty());
}

// ---------------------------------------------------------------------------
// Rule 2: facet-level statements
// ---------------------------------------------------------------------------

#[test]
fn rule2_identity_added_needs_an_add_namespace() {
    let baseline = base();
    let candidate = edit(
        &baseline,
        TESTS,
        "    assert double(2) == 4\n}\n",
        "    assert double(2) == 4\n}\n\ntest \"doubles again\" {\n    assert double(3) == 6\n}\n",
    );
    let e = check(&baseline, &candidate, &Grant::default());
    assert_eq!(statements(&e), ["identity_added"]);
    assert_eq!(e.violations[0].data["module"], json!("tests.app"));
    assert_eq!(
        e.violations[0].qualified_name.as_deref(),
        Some("tests.app.\"doubles again\"")
    );
    // ... and the file now holds text no granted statement covers (rule 5).
    assert_eq!(paths(&e), [TESTS]);
    for covering in [add(&[&["tests"]]), add(&[&["tests", "app"]]), add(&[&[]])] {
        let grant = Grant {
            add: covering,
            ..Grant::default()
        };
        assert!(check(&baseline, &candidate, &grant).accepted());
    }
    for narrower in [
        add(&[&["tests", "app", "edge"]]),
        add(&[&["test"]]),
        add(&[&["app"]]),
    ] {
        let grant = Grant {
            add: narrower,
            ..Grant::default()
        };
        assert_eq!(
            statements(&check(&baseline, &candidate, &grant)),
            ["identity_added"]
        );
    }
}

#[test]
fn rule2_identity_removed_needs_every_facet() {
    let baseline = base();
    let candidate = edit(&baseline, PAGES, NOOP, "");
    let mut partial = write_all(&noop());
    partial.write.remove(&(noop(), Facet::Docs));
    let e = check(&baseline, &candidate, &partial);
    assert_eq!(statements(&e), ["identity_removed"]);
    assert_eq!(e.violations[0].symbol, Some(noop()));
    assert_eq!(paths(&e), [PAGES]);
    assert!(check(&baseline, &candidate, &write_all(&noop())).accepted());
}

#[test]
fn rule2_a_renaming_kind_change_names_the_removed_declaration() {
    // 9.1: both statements of a kind change are at the id's new path (the
    // fn `noop` became the test "no op"), but the refused removal names
    // the declaration it removes by the baseline's path.
    let baseline = base();
    let as_test = "@id(\"fn_bbbbbbbbbbbbbbbb\")\ntest \"no op\" {\n    assert true\n}\n\n";
    let candidate = edit(&baseline, PAGES, NOOP, as_test);
    let e = check(&baseline, &candidate, &Grant::default());
    let removal = e
        .violations
        .iter()
        .find(|d| d.data.get("statement") == Some(&json!("identity_removed")))
        .expect("the removal is refused");
    assert_eq!(removal.symbol, Some(noop()));
    assert_eq!(
        removal.qualified_name.as_deref(),
        Some("app.pages.\"no op\"")
    );
    assert!(
        removal.message.contains("removing `app.pages.noop`"),
        "{}",
        removal.message
    );
}

#[test]
fn rule2_identity_renamed_needs_name() {
    let baseline = base();
    let candidate = edit(&baseline, PAGES, "fn noop()", "fn nothing()");
    let e = check(&baseline, &candidate, &write(&[(noop(), Facet::Body)]));
    assert_eq!(statements(&e), ["identity_renamed"]);
    assert_eq!(e.violations[0].facet, Some(Facet::Name));
    let renamed = check(&baseline, &candidate, &write(&[(noop(), Facet::Name)]));
    assert!(renamed.accepted(), "{:?}", renamed.violations);
    assert_eq!(renamed.statements.len(), 1);
    assert_eq!(
        renamed.statements[0].statement,
        DiffStatementKind::IdentityRenamed
    );
}

#[test]
fn rule2_moved_needs_every_facet() {
    let baseline = base();
    let candidate = edit(
        &edit(&baseline, PAGES, NOOP, ""),
        UTIL,
        "pub fn helper",
        &format!("{NOOP}pub fn helper"),
    );
    let e = check(
        &baseline,
        &candidate,
        &write(&[(noop(), Facet::Name), (noop(), Facet::Body)]),
    );
    assert_eq!(statements(&e), ["moved"]);
    // Both files hold the move, which is not granted.
    assert_eq!(paths(&e), [PAGES, UTIL]);
    let moved = check(&baseline, &candidate, &write_all(&noop()));
    assert!(moved.accepted(), "{:?}", moved.violations);
    assert_eq!(moved.statements[0].statement, DiffStatementKind::Moved);
}

#[test]
fn rule2_facet_statements_need_their_facet() {
    let baseline = base();
    let candidate = edit(
        &edit(
            &edit(
                &edit(
                    &edit(
                        &baseline,
                        PAGES,
                        "/// Doubles.\n",
                        "/// Doubles its input.\n",
                    ),
                    PAGES,
                    "@id(\"fn_aaaaaaaaaaaaaaaa\")\npub fn double",
                    "@id(\"fn_aaaaaaaaaaaaaaaa\")\n@prove\nfn double",
                ),
                PAGES,
                "requires x > 0",
                "requires x >= 0",
            ),
            PAGES,
            "    helper(x) * 2\n",
            "    helper(x) + helper(x)\n",
        ),
        PAGES,
        "effects { s.get }",
        "effects { s.get, s.put }",
    );
    let e = check(&baseline, &candidate, &Grant::default());
    assert_eq!(
        statements(&e),
        [
            "signature_changed",
            "contract_changed",
            "body_changed",
            "docs_changed",
            "verification_policy_changed",
            "effect_bound_widened",
        ]
    );
    let facets: Vec<Option<Facet>> = e.violations.iter().map(|d| d.facet).take(6).collect();
    assert_eq!(
        facets,
        [
            Some(Facet::Signature),
            Some(Facet::Contract),
            Some(Facet::Body),
            Some(Facet::Docs),
            Some(Facet::VerificationPolicy),
            Some(Facet::Effects),
        ]
    );
    assert_eq!(paths(&e), [PAGES]);
    let grant = write(&[
        (double(), Facet::Signature),
        (double(), Facet::Contract),
        (double(), Facet::Body),
        (double(), Facet::Docs),
        (double(), Facet::VerificationPolicy),
        (fetch(), Facet::Effects),
    ]);
    let granted = check(&baseline, &candidate, &grant);
    assert!(granted.accepted(), "{:?}", granted.violations);

    // A narrowed bound needs `effects` too.
    let narrowed = edit(&baseline, PAGES, "effects { s.get }", "effects {}");
    let e = check(&baseline, &narrowed, &write(&[(fetch(), Facet::Body)]));
    assert_eq!(statements(&e), ["effect_bound_narrowed"]);
    assert!(check(&baseline, &narrowed, &write(&[(fetch(), Facet::Effects)])).accepted());
}

// ---------------------------------------------------------------------------
// Rule 3: preserve
// ---------------------------------------------------------------------------

#[test]
fn rule3_preserved_facets_keep_their_baseline_hash() {
    let baseline = base();
    let preserve_contract = |mut grant: Grant| {
        grant.preserve = [(double(), Facet::Signature), (double(), Facet::Contract)].into();
        grant
    };
    let body = edit(
        &baseline,
        PAGES,
        "    helper(x) * 2\n",
        "    helper(x) * 3\n",
    );
    assert!(check(
        &baseline,
        &body,
        &preserve_contract(write(&[(double(), Facet::Body)]))
    )
    .accepted());

    let contract = edit(&body, PAGES, "requires x > 0", "requires x > 1");
    let grant = preserve_contract(write(&[
        (double(), Facet::Body),
        (double(), Facet::Contract),
    ]));
    let e = check(&baseline, &contract, &grant);
    assert_eq!(violation_codes(&e), ["patch_preserved_facet"]);
    let d = &e.violations[0];
    assert_eq!(d.facet, Some(Facet::Contract));
    assert_eq!(d.qualified_name.as_deref(), Some("app.pages.double"));
    let map = map_of(&baseline);
    assert_eq!(
        d.data["expected"],
        json!(map.get(&double()).and_then(|e| e.facet(Facet::Contract)))
    );

    // A removed declaration keeps none of its preserved facets.
    let removed = edit(
        &baseline,
        PAGES,
        "/// Doubles.\n@id(\"fn_aaaaaaaaaaaaaaaa\")\npub fn double(x: Int) -> Int\n    requires x > 0\n{\n    // the work\n    helper(x) * 2\n}\n\n",
        "",
    );
    let e = check(
        &baseline,
        &removed,
        &preserve_contract(write_all(&double())),
    );
    assert_eq!(
        violation_codes(&e),
        ["patch_preserved_facet", "patch_preserved_facet"]
    );
    assert_eq!(e.violations[0].data["actual"], json!(null));
}

// ---------------------------------------------------------------------------
// Rule 4: identity integrity
// ---------------------------------------------------------------------------

#[test]
fn rule4_identity_change_needs_write_authority_over_name() {
    let provisional = id("app.util.helper");
    let assigned = id("fn_eeeeeeeeeeeeeeee");
    let baseline = base();
    let candidate = edit(
        &baseline,
        UTIL,
        "pub fn helper",
        "@id(\"fn_eeeeeeeeeeeeeeee\")\npub fn helper",
    );
    // With authority over the whole declaration and its module, the
    // identity may become an `@id`.
    let full = Grant {
        add: add(&[&["app"]]),
        ..write_all(&provisional)
    };
    assert!(check(&baseline, &candidate, &full).accepted());
    // Without `name`, step 2 refuses the removal and step 4 the change.
    let add_only = Grant {
        add: add(&[&["app"]]),
        ..Grant::default()
    };
    let e = check(&baseline, &candidate, &add_only);
    assert_eq!(
        violation_codes(&e),
        ["patch_out_of_scope", "patch_identity_changed"]
    );
    let d = &e.violations[1];
    assert_eq!(d.symbol, Some(assigned.clone()));
    assert_eq!(d.facet, Some(Facet::Identity));
    assert_eq!(d.data["previous"], json!("app.util.helper"));
    // ... and the other way round: an `@id` removed.
    let e = check(&candidate, &baseline, &add_only);
    assert!(violation_codes(&e).contains(&"patch_identity_changed"));
    let full = Grant {
        add: add(&[&["app"]]),
        ..write_all(&assigned)
    };
    assert!(check(&candidate, &baseline, &full).accepted());
}

#[test]
fn rule4_a_changed_id_value_is_never_granted() {
    let baseline = base();
    let candidate = edit(
        &baseline,
        PAGES,
        "fn_bbbbbbbbbbbbbbbb",
        "fn_ffffffffffffffff",
    );
    let grant = Grant {
        add: add(&[&[]]),
        ..write_all(&noop())
    };
    let e = check(&baseline, &candidate, &grant);
    assert_eq!(violation_codes(&e), ["patch_identity_changed"]);
    assert_eq!(
        e.violations[0].data["previous"],
        json!("fn_bbbbbbbbbbbbbbbb")
    );
}

#[test]
fn rule4_duplicate_identities_are_rejected() {
    let baseline = base();
    let candidate = edit(
        &baseline,
        UTIL,
        "pub fn helper",
        "@id(\"fn_bbbbbbbbbbbbbbbb\")\nfn noop_copy() -> Int {\n    0\n}\n\npub fn helper",
    );
    let e = check(&baseline, &candidate, &Grant::default());
    let duplicate = e
        .violations
        .iter()
        .find(|d| d.code == "duplicate_identity")
        .expect("a duplicate_identity violation");
    assert_eq!(duplicate.category, Category::Resolve);
    assert_eq!(duplicate.symbol, Some(noop()));
    assert_eq!(duplicate.data["files"], json!([PAGES, UTIL]));
}

// ---------------------------------------------------------------------------
// Rule 5: non-facet text in .cpl files
// ---------------------------------------------------------------------------

#[test]
fn rule5_comments_and_use_lines_ride_on_granted_statements() {
    let baseline = base();
    let comment = edit(&baseline, PAGES, "// the work", "// the real work");
    let body_writer = write(&[(double(), Facet::Body)]);
    let e = check(&baseline, &comment, &body_writer);
    assert!(e.statements.is_empty());
    assert_eq!(violation_codes(&e), ["patch_out_of_scope"]);
    assert_eq!(paths(&e), [PAGES]);

    let with_body = edit(
        &comment,
        PAGES,
        "    helper(x) * 2\n",
        "    helper(x) * 4\n",
    );
    assert!(check(&baseline, &with_body, &body_writer).accepted());

    // A `use` line in a file without granted statements.
    let use_line = edit(
        &with_body,
        UTIL,
        "module app.util\n",
        "module app.util\n\nuse app.pages.{noop}\n",
    );
    let e = check(&baseline, &use_line, &body_writer);
    assert_eq!(paths(&e), [UTIL]);
}

#[test]
fn rule5_module_header_changes_are_out_of_scope() {
    let extra = "module app.extra\n\n@id(\"fn_gggggggggggggggg\")\nfn extra() -> Int {\n    1\n}\n";
    let baseline = with_file(&base(), "src/app/extra.cpl", extra);
    let candidate = edit(
        &baseline,
        "src/app/extra.cpl",
        "module app.extra",
        "module app.other",
    );
    // The move itself is granted; the changed header is not.
    let e = check(
        &baseline,
        &candidate,
        &write_all(&id("fn_gggggggggggggggg")),
    );
    assert_eq!(e.statements.len(), 1);
    assert_eq!(e.statements[0].statement, DiffStatementKind::Moved);
    assert_eq!(violation_codes(&e), ["patch_out_of_scope"]);
    assert_eq!(paths(&e), ["src/app/extra.cpl"]);
    assert!(e.violations[0].message.contains("`module` header"));
}

#[test]
fn rule5_new_files_need_an_add_namespace() {
    let baseline = base();
    let candidate = with_file(
        &baseline,
        "src/tests/more.cpl",
        "module tests.more\n\ntest \"more\" {\n    assert true\n}\n",
    );
    let e = check(&baseline, &candidate, &Grant::default());
    assert_eq!(statements(&e), ["identity_added"]);
    assert_eq!(paths(&e), ["src/tests/more.cpl"]);
    let tests = Grant {
        add: add(&[&["tests"]]),
        ..Grant::default()
    };
    assert!(check(&baseline, &candidate, &tests).accepted());
    let header_only = with_file(&baseline, "src/app/extra.cpl", "module app.extra\n");
    let e = check(&baseline, &header_only, &tests);
    assert_eq!(paths(&e), ["src/app/extra.cpl"]);
}

#[test]
fn rule5_removed_files_need_granted_removals() {
    let extra = "module app.extra\n\n@id(\"fn_gggggggggggggggg\")\nfn extra() -> Int {\n    1\n}\n";
    let baseline = with_file(&base(), "src/app/extra.cpl", extra);
    let candidate = without_file(&baseline, "src/app/extra.cpl");
    assert!(check(
        &baseline,
        &candidate,
        &write_all(&id("fn_gggggggggggggggg"))
    )
    .accepted());
    let e = check(&baseline, &candidate, &Grant::default());
    assert_eq!(statements(&e), ["identity_removed"]);
    assert_eq!(paths(&e), ["src/app/extra.cpl"]);

    let empty = with_file(&base(), "src/app/empty.cpl", "module app.empty\n");
    let e = check(&empty, &base(), &write_all(&noop()));
    assert_eq!(paths(&e), ["src/app/empty.cpl"]);
}

#[test]
fn enforcement_reports_the_facet_diff() {
    let baseline = base();
    let candidate = edit(&baseline, PAGES, "    0\n", "    1\n");
    let e = check(&baseline, &candidate, &write(&[(noop(), Facet::Body)]));
    assert!(e.accepted());
    let diff = cpl_index::semantic_diff(&map_of(&baseline), &map_of(&candidate));
    assert_eq!(e.statements, diff.statements);
    assert_eq!(e.statements[0].statement, DiffStatementKind::BodyChanged);
}

// ---------------------------------------------------------------------------
// Rule 5 and imports: design lines 285 and 353 make everything outside the
// granted write and add sets read-only; imports are scoped to the file
// (semantics S5), so a `use` line can change what unchanged text refers to.
// ---------------------------------------------------------------------------

#[test]
fn rule5_a_repointed_import_needs_the_facets_that_use_it() {
    // The `use` line rides on the granted `noop` body (10.5 rule 5), but the
    // unchanged body of `double` now calls another `helper`.
    let baseline = base();
    let candidate = edit(
        &edit(
            &baseline,
            PAGES,
            "use app.util.{helper}",
            "use app.other.{helper}",
        ),
        PAGES,
        "    0\n",
        "    1\n",
    );
    let e = check(&baseline, &candidate, &write(&[(noop(), Facet::Body)]));
    assert_eq!(violation_codes(&e), ["patch_out_of_scope"]);
    assert_eq!(e.violations[0].symbol, Some(double()));
    assert_eq!(e.violations[0].facet, Some(Facet::Body));
    assert_eq!(e.violations[0].data["name"], json!("helper"));
    assert_eq!(paths(&e), [PAGES]);
    let diff = tree_diff(&baseline, &candidate, &["src".to_string()]);
    let rebound: Vec<(&str, &[String], &[String])> = diff
        .rebound
        .iter()
        .map(|r| (r.name.as_str(), r.old.as_slice(), r.new.as_slice()))
        .collect();
    assert_eq!(
        rebound,
        [(
            "helper",
            &["app.util.helper".to_string()][..],
            &["app.other.helper".to_string()][..]
        )]
    );
    // Write authority over the body that uses the name covers the change.
    let both = write(&[(noop(), Facet::Body), (double(), Facet::Body)]);
    assert!(check(&baseline, &candidate, &both).accepted());
}

#[test]
fn rule5_an_added_import_can_rebind_a_chain_root_but_a_respelled_one_cannot() {
    // Semantics 2.1: a file's imports (scope 2) hide root module names
    // (scope 1), so a new `use` redirects a qualified chain in unchanged
    // text; spelling the same import another way imports the same item.
    let baseline = edit(
        &base(),
        PAGES,
        "    helper(x) * 2\n",
        "    app.util.helper(x) * 2\n",
    );
    let noop_writer = write(&[(noop(), Facet::Body)]);
    let noop_changed = edit(&baseline, PAGES, "    0\n", "    1\n");
    let shadowed = edit(
        &noop_changed,
        PAGES,
        "use app.util.{helper}",
        "use app.util.{helper}\nuse evil.{app}",
    );
    let e = check(&baseline, &shadowed, &noop_writer);
    assert_eq!(violation_codes(&e), ["patch_out_of_scope"]);
    assert_eq!(e.violations[0].symbol, Some(double()));
    assert_eq!(e.violations[0].data["name"], json!("app"));
    let respelled = edit(
        &noop_changed,
        PAGES,
        "use app.util.{helper}",
        "use app.util.helper",
    );
    assert!(check(&baseline, &respelled, &noop_writer).accepted());
}

#[test]
fn rule5_a_move_to_a_file_with_other_imports_needs_the_facets_it_redirects() {
    // Within one module a move is no statement (9.1); the moved test keeps
    // its text, and its old file holds a granted statement (the added test),
    // but its new file imports another `double`.
    let baseline = base();
    let doubles = "test \"doubles\" {\n    assert double(2) == 4\n}\n";
    let more = "src/tests/app/more.cpl";
    let moved = edit(
        &baseline,
        TESTS,
        doubles,
        "test \"quadruples\" {\n    assert double(double(1)) == 4\n}\n",
    );
    let adder = Grant {
        add: add(&[&["tests", "app"]]),
        ..Grant::default()
    };
    let elsewhere = with_file(
        &moved,
        more,
        &format!("module tests.app\n\nuse app.fake.{{double}}\n\n{doubles}"),
    );
    let e = check(&baseline, &elsewhere, &adder);
    assert_eq!(violation_codes(&e), ["patch_out_of_scope"]);
    assert_eq!(e.violations[0].data["name"], json!("double"));
    assert_eq!(paths(&e), [more]);
    let same = with_file(
        &moved,
        more,
        &format!("module tests.app\n\nuse app.pages.{{double}}\n\n{doubles}"),
    );
    assert!(check(&baseline, &same, &adder).accepted());
}
