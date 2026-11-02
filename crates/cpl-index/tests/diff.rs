//! Semantic diff (`spec/toolchain.md` 9, 12): the corpus of
//! `tests/corpus/diff/<case>/{before,after}/` projects with the expected
//! `DiffOutput` of 9.2 in `<case>/diff.json` (regenerated with
//! `UPDATE_GOLDEN=1`), covering every statement of 9.1, plus hand-checked
//! details and the ordering rule.
//!
//! | Case | Shows |
//! | - | - |
//! | `added_and_removed` | `identity_added`, `identity_removed` (functions, tests, a new file) |
//! | `renamed_with_callers` | `identity_renamed` of a fn and a test kept by `@id`; the caller's `body_changed` |
//! | `moved` | `moved` to another module; `identity_renamed` plus `moved`; a move between files of one module is silent |
//! | `signature_and_contract` | `signature_changed`, `contract_changed` on functions, a refinement, a capability, a struct, a `decreases` (T23) |
//! | `effect_bounds` | `effect_bound_widened`, `effect_bound_narrowed`, both at once; reordering, repeating, and an empty bound are silent (T26) |
//! | `body_docs_and_policy` | `body_changed`, `docs_changed` (member docs too, T25), `verification_policy_changed` both ways |
//! | `kind_changed` | a facet set that changes is `identity_removed` plus `identity_added` with `kind_changed`; a wrapper that became a struct is `signature_changed` |
//! | `id_changed` | a changed or newly assigned `@id` is a new identity |
//! | `duplicates` | `duplicates.old` and `.new`; statements use first occurrences |
//! | `layout_only` | comments, blank lines, line breaks, spacing, `use` lines, and work files change no statement |

mod common;

use common::*;
use cpl_diag::json::{DiffOutput, DiffStatementKind};
use cpl_diag::{Facet, StableId};
use cpl_index::{facet_map, semantic_diff};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn corpus_dir() -> PathBuf {
    crate_dir().join("tests/corpus/diff")
}

fn cases() -> Vec<PathBuf> {
    let mut cases: Vec<PathBuf> = std::fs::read_dir(corpus_dir())
        .expect("the diff corpus")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.is_dir())
        .collect();
    cases.sort();
    cases
}

/// The diff of one corpus case, both sides loaded from their directories.
fn diff_case(case: &Path) -> DiffOutput {
    let load = |side: &str| {
        let project = load_dir(&case.join(side));
        let errors: Vec<_> = project
            .diagnostics
            .iter()
            .filter(|d| d.is_error())
            .collect();
        assert!(errors.is_empty(), "{}/{side}: {errors:?}", case.display());
        project
    };
    let (before, after) = (load("before"), load("after"));
    semantic_diff(
        &facet_map(&before, unversioned(&before)),
        &facet_map(&after, unversioned(&after)),
    )
}

fn case(name: &str) -> DiffOutput {
    diff_case(&corpus_dir().join(name))
}

/// `(statement, path)` of every statement.
fn summary(diff: &DiffOutput) -> Vec<(String, String)> {
    diff.statements
        .iter()
        .map(|s| {
            let kind = serde_json::to_value(s.statement).expect("serializes");
            (kind.as_str().expect("a string").to_string(), s.path.clone())
        })
        .collect()
}

#[test]
fn the_diff_corpus_matches_its_expected_output() {
    let cases = cases();
    assert_eq!(cases.len(), 10);
    for case in cases {
        let diff = diff_case(&case);
        let mut json = serde_json::to_string_pretty(&diff).expect("serializes");
        json.push('\n');
        check_golden(&case.join("diff.json"), &json);
        // The expected file is a DiffOutput.
        let parsed: DiffOutput = serde_json::from_str(&json).expect("round-trips");
        assert_eq!(parsed, diff);
    }
}

#[test]
fn the_corpus_covers_every_statement_of_section_9_1() {
    let mut seen: BTreeSet<DiffStatementKind> = BTreeSet::new();
    let mut kind_changed = 0;
    let mut duplicates = (0, 0);
    for case in cases() {
        let diff = diff_case(&case);
        for s in &diff.statements {
            seen.insert(s.statement);
            if s.detail.get("note") == Some(&json!("kind_changed")) {
                kind_changed += 1;
            }
        }
        duplicates.0 += diff.duplicates.old.len();
        duplicates.1 += diff.duplicates.new.len();
    }
    use DiffStatementKind::*;
    let all: BTreeSet<DiffStatementKind> = [
        IdentityAdded,
        IdentityRemoved,
        IdentityRenamed,
        Moved,
        SignatureChanged,
        ContractChanged,
        EffectBoundWidened,
        EffectBoundNarrowed,
        BodyChanged,
        DocsChanged,
        VerificationPolicyChanged,
    ]
    .into_iter()
    .collect();
    assert_eq!(seen, all);
    assert_eq!(kind_changed, 4);
    assert_eq!(duplicates, (1, 1));
}

#[test]
fn statements_are_ordered_by_path_then_table_order() {
    for case in cases() {
        let diff = diff_case(&case);
        // The sort path is the new side's when the id is on the new side.
        let keys: Vec<(String, DiffStatementKind, StableId)> = diff
            .statements
            .iter()
            .map(|s| {
                let new_path = diff
                    .statements
                    .iter()
                    .find(|t| t.id == s.id && t.statement != DiffStatementKind::IdentityRemoved)
                    .map_or(&s.path, |t| &t.path);
                (new_path.clone(), s.statement, s.id.clone())
            })
            .collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted, "{}", case.display());
    }
}

// ---------------------------------------------------------------------------
// Hand-checked details (independent of the expected files)
// ---------------------------------------------------------------------------

#[test]
fn added_and_removed_carry_the_version_of_their_side() {
    let diff = case("added_and_removed");
    assert_eq!(
        summary(&diff),
        [
            ("identity_added".into(), "m.a.\"new test\"".into()),
            ("identity_removed".into(), "m.a.\"old test\"".into()),
            ("identity_added".into(), "m.a.fresh".into()),
            ("identity_removed".into(), "m.a.gone".into()),
            ("identity_added".into(), "m.b.Extra".into()),
        ]
    );
    let gone = &diff.statements[3];
    assert_eq!(gone.kind, "fn");
    assert_eq!(gone.facet, None);
    assert_eq!(gone.detail["version"]["id"], json!("m.a.gone"));
    assert_eq!(gone.detail["version"]["provisional"], json!(true));
    assert_eq!(diff.statements[0].kind, "test");
    assert_eq!(diff.statements[4].kind, "ty");
    assert!(diff.statements[4].detail["version"]["facets"]
        .get("signature")
        .is_some());
}

#[test]
fn renames_keep_the_identity_and_change_the_callers_bodies() {
    let diff = case("renamed_with_callers");
    assert_eq!(
        summary(&diff),
        [
            ("identity_renamed".into(), "lib.pages.\"new name\"".into()),
            ("body_changed".into(), "lib.pages.caller".into()),
            ("identity_renamed".into(), "lib.pages.paginate".into()),
        ]
    );
    let renamed = &diff.statements[2];
    assert_eq!(renamed.id.as_str(), "fn_abcdefghijklmnop");
    assert_eq!(renamed.facet, Some(Facet::Name));
    assert_eq!(
        renamed.detail,
        json!({ "from": "lib.pages.pages_of", "to": "lib.pages.paginate" })
    );
    assert_eq!(diff.statements[0].kind, "test");
}

#[test]
fn moves_report_both_modules() {
    let diff = case("moved");
    assert_eq!(
        summary(&diff),
        [
            ("moved".into(), "shop.orders.Item".into()),
            ("identity_renamed".into(), "shop.orders.order_total".into()),
            ("moved".into(), "shop.orders.order_total".into()),
        ]
    );
    assert_eq!(
        diff.statements[0].detail,
        json!({ "from_module": "shop.items", "to_module": "shop.orders" })
    );
    assert_eq!(diff.statements[0].facet, None);
}

#[test]
fn signature_and_contract_statements_carry_both_hashes() {
    let diff = case("signature_and_contract");
    assert_eq!(
        summary(&diff),
        [
            ("signature_changed".into(), "calc.ops.Pair".into()),
            ("contract_changed".into(), "calc.ops.Small".into()),
            ("signature_changed".into(), "calc.ops.Store".into()),
            ("contract_changed".into(), "calc.ops.Store".into()),
            ("contract_changed".into(), "calc.ops.count".into()),
            ("signature_changed".into(), "calc.ops.scale".into()),
            ("contract_changed".into(), "calc.ops.scale".into()),
        ]
    );
    let small = &diff.statements[1];
    assert_eq!(small.facet, Some(Facet::Contract));
    assert_eq!(
        small.detail,
        json!({
            "old": cpl_index::facet_hash("where self < 100"),
            "new": cpl_index::facet_hash("where self < 1000"),
        })
    );
}

#[test]
fn effect_bounds_are_compared_as_sets() {
    let diff = case("effect_bounds");
    let details: Vec<(String, String, Value)> = diff
        .statements
        .iter()
        .map(|s| {
            let kind = serde_json::to_value(s.statement).expect("serializes");
            (
                kind.as_str().expect("string").to_string(),
                s.path.clone(),
                s.detail.clone(),
            )
        })
        .collect();
    assert_eq!(
        details,
        [
            (
                "effect_bound_widened".to_string(),
                "io.bounds.bound_added".to_string(),
                json!({ "added": ["db.get"] })
            ),
            (
                "effect_bound_narrowed".to_string(),
                "io.bounds.narrow".to_string(),
                json!({ "removed": ["db.put"] })
            ),
            (
                "effect_bound_widened".to_string(),
                "io.bounds.swap".to_string(),
                json!({ "added": ["db.put"] })
            ),
            (
                "effect_bound_narrowed".to_string(),
                "io.bounds.swap".to_string(),
                json!({ "removed": ["db.get"] })
            ),
            (
                "effect_bound_widened".to_string(),
                "io.bounds.widen".to_string(),
                json!({ "added": ["db.put"] })
            ),
        ]
    );
    assert!(diff
        .statements
        .iter()
        .all(|s| s.facet == Some(Facet::Effects)));
}

#[test]
fn body_docs_and_policy_statements() {
    let diff = case("body_docs_and_policy");
    assert_eq!(
        summary(&diff),
        [
            ("body_changed".into(), "doc.items.\"a test\"".into()),
            ("docs_changed".into(), "doc.items.Rec".into()),
            ("docs_changed".into(), "doc.items.inc".into()),
            (
                "verification_policy_changed".into(),
                "doc.items.proved".into()
            ),
            ("body_changed".into(), "doc.items.twice".into()),
            (
                "verification_policy_changed".into(),
                "doc.items.unproved".into()
            ),
        ]
    );
    // The policy statement carries texts, not hashes (9.1).
    assert_eq!(
        diff.statements[3].detail,
        json!({ "old": "@prove", "new": "" })
    );
    assert_eq!(
        diff.statements[5].detail,
        json!({ "old": "", "new": "@prove" })
    );
    assert_eq!(diff.statements[3].facet, Some(Facet::VerificationPolicy));
}

#[test]
fn a_changed_facet_set_is_a_removal_and_an_addition() {
    let diff = case("kind_changed");
    // 9.1: a statement's `path` is the new side's when the id is in the new
    // map ("ordered by `path` (new side, else old)"), so both statements of
    // the fn `check_it` that became the test "check it" are at the new
    // path; the removal's `detail.version` keeps the old side's.
    assert_eq!(
        summary(&diff),
        [
            ("identity_added".into(), "k.kinds.\"check it\"".into()),
            ("identity_removed".into(), "k.kinds.\"check it\"".into()),
            ("identity_added".into(), "k.kinds.Size".into()),
            ("identity_removed".into(), "k.kinds.Size".into()),
            ("signature_changed".into(), "k.kinds.Wrapped".into()),
        ]
    );
    for s in &diff.statements[..4] {
        assert_eq!(s.detail["note"], json!("kind_changed"));
    }
    assert_eq!(diff.statements[0].kind, "test");
    assert_eq!(diff.statements[1].kind, "fn");
    assert_eq!(diff.statements[0].id, diff.statements[1].id);
    assert_eq!(
        diff.statements[0].detail["version"]["path"],
        json!("k.kinds.\"check it\"")
    );
    assert_eq!(
        diff.statements[1].detail["version"]["path"],
        json!("k.kinds.check_it")
    );
}

#[test]
fn a_new_id_value_is_a_new_identity() {
    let diff = case("id_changed");
    let ids: Vec<(String, &str)> = summary(&diff)
        .into_iter()
        .zip(&diff.statements)
        .map(|((kind, _), s)| (kind, s.id.as_str()))
        .collect();
    assert_eq!(
        ids,
        [
            ("identity_added".to_string(), "fn_gggggggggggggggg"),
            ("identity_removed".to_string(), "fn_ffffffffffffffff"),
            ("identity_added".to_string(), "fn_hhhhhhhhhhhhhhhh"),
            ("identity_removed".to_string(), "ids.a.tagged_later"),
        ]
    );
}

#[test]
fn duplicates_are_listed_per_side() {
    let diff = case("duplicates");
    assert_eq!(
        serde_json::to_value(&diff.duplicates).expect("serializes"),
        json!({
            "old": [ { "id": "fn_iiiiiiiiiiiiiiii", "files": ["src/dup/one.cpl", "src/dup/two.cpl"] } ],
            "new": [ { "id": "dup.two.helper", "files": ["src/dup/two.cpl", "src/dup/two/more.cpl"] } ],
        })
    );
    assert_eq!(
        summary(&diff),
        [
            ("identity_added".into(), "dup.two.copy".into()),
            ("identity_added".into(), "dup.two.helper".into()),
        ]
    );
}

#[test]
fn layout_comments_and_work_files_change_nothing() {
    let diff = case("layout_only");
    assert!(diff.statements.is_empty(), "{:?}", diff.statements);
    // The revisions still differ: the bytes did.
    assert_ne!(diff.old.digest, diff.new.digest);
}

// ---------------------------------------------------------------------------
// In-memory cases
// ---------------------------------------------------------------------------

#[test]
fn a_revision_diffed_with_itself_has_no_statements() {
    let project = load_dir(&workspace_path("examples/reader"));
    let map = facet_map(&project, unversioned(&project));
    let diff = semantic_diff(&map, &map);
    assert!(diff.statements.is_empty());
    assert_eq!(diff.old, diff.new);
    assert!(diff.duplicates.old.is_empty() && diff.duplicates.new.is_empty());
    // The 9.2 shape.
    let json = serde_json::to_value(&diff).expect("serializes");
    let keys: Vec<&str> = json
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["duplicates", "new", "old", "statements"]);
}

#[test]
fn a_file_that_stops_parsing_removes_its_declarations() {
    // The facet map is syntactic: a revision whose file does not parse has
    // none of its declarations (T31).
    let good = "module m\n\nfn f() -> Int {\n    1\n}\n\nfn g() -> Int {\n    2\n}\n";
    let broken = "module m\n\nfn f() -> Int {\n    1\n}\n\nfn g( -> Int {\n    2\n}\n";
    let before = load_tree(&tree(&[("src/m.cpl", good)]));
    let after = load_tree(&tree(&[("src/m.cpl", broken)]));
    let diff = semantic_diff(
        &facet_map(&before, unversioned(&before)),
        &facet_map(&after, unversioned(&after)),
    );
    assert_eq!(
        summary(&diff),
        [
            ("identity_removed".into(), "m.f".into()),
            ("identity_removed".into(), "m.g".into()),
        ]
    );
}

#[test]
fn an_unknown_policy_hash_is_shown_as_the_hash() {
    // A facet map from another toolchain could carry a policy text this one
    // does not produce; the statement then shows the hash.
    let src = "module m\n\n@prove\nfn f() -> Int {\n    1\n}\n";
    let project = load_tree(&tree(&[("src/m.cpl", src)]));
    let old = facet_map(&project, unversioned(&project));
    let mut new = old.clone();
    let id = StableId::parse("m.f").expect("id");
    let strange = cpl_index::facet_hash("@prove(later)");
    new.entries
        .get_mut(&id)
        .expect("entry")
        .version
        .facets
        .insert(Facet::VerificationPolicy, strange.clone());
    let diff = semantic_diff(&old, &new);
    assert_eq!(
        diff.statements[0].detail,
        json!({ "old": "@prove", "new": strange.as_str() })
    );
}
