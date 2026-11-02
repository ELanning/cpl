//! The semantic index over checked programs and the evidence the commands
//! attach (`spec/toolchain.md` 6, 7.2, 8.1, 8.2; semantics S48, S59):
//!
//! - goldens of every `cpl inspect` response (the full include set and the
//!   default set), of `cpl check`'s completed diagnostics and `CheckOutput`
//!   (development and release), and of `cpl prove` for every function of
//!   `examples/reader` and `examples/holes` (`tests/golden/{inspect,check,
//!   prove}/`, regenerated with `UPDATE_GOLDEN=1`), with entries
//!   hand-checked against the examples of 6.4, 7.2, and 8.1;
//! - every include item of 6.2, every target form of 6.1 and its errors;
//! - callers and dependencies of `paginate` and `summarize` (6.5, T17);
//! - obligations (8.1), hole dependencies (S48), receipts (S59, 8.1, 8.2),
//!   `index_partial`, duplicate identities (4.5);
//! - revision states in temporary repositories (clean, worktree) and
//!   without one, and `changes` against `--since` (6.2, 9.3);
//! - receipt digests stable across two independent builds (Section 8).

mod common;

use common::*;
use cpl_diag::json::{
    CheckOutput, DiagnosticJson, DiffStatementKind, InspectOutput, Location, ModuleHash,
    ObligationStatus, ProofStatus, ProveOutput, ReceiptKind, Relation, Revision, RevisionState,
    Status, SymbolBlock, TestCaseRecord, TestKind, TestReport, TestRunConfig, TestSelectorSpec,
    TestStatus, VerificationPolicyJson,
};
use cpl_diag::{render_all, Category, DeclName, Facet, FileOrigin, Severity, StableId, SymbolPath};
use cpl_index::{
    changes_base, current_revision, definition_hash, discover_repo, facet_hash, facet_map,
    facet_map_at, git_error_diagnostic, module_content_hash, receipt_digest, source_digest,
    symbol_version, test_check_configuration, test_environment, Dependency, GitError, GitRepo,
    Include, IncludeSet, Index, Target, DEFAULT_SINCE,
};
use cpl_project::Project;
use cpl_sema::hir::{DefId, RefKind};
use cpl_sema::{check, check_with, CheckConfig, CheckResult};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A loaded project and its check.
struct Checked {
    project: Project,
    check: CheckResult,
}

impl Checked {
    fn of(project: Project) -> Checked {
        let check = check(&project);
        Checked { project, check }
    }

    fn with(project: Project, config: &CheckConfig) -> Checked {
        let check = check_with(&project, config);
        Checked { project, check }
    }

    /// `examples/<name>`, loaded as `cpl check` loads it.
    fn example(name: &str) -> Checked {
        Checked::of(load_dir(&workspace_path(&format!("examples/{name}"))))
    }

    /// An in-memory project of `(path, text)` files.
    fn files(files: &[(&str, &str)]) -> Checked {
        Checked::of(load_tree(&tree(files)))
    }

    /// The index at the revision of a tree outside any repository.
    fn index(&self) -> Index<'_> {
        Index::from_check(&self.check, &self.project, unversioned(&self.project))
    }

    fn index_at(&self, revision: Revision) -> Index<'_> {
        Index::from_check(&self.check, &self.project, revision)
    }

    #[track_caller]
    fn assert_no_errors(&self) {
        assert!(
            !self.check.has_errors(),
            "{}",
            render_all(&self.check.diagnostics, &self.project.source_map)
        );
    }
}

/// The definition a qualified name names.
#[track_caller]
fn def(index: &Index<'_>, path: &str) -> DefId {
    let target = Target::Path(SymbolPath::parse(path).expect("a symbol path"));
    index
        .resolve(&target)
        .unwrap_or_else(|d| panic!("{path}: {d}"))
}

fn paths(index: &Index<'_>, defs: &[DefId]) -> Vec<String> {
    defs.iter()
        .map(|&d| index.version(d).path.clone())
        .collect()
}

fn block_paths(blocks: &[SymbolBlock]) -> Vec<&str> {
    blocks.iter().map(|b| b.version.path.as_str()).collect()
}

/// Every item but `changes` (which needs a repository), and the
/// interface dependencies the default set implies.
fn everything() -> IncludeSet {
    let mut set = IncludeSet::parse(
        "signature,contract,effects,implementation,docs,callers,dependencies,obligations,tests",
    )
    .expect("parses");
    set.items.insert(Include::InterfaceDependencies);
    set
}

fn loc(file: &str, line: u32, col: u32, end_line: u32, end_col: u32) -> Location {
    Location {
        file: file.to_string(),
        line,
        col,
        end_line,
        end_col,
    }
}

fn to_json<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("serializes")
}

/// The keys of a JSON object's text in the order they are written (a
/// `serde_json::Value` would sort them).
fn top_level_keys(text: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut depth = 0;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' | '[' => depth += 1,
            '}' | ']' => depth -= 1,
            '"' => {
                let mut s = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => {
                            s.push(c);
                            s.extend(chars.next());
                        }
                        '"' => break,
                        _ => s.push(c),
                    }
                }
                if depth == 1 && chars.peek() == Some(&':') {
                    keys.push(s);
                }
            }
            _ => {}
        }
    }
    keys
}

fn keys(value: &Value) -> Vec<&str> {
    value
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect()
}

#[track_caller]
fn inspect(index: &Index<'_>, target: &str, include: &IncludeSet) -> InspectOutput {
    index
        .inspect(&Target::parse(target), include)
        .unwrap_or_else(|d| panic!("{target}: {d}"))
}

#[track_caller]
fn resolve_error(index: &Index<'_>, target: Target) -> cpl_diag::Diagnostic {
    match index.resolve(&target) {
        Ok(d) => panic!("{target} resolved to {}", index.version(d).path),
        Err(d) => d,
    }
}

/// Every project definition (std left out), in display-path order.
fn project_defs(index: &Index<'_>) -> Vec<DefId> {
    let program = index.program();
    let mut defs: Vec<DefId> = program
        .defs()
        .filter(|d| program.module(d.module).origin != FileOrigin::Std)
        .map(|d| d.id)
        .collect();
    defs.sort_by_key(|&d| index.version(d).path.clone());
    defs
}

const PRODUCED_AT: &str = "<produced_at>";

// ---------------------------------------------------------------------------
// Goldens
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct InspectGolden<'a> {
    project: &'a str,
    revision: &'a Revision,
    /// The full include set but `changes`, per project definition.
    everything: BTreeMap<String, InspectOutput>,
    /// The default include set (no `--include`), per project definition.
    default: BTreeMap<String, InspectOutput>,
}

fn inspect_golden(name: &str) {
    let checked = Checked::example(name);
    checked.assert_no_errors();
    let index = checked.index();
    let mut golden = InspectGolden {
        project: &format!("examples/{name}"),
        revision: index.revision(),
        everything: BTreeMap::new(),
        default: BTreeMap::new(),
    };
    for d in project_defs(&index) {
        let path = index.version(d).path.clone();
        let target = Target::Path(SymbolPath::parse(&path).expect("path"));
        let all = index.inspect(&target, &everything()).expect("resolves");
        let default = index
            .inspect(&target, &IncludeSet::default_set())
            .expect("resolves");
        golden.everything.insert(path.clone(), all);
        golden.default.insert(path, default);
    }
    let mut text = serde_json::to_string_pretty(&golden).expect("serialize");
    text.push('\n');
    check_golden(
        &crate_dir().join(format!("tests/golden/inspect/{name}.json")),
        &text,
    );
}

#[test]
fn inspect_golden_for_every_definition_of_examples_reader() {
    inspect_golden("reader");
}

#[test]
fn inspect_golden_for_every_definition_of_examples_holes() {
    inspect_golden("holes");
}

#[derive(Serialize)]
struct CheckMode {
    diagnostics: Vec<DiagnosticJson>,
    output: CheckOutput,
}

#[derive(Serialize)]
struct CheckGolden<'a> {
    project: &'a str,
    development: CheckMode,
    release: CheckMode,
}

fn check_mode(name: &str, release: bool) -> CheckMode {
    let project = load_dir(&workspace_path(&format!("examples/{name}")));
    let checked = Checked::with(
        project,
        &CheckConfig {
            release,
            ..CheckConfig::default()
        },
    );
    let index = checked.index();
    let sm = &checked.project.source_map;
    CheckMode {
        diagnostics: index
            .complete_check_diagnostics(&checked.check.diagnostics, release)
            .iter()
            .map(|d| d.to_json(sm))
            .collect(),
        output: index.check_output(&checked.check, release),
    }
}

fn check_golden_for(name: &str) {
    let golden = CheckGolden {
        project: &format!("examples/{name}"),
        development: check_mode(name, false),
        release: check_mode(name, true),
    };
    let mut text = serde_json::to_string_pretty(&golden).expect("serialize");
    text.push('\n');
    check_golden(
        &crate_dir().join(format!("tests/golden/check/{name}.json")),
        &text,
    );
}

#[test]
fn check_golden_for_examples_reader() {
    check_golden_for("reader");
}

#[test]
fn check_golden_for_examples_holes() {
    check_golden_for("holes");
}

#[derive(Serialize)]
struct ProveGolden {
    output: ProveOutput,
    diagnostic: DiagnosticJson,
}

fn prove_golden_for(name: &str) {
    let checked = Checked::example(name);
    let index = checked.index();
    let mut golden: BTreeMap<String, ProveGolden> = BTreeMap::new();
    for d in project_defs(&index) {
        if index.program().def(d).kind.as_fn().is_none() {
            continue;
        }
        let path = index.version(d).path.clone();
        let mut proof = index
            .prove(&Target::Path(SymbolPath::parse(&path).expect("path")))
            .expect("a function");
        proof.output.receipt.produced_at = PRODUCED_AT.to_string();
        golden.insert(
            path,
            ProveGolden {
                output: proof.output,
                diagnostic: proof.diagnostic.to_json(&checked.project.source_map),
            },
        );
    }
    let mut text = serde_json::to_string_pretty(&golden).expect("serialize");
    text.push('\n');
    check_golden(
        &crate_dir().join(format!("tests/golden/prove/{name}.json")),
        &text,
    );
}

#[test]
fn prove_golden_for_every_function_of_examples_reader() {
    prove_golden_for("reader");
}

#[test]
fn prove_golden_for_every_function_of_examples_holes() {
    prove_golden_for("holes");
}

// ---------------------------------------------------------------------------
// 6.4: hand-checked responses
// ---------------------------------------------------------------------------

/// The request of Section 6.4's example, field by field.
#[test]
fn paginate_answers_the_example_of_section_6_4() {
    let checked = Checked::example("reader");
    let index = checked.index();
    let include = IncludeSet::parse("signature,contract,effects,callers").expect("parses");
    let out = inspect(&index, "reader.pages.paginate", &include);
    assert_eq!(
        out.include,
        [
            Include::Signature,
            Include::Contract,
            Include::Effects,
            Include::Callers
        ]
    );
    let json = to_json(&out);
    assert_eq!(keys(&json), ["callers", "include", "symbol"]);

    let symbol = &out.symbol;
    assert_eq!(symbol.version.id.as_str(), "pgn_7c2d");
    assert_eq!(symbol.version.path, "reader.pages.paginate");
    assert!(!symbol.version.provisional);
    assert_eq!(
        symbol.version.facets.keys().copied().collect::<Vec<_>>(),
        Facet::ALL
    );
    assert_eq!(
        symbol.version.definition,
        definition_hash(&symbol.version.facets)
    );
    assert_eq!(symbol.kind, "fn");
    assert_eq!(symbol.module, "reader.pages");
    assert!(symbol.public);
    assert_eq!(symbol.location, loc("src/reader/pages.cpl", 43, 1, 67, 2));
    assert_eq!(
        symbol.verification_policy,
        Some(VerificationPolicyJson { prove: true })
    );
    assert_eq!(
        symbol.signature.as_deref(),
        Some("pub fn paginate<T>(items: List<T>, size: Positive) -> List<List<T>>")
    );
    assert_eq!(
        symbol.contract.as_deref(),
        Some(
            &[
                "ensures flatten(result) == items".to_string(),
                "ensures forall page in result: page.len > 0 and page.len <= size".to_string(),
                "ensures forall page in result.drop_last(): page.len == size".to_string(),
            ][..]
        )
    );
    assert_eq!(symbol.effects.as_deref(), Some(&[][..]));
    assert_eq!(symbol.docs, None);
    assert_eq!(symbol.implementation, None);
    assert_eq!(symbol.holes, None);
    // The symbol block's keys, in the order 6.4 fixes.
    let text = serde_json::to_string(&symbol).expect("serialize");
    assert_eq!(
        top_level_keys(&text),
        [
            "id",
            "path",
            "provisional",
            "facets",
            "definition",
            "kind",
            "module",
            "public",
            "location",
            "verification_policy",
            "signature",
            "contract",
            "effects",
        ]
    );

    let callers = out.callers.as_ref().expect("callers included");
    assert_eq!(
        block_paths(callers),
        [
            "reader.pages.paginate_ints",
            "tests.pagination.\"pagination handles empty input\"",
            "tests.pagination.\"pagination preserves a short tail\"",
            "tests.pagination.\"pagination preserves input\"",
        ]
    );
    // 6.4: `paginate_ints` calls it at 75:8, `paginate(items, checked_size)`.
    assert_eq!(
        callers[0].call_sites.as_deref(),
        Some(&[loc("src/reader/pages.cpl", 75, 8, 75, 37)][..])
    );
    assert_eq!(callers[1].kind, "test");
    assert_eq!(callers[3].kind, "prop");
    for caller in callers {
        // Caller blocks carry no facet texts and no relation.
        assert_eq!(caller.signature, None);
        assert_eq!(caller.relation, None);
        assert_eq!(
            caller.version,
            *index.version(def(&index, &caller.version.path))
        );
    }
    assert_eq!(
        callers[2].call_sites.as_deref(),
        Some(&[loc("src/tests/pagination.cpl", 8, 12, 8, 34)][..])
    );
}

/// The default set: the target with `signature`, `contract`, `effects`,
/// `docs`, and its interface dependencies with the same four (6.2, T17).
#[test]
fn the_default_set_gives_the_interface_dependencies_with_the_same_four() {
    let checked = Checked::example("reader");
    let index = checked.index();
    let out = inspect(&index, "reader.pages.paginate", &IncludeSet::default_set());
    let json = to_json(&out);
    assert_eq!(keys(&json), ["include", "interface_dependencies", "symbol"]);
    assert_eq!(
        json["include"],
        json!([
            "signature",
            "contract",
            "effects",
            "docs",
            "interface_dependencies"
        ])
    );
    assert_eq!(
        out.symbol.docs,
        Some(None),
        "docs included: null without docs"
    );
    let deps = out.interface_dependencies.as_ref().expect("default set");
    assert_eq!(
        block_paths(deps),
        [
            "reader.pages.Positive",
            "std.prelude.List",
            "std.prelude.List.drop_last",
            "std.prelude.List.len",
            "std.prelude.flatten",
        ]
    );
    let positive = &deps[0];
    assert_eq!(positive.kind, "ty");
    assert_eq!(positive.relation, Some(Relation::TypeUse));
    assert_eq!(
        positive.interface, None,
        "no `interface` in the interface list"
    );
    assert_eq!(
        positive.signature.as_deref(),
        Some("pub type Positive = Int")
    );
    assert_eq!(
        positive.contract.as_deref(),
        Some(&["where self > 0".to_string()][..])
    );
    // "each with the same four" (6.2): a refinement has no effects facet
    // (5.1), so its `effects` is the empty set, the key being a function of
    // the include set alone (6.2, 6.4's table).
    assert_eq!(positive.effects.as_deref(), Some(&[][..]));
    assert_eq!(positive.docs, Some(None));
    assert_eq!(positive.verification_policy, None, "not a function");
    let flatten = &deps[4];
    assert_eq!(flatten.relation, Some(Relation::Call));
    assert_eq!(
        flatten.signature.as_deref(),
        Some("pub fn flatten<T>(lists: List<List<T>>) -> List<T>")
    );
    assert_eq!(flatten.location.file, "std/prelude.cpl");
    assert_eq!(flatten.effects.as_deref(), Some(&[][..]));
    assert_eq!(
        flatten.docs,
        Some(Some(
            "Concatenation of a list of lists, in order.".to_string()
        ))
    );
    assert_eq!(
        flatten.verification_policy,
        Some(VerificationPolicyJson { prove: false })
    );
    // Naming the four explicitly yields no dependency list (6.2).
    let explicit = inspect(
        &index,
        "reader.pages.paginate",
        &IncludeSet::parse("signature,contract,effects,docs").expect("parses"),
    );
    assert_eq!(explicit.interface_dependencies, None);
    assert_eq!(explicit.symbol, out.symbol);
}

/// Each include item adds exactly its key (6.2, 6.4).
#[test]
fn every_include_item_adds_its_key() {
    let checked = Checked::example("reader");
    let index = checked.index();
    let block_keys = |item: &str| -> (Vec<String>, Vec<String>) {
        let out = inspect(
            &index,
            "reader.pages.paginate",
            &IncludeSet::parse(item).expect("parses"),
        );
        let json = to_json(&out);
        let top: Vec<String> = keys(&json).into_iter().map(String::from).collect();
        let symbol: Vec<String> = keys(&json["symbol"])
            .into_iter()
            .map(String::from)
            .collect();
        (top, symbol)
    };
    let always = [
        "definition",
        "facets",
        "id",
        "kind",
        "location",
        "module",
        "path",
        "provisional",
        "public",
        "verification_policy",
    ];
    let with = |extra: &[&str]| -> Vec<String> {
        let mut all: Vec<String> = always.iter().chain(extra).map(|s| s.to_string()).collect();
        all.sort();
        all
    };
    for (item, extra) in [
        ("signature", &["signature"][..]),
        ("contract", &["contract"]),
        ("effects", &["effects"]),
        ("docs", &["docs"]),
        ("implementation", &["holes", "implementation"]),
    ] {
        let (top, symbol) = block_keys(item);
        assert_eq!(top, ["include", "symbol"], "{item}");
        assert_eq!(symbol, with(extra), "{item}");
    }
    for (item, key) in [
        ("callers", "callers"),
        ("dependencies", "dependencies"),
        ("obligations", "obligations"),
        ("tests", "tests"),
    ] {
        let (top, symbol) = block_keys(item);
        let mut expected = vec!["include", key, "symbol"];
        expected.sort();
        assert_eq!(top, expected, "{item}");
        assert_eq!(symbol, with(&[]), "{item}");
    }

    // implementation: the body's canonical text and its holes.
    let out = inspect(
        &index,
        "reader.pages.divide_exactly",
        &IncludeSet::parse("implementation").expect("parses"),
    );
    assert_eq!(
        out.symbol.implementation.as_deref(),
        Some("{\n    a / b\n}")
    );
    assert_eq!(out.symbol.holes.as_deref(), Some(&[][..]));
    // ... not for a kind without a body.
    let out = inspect(
        &index,
        "reader.pages.Page",
        &IncludeSet::parse("implementation,contract,effects,signature").expect("parses"),
    );
    assert_eq!(out.symbol.implementation, None);
    assert_eq!(out.symbol.holes, None);
    // 6.4 limits only `implementation` and `verification_policy` by kind:
    // a struct, which has neither a contract nor an effects facet (5.1),
    // has no clauses and no effects once they are included.
    assert_eq!(out.symbol.contract.as_deref(), Some(&[][..]));
    assert_eq!(out.symbol.effects.as_deref(), Some(&[][..]));
    assert_eq!(
        out.symbol.signature.as_deref(),
        Some("pub struct Page {\n    id: PageId\n    text: Text\n}")
    );

    // dependencies: every reference target with its relation and interface flag.
    let out = inspect(
        &index,
        "reader.pages.paginate",
        &IncludeSet::parse("dependencies").expect("parses"),
    );
    let deps = out.dependencies.expect("included");
    assert_eq!(
        deps.iter()
            .map(|b| (b.version.path.as_str(), b.relation, b.interface))
            .collect::<Vec<_>>(),
        [
            ("reader.pages.Positive", Some(Relation::TypeUse), Some(true)),
            ("std.prelude.List", Some(Relation::TypeUse), Some(true)),
            ("std.prelude.List.drop", Some(Relation::Call), Some(false)),
            (
                "std.prelude.List.drop_last",
                Some(Relation::Call),
                Some(true)
            ),
            (
                "std.prelude.List.is_empty",
                Some(Relation::Call),
                Some(false)
            ),
            ("std.prelude.List.len", Some(Relation::Call), Some(true)),
            ("std.prelude.List.push", Some(Relation::Call), Some(false)),
            ("std.prelude.List.take", Some(Relation::Call), Some(false)),
            ("std.prelude.flatten", Some(Relation::Call), Some(true)),
        ]
    );
    for dep in &deps {
        // signature and contract always (6.2), effects and docs only when included.
        assert!(dep.signature.is_some(), "{}", dep.version.path);
        assert!(dep.contract.is_some(), "{}", dep.version.path);
        assert_eq!(dep.docs, None);
        assert_eq!(dep.effects, None);
        assert_eq!(dep.call_sites, None);
    }
    assert_eq!(
        deps[7].contract.as_deref(),
        Some(
            &[
                "requires n >= 0".to_string(),
                "ensures result.len == n.min(list.len)".to_string(),
                "ensures result ++ list.drop(n) == list".to_string(),
            ][..]
        )
    );
    // "each with its own `symbol` block including signature and contract"
    // (6.2): a builtin type has no contract facet (5.1), so no clauses.
    assert_eq!(deps[1].contract.as_deref(), Some(&[][..]));

    // tests: direct, or transitive through the call closure.
    let out = inspect(
        &index,
        "std.prelude.flatten",
        &IncludeSet::parse("tests").expect("parses"),
    );
    assert_eq!(
        out.tests
            .expect("included")
            .iter()
            .map(|b| (b.version.path.as_str(), b.relation))
            .collect::<Vec<_>>(),
        [
            (
                "tests.pagination.\"pagination handles empty input\"",
                Some(Relation::Transitive)
            ),
            (
                "tests.pagination.\"pagination preserves a short tail\"",
                Some(Relation::Transitive)
            ),
            (
                "tests.pagination.\"pagination preserves input\"",
                Some(Relation::Direct)
            ),
        ]
    );

    // obligations: Section 8.1's list for the target.
    let out = inspect(
        &index,
        "reader.pages.paginate",
        &IncludeSet::parse("obligations").expect("parses"),
    );
    assert_eq!(
        out.obligations.expect("included"),
        index.obligations(def(&index, "reader.pages.paginate"))
    );

    // changes: without a base (no repository) the response is refused.
    let error = index
        .inspect(
            &Target::parse("paginate"),
            &IncludeSet::parse("changes").expect("parses"),
        )
        .expect_err("no base");
    assert_eq!(error.code, "git_not_a_repository");
    assert_eq!(error.category, Category::Git);
}

/// A property's header, a test's, and a capability's signature, as "the
/// header with its name" (6.4); a test has no signature facet (5.1), but
/// the key is written whenever `signature` is included (6.4's table).
#[test]
fn signatures_carry_the_declaration_name() {
    let checked = Checked::example("reader");
    let index = checked.index();
    let signature = |target: &str| {
        inspect(
            &index,
            target,
            &IncludeSet::parse("signature").expect("parses"),
        )
        .symbol
        .signature
    };
    assert_eq!(
        signature("tests.pagination.\"pagination preserves input\"").as_deref(),
        Some(
            "property \"pagination preserves input\"\n    for items in gen.list(gen.int)\n    for size in gen.positive_int"
        )
    );
    assert_eq!(
        signature("tests.pagination.\"pagination handles empty input\"").as_deref(),
        Some("test \"pagination handles empty input\"")
    );
    assert_eq!(
        signature("reader.pages.PageRead").as_deref(),
        Some("pub capability PageRead {\n    fn get(id: PageId) -> Result<Page, StoreError>\n}")
    );
    assert_eq!(
        signature("reader.pages.PageId").as_deref(),
        Some("pub type PageId(Text)")
    );
    assert_eq!(
        signature("reader.pages.clean").as_deref(),
        Some("fn clean(page: Page) -> Page")
    );
    assert_eq!(
        signature("reader.pages.load_page").as_deref(),
        Some("pub fn load_page(id: PageId, using db: PageRead) -> Result<Page, StoreError>")
    );
    assert_eq!(
        signature("std.prelude.List.take").as_deref(),
        Some("pub fn take<T>(list: List<T>, n: Int) -> List<T>")
    );
    assert_eq!(
        signature("std.prelude.Option").as_deref(),
        Some("pub enum Option<T> {\n    Some(T)\n    None\n}")
    );
    let effects = inspect(
        &index,
        "reader.pages.load_text",
        &IncludeSet::parse("effects").expect("parses"),
    )
    .symbol
    .effects;
    assert_eq!(effects.as_deref(), Some(&["db.get".to_string()][..]));
}

/// A capability's contract lists each operation's clauses (6.4, 5.2).
#[test]
fn capability_contracts_name_their_operations() {
    let checked = Checked::files(&[(
        "src/shop/store.cpl",
        "module shop.store

pub capability Store {
    fn get(key: Text) -> Int
        requires key.len > 0
        ensures result >= 0
    fn put(key: Text, value: Int) -> Bool
        requires value >= 0
}
",
    )]);
    checked.assert_no_errors();
    let index = checked.index();
    let out = inspect(
        &index,
        "shop.store.Store",
        &IncludeSet::parse("contract").expect("parses"),
    );
    assert_eq!(
        out.symbol.contract.as_deref(),
        Some(
            &[
                "fn get requires key.len > 0".to_string(),
                "fn get ensures result >= 0".to_string(),
                "fn put requires value >= 0".to_string(),
            ][..]
        )
    );
}

// ---------------------------------------------------------------------------
// 6.1: targets
// ---------------------------------------------------------------------------

#[test]
fn targets_parse_into_the_forms_of_section_6_1() {
    assert_eq!(
        Target::parse("pgn_7c2d"),
        Target::Id(StableId::parse("pgn_7c2d").expect("id"))
    );
    assert_eq!(
        Target::parse("fn_k7w3qz2mb6ptx4ve"),
        Target::Id(StableId::parse("fn_k7w3qz2mb6ptx4ve").expect("id"))
    );
    assert_eq!(
        Target::parse("reader.pages.paginate"),
        Target::Path(SymbolPath::parse("reader.pages.paginate").expect("path"))
    );
    assert_eq!(
        Target::parse("tests.pagination.\"pagination handles empty input\""),
        Target::Path(
            SymbolPath::parse("tests.pagination.\"pagination handles empty input\"").expect("path")
        )
    );
    assert_eq!(
        Target::parse("paginate"),
        Target::Name(DeclName::Ident("paginate".into()))
    );
    assert_eq!(
        Target::parse("Page"),
        Target::Name(DeclName::Ident("Page".into()))
    );
    assert_eq!(
        Target::parse("List.take"),
        Target::Name(DeclName::Member {
            receiver: "List".into(),
            name: "take".into()
        })
    );
    assert_eq!(
        Target::parse("\"pagination preserves input\""),
        Target::Name(DeclName::Text("pagination preserves input".into()))
    );
    assert_eq!(
        Target::parse("pagination preserves input"),
        Target::Name(DeclName::Text("pagination preserves input".into()))
    );
    assert_eq!(
        Target::parse("Foo.Bar"),
        Target::Name(DeclName::Text("Foo.Bar".into()))
    );
    // Display is the spelling diagnostics quote.
    for arg in [
        "pgn_7c2d",
        "reader.pages.paginate",
        "paginate",
        "List.take",
        "\"pagination preserves input\"",
    ] {
        assert_eq!(Target::parse(arg).to_string(), arg);
    }
}

#[test]
fn every_target_form_resolves() {
    let checked = Checked::example("reader");
    let index = checked.index();
    let path_of = |target: Target| -> String {
        let d = index
            .resolve(&target)
            .unwrap_or_else(|e| panic!("{target}: {e}"));
        index.version(d).path.clone()
    };
    // Row 1: a stable id.
    assert_eq!(path_of(Target::parse("pgn_7c2d")), "reader.pages.paginate");
    // Row 2: a provisional id, through the same table.
    assert_eq!(
        path_of(Target::Id(
            StableId::parse("reader.pages.clean").expect("id")
        )),
        "reader.pages.clean"
    );
    // ... and spelling an annotated declaration's qualified name: rows 2 and
    // 3 resolve identically.
    assert_eq!(
        path_of(Target::Id(
            StableId::parse("reader.pages.paginate").expect("id")
        )),
        "reader.pages.paginate"
    );
    // Row 3: qualified paths, tests and std members included.
    assert_eq!(
        path_of(Target::parse("reader.pages.paginate")),
        "reader.pages.paginate"
    );
    assert_eq!(
        path_of(Target::parse(
            "tests.pagination.\"pagination handles empty input\""
        )),
        "tests.pagination.\"pagination handles empty input\""
    );
    assert_eq!(
        path_of(Target::parse("std.prelude.List.take")),
        "std.prelude.List.take"
    );
    assert_eq!(path_of(Target::parse("std.text.trim")), "std.text.trim");
    // Row 4: bare names. `divide_exactly` has the accepted `@id` syntax but no
    // entry, so it is tried as a name.
    assert!(matches!(Target::parse("divide_exactly"), Target::Id(_)));
    assert_eq!(
        path_of(Target::parse("divide_exactly")),
        "reader.pages.divide_exactly"
    );
    assert_eq!(path_of(Target::parse("paginate")), "reader.pages.paginate");
    assert_eq!(path_of(Target::parse("Page")), "reader.pages.Page");
    // A path built without a module is a bare name.
    let bare = |name: &str| {
        Target::Path(SymbolPath::new(
            Vec::new(),
            DeclName::Ident(name.to_string()),
        ))
    };
    assert_eq!(bare("paginate").to_string(), "paginate");
    assert_eq!(path_of(bare("paginate")), "reader.pages.paginate");
    let d = index.resolve(&bare("nope")).expect_err("unknown");
    assert_eq!(d.data["name"], json!("nope"));
    assert!(d.notes.is_empty());
    assert_eq!(path_of(Target::parse("take")), "std.prelude.List.take");
    assert_eq!(path_of(Target::parse("List.take")), "std.prelude.List.take");
    assert_eq!(
        path_of(Target::parse("\"pagination preserves input\"")),
        "tests.pagination.\"pagination preserves input\""
    );
    assert_eq!(
        path_of(Target::parse("pagination preserves input")),
        "tests.pagination.\"pagination preserves input\""
    );
}

#[test]
fn ambiguous_and_unknown_targets_are_index_errors() {
    let checked = Checked::example("reader");
    let index = checked.index();
    let sm = &checked.project.source_map;

    let d = resolve_error(&index, Target::parse("len"));
    assert_eq!(d.code, "index_ambiguous_symbol");
    assert_eq!(d.category, Category::Index);
    assert_eq!(d.severity, Severity::Error);
    assert_eq!(d.span, None);
    assert_eq!(d.data["name"], json!("len"));
    assert_eq!(
        d.data["candidates"],
        json!([
            "std.prelude.Bytes.len",
            "std.prelude.List.len",
            "std.prelude.Text.len"
        ])
    );
    assert_eq!(d.notes.len(), 3);
    let first = d.notes[0].span.expect("the declaration");
    assert_eq!(sm.text(first), "len");
    assert_eq!(sm.location(first).file, "std/prelude.cpl");
    assert_eq!(
        d.message,
        "`len` names 3 declarations: std.prelude.Bytes.len, std.prelude.List.len, std.prelude.Text.len; name one by its qualified name"
    );
    // A member and a function of the same name.
    let d = resolve_error(&index, Target::parse("contains"));
    assert_eq!(
        d.data["candidates"],
        json!(["std.prelude.List.contains", "std.text.contains"])
    );

    for (arg, note) in [
        ("nope", None),
        ("pgn_7c2e", None),
        ("Foo.Bar", None),
        ("reader.pages.nope", None),
        ("reader.nowhere.thing", Some("there is no module `reader.nowhere`")),
        (
            "reader.pages.PageRead.get",
            Some("`reader.pages.PageRead.get` is an operation of the capability `reader.pages.PageRead`, which is the declaration to query"),
        ),
        ("\"no such test\"", None),
    ] {
        let d = resolve_error(&index, Target::parse(arg));
        // The target as it was read: `Foo.Bar` can only be a test name.
        let name = Target::parse(arg).to_string();
        assert_eq!(d.code, "index_unknown_symbol", "{arg}");
        assert_eq!(d.category, Category::Index);
        assert_eq!(d.severity, Severity::Error);
        assert_eq!(d.span, None);
        assert_eq!(d.data["name"], json!(name));
        assert_eq!(
            d.message,
            format!("no declaration `{name}` in the project or the standard library")
        );
        assert_eq!(d.notes.first().map(|n| n.message.as_str()), note, "{arg}");
        // inspect and prove report the same error.
        assert_eq!(
            index.inspect(&Target::parse(arg), &IncludeSet::default_set()),
            Err(d.clone())
        );
        assert_eq!(index.prove(&Target::parse(arg)).map(|_| ()), Err(d));
    }
}

#[test]
fn bare_names_across_modules_are_ambiguous() {
    let checked = Checked::files(&[
        (
            "src/a/x.cpl",
            "module a.x\n\npub fn twice(n: Int) -> Int {\n    n * 2\n}\n",
        ),
        (
            "src/b/y.cpl",
            "module b.y\n\npub fn twice(n: Int) -> Int {\n    n + n\n}\n",
        ),
        (
            "src/t/z.cpl",
            "module t.z\n\nuse a.x.{twice}\n\ntest \"twice\" {\n    assert twice(2) == 4\n}\n",
        ),
    ]);
    checked.assert_no_errors();
    let index = checked.index();
    let d = resolve_error(&index, Target::parse("twice"));
    assert_eq!(
        d.data["candidates"],
        json!(["a.x.twice", "b.y.twice", "t.z.\"twice\""])
    );
    assert_eq!(
        index
            .resolve(&Target::parse("\"twice\""))
            .map(|d| index.version(d).path.clone()),
        Ok("t.z.\"twice\"".to_string())
    );
    assert_eq!(
        index
            .resolve(&Target::parse("b.y.twice"))
            .map(|d| index.version(d).path.clone()),
        Ok("b.y.twice".to_string())
    );
}

/// The later occurrence of a duplicated `@id` has its provisional identity
/// in the index, as in the `Program` (Section 4.5).
#[test]
fn a_duplicated_id_keeps_the_programs_identity() {
    let checked = Checked::files(&[(
        "src/dup/a.cpl",
        "module dup.a

@id(\"fn_aaaaaaaaaaaaaaaa\")
pub fn first() -> Int {
    1
}

@id(\"fn_aaaaaaaaaaaaaaaa\")
pub fn second() -> Int {
    2
}
",
    )]);
    assert!(checked
        .check
        .diagnostics
        .iter()
        .any(|d| d.code == "duplicate_identity"));
    let index = checked.index();
    let first = index
        .resolve(&Target::parse("fn_aaaaaaaaaaaaaaaa"))
        .expect("the first occurrence");
    assert_eq!(index.version(first).path, "dup.a.first");
    let second = def(&index, "dup.a.second");
    let version = index.version(second);
    assert_eq!(version.id.as_str(), "dup.a.second");
    assert!(version.provisional);
    assert_eq!(version.facets[&Facet::Identity], facet_hash("dup.a.second"));
    assert_eq!(version.definition, definition_hash(&version.facets));
    assert_eq!(
        index
            .resolve(&Target::Id(StableId::parse("dup.a.second").expect("id")))
            .expect("by its provisional identity"),
        second
    );
    // The syntax still shows the @id value: the facet map keeps the first.
    let map = index.facet_map();
    assert_eq!(
        map.get(&StableId::parse("fn_aaaaaaaaaaaaaaaa").expect("id"))
            .map(|e| e.version.path.as_str()),
        Some("dup.a.first")
    );
    assert!(index.partial().is_some(), "the project has errors");
}

/// Program-level versions are the facet map's (Section 5.4: one producer).
#[test]
fn versions_agree_with_the_facet_map_and_the_syntax() {
    let checked = Checked::example("reader");
    let index = checked.index();
    let map = facet_map(&checked.project, index.revision().clone());
    let program = index.program();
    for d in program.defs() {
        let version = index.version(d.id);
        assert_eq!(version.id, d.stable_id);
        assert_eq!(version.path, program.display_path(d.id));
        let module = &program.module(d.module).path;
        assert_eq!(*version, symbol_version(module, program.ast(d.id)));
        if program.module(d.module).origin != FileOrigin::Std {
            assert_eq!(map.get(&version.id).map(|e| &e.version), Some(version));
        }
    }
    assert_eq!(index.facet_map(), &map);
}

// ---------------------------------------------------------------------------
// 6.5: callers, dependencies, tests
// ---------------------------------------------------------------------------

#[test]
fn callers_and_dependencies_of_paginate() {
    let checked = Checked::example("reader");
    let index = checked.index();
    let paginate = def(&index, "reader.pages.paginate");
    assert_eq!(
        paths(&index, &index.callers(paginate)),
        [
            "reader.pages.paginate_ints",
            "tests.pagination.\"pagination handles empty input\"",
            "tests.pagination.\"pagination preserves a short tail\"",
            "tests.pagination.\"pagination preserves input\"",
        ]
    );
    let deps = index.dependencies(paginate);
    let summary: Vec<(String, RefKind, bool)> = deps
        .iter()
        .map(|d| (index.version(d.def).path.clone(), d.kind, d.interface))
        .collect();
    let expected: Vec<(String, RefKind, bool)> = [
        ("reader.pages.Positive", RefKind::TypeUse, true),
        ("std.prelude.List", RefKind::TypeUse, true),
        ("std.prelude.List.drop", RefKind::Call, false),
        ("std.prelude.List.drop_last", RefKind::Call, true),
        ("std.prelude.List.is_empty", RefKind::Call, false),
        ("std.prelude.List.len", RefKind::Call, true),
        ("std.prelude.List.push", RefKind::Call, false),
        ("std.prelude.List.take", RefKind::Call, false),
        ("std.prelude.flatten", RefKind::Call, true),
    ]
    .into_iter()
    .map(|(p, k, i)| (p.to_string(), k, i))
    .collect();
    assert_eq!(summary, expected);
    let interface: Vec<Dependency> = deps.iter().copied().filter(|d| d.interface).collect();
    assert_eq!(index.interface_dependencies(paginate), interface);
    assert_eq!(
        paths(&index, &index.tests_for(paginate)),
        [
            "tests.pagination.\"pagination handles empty input\"",
            "tests.pagination.\"pagination preserves a short tail\"",
            "tests.pagination.\"pagination preserves input\"",
        ]
    );
    // Positive is used by type, by `check<Positive>`, and from another module.
    let positive = def(&index, "reader.pages.Positive");
    assert_eq!(
        paths(&index, &index.callers(positive)),
        [
            "reader.images.Dimensions",
            "reader.images.image_width",
            "reader.pages.paginate",
            "reader.pages.paginate_ints",
            "reader.pages.positive_size",
        ]
    );
    let positive_size = def(&index, "reader.pages.positive_size");
    // `check<Positive>` is a type use and a check at one span: one site.
    assert_eq!(
        index.call_sites(positive_size, positive),
        [
            loc("src/reader/pages.cpl", 38, 40, 38, 48),
            loc("src/reader/pages.cpl", 40, 11, 40, 19),
        ]
    );
    assert!(
        index.tests_for(positive).is_empty(),
        "no call reaches a type"
    );
    // A capability operation is a call site of the capability.
    let read = def(&index, "reader.pages.PageRead");
    let load_page = def(&index, "reader.pages.load_page");
    assert_eq!(
        index.call_sites(load_page, read),
        [
            loc("src/reader/pages.cpl", 88, 15, 88, 23),
            loc("src/reader/pages.cpl", 92, 5, 92, 15),
        ]
    );
    // A recursive builtin axiom (`ensures trim(result) == result`) makes
    // `trim` its own caller and dependency, as 6.5's formulas say.
    let trim = def(&index, "std.text.trim");
    assert!(index.callers(trim).contains(&trim));
}

#[test]
fn callers_and_dependencies_of_summarize() {
    let checked = Checked::example("holes");
    let index = checked.index();
    let summarize = def(&index, "holes.pages.summarize");
    assert!(index.callers(summarize).is_empty());
    assert!(index.tests_for(summarize).is_empty());
    assert_eq!(
        index
            .dependencies(summarize)
            .iter()
            .map(|d| (index.version(d.def).path.as_str(), d.kind, d.interface))
            .collect::<Vec<_>>(),
        [
            ("holes.pages.Page", RefKind::TypeUse, true),
            ("std.prelude.Text", RefKind::TypeUse, true),
        ]
    );
    // Its implementation is a hole, reported with the implementation.
    let out = inspect(&index, "summarize", &everything());
    assert_eq!(
        out.symbol.implementation.as_deref(),
        Some("{\n    ?summary\n}")
    );
    let holes = out.symbol.holes.as_ref().expect("implementation included");
    assert_eq!(holes.len(), 1);
    assert_eq!(holes[0].hole, "summary");
    assert_eq!(holes[0].expected_type, "Text");
    assert_eq!(
        to_json(&holes[0].bindings),
        json!([{ "name": "page", "type": "Page", "kind": "parameter" }])
    );
    assert_eq!(
        block_paths(out.dependencies.as_ref().expect("included")),
        ["holes.pages.Page", "std.prelude.Text"]
    );
    assert_eq!(out.callers.as_deref(), Some(&[][..]));
    assert_eq!(out.tests.as_deref(), Some(&[][..]));
    assert_eq!(out.obligations.as_deref(), Some(&[][..]));

    // paginate's body is a hole: every dependency is an interface one.
    let paginate = def(&index, "holes.pages.paginate");
    assert!(index.callers(paginate).is_empty());
    let deps = index.dependencies(paginate);
    assert!(deps.iter().all(|d| d.interface));
    assert_eq!(
        deps.iter()
            .map(|d| index.version(d.def).path.as_str())
            .collect::<Vec<_>>(),
        [
            "holes.pages.Positive",
            "std.prelude.List",
            "std.prelude.List.drop_last",
            "std.prelude.List.len",
            "std.prelude.flatten",
        ]
    );
    let out = inspect(&index, "pgn_7c2d", &everything());
    let holes = out.symbol.holes.expect("included");
    assert_eq!(holes.len(), 1);
    assert_eq!(holes[0].hole, "build_pages");
    assert_eq!(holes[0].expected_type, "List<List<T>>");
    assert_eq!(holes[0].ensures.len(), 3);
}

const NESTED_GENERATORS: &str = "module m.g

pub fn limit() -> Int {
    10
}

property \"nested generators\"
    for xs in gen.list(gen.range(0, limit()))
    for o in gen.option(gen.list(gen.int))
{
    assert xs.len >= 0
}
";

/// Generator calls are ordinary calls (3.5) that the HIR lowers to
/// span-less `GenExpr`s (T18): each call site is still its own call
/// expression, read from the syntax, nested ones included, beside the HIR
/// calls of `gen.range`'s bounds and the `gen.int` property reads.
#[test]
fn nested_generator_calls_are_call_sites_of_their_own() {
    let checked = Checked::files(&[("src/m/g.cpl", NESTED_GENERATORS)]);
    checked.assert_no_errors();
    let index = checked.index();
    let file = "src/m/g.cpl";
    let property = def(&index, "m.g.\"nested generators\"");
    let sites = |callee: &str| index.call_sites(property, def(&index, callee));
    assert_eq!(
        sites("std.gen.list"),
        [loc(file, 8, 15, 8, 46), loc(file, 9, 25, 9, 42)]
    );
    assert_eq!(sites("std.gen.range"), [loc(file, 8, 24, 8, 45)]);
    assert_eq!(sites("m.g.limit"), [loc(file, 8, 37, 8, 44)]);
    assert_eq!(sites("std.gen.option"), [loc(file, 9, 14, 9, 43)]);
    assert_eq!(sites("std.gen.int"), [loc(file, 9, 34, 9, 41)]);
    let callers = inspect(
        &index,
        "std.gen.range",
        &IncludeSet::parse("callers").expect("parses"),
    )
    .callers
    .expect("included");
    assert_eq!(block_paths(&callers), ["m.g.\"nested generators\""]);
    assert_eq!(
        callers[0].call_sites.as_deref(),
        Some(&[loc(file, 8, 24, 8, 45)][..])
    );
}

// ---------------------------------------------------------------------------
// 8.1: obligations
// ---------------------------------------------------------------------------

const OBLIGATIONS: &str = "module ob.math

pub type Small = Int where self < 10

pub capability Store {
    fn get(key: Text) -> Int
        requires key.len > 0
}

fn half(n: Int) -> Int
    requires n % 2 == 0
    ensures result * 2 == n
{
    n / 2
}

fn count(n: Int) -> Int
    requires n >= 0
{
    if n == 0 {
        0
    } else {
        count(n - 1) + 1
    }
}

fn measured(n: Int) -> Int
    requires n >= 0
    decreases n
{
    if n == 0 {
        0
    } else {
        measured(n - 1)
    }
}

fn small() -> Small {
    3
}

fn loops(xs: List<Int>) -> Int {
    var i = 0
    while i < xs.len
        invariant i >= 0
    {
        assert i < xs.len
        i = i + 1
    }
    let _prefix = xs.take(i)
    i
}

fn read(using store: Store) -> Int
    effects { store.get }
{
    store.get(\"k\")
}
";

#[test]
fn obligations_are_numbered_in_body_order() {
    let checked = Checked::files(&[("src/ob/math.cpl", OBLIGATIONS)]);
    checked.assert_no_errors();
    let index = checked.index();
    let file = "src/ob/math.cpl";
    let summary = |path: &str| -> Vec<(String, String, String, ObligationStatus, Option<String>)> {
        index
            .obligations(def(&index, path))
            .into_iter()
            .map(|o| (o.id, o.kind, o.clause, o.status, o.reason))
            .collect()
    };
    let unknown = |id: &str, kind: &str, clause: &str| {
        (
            id.to_string(),
            kind.to_string(),
            clause.to_string(),
            ObligationStatus::Unknown,
            Some("verifier_unavailable".to_string()),
        )
    };
    // Divisions in clauses count before the body's (8.1: "clauses included").
    assert_eq!(
        summary("ob.math.half"),
        [
            unknown("ob.math.half#ensures/1", "ensures", "result * 2 == n"),
            unknown("ob.math.half#div/1", "division", "2 != 0"),
            unknown("ob.math.half#div/2", "division", "2 != 0"),
        ]
    );
    let half = index.obligations(def(&index, "ob.math.half"));
    assert_eq!(half[0].location, loc(file, 12, 5, 12, 28));
    assert_eq!(half[1].location, loc(file, 11, 14, 11, 19));
    assert_eq!(half[2].location, loc(file, 14, 5, 14, 10));
    assert_eq!(half[0].evidence, None);
    assert!(half[0].assumptions.is_empty());

    // A recursive function without a measure: termination, unknown (no_measure).
    assert!(index.is_recursive(def(&index, "ob.math.count")));
    assert!(!index.is_recursive(def(&index, "ob.math.half")));
    assert_eq!(
        summary("ob.math.count"),
        [
            (
                "ob.math.count#termination".to_string(),
                "termination".to_string(),
                String::new(),
                ObligationStatus::Unknown,
                Some("no_measure".to_string()),
            ),
            unknown("ob.math.count#call/1/requires/1", "call_requires", "n >= 0"),
        ]
    );
    let count = index.obligations(def(&index, "ob.math.count"));
    assert_eq!(count[0].location, loc(file, 17, 4, 17, 9), "the name token");
    assert_eq!(count[1].location, loc(file, 23, 9, 23, 21), "the call");

    assert!(index.is_recursive(def(&index, "ob.math.measured")));
    assert_eq!(
        summary("ob.math.measured"),
        [
            unknown("ob.math.measured#decreases", "decreases", "n"),
            unknown(
                "ob.math.measured#call/1/requires/1",
                "call_requires",
                "n >= 0"
            ),
        ]
    );

    // A literal proved against a refinement (S16): proved, by literal.
    let small = index.obligations(def(&index, "ob.math.small"));
    assert_eq!(small.len(), 1);
    assert_eq!(small[0].id, "ob.math.small#refine/1");
    assert_eq!(small[0].kind, "refinement");
    assert_eq!(small[0].clause, "self < 10");
    assert_eq!(small[0].status, ObligationStatus::Proved);
    assert_eq!(small[0].reason, None);
    assert_eq!(small[0].evidence, Some(json!({ "by": "literal" })));
    assert_eq!(small[0].location, loc(file, 39, 5, 39, 6));

    // Loops, calls in loop clauses, and asserts.
    assert_eq!(
        summary("ob.math.loops"),
        [
            unknown(
                "ob.math.loops#loop/1/invariant/1/init",
                "invariant_init",
                "i >= 0"
            ),
            unknown(
                "ob.math.loops#loop/1/invariant/1/preserve",
                "invariant_preserve",
                "i >= 0"
            ),
            unknown("ob.math.loops#call/3/requires/1", "call_requires", "n >= 0"),
            unknown("ob.math.loops#assert/1", "assert", "i < xs.len"),
        ]
    );
    let loops = index.obligations(def(&index, "ob.math.loops"));
    assert_eq!(loops[0].location, loc(file, 45, 9, 45, 25));
    assert_eq!(loops[2].location, loc(file, 50, 19, 50, 29));
    assert_eq!(loops[3].location, loc(file, 47, 9, 47, 26), "from `assert`");

    // A capability operation's precondition is a call precondition.
    assert_eq!(
        summary("ob.math.read"),
        [unknown(
            "ob.math.read#call/1/requires/1",
            "call_requires",
            "key.len > 0"
        )]
    );

    // Only functions with bodies have obligations.
    assert!(index.obligations(def(&index, "ob.math.Small")).is_empty());
    assert!(index.obligations(def(&index, "ob.math.Store")).is_empty());
    assert!(index
        .obligations(def(&index, "std.prelude.List.take"))
        .is_empty());
}

// ---------------------------------------------------------------------------
// 7.2 cpl check: hole dependencies and verification receipts
// ---------------------------------------------------------------------------

/// `CheckOutput::holes` are the `hole` diagnostics' payloads (7.2).
#[test]
fn hole_payloads_are_the_hole_diagnostics() {
    let checked = Checked::example("holes");
    let index = checked.index();
    let holes: Vec<&cpl_diag::Diagnostic> = checked
        .check
        .diagnostics
        .iter()
        .filter(|d| d.code == "hole")
        .collect();
    assert_eq!(holes.len(), 2);
    for d in holes {
        let report = checked
            .check
            .holes
            .iter()
            .find(|h| Some(h.span) == d.span)
            .expect("a report");
        let mut payload = to_json(&index.hole_report_json(report));
        payload
            .as_object_mut()
            .expect("an object")
            .remove("dependencies");
        assert_eq!(payload, Value::Object(d.data.clone()), "{}", d.message);
    }
}

/// The hole reports are the check's (`CheckResult::holes`, 1.6), so an
/// index built by `Index::build` alone (1.8) cannot list "the holes inside
/// the body" (6.4): it refuses (`internal_error`, kind
/// `hole_reports_missing`) rather than answer that a holed body has none,
/// while it answers every other request, a body without holes included,
/// as the index with the reports does, and completes the check's `hole`
/// diagnostics from their `symbol` to the same `dependencies`.
#[test]
fn an_index_without_hole_reports_refuses_to_list_holes() {
    let checked = Checked::example("holes");
    let full = checked.index();
    let bare = Index::build(
        &checked.check.program,
        &checked.project,
        unversioned(&checked.project),
    );
    assert!(bare.holes().is_empty());
    let implementation = IncludeSet::parse("implementation").expect("parses");
    for target in ["holes.pages.paginate", "holes.pages.summarize"] {
        let error = bare
            .inspect(&Target::parse(target), &implementation)
            .expect_err("the holes are unknown");
        assert_eq!(error.code, "internal_error", "{target}");
        assert_eq!(error.category, Category::Internal);
        assert_eq!(error.severity, Severity::Error);
        assert_eq!(error.data["kind"], json!("hole_reports_missing"));
        assert!(error.message.contains(target), "{}", error.message);
        let default = IncludeSet::default_set();
        assert_eq!(
            bare.inspect(&Target::parse(target), &default)
                .expect("no holes asked for"),
            inspect(&full, target, &default)
        );
    }
    assert_eq!(
        bare.complete_check_diagnostics(&checked.check.diagnostics, false),
        full.complete_check_diagnostics(&checked.check.diagnostics, false)
    );
    assert_eq!(
        bare.with_holes(&checked.check.holes)
            .inspect(&Target::parse("holes.pages.summarize"), &implementation)
            .expect("reports given"),
        inspect(&full, "holes.pages.summarize", &implementation)
    );

    let reader = Checked::example("reader");
    let bare = Index::build(
        &reader.check.program,
        &reader.project,
        unversioned(&reader.project),
    );
    let out = bare
        .inspect(
            &Target::parse("reader.pages.divide_exactly"),
            &implementation,
        )
        .expect("a body without holes");
    assert_eq!(out.symbol.holes.as_deref(), Some(&[][..]));
    assert_eq!(
        out,
        inspect(
            &reader.index(),
            "reader.pages.divide_exactly",
            &implementation
        )
    );
}

#[test]
fn check_diagnostics_gain_hole_dependencies_and_receipts() {
    let checked = Checked::example("holes");
    let index = checked.index();
    let completed = index.complete_check_diagnostics(&checked.check.diagnostics, false);
    assert_eq!(completed.len(), checked.check.diagnostics.len());
    let std = checked.project.std_digest();
    let module = ModuleHash {
        module: "holes.pages".to_string(),
        content_hash: module_content_hash(
            &checked.project,
            &[checked
                .project
                .source_map
                .find("src/holes/pages.cpl")
                .expect("the file")],
        ),
    };
    for (before, after) in checked.check.diagnostics.iter().zip(&completed) {
        match before.code.as_str() {
            "hole" => {
                let deps = &after.data["dependencies"];
                assert_eq!(deps["source_revision"], json!("unversioned"));
                assert_eq!(deps.get("dirty"), None);
                assert_eq!(
                    deps["toolchain"],
                    json!({ "cpl": "0.1.0", "edition": "0.1", "std": std })
                );
                assert_eq!(deps["modules"], to_json(&[&module]));
                let mut rest = after.clone();
                rest.data.remove("dependencies");
                assert_eq!(&rest, before, "only `dependencies` is added");
            }
            "verification_result" => {
                let receipt = &after.data["receipt"];
                assert_eq!(
                    *receipt,
                    json!({
                        "source_revision": "unversioned",
                        "toolchain": { "cpl": "0.1.0", "edition": "0.1", "std": std },
                        "modules": [to_json(&module)],
                        "check_configuration": { "release": false },
                        "assumptions": [],
                    })
                );
                let mut rest = after.clone();
                rest.data.remove("receipt");
                assert_eq!(&rest, before);
            }
            _ => assert_eq!(after, before),
        }
    }
    let summarize_hole = completed
        .iter()
        .find(|d| d.data.get("hole") == Some(&json!("summary")))
        .expect("?summary");
    let symbols: Vec<&str> = summarize_hole.data["dependencies"]["symbols"]
        .as_array()
        .expect("symbols")
        .iter()
        .map(|s| s["path"].as_str().expect("path"))
        .collect();
    assert_eq!(symbols, ["holes.pages.Page", "std.prelude.Text"]);
    let page = def(&index, "holes.pages.Page");
    assert_eq!(
        summarize_hole.data["dependencies"]["symbols"][0],
        to_json(index.version(page))
    );

    // The summary of 7.2's example.
    let output = index.check_output(&checked.check, false);
    assert_eq!(
        to_json(&output.summary),
        json!({ "errors": 0, "warnings": 0, "holes": 2, "definitions": 5, "modules": 1, "files": 1 })
    );
    assert_eq!(
        to_json(&output.verification),
        json!([{ "symbol": "pgn_7c2d", "qualified_name": "holes.pages.paginate", "status": "unknown", "reason": "verifier_unavailable" }])
    );
    assert_eq!(output.holes.len(), 2);
    assert!(!output.release);
}

#[test]
fn release_checks_keep_their_receipts_and_counts() {
    let project = load_dir(&workspace_path("examples/holes"));
    let checked = Checked::with(
        project,
        &CheckConfig {
            release: true,
            ..CheckConfig::default()
        },
    );
    let index = checked.index();
    let completed = index.complete_check_diagnostics(&checked.check.diagnostics, true);
    let receipt = completed
        .iter()
        .find(|d| d.code == "verification_result")
        .map(|d| d.data["receipt"]["check_configuration"].clone());
    assert_eq!(receipt, Some(json!({ "release": true })));
    for d in completed.iter().filter(|d| d.code == "hole_in_release") {
        assert_eq!(d.data.get("dependencies"), None, "S47's fields only");
    }
    let output = index.check_output(&checked.check, true);
    assert!(output.release);
    assert_eq!(
        to_json(&output.summary),
        json!({ "errors": 3, "warnings": 1, "holes": 2, "definitions": 5, "modules": 1, "files": 1 })
    );
    assert_eq!(output.holes.len(), 2, "every report in both modes");
}

/// A hole's `modules` and `symbols` across modules (S48, 7.2).
#[test]
fn hole_dependencies_span_modules() {
    let checked = Checked::files(&[
        (
            "src/app/core.cpl",
            "module app.core

pub struct Item {
    name: Text
}

pub capability Shelf {
    fn find(name: Text) -> Item
    fn count() -> Int
}

pub fn label(item: Item) -> Text {
    item.name
}
",
        ),
        (
            "src/app/main.cpl",
            "module app.main

use app.core.{Item, Shelf, label}

fn show(item: Item, using shelf: Shelf) -> Text
    effects { shelf.find }
{
    let _found = shelf.find(label(item))
    ?rest
}
",
        ),
    ]);
    checked.assert_no_errors();
    let index = checked.index();
    assert_eq!(checked.check.holes.len(), 1);
    let report = &checked.check.holes[0];
    let deps = index.hole_dependencies(report);
    let file = |path: &str| checked.project.source_map.find(path).expect("the file");
    assert_eq!(
        deps.modules,
        [
            ModuleHash {
                module: "app.core".to_string(),
                content_hash: module_content_hash(&checked.project, &[file("src/app/core.cpl")]),
            },
            ModuleHash {
                module: "app.main".to_string(),
                content_hash: module_content_hash(&checked.project, &[file("src/app/main.cpl")]),
            },
        ]
    );
    assert_eq!(
        deps.symbols
            .iter()
            .map(|s| s.path.as_str())
            .collect::<Vec<_>>(),
        [
            "app.core.Item",
            "app.core.Shelf",
            "app.core.label",
            "std.prelude.Text"
        ]
    );
    let json = index.hole_report_json(report);
    assert_eq!(json.expected_type, "Text");
    assert_eq!(
        to_json(&json.capabilities),
        json!([{
            "parameter": "shelf",
            "capability": "app.core.Shelf",
            "operations": [
                { "name": "find", "signature": "fn find(name: Text) -> Item", "in_bound": true },
                { "name": "count", "signature": "fn count() -> Int", "in_bound": false },
            ],
        }])
    );
    assert_eq!(json.bindings[0].name, "_found");
    assert_eq!(json.dependencies, deps);
}

// ---------------------------------------------------------------------------
// 7.2 cpl prove, 8.1 verification receipts
// ---------------------------------------------------------------------------

#[test]
fn prove_reports_unknown_with_an_honest_receipt() {
    let checked = Checked::example("reader");
    let index = checked.index();
    let paginate = def(&index, "reader.pages.paginate");
    let proof = index.prove(&Target::parse("pgn_7c2d")).expect("a function");
    let out = &proof.output;
    assert_eq!(out.symbol, *index.version(paginate));
    assert_eq!(out.result, ProofStatus::Unknown);
    // T29: unknown exits 1, as `unsuccessful`.
    assert_eq!(proof.status(), Status::Unsuccessful);
    assert_eq!(out.obligations, index.obligations(paginate));
    assert!(out
        .obligations
        .iter()
        .all(|o| o.status == ObligationStatus::Unknown
            && o.reason.as_deref() == Some("verifier_unavailable")));

    let receipt = &out.receipt;
    assert_eq!(receipt.receipt, ReceiptKind::Verification);
    assert_eq!(receipt.schema, 1);
    assert_eq!(
        to_json(&receipt.verifier),
        json!({ "name": "none", "version": null, "configuration": {} })
    );
    assert_eq!(receipt.revision, *index.revision());
    assert_eq!(receipt.target, out.symbol);
    assert_eq!(receipt.status, ProofStatus::Unknown);
    assert_eq!(receipt.obligations, out.obligations);
    assert!(receipt.assumptions.is_empty());
    assert_eq!(
        to_json(&receipt.configuration),
        json!({ "release": false, "policy": { "prove": true }, "proof_timeout_ms": 10000, "admitted": [] })
    );
    let dependencies: Vec<String> = receipt
        .dependencies
        .iter()
        .map(|v| v.path.clone())
        .collect();
    assert_eq!(
        dependencies,
        paths(
            &index,
            &index
                .dependencies(paginate)
                .iter()
                .map(|d| d.def)
                .collect::<Vec<_>>()
        )
    );
    assert_eq!(
        receipt.inputs.sources.keys().collect::<Vec<_>>(),
        [
            "cpl.toml",
            "src/reader/images.cpl",
            "src/reader/pages.cpl",
            "src/tests/pagination.cpl",
            "work/pagination.patch",
            "work/pagination.work",
        ]
    );
    assert_eq!(receipt.inputs.lock, None);
    assert_eq!(receipt.inputs.std, checked.project.std_digest());
    assert_eq!(receipt.inputs.manifest, checked.project.manifest_digest);
    assert_eq!(receipt.duration_ms, 0);
    // The core is the `receipt` of the check's verification_result (Section 8).
    assert_eq!(
        receipt.core,
        index.verification_receipt_core(paginate, false)
    );
    let completed = index.complete_check_diagnostics(&checked.check.diagnostics, false);
    let from_check = completed
        .iter()
        .find(|d| d.code == "verification_result")
        .expect("paginate is @prove");
    assert_eq!(from_check.data["receipt"], to_json(&receipt.core));

    // The diagnostic of S36.
    let d = &proof.diagnostic;
    assert_eq!(d.code, "verification_result");
    assert_eq!(d.category, Category::Contract);
    assert_eq!(d.severity, Severity::Info);
    assert_eq!(d.message, "paginate: unknown (verifier unavailable)");
    assert_eq!(
        d.span.map(|s| checked.project.source_map.location(s)),
        Some(loc("src/reader/pages.cpl", 45, 8, 45, 16))
    );
    assert_eq!(d.symbol.as_ref().map(|s| s.as_str()), Some("pgn_7c2d"));
    assert_eq!(d.qualified_name.as_deref(), Some("reader.pages.paginate"));
    assert_eq!(d.facet, Some(Facet::VerificationPolicy));
    assert_eq!(d.data["status"], json!("unknown"));
    assert_eq!(d.data["reason"], json!("verifier_unavailable"));
    assert_eq!(d.data["receipt"], to_json(&receipt.core));
    // The check's own verification_result for paginate says the same.
    let mut ours = d.clone();
    ours.data.remove("receipt");
    let mut theirs = from_check.clone();
    theirs.data.remove("receipt");
    assert_eq!(ours, theirs);

    // A function without @prove still has its status reported.
    let proof = index
        .prove(&Target::parse("divide_exactly"))
        .expect("a function");
    assert_eq!(proof.output.result, ProofStatus::Unknown);
    assert_eq!(
        to_json(&proof.output.receipt.configuration.policy),
        json!({ "prove": false })
    );
    assert_eq!(proof.output.obligations.len(), 3);

    // Other kinds are not provable (exit 2).
    for target in [
        "reader.pages.Positive",
        "reader.pages.PageRead",
        "tests.pagination.\"pagination preserves input\"",
    ] {
        let d = index
            .prove(&Target::parse(target))
            .map(|_| ())
            .expect_err(target);
        assert_eq!(d.code, "usage_not_provable", "{target}");
        assert_eq!(d.category, Category::Usage);
        assert_eq!(d.qualified_name.as_deref(), Some(target));
    }
}

#[test]
fn a_recursive_function_without_a_measure_notes_its_termination() {
    let checked = Checked::files(&[("src/ob/math.cpl", OBLIGATIONS)]);
    let index = checked.index();
    let proof = index.prove(&Target::parse("count")).expect("a function");
    assert_eq!(proof.diagnostic.notes.len(), 1);
    assert!(proof.diagnostic.notes[0]
        .message
        .contains("termination is unknown"));
    let proof = index.prove(&Target::parse("measured")).expect("a function");
    assert!(proof.diagnostic.notes.is_empty());
}

#[test]
fn a_loop_without_a_measure_notes_its_termination() {
    // Semantics 5.4 and S36 (b): a `while` loop without `decreases` leaves
    // the function's termination unknown too (the other condition of
    // `missing_decreases`), and 5.5 puts the note on its result, although
    // no obligation states that loop's termination (8.1).
    let checked = Checked::files(&[("src/ob/math.cpl", OBLIGATIONS)]);
    let index = checked.index();
    let proof = index.prove(&Target::parse("loops")).expect("a function");
    assert_eq!(proof.diagnostic.notes.len(), 1);
    assert!(proof.diagnostic.notes[0]
        .message
        .contains("termination is unknown"));
    assert!(proof
        .output
        .obligations
        .iter()
        .all(|o| !matches!(o.kind.as_str(), "termination" | "loop_decreases")));
    for target in ["half", "small"] {
        let proof = index.prove(&Target::parse(target)).expect("a function");
        assert!(proof.diagnostic.notes.is_empty(), "{target}");
    }
    // For a `@prove` function the note is the check's own, which comes
    // with its `missing_decreases` warning.
    let walk = "module ob.walk\n\n@prove\nfn walk(xs: List<Int>) -> Int {\n    var rest = xs\n    while not rest.is_empty() {\n        rest = rest.drop(1)\n    }\n    0\n}\n";
    let checked = Checked::files(&[("src/ob/walk.cpl", walk)]);
    checked.assert_no_errors();
    let codes: Vec<&str> = checked
        .check
        .diagnostics
        .iter()
        .map(|d| d.code.as_str())
        .collect();
    assert!(codes.contains(&"missing_decreases"), "{codes:?}");
    let from_check = checked
        .check
        .diagnostics
        .iter()
        .find(|d| d.code == "verification_result")
        .expect("walk is @prove");
    let index = checked.index();
    let proof = index.prove(&Target::parse("walk")).expect("a function");
    assert_eq!(proof.diagnostic.notes.len(), 1);
    assert_eq!(proof.diagnostic.notes, from_check.notes);
}

// ---------------------------------------------------------------------------
// 8.2 test receipts
// ---------------------------------------------------------------------------

fn record(path: &str, kind: TestKind) -> TestCaseRecord {
    let property = kind == TestKind::Property;
    TestCaseRecord {
        symbol: StableId::parse(path).expect("an identity"),
        qualified_name: path.to_string(),
        kind,
        status: TestStatus::Tested,
        duration_ms: 3,
        seed: property.then_some(1),
        property_seed: property.then(|| "c0".repeat(32)),
        cases: property.then_some(100),
        failing_case: None,
        shrunk: None,
        fault: None,
        counterexamples: Vec::new(),
        receipt: None,
    }
}

const TESTS: [(&str, TestKind); 3] = [
    (
        "tests.pagination.\"pagination handles empty input\"",
        TestKind::Test,
    ),
    (
        "tests.pagination.\"pagination preserves a short tail\"",
        TestKind::Test,
    ),
    (
        "tests.pagination.\"pagination preserves input\"",
        TestKind::Property,
    ),
];

fn report() -> TestReport {
    TestReport {
        seed: 1,
        cases: 100,
        selected: 3,
        tested: 3,
        failed: 0,
        results: TESTS.iter().map(|(p, k)| record(p, *k)).collect(),
    }
}

fn run_config() -> TestRunConfig {
    TestRunConfig {
        seed: 1,
        cases: 100,
        max_shrinks: 1000,
        timeout_ms: 0,
        selector: TestSelectorSpec {
            namespace: Some(vec!["tests".to_string(), "pagination".to_string()]),
            name: None,
            id: None,
        },
        check_contracts: true,
        fail_fast: false,
    }
}

#[test]
fn test_records_get_receipts_and_the_run_a_test_receipt() {
    let checked = Checked::example("reader");
    let index = checked.index();
    let selected: Vec<DefId> = TESTS.iter().map(|(p, _)| def(&index, p)).collect();
    let mut report = report();
    let config = run_config();
    let receipt = index.complete_test_report(&mut report, &selected, &config, 10_000);

    let configuration = test_check_configuration(&config, 10_000);
    assert_eq!(
        configuration,
        json!({ "cases": 100, "max_shrinks": 1000, "max_call_depth": 10000, "seed": 1 })
    );
    let modules = index.modules_of([
        def(&index, "reader.pages.paginate"),
        def(&index, "tests.pagination.\"pagination preserves input\""),
    ]);
    assert_eq!(
        modules
            .iter()
            .map(|m| m.module.as_str())
            .collect::<Vec<_>>(),
        ["reader.pages", "tests.pagination"]
    );
    for (record, &test) in report.results.iter().zip(&selected) {
        let core = record.receipt.as_ref().expect("filled");
        assert_eq!(
            *core,
            index.test_receipt_core(&[test], configuration.clone())
        );
        // Semantics 11: each receipt's modules include reader.pages and tests.pagination.
        assert_eq!(core.modules, modules);
        assert_eq!(core.check_configuration, configuration);
        assert_eq!(core.source_revision, "unversioned");
        assert_eq!(core.fixtures.as_deref(), Some(&[][..]));
        assert_eq!(core.environment, Some(test_environment()));
        assert!(core.assumptions.is_empty());
    }
    assert_eq!(
        test_environment().keys().collect::<Vec<_>>(),
        ["arch", "os"]
    );
    assert_eq!(receipt.receipt, ReceiptKind::Test);
    assert_eq!(
        receipt.core,
        index.test_receipt_core(&selected, configuration)
    );
    assert_eq!(receipt.configuration, config);
    assert_eq!(receipt.status, TestStatus::Tested);
    assert_eq!(
        to_json(&receipt.summary),
        json!({ "selected": 3, "tested": 3, "failed": 0 })
    );
    assert_eq!(receipt.tests.len(), 3);
    assert_eq!(
        receipt.tests[2].property_seed.as_deref(),
        Some("c0".repeat(32).as_str())
    );
    let tested: Vec<&str> = receipt.tested.iter().map(|v| v.path.as_str()).collect();
    let mut sorted = tested.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(tested, sorted, "sorted by display path, one per id");
    for path in [
        "reader.pages.paginate",
        "std.prelude.List.take",
        "std.prelude.flatten",
        "std.gen.positive_int",
        "tests.pagination.\"pagination preserves input\"",
    ] {
        assert!(tested.contains(&path), "{path} in {tested:?}");
    }
    assert!(
        !tested.contains(&"reader.pages.divide_exactly"),
        "not in the call closure"
    );
    assert_eq!(
        receipt
            .tested
            .iter()
            .find(|v| v.path == "reader.pages.paginate")
            .map(|v| v.id.as_str()),
        Some("pgn_7c2d")
    );
}

// ---------------------------------------------------------------------------
// index_partial
// ---------------------------------------------------------------------------

#[test]
fn a_project_with_errors_is_answered_with_index_partial() {
    let checked = Checked::files(&[(
        "src/bad/m.cpl",
        "module bad.m

pub fn broken() -> Int {
    \"text\"
}

pub fn user() -> Int {
    broken()
}
",
    )]);
    assert!(checked.check.has_errors());
    let index = checked.index();
    let partial = index.partial().expect("the project has errors");
    assert_eq!(partial.code, "index_partial");
    assert_eq!(partial.category, Category::Index);
    assert_eq!(partial.severity, Severity::Warning);
    assert_eq!(partial.span, None);
    let out = inspect(&index, "broken", &everything());
    assert_eq!(
        block_paths(out.callers.as_ref().expect("included")),
        ["bad.m.user"]
    );
    // A clean project is complete.
    let reader = Checked::example("reader");
    assert!(reader.index().partial().is_none());
    // So is one checked by statuses alone (Index::build without the check).
    let built = Index::build(
        &checked.check.program,
        &checked.project,
        unversioned(&checked.project),
    );
    assert!(built.partial().is_some(), "broken has errors");

    // A file that does not parse leaves a program without definitions
    // (semantics S52): every target is unknown, and the index is partial.
    let unparsed = Checked::files(&[("src/bad/p.cpl", "module bad.p\n\npub fn (\n")]);
    assert!(unparsed.check.program.defs.is_empty());
    let index = unparsed.index();
    assert!(index.partial().is_some());
    let d = index
        .inspect(&Target::parse("std.text.trim"), &IncludeSet::default_set())
        .expect_err("nothing was checked");
    assert_eq!(d.code, "index_unknown_symbol");
    assert!(index.facet_map().entries.is_empty());
}

// ---------------------------------------------------------------------------
// Revisions: git repositories in temporary directories
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

/// A repository in a fresh temporary directory, driven through the git
/// command line with the identity in the child's environment.
struct Repo {
    dir: PathBuf,
}

impl Repo {
    fn init(name: &str) -> Repo {
        let repo = Repo {
            dir: temp_dir(name),
        };
        repo.git(&["init", "-q", "-b", "main"]);
        repo
    }

    fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.dir)
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

    /// Commit everything; the new HEAD.
    fn commit(&self, message: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&[
            "commit",
            "-q",
            "--no-verify",
            "--no-gpg-sign",
            "-m",
            message,
        ]);
        self.git(&["rev-parse", "HEAD"])
    }

    fn repo(&self) -> GitRepo {
        discover_repo(&self.dir)
            .expect("git runs")
            .expect("a repository")
    }
}

/// Copy a corpus project's files into `dir`.
fn copy_example(name: &str, dir: &Path) {
    let tree = tree_of_dir(&workspace_path(&format!("examples/{name}")));
    for (path, bytes) in &tree.files {
        let full = dir.join(path);
        std::fs::create_dir_all(full.parent().expect("a parent")).expect("create directories");
        std::fs::write(&full, bytes).expect("write");
    }
}

/// The holes project at a revision: the hole's `dependencies` and the
/// verification receipt read `source_revision` and `dirty` from it (S48).
fn evidence_at(project: Project, revision: Revision) -> (Value, Value) {
    let checked = Checked::of(project);
    let index = checked.index_at(revision);
    let completed = index.complete_check_diagnostics(&checked.check.diagnostics, false);
    let hole = completed
        .iter()
        .find(|d| d.code == "hole")
        .map(|d| d.data["dependencies"].clone())
        .expect("a hole");
    let receipt = completed
        .iter()
        .find(|d| d.code == "verification_result")
        .map(|d| d.data["receipt"].clone())
        .expect("a verification result");
    (hole, receipt)
}

#[test]
fn revision_states_reach_hole_dependencies_and_receipts() {
    // No repository: unversioned, never dirty.
    let plain = temp_dir("revision-none");
    copy_example("holes", &plain);
    if git_available() {
        assert_eq!(discover_repo(&plain).expect("git runs"), None);
    }
    let project = load_dir(&plain);
    let revision = current_revision(&project, None).expect("no git needed");
    assert_eq!(revision.commit, None);
    assert_eq!(revision.state, RevisionState::Worktree);
    let (hole, receipt) = evidence_at(project, revision);
    for evidence in [&hole, &receipt] {
        assert_eq!(evidence["source_revision"], json!("unversioned"));
        assert_eq!(evidence.get("dirty"), None);
    }

    if !git_available() {
        return;
    }
    // A clean checkout: the commit, not dirty.
    let repo = Repo::init("revision-clean");
    copy_example("holes", &repo.dir);
    let head = repo.commit("holes");
    let git = repo.repo();
    let project = load_dir(&repo.dir);
    let revision = current_revision(&project, Some(&git)).expect("revision");
    assert_eq!(revision.commit.as_deref(), Some(head.as_str()));
    assert_eq!(revision.state, RevisionState::Clean);
    assert_eq!(revision.digest, source_digest(&project));
    let (hole, receipt) = evidence_at(project, revision.clone());
    for evidence in [&hole, &receipt] {
        assert_eq!(evidence["source_revision"], json!(head));
        assert_eq!(evidence.get("dirty"), None);
    }
    let checked = Checked::of(load_dir(&repo.dir));
    let proof = checked
        .index_at(revision.clone())
        .prove(&Target::parse("paginate"))
        .expect("a function");
    assert_eq!(proof.output.receipt.revision, revision);
    assert_eq!(proof.output.receipt.core.source_revision, head);

    // The working tree changed: the same commit, dirty.
    let pages = repo.dir.join("src/holes/pages.cpl");
    let text = std::fs::read_to_string(&pages).expect("read");
    std::fs::write(&pages, format!("{text}\n// an edit\n")).expect("write");
    let project = load_dir(&repo.dir);
    let revision = current_revision(&project, Some(&git)).expect("revision");
    assert_eq!(revision.commit.as_deref(), Some(head.as_str()));
    assert_eq!(revision.state, RevisionState::Worktree);
    let (hole, receipt) = evidence_at(project, revision);
    for evidence in [&hole, &receipt] {
        assert_eq!(evidence["source_revision"], json!(head));
        assert_eq!(evidence["dirty"], json!(true));
    }
}

#[test]
fn changes_compare_with_the_since_revision() {
    assert_eq!(DEFAULT_SINCE, "HEAD~1");
    let checked = Checked::example("reader");
    let error = changes_base(None, DEFAULT_SINCE).expect_err("no repository");
    assert_eq!(error.code, "git_not_a_repository");
    drop(checked);
    if !git_available() {
        return;
    }
    let repo = Repo::init("changes");
    std::fs::write(repo.dir.join("README.md"), "no project yet\n").expect("write");
    let empty = repo.commit("readme");
    copy_example("reader", &repo.dir);
    let base = repo.commit("reader");
    let pages = repo.dir.join("src/reader/pages.cpl");
    let text = std::fs::read_to_string(&pages).expect("read");
    let edited = text
        .replace(
            "ensures forall page in result.drop_last(): page.len == size",
            "ensures forall page in result.drop_last(): page.len <= size",
        )
        .replace(
            "Page { ..page, text: trim(page.text) }",
            "Page { ..page, text: trim(trim(page.text)) }",
        );
    assert_ne!(edited, text);
    std::fs::write(&pages, edited).expect("write");
    let head = repo.commit("edit");
    let git = repo.repo();

    let checked = Checked::of(load_dir(&repo.dir));
    checked.assert_no_errors();
    let revision = current_revision(&checked.project, Some(&git)).expect("revision");
    assert_eq!(revision.commit.as_deref(), Some(head.as_str()));
    let since = changes_base(Some(&git), DEFAULT_SINCE).expect("HEAD~1");
    assert_eq!(since.revision.commit.as_deref(), Some(base.as_str()));
    assert_eq!(since.revision.state, RevisionState::Clean);
    let at_base = cpl_project::load_project_from_tree(
        &cpl_index::tree_at(&git, &base).expect("tree"),
        FileOrigin::Revision(base.clone()),
        &cpl_project::LoadOptions::default(),
    )
    .expect("load");
    assert_eq!(since.revision.digest, source_digest(&at_base));

    let index = checked.index_at(revision).with_changes_base(since.clone());
    let changes = |target: &str| {
        inspect(
            &index,
            target,
            &IncludeSet::parse("changes").expect("parses"),
        )
        .changes
        .expect("included")
    };
    let paginate = changes("paginate");
    assert_eq!(paginate.since, since.revision);
    assert_eq!(
        paginate
            .statements
            .iter()
            .map(|s| (s.statement, s.id.as_str(), s.facet))
            .collect::<Vec<_>>(),
        [(
            DiffStatementKind::ContractChanged,
            "pgn_7c2d",
            Some(Facet::Contract)
        )]
    );
    let clean = changes("reader.pages.clean");
    assert_eq!(
        clean
            .statements
            .iter()
            .map(|s| s.statement)
            .collect::<Vec<_>>(),
        [DiffStatementKind::BodyChanged]
    );
    assert!(changes("divide_exactly").statements.is_empty());
    assert!(
        changes("std.text.trim").statements.is_empty(),
        "std is no revision"
    );

    // Since HEAD: nothing changed.
    let at_head = facet_map_at(&git, "HEAD").expect("HEAD");
    let index = checked
        .index_at(current_revision(&checked.project, Some(&git)).expect("revision"))
        .with_changes_base(at_head);
    assert!(inspect(
        &index,
        "paginate",
        &IncludeSet::parse("changes").expect("parses")
    )
    .changes
    .expect("included")
    .statements
    .is_empty());

    // The errors of `--since`.
    let d = facet_map_at(&git, "no-such-revision").expect_err("unknown");
    assert_eq!(d.code, "git_unknown_revision");
    assert_eq!(d.category, Category::Git);
    assert_eq!(d.data["revision"], json!("no-such-revision"));
    let d = facet_map_at(&git, "-x").expect_err("option-like");
    assert_eq!(d.code, "git_unknown_revision");
    let d = facet_map_at(&git, &empty).expect_err("no cpl.toml there");
    assert_eq!(d.code, "git_no_project_at_revision");
    assert_eq!(d.data["commit"], json!(empty));
}

#[test]
fn git_errors_map_to_the_git_codes() {
    let d = git_error_diagnostic(&GitError::NotARepository("/x".to_string()));
    assert_eq!(d.code, "git_not_a_repository");
    assert_eq!(d.data["path"], json!("/x"));
    let d = git_error_diagnostic(&GitError::UnknownRevision("v9".to_string()));
    assert_eq!(d.code, "git_unknown_revision");
    assert_eq!(d.message, "unknown revision `v9`");
    let d = git_error_diagnostic(&GitError::Command {
        args: vec!["ls-tree".to_string(), "HEAD".to_string()],
        status: 128,
        stderr: "fatal: bad".to_string(),
    });
    assert_eq!(d.code, "git_command_failed");
    assert_eq!(d.data["args"], json!(["ls-tree", "HEAD"]));
    assert_eq!(d.data["exit_status"], json!(128));
    assert_eq!(d.data["stderr"], json!("fatal: bad"));
    assert_eq!(d.data.get("status"), None, "status is S50's key");
    for error in [
        GitError::NotInstalled,
        GitError::Io(std::io::Error::other("boom")),
    ] {
        let d = git_error_diagnostic(&error);
        assert_eq!(d.code, "git_command_failed");
        assert_eq!(d.category, Category::Git);
        assert_eq!(d.severity, Severity::Error);
    }
}

// ---------------------------------------------------------------------------
// Receipt digests (Section 8)
// ---------------------------------------------------------------------------

#[test]
fn receipt_digests_are_stable_across_builds() {
    let digests = || {
        let checked = Checked::example("reader");
        let index = checked.index();
        let proof = index.prove(&Target::parse("paginate")).expect("a function");
        let selected: Vec<DefId> = TESTS.iter().map(|(p, _)| def(&index, p)).collect();
        let mut report = report();
        let test = index.complete_test_report(&mut report, &selected, &run_config(), 10_000);
        (proof.output.receipt, test, report)
    };
    let (verification_a, test_a, report_a) = digests();
    let (mut verification_b, mut test_b, report_b) = digests();
    // The informational fields are outside the digest.
    verification_b.produced_at = "1999-12-31T23:59:59Z".to_string();
    verification_b.duration_ms = 1234;
    test_b.produced_at = "1999-12-31T23:59:59Z".to_string();
    test_b.duration_ms = 99;
    assert_eq!(
        receipt_digest(&to_json(&verification_a)),
        receipt_digest(&to_json(&verification_b))
    );
    assert_eq!(
        receipt_digest(&to_json(&test_a)),
        receipt_digest(&to_json(&test_b))
    );
    assert_eq!(report_a, report_b, "the records' receipts too");
    // Evidence about a different tree is a different receipt.
    let mut tree = tree_of_dir(&workspace_path("examples/reader"));
    tree.files
        .get_mut("src/reader/pages.cpl")
        .expect("pages")
        .extend_from_slice(b"\n// edited\n");
    let edited = Checked::of(load_tree(&tree));
    let receipt = edited
        .index()
        .prove(&Target::parse("paginate"))
        .expect("a function")
        .output
        .receipt;
    assert_ne!(
        receipt_digest(&to_json(&receipt)),
        receipt_digest(&to_json(&verification_a))
    );
}
