use cpl_evolve::{merge::merge, verify::VerifyConfig, Snapshot};
use cpl_project::{LoadOptions, VirtualTree};
use std::collections::BTreeMap;

const MANIFEST: &[u8] = b"[project]\nname = \"merge\"\nedition = \"0.1\"\n";

fn tree(files: &[(&str, &str)]) -> VirtualTree {
    let mut tree = VirtualTree {
        files: BTreeMap::from([("cpl.toml".to_string(), MANIFEST.to_vec())]),
    };
    for (path, text) in files {
        tree.files
            .insert(path.to_string(), text.as_bytes().to_vec());
    }
    tree
}
fn snapshot(files: &[(&str, &str)]) -> Snapshot {
    Snapshot::new(tree(files), &LoadOptions::default())
        .unwrap_or_else(|d| panic!("{}: {:?}", d.message, d.data))
}
fn one(text: &str) -> Snapshot {
    snapshot(&[("src/m.cpl", text)])
}
fn run(b: &Snapshot, l: &Snapshot, r: &Snapshot) -> cpl_evolve::merge::Merge {
    merge(b, l, r, &VerifyConfig::default(), &LoadOptions::default())
        .unwrap_or_else(|d| panic!("{}: {:?}", d.message, d.data))
}
/// A merged file; merging formatted inputs yields formatted output.
fn text(m: &cpl_evolve::merge::Merge, path: &str) -> String {
    let text =
        String::from_utf8(m.tree.as_ref().expect("a composed tree").files[path].clone()).unwrap();
    if path.ends_with(".cpl") {
        let formatted =
            cpl_syntax::format_source(&text, cpl_syntax::FileKind::Cpl, cpl_diag::FileId(0))
                .unwrap();
        assert_eq!(formatted, text, "merged {path} is not in canonical layout");
    }
    text
}

const BASE: &str = r#"module m

@id("fn_calc")
pub pure fn calc(n: Int) -> Int {
    n + 1
}

@id("fn_caller")
pure fn caller(n: Int) -> Int {
    calc(n)
}

@id("test_calc")
test "calc result" {
    assert caller(1) == 2
}
"#;

#[test]
fn a_rename_and_a_body_change_of_one_function_merge_by_facet() {
    let base = one(BASE);
    let left = one(&BASE
        .replace("fn calc(", "fn compute(")
        .replace("    calc(n)", "    compute(n)"));
    let right = one(&BASE.replace("    n + 1", "    1 + n"));
    let m = run(&base, &left, &right);
    assert_eq!(m.report.status, "tested", "{:#?}", m.report);
    assert!(m.report.accepted);
    let merged = text(&m, "src/m.cpl");
    assert!(
        merged.contains("pub pure fn compute(n: Int) -> Int {\n    1 + n\n}"),
        "{merged}"
    );
    assert!(merged.contains("    compute(n)"), "{merged}");
    assert!(!merged.contains("calc("), "{merged}");
    // Symmetric: the same result with the sides exchanged.
    let m = run(&base, &right, &left);
    assert!(m.report.accepted);
    assert_eq!(text(&m, "src/m.cpl"), merged);
}

#[test]
fn references_added_on_one_side_follow_a_rename_on_the_other() {
    let base = one(BASE);
    let left = one(&BASE
        .replace("fn calc(", "fn compute(")
        .replace("    calc(n)", "    compute(n)"));
    let right = one(&format!(
        "{BASE}\n@id(\"fn_twice\")\npure fn twice(n: Int) -> Int {{\n    calc(calc(n))\n}}\n\n@id(\"test_twice\")\ntest \"twice\" {{\n    assert twice(1) == 3\n}}\n"
    ));
    let m = run(&base, &left, &right);
    assert_eq!(m.report.status, "tested", "{:#?}", m.report);
    let merged = text(&m, "src/m.cpl");
    assert!(merged.contains("compute(compute(n))"), "{merged}");
    let evidence = m.report.evidence.as_ref().unwrap();
    assert!(evidence
        .tests
        .iter()
        .any(|t| t.symbol.as_str() == "test_twice"));
    // The interaction is visible even though the composition passes its tests.
    assert_eq!(m.report.compatibility, "tested");
    assert!(m
        .report
        .interactions
        .iter()
        .any(|i| i["symbol"] == "fn_twice"));
}

#[test]
fn different_replacements_of_one_facet_are_structural_conflicts() {
    let base = one(BASE);
    let left = one(&BASE.replace("n + 1", "n + 2"));
    let right = one(&BASE.replace("n + 1", "n + 3"));
    let m = run(&base, &left, &right);
    assert_eq!(m.report.status, "conflict");
    assert!(m.tree.is_none() && !m.report.accepted);
    assert_eq!(m.report.conflicts[0]["symbol"], "fn_calc");
    assert_eq!(m.report.conflicts[0]["facet"], "body");
    // Identical changes on both sides are not conflicts.
    let m = run(&base, &left, &left);
    assert_eq!(
        m.report.status, "failed",
        "the shared change breaks test_calc"
    );
    assert!(m.report.regressions.is_empty());
    assert_eq!(m.report.inherited_failures[0]["symbol"], "test_calc");
}

const NORMALIZE: &str = r#"module m

@id("fn_normalize")
pub pure fn normalize(x: Int) -> Int {
    x
}

@id("fn_score")
pub pure fn score(x: Int) -> Int {
    normalize(x) * 2
}

@id("test_score")
test "score doubles" {
    assert score(2) == 4
}
"#;

#[test]
fn a_test_that_passes_on_each_side_but_fails_composed_is_a_regression() {
    // Agent A changes normalize; agent B starts relying on negative values being preserved.
    // The edits touch different declarations, so a text merge reports nothing.
    let base = one(NORMALIZE);
    let left = one(&NORMALIZE.replace("{\n    x\n}", "{\n    x.abs()\n}"));
    let right = one(&format!(
        "{NORMALIZE}\n@id(\"test_negative\")\ntest \"negative scores stay negative\" {{\n    assert score(-3) == -6\n}}\n"
    ));
    let m = run(&base, &left, &right);
    assert_eq!(m.report.status, "regression", "{:#?}", m.report);
    assert!(!m.report.accepted);
    let regression = &m.report.regressions[0];
    assert_eq!(regression["symbol"], "test_negative");
    assert_eq!(regression["passes_on"], serde_json::json!(["right"]));
    assert!(m.tree.is_some(), "the composed tree is kept for inspection");
    assert!(m.install(&left).is_err());
}

#[test]
fn a_test_both_sides_share_detects_a_combined_regression() {
    let source = r#"module m

@id("fn_x")
pure fn x() -> Int {
    1
}

@id("fn_y")
pure fn y() -> Int {
    2
}

@id("test_budget")
test "budget" {
    assert x() + y() < 5
}
"#;
    let base = one(source);
    let left = one(&source.replace("{\n    1\n}", "{\n    2\n}"));
    let right = one(&source.replace("{\n    2\n}", "{\n    3\n}"));
    let m = run(&base, &left, &right);
    assert_eq!(m.report.status, "regression", "{:#?}", m.report);
    assert_eq!(
        m.report.regressions[0]["passes_on"],
        serde_json::json!(["left", "right"])
    );
    assert!(m
        .report
        .interactions
        .iter()
        .any(|i| i["symbol"] == "test_budget"));
}

#[test]
fn a_removed_dependency_of_new_code_fails_composed_checking() {
    let source =
        format!("{BASE}\n@id(\"fn_helper\")\npure fn helper(n: Int) -> Int {{\n    n\n}}\n");
    let base = one(&source);
    let left = one(BASE);
    let right = one(&source.replace("    calc(n)", "    helper(calc(n))"));
    let m = run(&base, &left, &right);
    assert_eq!(m.report.status, "check_failed", "{:#?}", m.report);
    assert!(!m.report.diagnostics.is_empty());
    assert!(m.snapshot.is_none());
}

#[test]
fn removal_and_modification_or_divergent_additions_conflict() {
    let source =
        format!("{BASE}\n@id(\"fn_helper\")\npure fn helper(n: Int) -> Int {{\n    n\n}}\n");
    let base = one(&source);
    let removed = one(BASE);
    let changed = one(&source.replace("{\n    n\n}", "{\n    n + 0\n}"));
    let m = run(&base, &removed, &changed);
    assert_eq!(m.report.status, "conflict");
    assert_eq!(
        m.report.conflicts[0]["reason"],
        "removed_on_left_and_changed_on_right"
    );
    let a = one(&format!(
        "{BASE}\n@id(\"fn_new\")\npure fn new_one() -> Int {{\n    1\n}}\n"
    ));
    let b = one(&format!(
        "{BASE}\n@id(\"fn_new\")\npure fn new_one() -> Int {{\n    2\n}}\n"
    ));
    let m = run(&one(BASE), &a, &b);
    assert_eq!(
        m.report.conflicts[0]["reason"],
        "added_differently_on_both_sides"
    );
    // Removal by one side of an untouched declaration is simply taken.
    let m = run(&base, &removed, &base);
    assert!(m.report.accepted);
    assert!(!text(&m, "src/m.cpl").contains("helper"));
}

const LIB: &str = r#"module lib

@id("fn_a")
pub pure fn a() -> Int {
    1
}

@id("fn_b")
pub pure fn b() -> Int {
    2
}
"#;
const CLIENT: &str = r#"module client

use lib.a

@id("fn_one")
pure fn one() -> Int {
    a()
}
"#;

#[test]
fn imports_merge_as_sets_of_resolved_targets_and_follow_renames() {
    let base = snapshot(&[("src/lib.cpl", LIB), ("src/client.cpl", CLIENT)]);
    // Left imports b for a new function; right renames a and adds a function using it.
    let left = snapshot(&[
        ("src/lib.cpl", LIB),
        (
            "src/client.cpl",
            &CLIENT.replace("use lib.a\n", "use lib.{a, b}\n").replace(
                "    a()\n}\n",
                "    a()\n}\n\n@id(\"fn_two\")\npure fn two() -> Int {\n    b()\n}\n",
            ),
        ),
    ]);
    let renamed = LIB.replace("fn a()", "fn first()");
    let right = snapshot(&[
        ("src/lib.cpl", &renamed),
        (
            "src/client.cpl",
            &CLIENT.replace("use lib.a", "use lib.first").replace(
                "    a()\n}\n",
                "    first()\n}\n\n@id(\"fn_three\")\npure fn three() -> Int {\n    first() + 2\n}\n",
            ),
        ),
    ]);
    let m = run(&base, &left, &right);
    assert_eq!(m.report.status, "tested", "{:#?}", m.report);
    let client = text(&m, "src/client.cpl");
    assert!(client.contains("use lib.{first, b}"), "{client}");
    assert!(
        client.contains("fn two()") && client.contains("fn three()"),
        "{client}"
    );
    assert!(client.contains("    first()\n}"), "{client}");
    assert!(!client.contains("a()"), "{client}");
    assert_eq!(text(&m, "src/lib.cpl"), renamed);
}

#[test]
fn unrelated_changes_are_independent_and_installable_over_the_left_side() {
    let source = format!("{BASE}\n@id(\"fn_other\")\npure fn other() -> Int {{\n    5\n}}\n");
    let base = one(&source);
    let left = one(&source.replace("    n + 1", "    1 + n"));
    let right = one(&source.replace("    5\n", "    2 + 3\n"));
    let m = run(&base, &left, &right);
    assert_eq!(m.report.status, "tested");
    assert_eq!(m.report.compatibility, "independent");
    assert!(m.install(&left).is_ok());
    assert!(m.install(&right).is_err(), "bound to the left side");
    let merged = text(&m, "src/m.cpl");
    assert!(merged.contains("1 + n") && merged.contains("2 + 3"));
}

#[test]
fn new_files_and_whole_file_inputs_merge() {
    let base = one(BASE);
    let left = snapshot(&[
        ("src/m.cpl", BASE),
        (
            "src/extra.cpl",
            "module extra\n\nuse m.calc\n\n@id(\"fn_extra\")\npure fn extra() -> Int {\n    calc(1)\n}\n",
        ),
    ]);
    let right = one(&BASE
        .replace("fn calc(", "fn compute(")
        .replace("    calc(n)", "    compute(n)"));
    let m = run(&base, &left, &right);
    assert_eq!(m.report.status, "tested", "{:#?}", m.report);
    let extra = text(&m, "src/extra.cpl");
    assert!(
        extra.contains("use m.compute") && extra.contains("compute(1)"),
        "{extra}"
    );
    let mut manifest = tree(&[("src/m.cpl", BASE)]);
    manifest.files.insert(
        "cpl.toml".into(),
        b"[project]\nname = \"renamed\"\nedition = \"0.1\"\n".to_vec(),
    );
    let changed = Snapshot::new(manifest, &LoadOptions::default()).unwrap();
    let m = run(&base, &changed, &right);
    assert!(m.report.accepted);
    assert!(text(&m, "cpl.toml").contains("renamed"));
}

#[test]
fn arrivals_keep_their_comments_and_test_names_merge_with_bodies() {
    let base = one(BASE);
    let left = one(&BASE.replace(
        "test \"calc result\" {",
        "test \"calc result is one more\" {",
    ));
    let right = one(&format!(
        "{}\n// Helpers for callers.\n/// Doubles a number.\n@id(\"fn_double\")\npure fn double(n: Int) -> Int {{\n    n * 2\n}}\n",
        BASE.replace("    assert caller(1) == 2", "    assert caller(1) == 2\n    assert caller(2) == 3")
    ));
    let m = run(&base, &left, &right);
    assert_eq!(m.report.status, "tested", "{:#?}", m.report);
    let merged = text(&m, "src/m.cpl");
    assert!(
        merged.contains("test \"calc result is one more\" {\n    assert caller(1) == 2\n    assert caller(2) == 3\n}"),
        "{merged}"
    );
    assert!(
        merged.contains("// Helpers for callers.\n/// Doubles a number.\n@id(\"fn_double\")"),
        "{merged}"
    );
}

#[test]
fn a_moved_declaration_keeps_the_other_sides_edits() {
    let helper = "\n@id(\"fn_helper\")\npub pure fn helper(n: Int) -> Int {\n    n\n}\n";
    let user = "module user\n\nuse a.helper\n\n@id(\"fn_user\")\npure fn user() -> Int {\n    helper(1)\n}\n";
    let base = snapshot(&[
        ("src/a.cpl", &format!("module a\n{helper}")),
        (
            "src/b.cpl",
            "module b\n\n@id(\"fn_b\")\npure fn b() -> Int {\n    0\n}\n",
        ),
        ("src/user.cpl", user),
    ]);
    // Left edits helper in place; right moves it to module b and updates the import.
    let left = snapshot(&[
        (
            "src/a.cpl",
            &format!("module a\n{}", helper.replace("    n\n", "    n + 0\n")),
        ),
        (
            "src/b.cpl",
            "module b\n\n@id(\"fn_b\")\npure fn b() -> Int {\n    0\n}\n",
        ),
        ("src/user.cpl", user),
    ]);
    let right = snapshot(&[
        ("src/a.cpl", "module a\n"),
        (
            "src/b.cpl",
            &format!("module b\n\n@id(\"fn_b\")\npure fn b() -> Int {{\n    0\n}}\n{helper}"),
        ),
        (
            "src/user.cpl",
            &user.replace("use a.helper", "use b.helper"),
        ),
    ]);
    let m = run(&base, &left, &right);
    assert_eq!(m.report.status, "tested", "{:#?}", m.report);
    let b = text(&m, "src/b.cpl");
    assert!(
        b.contains("pub pure fn helper(n: Int) -> Int {\n    n + 0\n}"),
        "{b}"
    );
    assert!(!text(&m, "src/a.cpl").contains("helper"));
    assert!(text(&m, "src/user.cpl").contains("use b.helper"));
}

#[test]
fn declaration_level_facets_from_both_sides_are_grafted_and_hash_checked() {
    let base = one(BASE);
    let left = one(&BASE.replace(
        "pub pure fn calc(n: Int) -> Int {",
        "pub pure fn calc(n: Int) -> Int\n    requires n >= 0\n{",
    ));
    let right = one(&BASE.replace(
        "@id(\"fn_calc\")\n",
        "/// The successor.\n@id(\"fn_calc\")\n",
    ));
    let m = run(&base, &left, &right);
    assert_eq!(m.report.status, "tested", "{:#?}", m.report);
    let merged = text(&m, "src/m.cpl");
    assert!(
        merged.contains("/// The successor.\n@id(\"fn_calc\")\npub pure fn calc(n: Int) -> Int\n    requires n >= 0\n{\n    n + 1\n}"),
        "{merged}"
    );
    // A verification policy and a contract graft the same way, in either direction.
    let proved = one(&BASE.replace("@id(\"fn_calc\")\n", "@id(\"fn_calc\")\n@prove\n"));
    let m = run(&base, &proved, &left);
    assert_eq!(m.report.status, "tested", "{:#?}", m.report);
    let merged = text(&m, "src/m.cpl");
    assert!(
        merged.contains("@prove\npub pure fn calc(n: Int) -> Int\n    requires n >= 0"),
        "{merged}"
    );
}

#[test]
fn facets_that_cannot_be_separated_still_conflict() {
    let source = "module m\n\n@id(\"cap_store\")\npub interface Store {\n    fn read() -> Int\n}\n";
    let base = one(source);
    let docs = one(&source.replace("@id(\"cap_store\")\n", "/// Storage.\n@id(\"cap_store\")\n"));
    let contract = one(&source.replace(
        "    fn read() -> Int\n",
        "    fn read() -> Int\n        ensures result >= 0\n",
    ));
    let m = run(&base, &docs, &contract);
    assert_eq!(m.report.status, "conflict", "{:#?}", m.report);
    assert_eq!(
        m.report.conflicts[0]["reason"],
        "both_sides_changed_declaration_level_facets"
    );
}

#[test]
fn references_to_moved_declarations_are_requalified() {
    let helper = "@id(\"fn_helper\")\npub pure fn helper(n: Int) -> Int {\n    n\n}\n";
    let caller = "@id(\"fn_caller\")\npure fn caller() -> Int {\n    helper(1)\n}\n";
    let user = "module user\n\n@id(\"fn_user\")\npure fn user() -> Int {\n    a.helper(1)\n}\n";
    let base = snapshot(&[
        ("src/a.cpl", &format!("module a\n\n{helper}\n{caller}")),
        (
            "src/b.cpl",
            "module b\n\n@id(\"fn_b\")\npure fn b() -> Int {\n    0\n}\n",
        ),
        ("src/user.cpl", user),
    ]);
    // Right moves helper to module b and requalifies its own references.
    let right = snapshot(&[
        (
            "src/a.cpl",
            &format!(
                "module a\n\n{}",
                caller.replace("    helper(1)", "    b.helper(1)")
            ),
        ),
        (
            "src/b.cpl",
            &format!("module b\n\n@id(\"fn_b\")\npure fn b() -> Int {{\n    0\n}}\n\n{helper}"),
        ),
        ("src/user.cpl", &user.replace("a.helper(1)", "b.helper(1)")),
    ]);
    // Left adds callers written against helper's old module, both qualified and unqualified.
    let left = snapshot(&[
        (
            "src/a.cpl",
            &format!("module a\n\n{helper}\n{caller}\n@id(\"fn_other\")\npure fn other() -> Int {{\n    helper(3)\n}}\n"),
        ),
        ("src/b.cpl", "module b\n\n@id(\"fn_b\")\npure fn b() -> Int {\n    0\n}\n"),
        (
            "src/user.cpl",
            &format!("{user}\n@id(\"fn_more\")\npure fn more() -> Int {{\n    a.helper(2)\n}}\n"),
        ),
    ]);
    let m = run(&base, &left, &right);
    assert_eq!(m.report.status, "tested", "{:#?}", m.report);
    let a = text(&m, "src/a.cpl");
    assert!(
        a.contains("    b.helper(3)") && a.contains("    b.helper(1)"),
        "{a}"
    );
    assert!(!a.contains("pub pure fn helper"), "{a}");
    let user = text(&m, "src/user.cpl");
    assert!(
        user.contains("    b.helper(2)") && user.contains("    b.helper(1)"),
        "{user}"
    );
}
