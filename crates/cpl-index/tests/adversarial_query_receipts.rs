//! Adversarial tests of the semantic index, its queries, and the evidence
//! the commands build from it (`spec/toolchain.md` 1.8, 6.1-6.5, 7.2
//! `cpl check`/`cpl prove`/`cpl test`, 8, Appendix C; semantics S48, S59;
//! T17, T28):
//!
//! - target forms (stable, provisional, qualified, bare, member, std, test
//!   and property names), unknown and ambiguous targets;
//! - include sets and the exact shape and key order of every response key;
//! - callers and dependencies with interface versus body edges (T17),
//!   contract clauses, refinement predicates, property `for` clauses,
//!   generics, cross-module and multi-file modules;
//! - `tests` and their `direct`/`transitive` relation (6.5);
//! - hole dependencies (S48 plus `symbols`) and receipt cores (S59): their
//!   modules, their revision fields for clean, worktree, and unversioned
//!   revisions, and their configurations;
//! - determinism of responses and receipt digests across two builds.
//!
//! Every expectation is the specification's answer, not the current
//! behaviour; where the specification is silent or ambiguous the comment
//! at the test says which reading it takes and why.

mod common;

use common::*;
use cpl_diag::json::{
    Digest, InspectOutput, Location, ModuleHash, ObligationStatus, Relation, Revision,
    RevisionState, SymbolBlock, TestCaseRecord, TestKind, TestReport, TestRunConfig,
    TestSelectorSpec, TestStatus, VerificationPolicyJson,
};
use cpl_diag::{render_all, Category, Diagnostic, Facet, Severity, StableId};
use cpl_index::{
    definition_hash, facet_map, module_content_hash, receipt_digest, source_digest, Include,
    IncludeSet, Index, Target,
};
use cpl_project::Project;
use cpl_sema::hir::{DefId, RefKind};
use cpl_sema::{check, check_with, CheckConfig, CheckResult};
use serde::Serialize;
use serde_json::{json, Value};

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

    /// An in-memory project of `(path, text)` files with a minimal manifest.
    fn files(files: &[(&str, &str)]) -> Checked {
        Checked::of(load_tree(&tree(files)))
    }

    /// As [`Checked::files`], asserting that the check reports neither
    /// errors nor warnings, so that the test input is what the test means.
    #[track_caller]
    fn clean(files: &[(&str, &str)]) -> Checked {
        let checked = Checked::files(files);
        checked.assert_clean();
        checked
    }

    /// `examples/<name>`, loaded as `cpl check` loads it.
    fn example(name: &str) -> Checked {
        Checked::of(load_dir(&workspace_path(&format!("examples/{name}"))))
    }

    fn index(&self) -> Index<'_> {
        Index::from_check(&self.check, &self.project, unversioned(&self.project))
    }

    fn index_at(&self, revision: Revision) -> Index<'_> {
        Index::from_check(&self.check, &self.project, revision)
    }

    #[track_caller]
    fn assert_clean(&self) {
        let noisy: Vec<Diagnostic> = self
            .check
            .diagnostics
            .iter()
            .filter(|d| d.severity != Severity::Info)
            .cloned()
            .collect();
        assert!(
            noisy.is_empty(),
            "the test input is not clean:\n{}",
            render_all(&noisy, &self.project.source_map)
        );
    }

    fn file(&self, path: &str) -> cpl_diag::FileId {
        self.project
            .source_map
            .find(path)
            .unwrap_or_else(|| panic!("no file {path}"))
    }
}

/// The definition an argument names, as `cpl inspect` reads it.
#[track_caller]
fn def(index: &Index<'_>, arg: &str) -> DefId {
    index
        .resolve(&Target::parse(arg))
        .unwrap_or_else(|d| panic!("{arg}: {d}"))
}

#[track_caller]
fn path_of(index: &Index<'_>, arg: &str) -> String {
    index.version(def(index, arg)).path.clone()
}

#[track_caller]
fn resolve_error(index: &Index<'_>, arg: &str) -> Diagnostic {
    match index.resolve(&Target::parse(arg)) {
        Ok(d) => panic!("{arg} resolved to {}", index.version(d).path),
        Err(d) => d,
    }
}

fn paths(index: &Index<'_>, defs: &[DefId]) -> Vec<String> {
    defs.iter()
        .map(|&d| index.version(d).path.clone())
        .collect()
}

/// `(path, kind, interface)` of every dependency of `arg`.
#[track_caller]
fn deps(index: &Index<'_>, arg: &str) -> Vec<(String, RefKind, bool)> {
    index
        .dependencies(def(index, arg))
        .iter()
        .map(|d| (index.version(d.def).path.clone(), d.kind, d.interface))
        .collect()
}

fn dep(path: &str, kind: RefKind, interface: bool) -> (String, RefKind, bool) {
    (path.to_string(), kind, interface)
}

#[track_caller]
fn callers(index: &Index<'_>, arg: &str) -> Vec<String> {
    paths(index, &index.callers(def(index, arg)))
}

#[track_caller]
fn tests_for(index: &Index<'_>, arg: &str) -> Vec<String> {
    paths(index, &index.tests_for(def(index, arg)))
}

#[track_caller]
fn inc(csv: &str) -> IncludeSet {
    IncludeSet::parse(csv).unwrap_or_else(|e| panic!("{csv}: {e}"))
}

/// Every item but `changes`, plus the interface dependencies the default
/// set implies.
fn everything() -> IncludeSet {
    let mut set = inc(
        "signature,contract,effects,implementation,docs,callers,dependencies,obligations,tests",
    );
    set.items.insert(Include::InterfaceDependencies);
    set
}

#[track_caller]
fn inspect(index: &Index<'_>, arg: &str, include: &IncludeSet) -> InspectOutput {
    index
        .inspect(&Target::parse(arg), include)
        .unwrap_or_else(|d| panic!("{arg}: {d}"))
}

fn block_paths(blocks: &[SymbolBlock]) -> Vec<&str> {
    blocks.iter().map(|b| b.version.path.as_str()).collect()
}

#[track_caller]
fn block<'b>(blocks: &'b [SymbolBlock], path: &str) -> &'b SymbolBlock {
    blocks
        .iter()
        .find(|b| b.version.path == path)
        .unwrap_or_else(|| panic!("no block {path} in {:?}", block_paths(blocks)))
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

/// The keys of the outermost JSON object of `text`, in the order they are
/// written (a `serde_json::Value` would sort them).
fn written_keys(text: &str) -> Vec<String> {
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
                while chars.peek().is_some_and(|c| c.is_whitespace()) {
                    chars.next();
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

fn keys_of<T: Serialize>(value: &T) -> Vec<String> {
    written_keys(&serde_json::to_string(value).expect("serializes"))
}

/// The fields of `SymbolBlock` in declaration order (6.4, Appendix C).
const BLOCK_ORDER: [&str; 19] = [
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
    "docs",
    "implementation",
    "holes",
    "relation",
    "interface",
    "call_sites",
];

/// The keys every symbol block carries (6.4: "the `\"…\"` keys above elide
/// `module`, `public`, and `location`, which every block carries").
const ALWAYS: [&str; 9] = [
    "id",
    "path",
    "provisional",
    "facets",
    "definition",
    "kind",
    "module",
    "public",
    "location",
];

/// The block's keys are written in `SymbolBlock` order and include the
/// keys every block carries.
#[track_caller]
fn assert_block_order(block: &SymbolBlock) -> Vec<String> {
    let keys = keys_of(block);
    let positions: Vec<usize> = keys
        .iter()
        .map(|k| {
            BLOCK_ORDER
                .iter()
                .position(|o| o == k)
                .unwrap_or_else(|| panic!("unexpected key {k} in {keys:?}"))
        })
        .collect();
    assert!(
        positions.windows(2).all(|w| w[0] < w[1]),
        "{}: keys out of SymbolBlock order: {keys:?}",
        block.version.path
    );
    for key in ALWAYS {
        assert!(keys.iter().any(|k| k == key), "{key} missing in {keys:?}");
    }
    keys
}

/// An ambiguity error listing `candidates` in this order (6.1 row 4).
#[track_caller]
fn assert_ambiguous(d: &Diagnostic, candidates: &[&str]) {
    assert_eq!(d.code, "index_ambiguous_symbol", "{d}");
    assert_eq!(d.category, Category::Index);
    assert_eq!(d.severity, Severity::Error);
    let positions: Vec<usize> = candidates
        .iter()
        .map(|c| {
            d.message
                .find(c)
                .unwrap_or_else(|| panic!("{c} not listed in `{}`", d.message))
        })
        .collect();
    assert!(
        positions.windows(2).all(|w| w[0] < w[1]),
        "candidates not listed in display-path order: `{}`",
        d.message
    );
    if let Some(listed) = d.data.get("candidates") {
        assert_eq!(*listed, json!(candidates));
    }
}

#[track_caller]
fn assert_unknown(d: &Diagnostic) {
    assert_eq!(d.code, "index_unknown_symbol", "{d}");
    assert_eq!(d.category, Category::Index);
    assert_eq!(d.severity, Severity::Error);
}

/// A revision with the given commit and state, digesting the loaded tree.
fn revision(project: &Project, commit: Option<&str>, state: RevisionState) -> Revision {
    Revision {
        commit: commit.map(str::to_string),
        state,
        digest: source_digest(project),
    }
}

/// blake3 (64 hex) of some bytes.
fn blake3_hex(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// The content hash of a module computed from scratch (semantics S48,
/// toolchain 1.8): blake3 of each file's `format_module` text, in path
/// order, joined with LF.
fn content_hash_of(project: &Project, paths: &[&str]) -> Digest {
    let mut sorted: Vec<&str> = paths.to_vec();
    sorted.sort();
    let texts: Vec<String> = sorted
        .iter()
        .map(|p| cpl_syntax::format_module(module(project, p)))
        .collect();
    Digest(blake3_hex(texts.join("\n").as_bytes()))
}

fn record(path: &str, kind: TestKind) -> TestCaseRecord {
    let property = kind == TestKind::Property;
    TestCaseRecord {
        symbol: StableId::parse(path).expect("an identity"),
        qualified_name: path.to_string(),
        kind,
        status: TestStatus::Tested,
        duration_ms: 1,
        seed: property.then_some(7),
        property_seed: property.then(|| "ab".repeat(32)),
        cases: property.then_some(100),
        failing_case: None,
        shrunk: None,
        fault: None,
        counterexamples: Vec::new(),
        receipt: None,
    }
}

fn report_of(records: Vec<TestCaseRecord>) -> TestReport {
    TestReport {
        seed: 7,
        cases: 100,
        selected: records.len() as u32,
        tested: records.len() as u32,
        failed: 0,
        results: records,
    }
}

fn run_config() -> TestRunConfig {
    TestRunConfig {
        seed: 7,
        cases: 100,
        max_shrinks: 1000,
        timeout_ms: 0,
        selector: TestSelectorSpec {
            namespace: None,
            name: None,
            id: None,
        },
        check_contracts: true,
        fail_fast: false,
    }
}

// ---------------------------------------------------------------------------
// Test inputs
// ---------------------------------------------------------------------------

/// Contract clauses as dependency sources (T17, 6.5).
const CONTRACTS: &str = r#"module m.a

pub fn valid(n: Int) -> Bool {
    n >= 0
}

pub fn bound(n: Int) -> Bool {
    n < 1000
}

pub fn helper(n: Int) -> Int {
    n + 1
}

pub fn step(n: Int) -> Int
    requires valid(n)
    ensures bound(result)
{
    helper(n)
}
"#;

/// A refinement whose predicate calls a function.
const REFINED: &str = r#"module m.r

pub fn is_even(n: Int) -> Bool {
    n % 2 == 0
}

pub type Even = Int where is_even(self)

pub fn halve(e: Even) -> Int {
    e / 2
}

pub fn to_even(n: Int) -> Result<Even, ConstraintError> {
    check<Even>(n)
}

test "four is even" {
    assert to_even(4).is_ok()
}
"#;

/// A property whose `for` clause calls a project function.
const PROPERTY: &str = r#"module m.p

pub fn limit() -> Int {
    10
}

pub fn square(n: Int) -> Int {
    n * n
}

property "squares are nonnegative"
    for n in gen.range(0, limit())
{
    assert square(n) >= 0
}
"#;

/// Generic functions and a generic struct.
const GENERIC: &str = r#"module m.g

pub struct Box<T> {
    value: T
}

pub fn head<T>(xs: List<T>) -> Option<T> {
    xs.first()
}

pub fn unbox<T>(b: Box<T>) -> T {
    b.value
}

pub fn boxed(n: Int) -> Box<Int> {
    Box { value: n }
}

pub fn use_both() -> Int {
    let b = boxed(3)
    let first = head([1, 2])
    unbox(b) + first.unwrap_or(0)
}
"#;

/// Member types of structs, enums, and capabilities.
const SHAPES: &str = r#"module m.s

pub type Key(Text)

pub struct Item {
    key: Key
    count: Int
}

pub enum Event {
    Added(Item)
    Removed(Key)
    Cleared
}

pub fn valid_key(k: Key) -> Bool {
    match k {
        Key(text) => text.len > 0
    }
}

pub capability Store {
    fn get(key: Key) -> Item
        requires valid_key(key)
    fn size() -> Int
}
"#;

/// One declaration of every kind, plus a private function.
const KINDS: &str = r#"module m.kinds

pub type Id(Text)

pub type Pos = Int where self > 0

pub struct Point {
    x: Int
}

pub enum Color {
    Red
    Green
}

pub capability Clock {
    fn now() -> Int
}

pub fn origin() -> Point {
    Point { x: 0 }
}

fn hidden() -> Int {
    2
}

pub fn shown() -> Int {
    hidden()
}

test "origin is zero" {
    assert origin().x == 0
}

property "positive ints are positive"
    for n in gen.positive_int
{
    assert n > 0
}
"#;

/// Tests reaching their targets directly, transitively, and through a
/// contract clause; and one that only builds a struct.
const TESTED: &str = r#"module m.t

pub struct Box {
    v: Int
}

pub fn f(n: Int) -> Int {
    n + 1
}

pub fn g(n: Int) -> Int {
    f(n) * 2
}

pub fn guard(n: Int) -> Bool {
    f(n) > 0
}

pub fn k(n: Int) -> Int
    requires guard(n)
{
    n
}

test "a direct" {
    assert f(1) == 2
}

test "b transitive" {
    assert g(1) == 4
}

test "c both" {
    assert f(g(1)) == 5
}

test "d contract" {
    assert k(1) == 1
}

test "e box" {
    let b = Box { v: 1 }
    assert b.v == 1
}
"#;

/// A module spanning two files (Section 3.7), used from another module.
const MULTI_CORE: &str = r#"module lib.core

pub fn helper(n: Int) -> Int {
    n + 1
}
"#;

const MULTI_EXTRA: &str = r#"module lib.core

pub fn twice(n: Int) -> Int {
    helper(helper(n))
}

pub fn later(n: Int) -> Int {
    let _m = helper(n)
    ?todo
}
"#;

const MULTI_MAIN: &str = r#"module app.main

use lib.core.{twice}

pub fn run() -> Int {
    twice(1)
}
"#;

fn multi_file() -> Checked {
    Checked::clean(&[
        ("src/lib/core.cpl", MULTI_CORE),
        ("src/lib/core/extra.cpl", MULTI_EXTRA),
        ("src/app/main.cpl", MULTI_MAIN),
    ])
}

// ---------------------------------------------------------------------------
// 6.1 Targets
// ---------------------------------------------------------------------------

/// 6.1 row 1: an argument with the accepted `@id` syntax is tried as a
/// stable id first, and an existing entry wins over the declarations that
/// merely have that name (two here, which as a bare name would be
/// ambiguous).
#[test]
fn an_id_entry_wins_over_declarations_named_like_it() {
    let checked = Checked::clean(&[
        (
            "src/m/ids.cpl",
            "module m.ids\n\n@id(\"beta_one\")\npub fn alpha() -> Int {\n    1\n}\n\npub fn beta_one() -> Int {\n    2\n}\n",
        ),
        (
            "src/m/x.cpl",
            "module m.x\n\npub fn beta_one() -> Int {\n    3\n}\n",
        ),
    ]);
    let index = checked.index();
    assert!(matches!(Target::parse("beta_one"), Target::Id(_)));
    assert_eq!(path_of(&index, "beta_one"), "m.ids.alpha");
    let out = inspect(&index, "beta_one", &inc("signature"));
    assert_eq!(out.symbol.version.id.as_str(), "beta_one");
    assert_eq!(
        out.symbol.signature.as_deref(),
        Some("pub fn alpha() -> Int")
    );
    assert_eq!(path_of(&index, "m.ids.beta_one"), "m.ids.beta_one");
    assert_eq!(path_of(&index, "m.x.beta_one"), "m.x.beta_one");
}

/// 6.1 row 2: a provisional id is the qualified name and goes through the
/// id table; for a test it is the quoted, canonically escaped name (4.3),
/// and the qualified path (row 3) and the quoted bare name (row 4) name
/// the same test.
#[test]
fn a_test_name_with_escapes_resolves_in_every_target_form() {
    let checked = Checked::clean(&[(
        "src/t/z.cpl",
        "module t.z\n\ntest \"say \\\"hi\\\"\\tnow\" {\n    assert true\n}\n",
    )]);
    let index = checked.index();
    let path = r#"t.z."say \"hi\"\tnow""#;
    let id = StableId::parse(path).expect("a provisional id");
    let d = index.resolve(&Target::Id(id.clone())).expect("by id");
    let version = index.version(d);
    assert_eq!(version.id, id);
    assert_eq!(version.path, path);
    assert!(version.provisional);
    assert_eq!(index.resolve(&Target::parse(path)), Ok(d));
    assert_eq!(index.resolve(&Target::parse(r#""say \"hi\"\tnow""#)), Ok(d));
    let out = inspect(&index, path, &inc("docs"));
    assert_eq!(out.symbol.kind, "test");
    assert_eq!(out.symbol.module, "t.z");
}

/// "Std declarations are first-class targets" (6.1): members by receiver,
/// generator properties, std types and refinements, and free functions
/// resolve by their qualified names, with their provisional identities
/// (3.6 Identity).
#[test]
fn std_members_generators_and_types_resolve_by_qualified_name() {
    let checked = Checked::example("reader");
    let index = checked.index();
    for path in [
        "std.prelude.Option.unwrap_or",
        "std.prelude.Result.unwrap_or",
        "std.prelude.Int.min",
        "std.prelude.Text.len",
        "std.gen.int",
        "std.gen.list",
        "std.gen.Positive",
        "std.gen.Gen",
        "std.text.parse_int",
        "std.prelude.List",
        "std.prelude.ConstraintError",
    ] {
        let d = def(&index, path);
        let version = index.version(d);
        assert_eq!(version.path, path);
        assert_eq!(version.id.as_str(), path);
        assert!(version.provisional, "{path}");
    }
}

/// 6.1 row 4: "every definition named `paginate` in project and std": a
/// project function sharing its name with a std function is ambiguous,
/// and so are two std members of different receivers; the qualified and
/// receiver-qualified forms stay unique.
#[test]
fn bare_names_shared_with_std_are_ambiguous() {
    let checked = Checked::clean(&[(
        "src/app/text.cpl",
        "module app.text\n\npub fn trim(t: Text) -> Text {\n    t\n}\n",
    )]);
    let index = checked.index();
    assert_ambiguous(
        &resolve_error(&index, "trim"),
        &["app.text.trim", "std.text.trim"],
    );
    assert_ambiguous(
        &resolve_error(&index, "unwrap_or"),
        &[
            "std.prelude.Option.unwrap_or",
            "std.prelude.Result.unwrap_or",
        ],
    );
    assert_eq!(
        path_of(&index, "Option.unwrap_or"),
        "std.prelude.Option.unwrap_or"
    );
    assert_eq!(path_of(&index, "app.text.trim"), "app.text.trim");
    assert_eq!(path_of(&index, "std.text.trim"), "std.text.trim");
}

/// `examples/reader` declares `Positive`, and std declares
/// `std.gen.Positive` (stdlib 8.1, S29): the bare name names both, so it is
/// `index_ambiguous_symbol` for `inspect` and `prove` alike (6.1 row 4).
#[test]
fn bare_positive_in_the_reader_is_ambiguous_with_std_gen_positive() {
    let checked = Checked::example("reader");
    let index = checked.index();
    let d = resolve_error(&index, "Positive");
    assert_ambiguous(&d, &["reader.pages.Positive", "std.gen.Positive"]);
    assert_eq!(
        index.inspect(&Target::parse("Positive"), &IncludeSet::default_set()),
        Err(d.clone())
    );
    assert_eq!(index.prove(&Target::parse("Positive")).map(|_| ()), Err(d));
}

/// Candidates are listed "sorted by display path" (6.5), which is byte
/// order: `.` < `_` < lowercase letters.
#[test]
fn ambiguous_candidates_are_in_byte_order_of_their_paths() {
    let user = |m: &str| {
        format!(
            "module {m}\n\nuse t.m.{{Target}}\n\npub fn user(x: Target) -> Target {{\n    x\n}}\n"
        )
    };
    let (ab, a_b, a) = (user("ab.m"), user("a_b.m"), user("a.m"));
    let checked = Checked::clean(&[
        (
            "src/t/m.cpl",
            "module t.m\n\npub struct Target {\n    v: Int\n}\n",
        ),
        ("src/ab/m.cpl", &ab),
        ("src/a_b/m.cpl", &a_b),
        ("src/a/m.cpl", &a),
    ]);
    let index = checked.index();
    assert_ambiguous(
        &resolve_error(&index, "user"),
        &["a.m.user", "a_b.m.user", "ab.m.user"],
    );
}

/// Variants, operations, and fields are members, not definitions
/// (`Program::lookup`, semantics 1.4); module paths, the `gen` alias
/// (which only project modules see), and a missing std member name no
/// declaration either: each is `index_unknown_symbol` (6.1, Appendix B).
#[test]
fn members_modules_and_aliases_are_not_targets() {
    let checked = Checked::example("reader");
    let index = checked.index();
    for arg in [
        "reader.pages.StoreError.Missing",
        "reader.pages.Page.text",
        "reader.pages.PageRead.get",
        "reader.pages",
        "reader",
        "std.prelude",
        "std.prelude.List.nope",
        "gen.int",
        "gen.Positive",
        "Missing",
        "tests.pagination.\"pagination\"",
    ] {
        let d = resolve_error(&index, arg);
        assert_unknown(&d);
        assert_eq!(
            index
                .inspect(&Target::parse(arg), &IncludeSet::default_set())
                .map(|_| ()),
            Err(d.clone()),
            "{arg}"
        );
        assert_eq!(
            index.prove(&Target::parse(arg)).map(|_| ()),
            Err(d),
            "{arg}"
        );
    }
}

// ---------------------------------------------------------------------------
// 6.2 Include sets
// ---------------------------------------------------------------------------

/// "Anything else is `usage_bad_argument`" (6.2): the grammar has no
/// whitespace, no other separator, no plural or upper-case spelling, and
/// no facet names that are not items.
#[test]
fn include_lists_with_stray_whitespace_or_other_spellings_are_refused() {
    for bad in [
        "signature ",
        " signature",
        "signature\t",
        "signature\n",
        "signature,\tdocs",
        "signature ,docs",
        "signatures",
        "callers;tests",
        "callers tests",
        "SIGNATURE",
        "Docs",
        "implementation,interface_dependencies",
        "body",
        "identity",
        "holes",
        "relation",
        "call_sites",
        ",",
        ",,",
    ] {
        assert!(IncludeSet::parse(bad).is_err(), "{bad:?} was accepted");
    }
    for item in [
        "signature",
        "contract",
        "effects",
        "implementation",
        "docs",
        "callers",
        "dependencies",
        "obligations",
        "tests",
        "changes",
    ] {
        let set = inc(item);
        assert_eq!(set.items.len(), 1, "{item}");
        assert!(!set.contains(Include::InterfaceDependencies));
    }
}

/// The `include` key is "the effective include set, sorted" (6.4): the
/// derived `Include` order, each item once, whatever the order and the
/// repetitions of the command line.
#[test]
fn the_include_key_is_sorted_and_counts_each_item_once() {
    let checked = Checked::clean(&[("src/m/a.cpl", CONTRACTS)]);
    let index = checked.index();
    let out = inspect(&index, "step", &inc("tests,docs,signature,tests,docs"));
    assert_eq!(
        out.include,
        [Include::Signature, Include::Docs, Include::Tests]
    );
    assert_eq!(
        to_json(&out)["include"],
        json!(["signature", "docs", "tests"])
    );
    let out = inspect(&index, "step", &inc("obligations,callers,contract"));
    assert_eq!(
        to_json(&out)["include"],
        json!(["contract", "callers", "obligations"])
    );
}

// ---------------------------------------------------------------------------
// 6.4 Response shape
// ---------------------------------------------------------------------------

const CHANGED_OLD: &str = r#"module m.c

@id("fn_aaaaaaaaaaaaaaaa")
pub fn old_name() -> Int {
    1
}

pub fn kept() -> Int {
    1
}
"#;

const CHANGED_NEW: &str = r#"module m.c

@id("fn_aaaaaaaaaaaaaaaa")
pub fn new_name() -> Int {
    1
}

pub fn kept() -> Int {
    new_name() + 1
}

pub fn added() -> Int {
    kept()
}

test "kept is two" {
    assert kept() == 2
}
"#;

/// The facet map of the old side and its revision, as `--since` gives it
/// (built from a tree here, so that no repository is needed).
fn changes_base_of(old: &[(&str, &str)]) -> cpl_index::FacetMap {
    let project = load_tree(&tree(old));
    let base = revision(&project, Some(&"b".repeat(40)), RevisionState::Clean);
    facet_map(&project, base)
}

/// With every item included, the response's keys come in the order of
/// 6.4's table (the declaration order of `InspectOutput`, Section 7.1) and
/// the target's block in `SymbolBlock` order.
#[test]
fn a_full_response_writes_its_keys_in_schema_order() {
    let checked = Checked::clean(&[("src/m/c.cpl", CHANGED_NEW)]);
    let base = changes_base_of(&[("src/m/c.cpl", CHANGED_OLD)]);
    let index = checked.index().with_changes_base(base);
    let mut all = everything();
    all.items.insert(Include::Changes);
    let out = inspect(&index, "kept", &all);
    assert_eq!(
        keys_of(&out),
        [
            "include",
            "symbol",
            "interface_dependencies",
            "dependencies",
            "callers",
            "obligations",
            "tests",
            "changes"
        ]
    );
    assert_eq!(
        to_json(&out)["include"],
        json!([
            "signature",
            "contract",
            "effects",
            "implementation",
            "docs",
            "callers",
            "dependencies",
            "obligations",
            "tests",
            "changes",
            "interface_dependencies"
        ])
    );
    assert_eq!(
        keys_of(&out.symbol),
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
            "docs",
            "implementation",
            "holes"
        ]
    );
    let changes = out.changes.as_ref().expect("included");
    assert_eq!(keys_of(changes), ["since", "statements"]);
}

/// Nested blocks are symbol blocks too (6.4): their keys follow
/// `SymbolBlock` order, every block carries the version, kind, module,
/// public, and location keys, and each list closes with its own keys:
/// `relation` and `interface` in `dependencies`, `relation` (without
/// `interface`) in `interface_dependencies` and `tests`, `call_sites` in
/// `callers`.
#[test]
fn nested_blocks_end_with_their_list_specific_keys() {
    let checked = Checked::clean(&[("src/m/t.cpl", TESTED)]);
    let index = checked.index();
    let out = inspect(&index, "f", &everything());
    let callers = out.callers.as_ref().expect("callers");
    assert!(!callers.is_empty());
    for b in callers {
        let keys = assert_block_order(b);
        assert_eq!(keys.last().map(String::as_str), Some("call_sites"));
        assert!(!keys.iter().any(|k| k == "relation" || k == "interface"));
    }
    let tests = out.tests.as_ref().expect("tests");
    assert!(!tests.is_empty());
    for b in tests {
        let keys = assert_block_order(b);
        assert_eq!(keys.last().map(String::as_str), Some("relation"));
        assert!(!keys.iter().any(|k| k == "interface" || k == "call_sites"));
    }
    let out = inspect(&index, "g", &everything());
    let dependencies = out.dependencies.as_ref().expect("dependencies");
    assert!(!dependencies.is_empty());
    for b in dependencies {
        let keys = assert_block_order(b);
        assert_eq!(
            keys[keys.len() - 2..],
            ["relation".to_string(), "interface".to_string()]
        );
    }
    for b in out.interface_dependencies.as_ref().expect("default items") {
        let keys = assert_block_order(b);
        assert_eq!(keys.last().map(String::as_str), Some("relation"));
        assert!(!keys.iter().any(|k| k == "interface"));
    }
}

/// `facets` holds exactly the facets of the declaration's kind (5.1), in
/// `Facet::ALL` order, with `definition` their definition hash (5.4);
/// `kind` is the id kind (4.1); `verification_policy` is present for
/// functions only, std functions included (6.4); `public` is the `pub`
/// prefix.
#[test]
fn every_kind_has_its_facets_kind_string_and_policy() {
    use Facet::*;
    let checked = Checked::clean(&[("src/m/kinds.cpl", KINDS)]);
    let index = checked.index();
    let all = Facet::ALL.to_vec();
    let cases: Vec<(&str, &str, Vec<Facet>, bool)> = vec![
        ("m.kinds.origin", "fn", all.clone(), true),
        ("m.kinds.hidden", "fn", all.clone(), false),
        (
            "m.kinds.Id",
            "ty",
            vec![Identity, Name, Signature, Docs],
            true,
        ),
        (
            "m.kinds.Pos",
            "ty",
            vec![Identity, Name, Signature, Contract, Docs],
            true,
        ),
        (
            "m.kinds.Point",
            "ty",
            vec![Identity, Name, Signature, Docs],
            true,
        ),
        (
            "m.kinds.Color",
            "ty",
            vec![Identity, Name, Signature, Docs],
            true,
        ),
        (
            "m.kinds.Clock",
            "cap",
            vec![Identity, Name, Signature, Contract, Docs],
            true,
        ),
        (
            "m.kinds.\"origin is zero\"",
            "test",
            vec![Identity, Name, Body, Docs],
            false,
        ),
        (
            "m.kinds.\"positive ints are positive\"",
            "prop",
            vec![Identity, Name, Signature, Body, Docs],
            false,
        ),
        (
            "std.prelude.List",
            "ty",
            vec![Identity, Name, Signature, Docs],
            true,
        ),
        ("std.text.trim", "fn", all.clone(), true),
        ("std.prelude.List.take", "fn", all.clone(), true),
    ];
    for (path, kind, facets, public) in cases {
        let out = inspect(&index, path, &inc("docs"));
        let b = &out.symbol;
        assert_eq!(b.kind, kind, "{path}");
        assert_eq!(b.public, public, "{path}");
        assert_eq!(
            b.version.facets.keys().copied().collect::<Vec<_>>(),
            facets,
            "{path}"
        );
        let written: Vec<String> = facets.iter().map(|f| f.as_str().to_string()).collect();
        assert_eq!(keys_of(&b.version.facets), written, "{path}");
        assert_eq!(b.version.definition, definition_hash(&b.version.facets));
        assert_eq!(
            b.verification_policy.is_some(),
            kind == "fn",
            "{path}: verification_policy is for functions only"
        );
        assert_eq!(*index.version(def(&index, path)), b.version, "{path}");
    }
    let std = inspect(&index, "std.text.trim", &inc("signature"));
    assert_eq!(
        std.symbol.verification_policy,
        Some(VerificationPolicyJson { prove: false })
    );
    assert_eq!(std.symbol.module, "std.text");
    assert_eq!(std.symbol.location.file, "std/text.cpl");
}

const DOCS: &str = r#"module m.d

/// An item.
pub struct Item {
    /// The item's name.
    name: Text
}

pub struct Bare {
    /// Only a member doc.
    v: Int
}

pub fn plain() -> Int {
    1
}
"#;

/// `docs` when included: the declaration's docs text, which for a struct
/// covers its members' doc comments (T25, 5.2 `docs`), `null` without docs
/// (Appendix C: present and `null`), and no key at all when not included.
#[test]
fn docs_are_the_docs_facet_text_null_or_absent() {
    let checked = Checked::clean(&[("src/m/d.cpl", DOCS)]);
    let index = checked.index();
    let docs = |arg: &str| inspect(&index, arg, &inc("docs")).symbol;
    assert_eq!(
        docs("m.d.Item").docs,
        Some(Some("An item.\nmember name\nThe item's name.".to_string()))
    );
    assert_eq!(
        docs("m.d.Bare").docs,
        Some(Some("\nmember v\nOnly a member doc.".to_string()))
    );
    let plain = docs("m.d.plain");
    assert_eq!(plain.docs, Some(None));
    let text = serde_json::to_string(&plain).expect("serializes");
    assert!(text.contains("\"docs\":null"), "{text}");
    let without = inspect(&index, "m.d.plain", &inc("signature")).symbol;
    assert_eq!(without.docs, None);
    assert!(!keys_of(&without).iter().any(|k| k == "docs"));
    // `location` is the declaration's span, which "includes the doc
    // comments and annotations" (2.2 `Declaration::span`).
    let file = "src/m/d.cpl";
    assert_eq!(docs("m.d.Item").location, loc(file, 3, 1, 7, 2));
    assert_eq!(docs("m.d.Bare").location, loc(file, 9, 1, 12, 2));
    assert_eq!(plain.location, loc(file, 14, 1, 16, 2));
}

/// `implementation`: the canonical body text for functions, tests, and
/// properties (a bodiless std function's body text is empty, 5.1), with
/// its `holes`; other kinds carry no `implementation` (6.4: "fn, test,
/// property only").
#[test]
fn implementations_of_tests_properties_and_std_stubs() {
    let checked = Checked::clean(&[("src/m/kinds.cpl", KINDS)]);
    let index = checked.index();
    let body = |arg: &str| inspect(&index, arg, &inc("implementation")).symbol;
    let test = body("\"origin is zero\"");
    assert_eq!(
        test.implementation.as_deref(),
        Some("{\n    assert origin().x == 0\n}")
    );
    assert_eq!(test.holes.as_deref(), Some(&[][..]));
    let property = body("\"positive ints are positive\"");
    assert_eq!(
        property.implementation.as_deref(),
        Some("{\n    assert n > 0\n}")
    );
    assert_eq!(property.holes.as_deref(), Some(&[][..]));
    let trim = body("std.text.trim");
    assert_eq!(trim.implementation.as_deref(), Some(""));
    assert_eq!(trim.holes.as_deref(), Some(&[][..]));
    for arg in [
        "m.kinds.Point",
        "m.kinds.Color",
        "m.kinds.Clock",
        "m.kinds.Pos",
        "m.kinds.Id",
        "std.prelude.List",
    ] {
        // Whether `holes` accompanies an absent `implementation` is not
        // fixed by 6.4 ("when `implementation` is included"), so only the
        // implementation key is asserted.
        assert_eq!(body(arg).implementation, None, "{arg}");
    }
}

/// The 6.4 table states when each facet-text key is present: `signature`
/// "when `signature` included; the header with its name", `contract` "when
/// `contract` included", `effects` "when `effects` included". Only
/// `implementation` ("fn, test, property only") and `verification_policy`
/// ("always for fn; omitted otherwise") are limited by kind, so the three
/// keys are written for every kind once included: a test's header is
/// `test "<name>"`, and a kind without clauses or an effect bound has an
/// empty list. (The implementation reads 5.1's facet applicability into
/// the table and omits the keys of facets the kind lacks.)
#[test]
fn included_facet_text_keys_are_written_for_every_kind() {
    let checked = Checked::clean(&[("src/m/kinds.cpl", KINDS)]);
    let index = checked.index();
    let symbol = |arg: &str, csv: &str| inspect(&index, arg, &inc(csv)).symbol;
    assert_eq!(
        symbol("\"origin is zero\"", "signature")
            .signature
            .as_deref(),
        Some("test \"origin is zero\"")
    );
    for arg in [
        "m.kinds.Point",
        "m.kinds.Color",
        "m.kinds.Id",
        "\"origin is zero\"",
    ] {
        assert_eq!(
            symbol(arg, "contract").contract.as_deref(),
            Some(&[][..]),
            "{arg}: contract"
        );
    }
    for arg in [
        "m.kinds.Point",
        "m.kinds.Pos",
        "m.kinds.Clock",
        "\"origin is zero\"",
    ] {
        assert_eq!(
            symbol(arg, "effects").effects.as_deref(),
            Some(&[][..]),
            "{arg}: effects"
        );
    }
}

/// 6.2: `dependencies` lists every referenced definition "each with its
/// own `symbol` block including signature and contract", and the default
/// set gives the interface dependencies "each with the same four"
/// (`signature, contract, effects, docs`), whatever the dependency's kind.
#[test]
fn dependency_blocks_carry_signature_contract_and_the_default_four() {
    let checked = Checked::example("reader");
    let index = checked.index();
    let out = inspect(&index, "reader.pages.paginate", &inc("dependencies"));
    for b in out.dependencies.as_ref().expect("included") {
        assert!(b.signature.is_some(), "{}: signature", b.version.path);
        assert!(b.contract.is_some(), "{}: contract", b.version.path);
    }
    let out = inspect(&index, "reader.pages.paginate", &IncludeSet::default_set());
    for b in out.interface_dependencies.as_ref().expect("default set") {
        assert!(b.signature.is_some(), "{}: signature", b.version.path);
        assert!(b.contract.is_some(), "{}: contract", b.version.path);
        assert!(b.effects.is_some(), "{}: effects", b.version.path);
        assert!(b.docs.is_some(), "{}: docs", b.version.path);
    }
}

// ---------------------------------------------------------------------------
// 6.5 Callers
// ---------------------------------------------------------------------------

const REFERENCED: &str = r#"module lib.core

pub struct Item {
    name: Text
    count: Int
}

pub type Name(Text)

pub enum Shape {
    Circle(Int)
    Square(Int)
}

pub fn is_even(n: Int) -> Bool {
    n % 2 == 0
}

pub type Even = Int where is_even(self)

pub capability Shelf {
    fn find(name: Text) -> Item
}
"#;

const REFERENCING: &str = r#"module lib.uses

use lib.core.{Item, Name, Shape, Even, Shelf}

pub fn make(name: Text) -> Item {
    Item { name: name, count: 0 }
}

pub fn ident(i: Item) -> Item {
    i
}

pub fn count_of(item: Item) -> Int {
    item.count
}

pub fn count_made() -> Int {
    make("x").count
}

pub fn named(text: Text) -> Name {
    Name(text)
}

pub fn unwrap_name(n: Name) -> Text {
    match n {
        Name(t) => t
    }
}

pub fn area(s: Shape) -> Int {
    match s {
        Circle(r) => r * r * 3
        Square(w) => w * w
    }
}

pub fn circle_area() -> Int {
    area(Circle(1))
}

pub fn to_even(n: Int) -> Result<Even, ConstraintError> {
    check<Even>(n)
}

pub fn is_even_ok(n: Int) -> Bool {
    check<Even>(n).is_ok()
}

pub fn lookup(using shelf: Shelf) -> Item
    effects { shelf.find }
{
    shelf.find("x")
}
"#;

fn referencing() -> Checked {
    Checked::clean(&[
        ("src/lib/core.cpl", REFERENCED),
        ("src/lib/uses.cpl", REFERENCING),
    ])
}

/// 6.5: `callers` takes the references of every kind (`Call`,
/// `CapabilityOp`, `Ctor`, `TypeUse`, `FieldAccess`, `Check`), each
/// referencing definition once, sorted by display path; a reference of a
/// single kind is enough (`count_made` only reads a field, `circle_area`
/// only constructs, `is_even_ok` only checks, `ident` only mentions the
/// type, and a refinement's predicate calls).
#[test]
fn callers_cover_every_reference_kind() {
    let checked = referencing();
    let index = checked.index();
    assert_eq!(
        callers(&index, "lib.core.Item"),
        [
            "lib.core.Shelf",
            "lib.uses.count_made",
            "lib.uses.count_of",
            "lib.uses.ident",
            "lib.uses.lookup",
            "lib.uses.make",
        ]
    );
    assert_eq!(
        callers(&index, "lib.core.Shape"),
        ["lib.uses.area", "lib.uses.circle_area"]
    );
    assert_eq!(
        callers(&index, "lib.core.Name"),
        ["lib.uses.named", "lib.uses.unwrap_name"]
    );
    assert_eq!(
        callers(&index, "lib.core.Even"),
        ["lib.uses.is_even_ok", "lib.uses.to_even"]
    );
    assert_eq!(callers(&index, "lib.core.is_even"), ["lib.core.Even"]);
    assert_eq!(callers(&index, "lib.core.Shelf"), ["lib.uses.lookup"]);
    assert_eq!(callers(&index, "lib.uses.make"), ["lib.uses.count_made"]);
    // Every caller block has at least one call site, in its own file.
    let out = inspect(&index, "lib.core.Item", &inc("callers"));
    for b in out.callers.as_ref().expect("included") {
        let sites = b.call_sites.as_ref().expect("call_sites");
        assert!(!sites.is_empty(), "{}", b.version.path);
        assert!(sites.iter().all(|s| s.file == b.location.file));
    }
    // A predicate's call is a call site in the refinement's declaration.
    let out = inspect(&index, "lib.core.is_even", &inc("callers"));
    let even_block = block(out.callers.as_ref().expect("callers"), "lib.core.Even");
    assert_eq!(
        even_block.call_sites.as_deref(),
        Some(&[loc("src/lib/core.cpl", 19, 27, 19, 40)][..])
    );
}

/// Calls in contract clauses make the function a caller, with the call
/// expression as its site (6.4's example `paginate(items, checked_size)`),
/// in span order; a body call likewise.
#[test]
fn contract_clause_calls_are_call_sites_of_their_callees() {
    let checked = Checked::clean(&[("src/m/a.cpl", CONTRACTS)]);
    let index = checked.index();
    let file = "src/m/a.cpl";
    let site = |callee: &str| {
        let out = inspect(&index, callee, &inc("callers"));
        let callers = out.callers.expect("included");
        assert_eq!(block_paths(&callers), ["m.a.step"], "{callee}");
        callers[0].call_sites.clone().expect("call sites")
    };
    assert_eq!(site("valid"), [loc(file, 16, 14, 16, 22)]);
    assert_eq!(site("bound"), [loc(file, 17, 13, 17, 26)]);
    assert_eq!(site("helper"), [loc(file, 19, 5, 19, 14)]);
}

/// A member call's site is the whole call expression with its receiver:
/// `xs.take(1)` for a method, `xs.len` for a property (T19 lowers both to
/// a `Call` of the stub).
#[test]
fn member_call_sites_span_the_whole_member_call() {
    let checked = Checked::clean(&[(
        "src/m/cs.cpl",
        "module m.cs\n\npub fn firsts(xs: List<Int>) -> List<Int> {\n    xs.take(1)\n}\n\npub fn size(xs: List<Int>) -> Int {\n    xs.len\n}\n",
    )]);
    let index = checked.index();
    let file = "src/m/cs.cpl";
    let out = inspect(&index, "std.prelude.List.take", &inc("callers"));
    let firsts = block(out.callers.as_ref().expect("callers"), "m.cs.firsts");
    assert_eq!(
        firsts.call_sites.as_deref(),
        Some(&[loc(file, 4, 5, 4, 15)][..])
    );
    let out = inspect(&index, "std.prelude.List.len", &inc("callers"));
    let size = block(out.callers.as_ref().expect("callers"), "m.cs.size");
    assert_eq!(
        size.call_sites.as_deref(),
        Some(&[loc(file, 8, 5, 8, 11)][..])
    );
}

/// A module of two files (3.7): its declarations reference each other
/// without imports, and every caller block names its own file, its module
/// path, and each call site in span order (the outer call before the
/// inner one it contains).
#[test]
fn callers_within_a_multi_file_module_carry_their_own_files() {
    let checked = multi_file();
    let index = checked.index();
    let extra = "src/lib/core/extra.cpl";
    let out = inspect(&index, "lib.core.helper", &inc("callers"));
    let blocks = out.callers.as_ref().expect("callers");
    assert_eq!(block_paths(blocks), ["lib.core.later", "lib.core.twice"]);
    for b in blocks {
        assert_eq!(b.module, "lib.core");
        assert_eq!(b.location.file, extra);
    }
    assert_eq!(
        blocks[1].call_sites.as_deref(),
        Some(&[loc(extra, 4, 5, 4, 22), loc(extra, 4, 12, 4, 21)][..])
    );
    assert_eq!(
        blocks[0].call_sites.as_deref(),
        Some(&[loc(extra, 8, 14, 8, 23)][..])
    );
    assert_eq!(callers(&index, "lib.core.twice"), ["app.main.run"]);
    let helper = inspect(&index, "lib.core.helper", &inc("signature")).symbol;
    assert_eq!(helper.location.file, "src/lib/core.cpl");
}

/// Callers are "sorted by display path" (6.5), byte order: an upper-case
/// name sorts before a lower-case one, and `a.m` < `a_b.m` < `ab.m`.
#[test]
fn callers_are_sorted_by_display_path_bytes() {
    let user = |m: &str| {
        format!(
            "module {m}\n\nuse t.m.{{Target}}\n\npub fn user(x: Target) -> Target {{\n    x\n}}\n"
        )
    };
    let (ab, a_b) = (user("ab.m"), user("a_b.m"));
    let checked = Checked::clean(&[
        ("src/t/m.cpl", "module t.m\n\npub struct Target {\n    v: Int\n}\n"),
        ("src/ab/m.cpl", &ab),
        ("src/a_b/m.cpl", &a_b),
        (
            "src/a/m.cpl",
            "module a.m\n\nuse t.m.{Target}\n\npub struct Zed {\n    t: Target\n}\n\npub fn alpha(x: Target) -> Target {\n    x\n}\n",
        ),
    ]);
    let index = checked.index();
    assert_eq!(
        callers(&index, "t.m.Target"),
        ["a.m.Zed", "a.m.alpha", "a_b.m.user", "ab.m.user"]
    );
    let out = inspect(&index, "t.m.Target", &inc("callers"));
    assert_eq!(
        block_paths(out.callers.as_ref().expect("callers")),
        ["a.m.Zed", "a.m.alpha", "a_b.m.user", "ab.m.user"]
    );
}

/// "Imports are not references, so a `use` line never makes a file a
/// caller" (6.5).
#[test]
fn a_use_import_alone_makes_no_caller() {
    let checked = Checked::files(&[
        (
            "src/a/x.cpl",
            "module a.x\n\npub fn twice(n: Int) -> Int {\n    n * 2\n}\n",
        ),
        (
            "src/b/y.cpl",
            "module b.y\n\nuse a.x.{twice}\n\npub fn noop() -> Int {\n    0\n}\n",
        ),
    ]);
    assert!(!checked.check.has_errors());
    let index = checked.index();
    assert!(callers(&index, "a.x.twice").is_empty());
    assert!(tests_for(&index, "a.x.twice").is_empty());
    let out = inspect(&index, "a.x.twice", &inc("callers"));
    assert_eq!(out.callers.as_deref(), Some(&[][..]));
    assert!(deps(&index, "b.y.noop")
        .iter()
        .all(|(path, _, _)| path != "a.x.twice"));
}

const RECURSIVE: &str = r#"module m.rec

pub fn count(n: Int) -> Int
    requires n >= 0
    decreases n
{
    if n == 0 {
        0
    } else {
        count(n - 1) + 1
    }
}

pub fn down(n: Int) -> Int
    requires n >= 0
    decreases n
{
    if n == 0 {
        0
    } else {
        let rest: Int = ?extra
        down(n - 1) + rest
    }
}
"#;

/// 6.5's formulas take every reference, so a recursive function is its own
/// caller (with its recursive call as the site) and its own dependency.
#[test]
fn a_recursive_function_is_its_own_caller_and_dependency() {
    let checked = Checked::files(&[("src/m/rec.cpl", RECURSIVE)]);
    assert!(!checked.check.has_errors());
    let index = checked.index();
    assert_eq!(callers(&index, "m.rec.count"), ["m.rec.count"]);
    let count = def(&index, "m.rec.count");
    assert_eq!(
        index.call_sites(count, count),
        [loc("src/m/rec.cpl", 10, 9, 10, 21)]
    );
    assert!(deps(&index, "m.rec.count").contains(&dep("m.rec.count", RefKind::Call, false)));
}

// ---------------------------------------------------------------------------
// 6.5 Dependencies and T17
// ---------------------------------------------------------------------------

/// "References into contract clauses are interface references with their
/// natural kind" (6.5, T17): the calls of `requires` and `ensures` are
/// interface `call` dependencies, the body's call is not, and the default
/// set's list holds the interface subset only.
#[test]
fn contract_clause_references_are_interface_dependencies() {
    let checked = Checked::clean(&[("src/m/a.cpl", CONTRACTS)]);
    let index = checked.index();
    assert_eq!(
        deps(&index, "m.a.step"),
        [
            dep("m.a.bound", RefKind::Call, true),
            dep("m.a.helper", RefKind::Call, false),
            dep("m.a.valid", RefKind::Call, true),
            dep("std.prelude.Int", RefKind::TypeUse, true),
        ]
    );
    let interface: Vec<String> = index
        .interface_dependencies(def(&index, "m.a.step"))
        .iter()
        .map(|d| index.version(d.def).path.clone())
        .collect();
    assert_eq!(interface, ["m.a.bound", "m.a.valid", "std.prelude.Int"]);
    let out = inspect(&index, "m.a.step", &IncludeSet::default_set());
    let blocks = out.interface_dependencies.as_ref().expect("default set");
    assert_eq!(
        blocks
            .iter()
            .map(|b| (b.version.path.as_str(), b.relation))
            .collect::<Vec<_>>(),
        [
            ("m.a.bound", Some(Relation::Call)),
            ("m.a.valid", Some(Relation::Call)),
            ("std.prelude.Int", Some(Relation::TypeUse)),
        ]
    );
    assert_eq!(
        to_json(&blocks[0])["relation"],
        json!("call"),
        "a RefKind spelling"
    );
    let out = inspect(&index, "m.a.step", &inc("dependencies"));
    let blocks = out.dependencies.as_ref().expect("included");
    assert_eq!(
        blocks
            .iter()
            .map(|b| (b.version.path.as_str(), b.interface))
            .collect::<Vec<_>>(),
        [
            ("m.a.bound", Some(true)),
            ("m.a.helper", Some(false)),
            ("m.a.valid", Some(true)),
            ("std.prelude.Int", Some(true)),
        ]
    );
    assert_eq!(out.interface_dependencies, None, "not in the effective set");
}

/// A refinement's `where` predicate is its contract (5.1), so its calls
/// are interface dependencies of the refinement (T17); a function using
/// the refinement depends on the refinement by type, not on what its
/// predicate calls.
#[test]
fn a_refinement_predicate_is_an_interface_dependency_source() {
    let checked = Checked::clean(&[("src/m/r.cpl", REFINED)]);
    let index = checked.index();
    assert_eq!(
        deps(&index, "m.r.Even"),
        [
            dep("m.r.is_even", RefKind::Call, true),
            dep("std.prelude.Int", RefKind::TypeUse, true),
        ]
    );
    assert_eq!(
        deps(&index, "m.r.halve"),
        [
            dep("m.r.Even", RefKind::TypeUse, true),
            dep("std.prelude.Int", RefKind::TypeUse, true),
        ]
    );
    let to_even = deps(&index, "m.r.to_even");
    assert!(to_even.contains(&dep("m.r.Even", RefKind::TypeUse, true)));
    assert!(to_even.iter().all(|(p, _, _)| p != "m.r.is_even"));
    let out = inspect(&index, "m.r.Even", &IncludeSet::default_set());
    assert_eq!(
        block_paths(out.interface_dependencies.as_ref().expect("default")),
        ["m.r.is_even", "std.prelude.Int"]
    );
    assert_eq!(
        out.symbol.contract.as_deref(),
        Some(&["where is_even(self)".to_string()][..])
    );
}

/// A property's `for` clauses are its signature (5.1), so the calls in its
/// ranges are interface references (T17: "not in its body, test body, or
/// property body"), while its body's calls are not.
#[test]
fn property_for_clauses_are_interface_references() {
    let checked = Checked::clean(&[("src/m/p.cpl", PROPERTY)]);
    let index = checked.index();
    let prop = "m.p.\"squares are nonnegative\"";
    assert_eq!(
        deps(&index, prop),
        [
            dep("m.p.limit", RefKind::Call, true),
            dep("m.p.square", RefKind::Call, false),
            dep("std.gen.range", RefKind::Call, true),
        ]
    );
    let out = inspect(&index, prop, &IncludeSet::default_set());
    assert_eq!(
        block_paths(out.interface_dependencies.as_ref().expect("default")),
        ["m.p.limit", "std.gen.range"]
    );
    assert_eq!(
        out.symbol.signature.as_deref(),
        Some("property \"squares are nonnegative\"\n    for n in gen.range(0, limit())")
    );
}

/// A test body is never an interface (T17): every dependency of a test is
/// a body dependency, and so is a type named only in a function body's
/// local annotation, while the same function's return type is an
/// interface one.
#[test]
fn test_bodies_and_local_annotations_are_body_references() {
    let checked = Checked::clean(&[(
        "src/m/l.cpl",
        "module m.l\n\npub type Small = Int where self < 10\n\npub fn three() -> Int {\n    let n: Small = 3\n    n\n}\n\ntest \"three is three\" {\n    let x: Small = 3\n    assert three() == x\n}\n",
    )]);
    let index = checked.index();
    assert_eq!(
        deps(&index, "m.l.three"),
        [
            dep("m.l.Small", RefKind::TypeUse, false),
            dep("std.prelude.Int", RefKind::TypeUse, true),
        ]
    );
    let test = deps(&index, "\"three is three\"");
    assert_eq!(
        test,
        [
            dep("m.l.Small", RefKind::TypeUse, false),
            dep("m.l.three", RefKind::Call, false),
        ]
    );
    assert!(index
        .interface_dependencies(def(&index, "\"three is three\""))
        .is_empty());
    let out = inspect(&index, "\"three is three\"", &IncludeSet::default_set());
    assert_eq!(out.interface_dependencies.as_deref(), Some(&[][..]));
}

/// The fields of a struct, the payloads of an enum's variants, and a
/// capability's operations (types and clauses) are interface references
/// of their declaration (5.1 signature and contract, T17).
#[test]
fn member_types_and_operation_clauses_are_interface_references() {
    let checked = Checked::clean(&[("src/m/s.cpl", SHAPES)]);
    let index = checked.index();
    assert_eq!(
        deps(&index, "m.s.Key"),
        [dep("std.prelude.Text", RefKind::TypeUse, true)]
    );
    assert_eq!(
        deps(&index, "m.s.Item"),
        [
            dep("m.s.Key", RefKind::TypeUse, true),
            dep("std.prelude.Int", RefKind::TypeUse, true),
        ]
    );
    assert_eq!(
        deps(&index, "m.s.Event"),
        [
            dep("m.s.Item", RefKind::TypeUse, true),
            dep("m.s.Key", RefKind::TypeUse, true),
        ]
    );
    assert_eq!(
        deps(&index, "m.s.Store"),
        [
            dep("m.s.Item", RefKind::TypeUse, true),
            dep("m.s.Key", RefKind::TypeUse, true),
            dep("m.s.valid_key", RefKind::Call, true),
            dep("std.prelude.Int", RefKind::TypeUse, true),
        ]
    );
    // The body's wrapper pattern comes after the signature's type use.
    assert_eq!(
        deps(&index, "m.s.valid_key"),
        [
            dep("m.s.Key", RefKind::TypeUse, true),
            dep("std.prelude.Bool", RefKind::TypeUse, true),
            dep("std.prelude.Text.len", RefKind::Call, false),
        ]
    );
    assert_eq!(callers(&index, "m.s.valid_key"), ["m.s.Store"]);
}

/// "keeping the first (kind, interface) per target" (6.5): references are
/// in span order, so a wrapper constructed before it is annotated or
/// matched is a `ctor` dependency, listed once; the call sites list every
/// reference in span order.
#[test]
fn the_first_reference_decides_kind_and_interface() {
    let checked = Checked::clean(&[(
        "src/m/k.cpl",
        "module m.k\n\npub type Name(Text)\n\npub fn shout() -> Text {\n    let n = Name(\"hey\")\n    let m: Name = n\n    match m {\n        Name(t) => t\n    }\n}\n",
    )]);
    let index = checked.index();
    assert_eq!(
        deps(&index, "m.k.shout"),
        [
            dep("m.k.Name", RefKind::Ctor, false),
            dep("std.prelude.Text", RefKind::TypeUse, true),
        ]
    );
    let out = inspect(&index, "m.k.shout", &inc("dependencies"));
    let name = block(out.dependencies.as_ref().expect("dependencies"), "m.k.Name");
    assert_eq!(name.relation, Some(Relation::Ctor));
    assert_eq!(name.interface, Some(false));
    let shout = def(&index, "m.k.shout");
    let sites = index.call_sites(shout, def(&index, "m.k.Name"));
    assert_eq!(
        sites.iter().map(|s| s.line).collect::<Vec<_>>(),
        [6, 7, 9],
        "{sites:?}"
    );
}

/// Generic parameters are no definitions, so they are no dependencies;
/// a generic type's arguments and a generic call's callee are; a generic
/// struct whose field has a parameter type depends on nothing.
#[test]
fn generics_depend_on_their_types_not_their_parameters() {
    let checked = Checked::clean(&[("src/m/g.cpl", GENERIC)]);
    let index = checked.index();
    assert_eq!(
        deps(&index, "m.g.head"),
        [
            dep("std.prelude.List", RefKind::TypeUse, true),
            dep("std.prelude.List.first", RefKind::Call, false),
            dep("std.prelude.Option", RefKind::TypeUse, true),
        ]
    );
    assert_eq!(
        deps(&index, "m.g.unbox"),
        [dep("m.g.Box", RefKind::TypeUse, true)]
    );
    assert!(deps(&index, "m.g.Box").is_empty());
    assert_eq!(
        deps(&index, "m.g.boxed"),
        [
            dep("m.g.Box", RefKind::TypeUse, true),
            dep("std.prelude.Int", RefKind::TypeUse, true),
        ]
    );
    assert_eq!(
        deps(&index, "m.g.use_both"),
        [
            dep("m.g.boxed", RefKind::Call, false),
            dep("m.g.head", RefKind::Call, false),
            dep("m.g.unbox", RefKind::Call, false),
            dep("std.prelude.Int", RefKind::TypeUse, true),
            dep("std.prelude.Option.unwrap_or", RefKind::Call, false),
        ]
    );
    assert_eq!(callers(&index, "m.g.Box"), ["m.g.boxed", "m.g.unbox"]);
    let head = inspect(&index, "m.g.head", &inc("signature,callers"));
    assert_eq!(
        head.symbol.signature.as_deref(),
        Some("pub fn head<T>(xs: List<T>) -> Option<T>")
    );
    let callers = head.callers.as_ref().expect("callers");
    assert_eq!(block_paths(callers), ["m.g.use_both"]);
    assert_eq!(
        callers[0].call_sites.as_deref(),
        Some(&[loc("src/m/g.cpl", 21, 17, 21, 29)][..])
    );
    let boxed = inspect(&index, "m.g.Box", &inc("signature"));
    assert_eq!(
        boxed.symbol.signature.as_deref(),
        Some("pub struct Box<T> {\n    value: T\n}")
    );
}

/// Dependencies and callers cross modules and the files of one module:
/// every block names the module and file of its own declaration.
#[test]
fn dependencies_cross_modules_with_their_own_locations() {
    let checked = multi_file();
    let index = checked.index();
    assert_eq!(
        deps(&index, "app.main.run"),
        [
            dep("lib.core.twice", RefKind::Call, false),
            dep("std.prelude.Int", RefKind::TypeUse, true),
        ]
    );
    let out = inspect(&index, "app.main.run", &inc("dependencies"));
    let twice = block(
        out.dependencies.as_ref().expect("included"),
        "lib.core.twice",
    );
    assert_eq!(twice.module, "lib.core");
    assert_eq!(twice.location.file, "src/lib/core/extra.cpl");
    assert_eq!(twice.location.line, 3);
    assert_eq!(*index.version(def(&index, "lib.core.twice")), twice.version);
}

// ---------------------------------------------------------------------------
// 6.5 Tests
// ---------------------------------------------------------------------------

/// `tests_for(d)`: every test or property whose transitive closure of
/// `Call` and `CapabilityOp` edges reaches `d` (6.5), through contract
/// clauses too; `direct` when the test itself calls `d`, else
/// `transitive`; sorted by display path.
#[test]
fn tests_reach_targets_directly_transitively_and_through_contracts() {
    let checked = Checked::clean(&[("src/m/t.cpl", TESTED)]);
    let index = checked.index();
    let relations = |arg: &str| -> Vec<(String, Option<Relation>)> {
        inspect(&index, arg, &inc("tests"))
            .tests
            .expect("included")
            .iter()
            .map(|b| (b.version.path.clone(), b.relation))
            .collect()
    };
    let t = |name: &str, r: Relation| (format!("m.t.\"{name}\""), Some(r));
    assert_eq!(
        relations("m.t.f"),
        [
            t("a direct", Relation::Direct),
            t("b transitive", Relation::Transitive),
            t("c both", Relation::Direct),
            t("d contract", Relation::Transitive),
        ]
    );
    assert_eq!(
        relations("m.t.g"),
        [
            t("b transitive", Relation::Direct),
            t("c both", Relation::Direct)
        ]
    );
    assert_eq!(
        relations("m.t.guard"),
        [t("d contract", Relation::Transitive)]
    );
    assert_eq!(relations("m.t.k"), [t("d contract", Relation::Direct)]);
    let json = to_json(&inspect(&index, "m.t.f", &inc("tests")));
    assert_eq!(json["tests"][1]["relation"], json!("transitive"));
    assert_eq!(json["tests"][0]["relation"], json!("direct"));
}

/// Type uses, field accesses, constructors, and checks are no call edges
/// (6.5), so a test that only builds a struct, or a function reached only
/// through a refinement's `check`, has no tests; nothing reaches a test.
#[test]
fn tests_do_not_reach_through_types_constructors_or_checks() {
    let checked = Checked::clean(&[("src/m/t.cpl", TESTED)]);
    let index = checked.index();
    assert!(tests_for(&index, "m.t.Box").is_empty());
    assert_eq!(
        inspect(&index, "\"a direct\"", &inc("tests"))
            .tests
            .as_deref(),
        Some(&[][..])
    );
    let refined = Checked::clean(&[("src/m/r.cpl", REFINED)]);
    let index = refined.index();
    assert_eq!(tests_for(&index, "m.r.to_even"), ["m.r.\"four is even\""]);
    assert!(tests_for(&index, "m.r.Even").is_empty());
    assert!(tests_for(&index, "m.r.is_even").is_empty());
}

/// A property's `for` clauses call at setup, so their callees are reached
/// directly, as are its body's.
#[test]
fn a_property_reaches_what_its_for_clauses_call() {
    let checked = Checked::clean(&[("src/m/p.cpl", PROPERTY)]);
    let index = checked.index();
    let prop = "m.p.\"squares are nonnegative\"";
    for target in ["m.p.limit", "m.p.square", "std.gen.range"] {
        let out = inspect(&index, target, &inc("tests"));
        let tests = out.tests.expect("included");
        assert_eq!(block_paths(&tests), [prop], "{target}");
        assert_eq!(tests[0].relation, Some(Relation::Direct), "{target}");
        assert_eq!(tests[0].kind, "prop");
    }
}

// ---------------------------------------------------------------------------
// 7.2 and S48: hole dependencies
// ---------------------------------------------------------------------------

const HOLE_CORE: &str = r#"module lib.core

pub struct Item {
    name: Text
}

pub fn helper(n: Int) -> Int {
    n + 1
}
"#;

const HOLE_USER: &str = r#"module app.h

use lib.core.{Item, helper}

pub fn fill(item: Item, n: Int) -> Int
    requires n >= 0
{
    let _k = helper(n)
    ?rest
}
"#;

fn holed() -> Checked {
    Checked::files(&[
        ("src/lib/core.cpl", HOLE_CORE),
        ("src/app/h.cpl", HOLE_USER),
    ])
}

/// `symbols`: "the `SymbolVersion`s of every definition the enclosing
/// declaration references" (7.2), each exactly the index's version;
/// `modules` lists the referenced project modules once each, with their
/// content hashes, in module-path order (std is covered by
/// `toolchain.std`, as every example of 7.2 and 8 shows); `toolchain` is
/// `{ cpl, edition, std }`.
#[test]
fn hole_dependencies_list_every_referenced_definition_and_module() {
    let checked = holed();
    assert!(!checked.check.has_errors());
    let index = checked.index();
    assert_eq!(checked.check.holes.len(), 1);
    let deps = index.hole_dependencies(&checked.check.holes[0]);
    assert_eq!(
        deps.symbols
            .iter()
            .map(|s| s.path.as_str())
            .collect::<Vec<_>>(),
        ["lib.core.Item", "lib.core.helper", "std.prelude.Int"]
    );
    for symbol in &deps.symbols {
        assert_eq!(*symbol, *index.version(def(&index, &symbol.path)));
    }
    let core = ModuleHash {
        module: "lib.core".to_string(),
        content_hash: module_content_hash(&checked.project, &[checked.file("src/lib/core.cpl")]),
    };
    assert!(deps.modules.contains(&core), "{:?}", deps.modules);
    assert!(deps.modules.iter().all(|m| !m.module.starts_with("std")));
    let names: Vec<&str> = deps.modules.iter().map(|m| m.module.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(names, sorted);
    assert_eq!(
        to_json(&deps.toolchain),
        json!({ "cpl": "0.1.0", "edition": "0.1", "std": checked.project.std_digest() })
    );
    assert_eq!(deps.source_revision, "unversioned");
    assert_eq!(deps.dirty, None);
}

/// A recursive function references itself (6.5 counts every reference),
/// so its hole's `symbols` include its own version (7.2: "every definition
/// the enclosing declaration references").
#[test]
fn a_recursive_holed_function_lists_itself_among_its_symbols() {
    let checked = Checked::files(&[("src/m/rec.cpl", RECURSIVE)]);
    assert!(!checked.check.has_errors());
    let index = checked.index();
    let report = checked
        .check
        .holes
        .iter()
        .find(|h| h.name == "extra")
        .expect("?extra");
    let symbols: Vec<String> = index
        .hole_dependencies(report)
        .symbols
        .iter()
        .map(|s| s.path.clone())
        .collect();
    assert_eq!(symbols, ["m.rec.down", "std.prelude.Int"]);
}

/// A module of two files is one `modules` entry, whose content hash is
/// blake3 of both files' formatted texts in path order joined with LF
/// (1.8 `module_content_hash`, semantics S48).
#[test]
fn a_multi_file_module_is_one_module_entry_with_both_files_hashed() {
    let checked = multi_file();
    let index = checked.index();
    let report = checked
        .check
        .holes
        .iter()
        .find(|h| h.name == "todo")
        .expect("?todo");
    let deps = index.hole_dependencies(report);
    let expected = content_hash_of(
        &checked.project,
        &["src/lib/core/extra.cpl", "src/lib/core.cpl"],
    );
    let core: Vec<&ModuleHash> = deps
        .modules
        .iter()
        .filter(|m| m.module == "lib.core")
        .collect();
    assert_eq!(core.len(), 1, "{:?}", deps.modules);
    assert_eq!(core[0].content_hash, expected);
    assert_ne!(
        expected,
        content_hash_of(&checked.project, &["src/lib/core.cpl"]),
        "both files count"
    );
    assert_eq!(
        deps.symbols
            .iter()
            .map(|s| s.path.as_str())
            .collect::<Vec<_>>(),
        ["lib.core.helper", "std.prelude.Int"]
    );
    // The verification receipt of a caller in another module lists the
    // module once as well.
    let core_receipt = index.verification_receipt_core(def(&index, "app.main.run"), false);
    assert_eq!(
        core_receipt
            .modules
            .iter()
            .map(|m| (m.module.as_str(), m.content_hash.clone()))
            .collect::<Vec<_>>(),
        [
            (
                "app.main",
                content_hash_of(&checked.project, &["src/app/main.cpl"])
            ),
            ("lib.core", expected),
        ]
    );
}

/// `source_revision` is the commit, or `"unversioned"` without one;
/// `dirty` is present, `true`, exactly when a commit's checkout has
/// uncommitted changes (S48, Appendix C), in the hole report and in every
/// receipt core; the JSON keys come in declaration order.
#[test]
fn revision_fields_of_hole_reports_and_receipt_cores() {
    let checked = holed();
    let commit = "c".repeat(40);
    let cases = [
        (
            Some(commit.as_str()),
            RevisionState::Clean,
            commit.as_str(),
            None,
        ),
        (
            Some(commit.as_str()),
            RevisionState::Worktree,
            commit.as_str(),
            Some(true),
        ),
        (None, RevisionState::Worktree, "unversioned", None),
    ];
    for (c, state, source_revision, dirty) in cases {
        let index = checked.index_at(revision(&checked.project, c, state));
        let hole = index.hole_dependencies(&checked.check.holes[0]);
        assert_eq!(hole.source_revision, source_revision);
        assert_eq!(hole.dirty, dirty);
        let fill = def(&index, "app.h.fill");
        let verification = index.verification_receipt_core(fill, false);
        assert_eq!(verification.source_revision, source_revision);
        assert_eq!(verification.dirty, dirty);
        let test = index.test_receipt_core(&[fill], json!({}));
        assert_eq!(test.source_revision, source_revision);
        assert_eq!(test.dirty, dirty);
        let proof = index
            .prove(&Target::parse("app.h.fill"))
            .expect("a function");
        assert_eq!(proof.output.receipt.revision, *index.revision());
        assert_eq!(proof.output.receipt.core, verification);
        let mut hole_keys = vec![
            "source_revision",
            "dirty",
            "toolchain",
            "modules",
            "symbols",
        ];
        let mut core_keys = vec![
            "source_revision",
            "dirty",
            "toolchain",
            "modules",
            "check_configuration",
            "assumptions",
        ];
        if dirty.is_none() {
            hole_keys.remove(1);
            core_keys.remove(1);
        }
        assert_eq!(keys_of(&hole), hole_keys);
        assert_eq!(keys_of(&verification), core_keys);
        core_keys.extend(["fixtures", "environment"]);
        assert_eq!(keys_of(&test), core_keys);
    }
}

/// Under `--release` a hole in a test body stays the diagnostic `hole`
/// (a warning, semantics S47), whose payload is S48's: `cpl check` adds its
/// `dependencies` like any other `hole` (7.2).
#[test]
fn a_test_body_hole_under_release_keeps_its_dependencies() {
    let project = load_tree(&tree(&[(
        "src/m/th.cpl",
        "module m.th\n\npub fn f() -> Int {\n    1\n}\n\ntest \"unfinished\" {\n    let x: Int = ?later\n    assert x == f()\n}\n",
    )]));
    for release in [false, true] {
        let checked = Checked::with(
            project.clone(),
            &CheckConfig {
                release,
                ..CheckConfig::default()
            },
        );
        let index = checked.index();
        let completed = index.complete_check_diagnostics(&checked.check.diagnostics, release);
        let hole = completed
            .iter()
            .find(|d| d.code == "hole")
            .unwrap_or_else(|| panic!("release {release}: no hole diagnostic"));
        let expected = if release {
            Severity::Warning
        } else {
            Severity::Info
        };
        assert_eq!(hole.severity, expected);
        let symbols: Vec<&str> = hole.data["dependencies"]["symbols"]
            .as_array()
            .unwrap_or_else(|| panic!("release {release}: no dependencies"))
            .iter()
            .map(|s| s["path"].as_str().expect("path"))
            .collect();
        assert_eq!(symbols, ["m.th.f", "std.prelude.Int"], "release {release}");
    }
}

/// An inspected body's `holes` are "the same hole payloads" as the check's
/// `hole` diagnostics (7.2), `dependencies` included: one shape for one
/// report (Appendix C `HoleReportJson`).
#[test]
fn inspected_holes_are_the_completed_hole_payloads() {
    let checked = Checked::example("holes");
    let index = checked.index();
    let completed = index.complete_check_diagnostics(&checked.check.diagnostics, false);
    for (arg, hole) in [
        ("holes.pages.paginate", "build_pages"),
        ("summarize", "summary"),
    ] {
        let out = inspect(&index, arg, &inc("implementation"));
        let holes = out.symbol.holes.expect("included");
        assert_eq!(holes.len(), 1, "{arg}");
        let diagnostic = completed
            .iter()
            .find(|d| d.code == "hole" && d.data["hole"] == json!(hole))
            .expect("the hole diagnostic");
        assert_eq!(to_json(&holes[0]), Value::Object(diagnostic.data.clone()));
    }
}

/// 1.8 builds the index with `Index::build(program, project, revision)`,
/// and 6.4 says the `holes` of an `implementation` are "the holes inside
/// the body": an index built that way must not answer that a holed body
/// has none (it may report them, or refuse, but an empty list is wrong).
#[test]
fn an_index_built_by_the_spec_constructor_still_reports_body_holes() {
    let checked = Checked::example("holes");
    let index = Index::build(
        &checked.check.program,
        &checked.project,
        unversioned(&checked.project),
    );
    let out = index.inspect(&Target::parse("summarize"), &inc("implementation"));
    if let Ok(out) = out {
        let holes = out.symbol.holes.expect("implementation included");
        assert_eq!(
            holes.iter().map(|h| h.hole.as_str()).collect::<Vec<_>>(),
            ["summary"],
            "the body `{{ ?summary }}` has one hole"
        );
    }
}

// ---------------------------------------------------------------------------
// S59 and 8: receipts built from the index
// ---------------------------------------------------------------------------

/// S59: a receipt's `modules` are "every module in the static call closure
/// of the tested or verified declaration": calls in bodies and contract
/// clauses and capability operations (and what the capability's clauses
/// call) count, a type named only in a signature does not; a hole's
/// `modules` are every module the declaration refers to, types included
/// (S48).
#[test]
fn receipt_modules_follow_the_call_closure_and_hole_modules_the_references() {
    let checked = Checked::clean(&[
        (
            "src/c/a.cpl",
            "module c.a\n\nuse c.b.{middle}\nuse c.d.{Tag}\nuse c.e.{ok}\nuse c.f.{Store}\n\npub fn top(t: Tag, n: Int) -> Int\n    requires ok(n)\n{\n    middle(n)\n}\n\npub fn fetch(using store: Store) -> Int\n    effects { store.get }\n{\n    store.get(\"k\")\n}\n\npub fn pending(t: Tag) -> Int {\n    ?todo\n}\n",
        ),
        (
            "src/c/b.cpl",
            "module c.b\n\nuse c.c.{leaf}\n\npub fn middle(n: Int) -> Int {\n    leaf(n)\n}\n",
        ),
        ("src/c/c.cpl", "module c.c\n\npub fn leaf(n: Int) -> Int {\n    n\n}\n"),
        ("src/c/d.cpl", "module c.d\n\npub struct Tag {\n    v: Int\n}\n"),
        ("src/c/e.cpl", "module c.e\n\npub fn ok(n: Int) -> Bool {\n    n > 0\n}\n"),
        (
            "src/c/f.cpl",
            "module c.f\n\nuse c.g.{nonempty}\n\npub capability Store {\n    fn get(key: Text) -> Int\n        requires nonempty(key)\n}\n",
        ),
        (
            "src/c/g.cpl",
            "module c.g\n\npub fn nonempty(t: Text) -> Bool {\n    t.len > 0\n}\n",
        ),
    ]);
    let index = checked.index();
    let modules = |arg: &str| -> Vec<String> {
        index
            .verification_receipt_core(def(&index, arg), false)
            .modules
            .iter()
            .map(|m| m.module.clone())
            .collect()
    };
    assert_eq!(modules("c.a.top"), ["c.a", "c.b", "c.c", "c.e"]);
    assert_eq!(modules("c.a.fetch"), ["c.a", "c.f", "c.g"]);
    let core = index.verification_receipt_core(def(&index, "c.a.top"), false);
    for m in &core.modules {
        let file = format!("src/{}.cpl", m.module.replace('.', "/"));
        assert_eq!(m.content_hash, content_hash_of(&checked.project, &[&file]));
    }
    // `cpl prove` records the same core.
    let proof = index.prove(&Target::parse("c.a.top")).expect("a function");
    assert_eq!(proof.output.receipt.core, core);
    // The hole of `pending` refers to `Tag`, so its module is listed there,
    // while the call closure of `pending` is `pending` alone.
    let report = checked
        .check
        .holes
        .iter()
        .find(|h| h.name == "todo")
        .expect("?todo");
    let hole_modules: Vec<String> = index
        .hole_dependencies(report)
        .modules
        .iter()
        .map(|m| m.module.clone())
        .collect();
    assert!(
        hole_modules.contains(&"c.d".to_string()),
        "{hole_modules:?}"
    );
    assert_eq!(modules("c.a.pending"), ["c.a"]);
}

const VERIFIED_MANIFEST: &str = "[project]\nname = \"v\"\nedition = \"0.1\"\n\n[verification]\nproof_timeout_ms = 2500\nadmit = [\"hlp_1x\", \"v.m.other\"]\n";

const VERIFIED: &str = r#"module v.m

@id("hlp_1x")
pub fn helper(n: Int) -> Int {
    n + 1
}

pub fn other(n: Int) -> Int {
    n
}

@prove
pub fn proved(n: Int) -> Int
    ensures result > n
{
    helper(n)
}
"#;

/// The verification receipt's `configuration` restates the manifest's
/// `[verification]` section (8.1: `proof_timeout_ms`, and `admitted`, the
/// "`[verification] admit` ids that were in effect"), the declaration's
/// policy, and `release: false` for `cpl prove`; its `dependencies` are the
/// target's interface and body dependencies with their versions, sorted
/// by display path.
#[test]
fn the_verification_receipt_restates_the_manifest_and_the_dependencies() {
    let mut files = tree(&[("src/v/m.cpl", VERIFIED)]);
    files.files.insert(
        "cpl.toml".to_string(),
        VERIFIED_MANIFEST.as_bytes().to_vec(),
    );
    let checked = Checked::of(load_tree(&files));
    checked.assert_clean();
    let index = checked.index();
    let proof = index.prove(&Target::parse("proved")).expect("a function");
    let receipt = &proof.output.receipt;
    assert_eq!(
        to_json(&receipt.configuration),
        json!({ "release": false, "policy": { "prove": true }, "proof_timeout_ms": 2500, "admitted": ["hlp_1x", "v.m.other"] })
    );
    assert_eq!(
        receipt
            .dependencies
            .iter()
            .map(|v| v.path.as_str())
            .collect::<Vec<_>>(),
        ["std.prelude.Int", "v.m.helper"]
    );
    assert_eq!(receipt.dependencies[1].id.as_str(), "hlp_1x");
    assert_eq!(
        receipt.dependencies[1],
        *index.version(def(&index, "v.m.helper"))
    );
    assert_eq!(receipt.target, *index.version(def(&index, "v.m.proved")));
    assert_eq!(receipt.inputs.manifest, checked.project.manifest_digest);
    assert_eq!(
        receipt.inputs.sources.keys().collect::<Vec<_>>(),
        ["cpl.toml", "src/v/m.cpl"]
    );
    let other = index
        .prove(&Target::parse("v.m.other"))
        .expect("a function");
    let configuration = to_json(&other.output.receipt.configuration);
    assert_eq!(configuration["policy"], json!({ "prove": false }));
    assert_eq!(configuration["admitted"], json!(["hlp_1x", "v.m.other"]));
}

const TESTS_X: &str = "module x.x\n\npub fn fx(n: Int) -> Int {\n    n + 1\n}\n";
const TESTS_Y: &str = "module y.y\n\npub fn fy(n: Int) -> Int {\n    n * 2\n}\n";
const TESTS_T: &str = r#"module tests.t

use x.x.{fx}
use y.y.{fy}

test "fx works" {
    assert fx(1) == 2
}

test "fy works" {
    assert fy(2) == 4
}
"#;

/// Each `test_result` receipt covers its own test's static call closure
/// (S59), the run's `core.modules` the union over the selected tests
/// (8: "for a test receipt the union over the selected tests"), and
/// `tested` the versions of the closure, sorted and one per id (8.2).
#[test]
fn test_record_receipts_cover_their_own_closure_and_the_run_the_union() {
    let checked = Checked::clean(&[
        ("src/x/x.cpl", TESTS_X),
        ("src/y/y.cpl", TESTS_Y),
        ("src/tests/t.cpl", TESTS_T),
    ]);
    let index = checked.index();
    let fx_test = "tests.t.\"fx works\"";
    let fy_test = "tests.t.\"fy works\"";
    let selected = [def(&index, fx_test), def(&index, fy_test)];
    let mut report = report_of(vec![
        record(fx_test, TestKind::Test),
        record(fy_test, TestKind::Test),
    ]);
    let receipt = index.complete_test_report(&mut report, &selected, &run_config(), 10_000);
    let modules = |core: &cpl_diag::json::ReceiptCore| -> Vec<String> {
        core.modules.iter().map(|m| m.module.clone()).collect()
    };
    assert_eq!(
        modules(report.results[0].receipt.as_ref().expect("filled")),
        ["tests.t", "x.x"]
    );
    assert_eq!(
        modules(report.results[1].receipt.as_ref().expect("filled")),
        ["tests.t", "y.y"]
    );
    assert_eq!(modules(&receipt.core), ["tests.t", "x.x", "y.y"]);
    let tested: Vec<&str> = receipt.tested.iter().map(|v| v.path.as_str()).collect();
    for path in ["x.x.fx", "y.y.fy"] {
        assert!(tested.contains(&path), "{path} in {tested:?}");
        let version = receipt
            .tested
            .iter()
            .find(|v| v.path == path)
            .expect("listed");
        assert_eq!(*version, *index.version(def(&index, path)));
    }
    let mut sorted = tested.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(tested, sorted);
    assert_eq!(receipt.status, TestStatus::Tested);
    assert_eq!(receipt.environment, cpl_index::test_environment());
}

/// The `check_configuration` of a test receipt is a sorted map (7.1 names
/// "a receipt's `check_configuration`" among the sorted `serde_json`
/// maps, T4): `cases`, `max_call_depth`, `max_shrinks`, `seed` in that
/// order, with the values in effect.
#[test]
fn test_check_configuration_keys_are_sorted() {
    let mut config = run_config();
    config.seed = u64::MAX;
    config.cases = 12;
    config.max_shrinks = 34;
    let value = cpl_index::test_check_configuration(&config, 56);
    assert_eq!(
        written_keys(&serde_json::to_string(&value).expect("serializes")),
        ["cases", "max_call_depth", "max_shrinks", "seed"]
    );
    assert_eq!(
        value,
        json!({ "cases": 12, "max_call_depth": 56, "max_shrinks": 34, "seed": u64::MAX })
    );
    let text = serde_json::to_string(&value).expect("serializes");
    assert!(text.contains("\"seed\":18446744073709551615"), "{text}");
}

/// A receipt binds its evidence to its revision (S59, 8): the same check
/// at a clean and at a dirty checkout of one commit, or unversioned, gives
/// three different receipt digests, while two builds at one revision give
/// one.
#[test]
fn receipt_digests_distinguish_clean_dirty_and_unversioned_revisions() {
    let checked = Checked::example("reader");
    let commit = "d".repeat(40);
    let digest_at = |c: Option<&str>, state: RevisionState| {
        let index = checked.index_at(revision(&checked.project, c, state));
        let proof = index.prove(&Target::parse("paginate")).expect("a function");
        receipt_digest(&to_json(&proof.output.receipt))
    };
    let clean = digest_at(Some(&commit), RevisionState::Clean);
    let dirty = digest_at(Some(&commit), RevisionState::Worktree);
    let none = digest_at(None, RevisionState::Worktree);
    assert_ne!(clean, dirty);
    assert_ne!(clean, none);
    assert_ne!(dirty, none);
    assert_eq!(clean, digest_at(Some(&commit), RevisionState::Clean));
}

// ---------------------------------------------------------------------------
// 8.1 Obligations
// ---------------------------------------------------------------------------

/// Appendix C: `Obligation::reason` and `evidence` are always written,
/// `null` when absent: a refinement the checker proved has `reason: null`
/// and `{"by": "literal"}` evidence, an open division `evidence: null`.
#[test]
fn obligation_json_always_writes_reason_and_evidence() {
    let checked = Checked::clean(&[(
        "src/m/l.cpl",
        "module m.l\n\npub type Small = Int where self < 10\n\npub fn three() -> Int {\n    let n: Small = 3\n    n\n}\n\npub fn half(n: Int) -> Int {\n    n / 2\n}\n",
    )]);
    let index = checked.index();
    let three = to_json(&inspect(&index, "three", &inc("obligations")));
    let refine = &three["obligations"][0];
    assert_eq!(refine["id"], json!("m.l.three#refine/1"));
    assert_eq!(refine["status"], json!("proved"));
    assert_eq!(refine.get("reason"), Some(&Value::Null));
    assert_eq!(refine["evidence"], json!({ "by": "literal" }));
    assert_eq!(refine["assumptions"], json!([]));
    let half = to_json(&inspect(&index, "half", &inc("obligations")));
    let division = &half["obligations"][0];
    assert_eq!(division["id"], json!("m.l.half#div/1"));
    assert_eq!(division["kind"], json!("division"));
    assert_eq!(division["clause"], json!("2 != 0"));
    assert_eq!(division["status"], json!("unknown"));
    assert_eq!(division["reason"], json!("verifier_unavailable"));
    assert_eq!(division.get("evidence"), Some(&Value::Null));
    let text = serde_json::to_string(&inspect(&index, "half", &inc("obligations")).obligations)
        .expect("serializes");
    assert_eq!(
        written_keys(&text[1..]),
        [
            "id",
            "kind",
            "clause",
            "location",
            "status",
            "reason",
            "evidence",
            "assumptions"
        ]
    );
    let status = inspect(&index, "half", &inc("obligations"))
        .obligations
        .expect("included")[0]
        .status;
    assert_eq!(status, ObligationStatus::Unknown);
}

/// 8.1 numbers `call/<n>` "in body order" and `div/<n>` "in body order,
/// clauses included"; whether the call row's count also covers the
/// function's own clauses is left open there. Semantics 5.1 settles it:
/// contract expressions must be "total under their preceding assumptions"
/// (design line 118), and every `/` and `%` "in the body or clauses" is an
/// obligation "exactly as a callee's precondition is" (likewise stdlib S8),
/// so a call in a clause is a call site whose callee's preconditions are
/// obligations, numbered in the one walk the division row describes: the
/// clauses in canonical order, then the body. Every call site counts,
/// whether or not its callee has a precondition, so `firsts`'s body call
/// is call site 3 after the two `len` calls of its `ensures`, and the
/// `take` of `guarded`'s second `requires` (call site 2, after the outer
/// `len`) is an obligation of `guarded`.
#[test]
fn call_obligations_number_the_call_sites_of_clauses_and_body() {
    let checked = Checked::clean(&[(
        "src/m/ob.cpl",
        "module m.ob\n\npub fn firsts(xs: List<Int>) -> List<Int>\n    ensures result.len <= xs.len\n{\n    xs.take(1)\n}\n\npub fn guarded(xs: List<Int>, n: Int) -> Int\n    requires n >= 0\n    requires xs.take(n).len <= n\n{\n    n\n}\n",
    )]);
    let index = checked.index();
    let calls = |path: &str| -> Vec<(String, String)> {
        index
            .obligations(def(&index, path))
            .into_iter()
            .filter(|o| o.kind == "call_requires")
            .map(|o| (o.id, o.clause))
            .collect()
    };
    assert_eq!(
        calls("m.ob.firsts"),
        [(
            "m.ob.firsts#call/3/requires/1".to_string(),
            "n >= 0".to_string()
        )]
    );
    assert_eq!(
        calls("m.ob.guarded"),
        [(
            "m.ob.guarded#call/2/requires/1".to_string(),
            "n >= 0".to_string()
        )]
    );
}

// ---------------------------------------------------------------------------
// index_partial, changes
// ---------------------------------------------------------------------------

/// "the project must check without errors for callers/tests to be
/// complete" (7.2 `cpl inspect`): warnings alone leave the index complete,
/// and a project-level error (an unknown manifest key, T35) makes it
/// partial although no definition has errors.
#[test]
fn index_partial_follows_errors_not_warnings() {
    let warned = Checked::files(&[(
        "src/m/w.cpl",
        "module m.w\n\nuse std.text.{trim}\n\npub fn f() -> Int {\n    1\n}\n",
    )]);
    assert!(warned
        .check
        .diagnostics
        .iter()
        .any(|d| d.severity == Severity::Warning));
    assert!(!warned.check.has_errors());
    assert_eq!(warned.index().partial(), None);

    let mut files = tree(&[(
        "src/m/w.cpl",
        "module m.w\n\npub fn f() -> Int {\n    1\n}\n",
    )]);
    files.files.insert(
        "cpl.toml".to_string(),
        b"[project]\nname = \"t\"\nedition = \"0.1\"\ncolour = \"red\"\n".to_vec(),
    );
    let erred = Checked::of(load_tree(&files));
    assert!(erred
        .check
        .diagnostics
        .iter()
        .any(|d| d.code == "project_manifest_unknown_key" && d.severity == Severity::Error));
    let index = erred.index();
    let partial = index.partial().expect("the project has an error");
    assert_eq!(partial.code, "index_partial");
    assert_eq!(partial.category, Category::Index);
    assert_eq!(partial.severity, Severity::Warning);
    // inspect still answers.
    assert!(index
        .inspect(&Target::parse("m.w.f"), &IncludeSet::default_set())
        .is_ok());
}

/// `changes` (6.2): the semantic-diff statements about the target between
/// the `--since` revision and the current one, with `since` that
/// revision: a rename of an annotated declaration is `identity_renamed`
/// alone, an edited body `body_changed`, a new declaration
/// `identity_added`; statements about other identities are left out.
#[test]
fn changes_report_the_statements_about_the_target_only() {
    use cpl_diag::json::DiffStatementKind as K;
    let checked = Checked::clean(&[("src/m/c.cpl", CHANGED_NEW)]);
    let base = changes_base_of(&[("src/m/c.cpl", CHANGED_OLD)]);
    let since = base.revision.clone();
    let index = checked.index().with_changes_base(base);
    let statements = |arg: &str| -> Vec<(K, String, Option<Facet>)> {
        let out = inspect(&index, arg, &inc("changes"));
        let changes = out.changes.expect("included");
        assert_eq!(changes.since, since);
        changes
            .statements
            .iter()
            .map(|s| (s.statement, s.id.as_str().to_string(), s.facet))
            .collect()
    };
    assert_eq!(
        statements("new_name"),
        [(
            K::IdentityRenamed,
            "fn_aaaaaaaaaaaaaaaa".to_string(),
            Some(Facet::Name)
        )]
    );
    assert_eq!(
        statements("m.c.kept"),
        [(K::BodyChanged, "m.c.kept".to_string(), Some(Facet::Body))]
    );
    assert_eq!(
        statements("added"),
        [(K::IdentityAdded, "m.c.added".to_string(), None)]
    );
    assert_eq!(
        statements("\"kept is two\""),
        [(K::IdentityAdded, "m.c.\"kept is two\"".to_string(), None)]
    );
}

// ---------------------------------------------------------------------------
// Determinism
// ---------------------------------------------------------------------------

/// "every output is byte-identical" given the same source set (7.1):
/// loading `examples/reader` from its directory and from an in-memory tree
/// of the same files ("Loading from a tree and from a directory produce
/// identical Projects", 1.6) gives identical inspect responses for every
/// definition, identical `cpl prove` outputs, and identical receipt
/// digests.
#[test]
fn two_builds_from_a_directory_and_a_tree_answer_identically() {
    let from_dir = Checked::example("reader");
    let from_tree = Checked::of(load_tree(&tree_of_dir(&workspace_path("examples/reader"))));
    let (a, b) = (from_dir.index(), from_tree.index());
    assert_eq!(a.revision(), b.revision());
    let project_defs = |index: &Index<'_>| -> Vec<String> {
        let program = index.program();
        let mut out: Vec<String> = program
            .defs()
            .filter(|d| program.module(d.module).origin != cpl_diag::FileOrigin::Std)
            .map(|d| index.version(d.id).path.clone())
            .collect();
        out.sort();
        out
    };
    let paths = project_defs(&a);
    assert_eq!(paths, project_defs(&b));
    assert!(paths.len() > 10);
    for path in &paths {
        let target = Target::parse(path);
        for include in [everything(), IncludeSet::default_set()] {
            let left = serde_json::to_string(&a.inspect(&target, &include).expect("resolves"))
                .expect("serializes");
            let right = serde_json::to_string(&b.inspect(&target, &include).expect("resolves"))
                .expect("serializes");
            assert_eq!(left, right, "{path}");
        }
        if let (Ok(mut p), Ok(mut q)) = (a.prove(&target), b.prove(&target)) {
            assert_eq!(
                receipt_digest(&to_json(&p.output.receipt)),
                receipt_digest(&to_json(&q.output.receipt)),
                "{path}"
            );
            p.output.receipt.produced_at.clear();
            q.output.receipt.produced_at.clear();
            assert_eq!(p, q, "{path}");
        }
    }
    let completed = |c: &Checked, index: &Index<'_>| -> String {
        let diagnostics: Vec<_> = index
            .complete_check_diagnostics(&c.check.diagnostics, false)
            .iter()
            .map(|d| d.to_json(&c.project.source_map))
            .collect();
        serde_json::to_string(&diagnostics).expect("serializes")
    };
    assert_eq!(completed(&from_dir, &a), completed(&from_tree, &b));
}

/// Responses do not depend on the order in which declarations and files
/// were collected: the same declarations split differently over two files
/// of one module give the same callers, dependencies, and tests lists
/// (all "sorted by display path", 6.5).
#[test]
fn lists_do_not_depend_on_declaration_or_file_order() {
    let one = Checked::clean(&[("src/m/t.cpl", TESTED)]);
    let (head, tail) = TESTED.split_at(TESTED.find("test \"a direct\"").expect("tests"));
    let split = Checked::clean(&[
        ("src/m/t/tests.cpl", &format!("module m.t\n\n{tail}")),
        ("src/m/t.cpl", head),
    ]);
    let (a, b) = (one.index(), split.index());
    for path in ["m.t.f", "m.t.g", "m.t.guard", "m.t.k", "m.t.Box"] {
        assert_eq!(callers(&a, path), callers(&b, path), "{path}");
        assert_eq!(tests_for(&a, path), tests_for(&b, path), "{path}");
        assert_eq!(deps(&a, path), deps(&b, path), "{path}");
        let left = to_json(&inspect(&a, path, &inc("tests,dependencies")));
        let right = to_json(&inspect(&b, path, &inc("tests,dependencies")));
        let strip = |mut v: Value| {
            // Locations name the file, which differs by construction.
            for list in ["tests", "dependencies"] {
                if let Some(items) = v[list].as_array_mut() {
                    for item in items {
                        item.as_object_mut().expect("block").remove("location");
                    }
                }
            }
            v["symbol"]
                .as_object_mut()
                .expect("block")
                .remove("location");
            v
        };
        assert_eq!(strip(left), strip(right), "{path}");
    }
}

// ---------------------------------------------------------------------------
// More shapes: effects, clause order, hole reports, check and prove outputs
// ---------------------------------------------------------------------------

const EFFECTS: &str = r#"module m.e

pub capability Alpha {
    fn read() -> Int
    fn write(v: Int) -> Int
}

pub capability Beta {
    fn ping() -> Int
}

pub fn both(using b: Beta, using a: Alpha) -> Int
    effects { b.ping, a.write, a.read }
{
    a.read() + b.ping() + a.write(1)
}
"#;

/// `effects`: "sorted `param.op` strings" (6.4), whatever the order of the
/// bound and of the `using` parameters; the capabilities are interface type
/// uses of the function (its `using` parameters are in its signature).
#[test]
fn effects_are_sorted_param_op_strings() {
    let checked = Checked::clean(&[("src/m/e.cpl", EFFECTS)]);
    let index = checked.index();
    let out = inspect(&index, "both", &inc("effects,signature"));
    assert_eq!(
        out.symbol.effects.as_deref(),
        Some(
            &[
                "a.read".to_string(),
                "a.write".to_string(),
                "b.ping".to_string()
            ][..]
        )
    );
    assert_eq!(
        out.symbol.signature.as_deref(),
        Some("pub fn both(using b: Beta, using a: Alpha) -> Int")
    );
    let interface: Vec<(String, RefKind, bool)> = deps(&index, "m.e.both")
        .into_iter()
        .filter(|d| d.2)
        .collect();
    assert_eq!(
        interface,
        [
            dep("m.e.Alpha", RefKind::TypeUse, true),
            dep("m.e.Beta", RefKind::TypeUse, true),
            dep("std.prelude.Int", RefKind::TypeUse, true),
        ]
    );
    assert_eq!(callers(&index, "m.e.Alpha"), ["m.e.both"]);
    assert_eq!(callers(&index, "m.e.Beta"), ["m.e.both"]);
}

const RATIO_SOURCE_ORDER: &str = r#"module m.o

pub fn ratio(n: Int, d: Int) -> Int
    ensures result <= n / d
    requires d != 0
    requires n % d >= 0
{
    n / d
}
"#;

const RATIO_CANONICAL_ORDER: &str = r#"module m.o

pub fn ratio(n: Int, d: Int) -> Int
    requires d != 0
    requires n % d >= 0
    ensures result <= n / d
{
    n / d
}
"#;

/// Clauses may come in any order (grammar 3.9, D66), but `contract` lists
/// "canonical clause texts in canonical order" (6.4: requires, then
/// ensures, then decreases, 5.2), and obligation ids are "stable across
/// runs for an unchanged definition" (8.1): reordering the clauses changes
/// no facet, so it changes no obligation id, kind, or clause either; the
/// divisions are numbered over the clauses in canonical order, then the
/// body ("body order, clauses included").
#[test]
fn clause_order_changes_neither_the_contract_list_nor_the_obligation_ids() {
    let a = Checked::clean(&[("src/m/o.cpl", RATIO_SOURCE_ORDER)]);
    let b = Checked::clean(&[("src/m/o.cpl", RATIO_CANONICAL_ORDER)]);
    let (ia, ib) = (a.index(), b.index());
    let contract = |index: &Index<'_>| {
        inspect(index, "ratio", &inc("contract"))
            .symbol
            .contract
            .expect("included")
    };
    let expected = [
        "requires d != 0".to_string(),
        "requires n % d >= 0".to_string(),
        "ensures result <= n / d".to_string(),
    ];
    assert_eq!(contract(&ia), expected);
    assert_eq!(contract(&ib), expected);
    assert_eq!(
        ia.version(def(&ia, "ratio")),
        ib.version(def(&ib, "ratio")),
        "the same facets"
    );
    let summary = |index: &Index<'_>| -> Vec<(String, String, String)> {
        index
            .obligations(def(index, "ratio"))
            .into_iter()
            .map(|o| (o.id, o.kind, o.clause))
            .collect()
    };
    let ids = summary(&ia);
    assert_eq!(ids, summary(&ib));
    assert_eq!(
        ids,
        [
            (
                "m.o.ratio#ensures/1".to_string(),
                "ensures".to_string(),
                "result <= n / d".to_string()
            ),
            (
                "m.o.ratio#div/1".to_string(),
                "division".to_string(),
                "d != 0".to_string()
            ),
            (
                "m.o.ratio#div/2".to_string(),
                "division".to_string(),
                "d != 0".to_string()
            ),
            (
                "m.o.ratio#div/3".to_string(),
                "division".to_string(),
                "d != 0".to_string()
            ),
        ]
    );
    // div/1 is the `%` of the second `requires`, wherever it is written.
    let file = "src/m/o.cpl";
    let at = |index: &Index<'_>| -> Vec<Location> {
        index
            .obligations(def(index, "ratio"))
            .into_iter()
            .map(|o| o.location)
            .collect()
    };
    assert_eq!(
        at(&ia),
        [
            loc(file, 4, 5, 4, 28),
            loc(file, 6, 14, 6, 19),
            loc(file, 4, 23, 4, 28),
            loc(file, 8, 5, 8, 10),
        ]
    );
}

const DECREASING: &str = r#"module m.dec

pub fn measure(n: Int) -> Int {
    n
}

pub fn down(n: Int) -> Int
    requires n >= 0
    decreases measure(n)
{
    if n == 0 {
        0
    } else {
        down(n - 1)
    }
}
"#;

/// A function's `decreases` clause is part of its contract (T23), so the
/// calls in its measure are interface dependencies, and the measure is the
/// `decreases` obligation's clause (8.1).
#[test]
fn a_decreases_measure_is_an_interface_dependency_source() {
    let checked = Checked::clean(&[("src/m/dec.cpl", DECREASING)]);
    let index = checked.index();
    assert_eq!(
        deps(&index, "m.dec.down"),
        [
            dep("m.dec.down", RefKind::Call, false),
            dep("m.dec.measure", RefKind::Call, true),
            dep("std.prelude.Int", RefKind::TypeUse, true),
        ]
    );
    assert_eq!(
        inspect(&index, "down", &inc("contract"))
            .symbol
            .contract
            .as_deref(),
        Some(
            &[
                "requires n >= 0".to_string(),
                "decreases measure(n)".to_string()
            ][..]
        )
    );
    let obligations = index.obligations(def(&index, "m.dec.down"));
    let decreases = obligations
        .iter()
        .find(|o| o.kind == "decreases")
        .expect("a decreases obligation");
    assert_eq!(decreases.id, "m.dec.down#decreases");
    assert_eq!(decreases.clause, "measure(n)");
    assert!(obligations.iter().all(|o| o.kind != "termination"));
}

const SCAN: &str = r#"module m.hr

pub fn scan(xs: List<Int>, limit: Int) -> Int
    requires limit >= 0
    ensures result >= 0
{
    var total = 0
    var i = 0
    while i < xs.len
        invariant i >= 0
    {
        let seen = i
        var j = 0
        while j < 1
            invariant j >= 0
        {
            j = j + 1
            let _x: Int = ?inner
        }
        i = seen + 1
    }
    total
}
"#;

/// The hole report as `cpl check` and `cpl inspect` write it (7.2,
/// Appendix C `HoleReportJson`): keys in declaration order; bindings
/// innermost scope first and most recent first, `mutable` written only
/// when `true`; the enclosing function's clauses as one-line texts; the
/// invariants of the enclosing loops innermost first (semantics S48).
#[test]
fn a_hole_report_is_written_in_the_shape_of_s48() {
    let checked = Checked::clean(&[("src/m/hr.cpl", SCAN)]);
    let index = checked.index();
    assert_eq!(checked.check.holes.len(), 1);
    let report = index.hole_report_json(&checked.check.holes[0]);
    assert_eq!(
        keys_of(&report),
        [
            "hole",
            "expected_type",
            "bindings",
            "capabilities",
            "requires",
            "ensures",
            "invariants",
            "dependencies"
        ]
    );
    let json = to_json(&report);
    assert_eq!(json["hole"], json!("inner"));
    assert_eq!(json["expected_type"], json!("Int"));
    assert_eq!(
        json["bindings"],
        json!([
            { "name": "j", "type": "Int", "kind": "var", "mutable": true },
            { "name": "seen", "type": "Int", "kind": "let" },
            { "name": "i", "type": "Int", "kind": "var", "mutable": true },
            { "name": "total", "type": "Int", "kind": "var", "mutable": true },
            { "name": "limit", "type": "Int", "kind": "parameter" },
            { "name": "xs", "type": "List<Int>", "kind": "parameter" },
        ])
    );
    assert_eq!(json["capabilities"], json!([]));
    assert_eq!(json["requires"], json!(["limit >= 0"]));
    assert_eq!(json["ensures"], json!(["result >= 0"]));
    assert_eq!(json["invariants"], json!(["j >= 0", "i >= 0"]));
    let out = inspect(&index, "scan", &inc("implementation"));
    assert_eq!(out.symbol.holes.as_deref(), Some(&[report][..]));
}

/// A property's body hole: its bindings are the `for` binders (kind `for`,
/// most recent first, typed by their generators: `gen.positive_int` gives
/// the refinement `Positive`, stdlib 8.1), it has no clauses (S48: "empty
/// arrays for tests"), and its `symbols` include what the `for` clauses
/// call.
#[test]
fn a_property_body_hole_reports_its_binders() {
    let checked = Checked::clean(&[(
        "src/m/ph.cpl",
        "module m.ph\n\nproperty \"pending property\"\n    for n in gen.positive_int\n    for xs in gen.list(gen.int)\n{\n    let total: Int = ?sum\n    assert total >= 0\n}\n",
    )]);
    let index = checked.index();
    assert_eq!(checked.check.holes.len(), 1);
    let json = to_json(&index.hole_report_json(&checked.check.holes[0]));
    assert_eq!(json["expected_type"], json!("Int"));
    assert_eq!(
        json["bindings"],
        json!([
            { "name": "xs", "type": "List<Int>", "kind": "for" },
            { "name": "n", "type": "Positive", "kind": "for" },
        ])
    );
    assert_eq!(json["requires"], json!([]));
    assert_eq!(json["ensures"], json!([]));
    assert_eq!(json["invariants"], json!([]));
    let symbols: Vec<&str> = json["dependencies"]["symbols"]
        .as_array()
        .expect("symbols")
        .iter()
        .map(|s| s["path"].as_str().expect("path"))
        .collect();
    assert_eq!(
        symbols,
        [
            "std.gen.int",
            "std.gen.list",
            "std.gen.positive_int",
            "std.prelude.Int"
        ]
    );
}

/// `cpl check` completes each `hole` diagnostic with the dependencies of
/// its own report, matched by span, not by name: two `?todo` holes in two
/// functions keep their own `symbols` (7.2).
#[test]
fn holes_of_one_name_keep_their_own_dependencies() {
    let checked = Checked::clean(&[(
        "src/m/two.cpl",
        "module m.two\n\npub struct A {\n    v: Int\n}\n\npub struct B {\n    w: Int\n}\n\npub fn fa(a: A) -> Int {\n    ?todo\n}\n\npub fn fb(b: B) -> Int {\n    ?todo\n}\n",
    )]);
    let index = checked.index();
    let completed = index.complete_check_diagnostics(&checked.check.diagnostics, false);
    let symbols_of = |qualified: &str| -> Vec<String> {
        let d = completed
            .iter()
            .find(|d| d.code == "hole" && d.qualified_name.as_deref() == Some(qualified))
            .unwrap_or_else(|| panic!("no hole in {qualified}"));
        d.data["dependencies"]["symbols"]
            .as_array()
            .expect("symbols")
            .iter()
            .map(|s| s["path"].as_str().expect("path").to_string())
            .collect()
    };
    assert_eq!(symbols_of("m.two.fa"), ["m.two.A", "std.prelude.Int"]);
    assert_eq!(symbols_of("m.two.fb"), ["m.two.B", "std.prelude.Int"]);
}

/// Every `verification_result` gets the receipt of its own function (S59),
/// matched by its `symbol`: two `@prove` functions with different call
/// closures have different `modules`; a provisional identity matches too.
#[test]
fn each_verification_result_gets_the_receipt_of_its_own_function() {
    let checked = Checked::clean(&[
        (
            "src/p/a.cpl",
            "module p.a\n\nuse p.b.{fb}\n\n@prove\npub fn fa(n: Int) -> Int\n    ensures result >= n\n{\n    fb(n)\n}\n",
        ),
        (
            "src/p/b.cpl",
            "module p.b\n\n@prove\npub fn fb(n: Int) -> Int\n    ensures result >= n\n{\n    n\n}\n",
        ),
    ]);
    let index = checked.index();
    let completed = index.complete_check_diagnostics(&checked.check.diagnostics, false);
    let modules_of = |qualified: &str| -> Vec<String> {
        let d = completed
            .iter()
            .find(|d| {
                d.code == "verification_result" && d.qualified_name.as_deref() == Some(qualified)
            })
            .unwrap_or_else(|| panic!("no verification_result for {qualified}"));
        assert_eq!(d.symbol.as_ref().map(StableId::as_str), Some(qualified));
        d.data["receipt"]["modules"]
            .as_array()
            .expect("modules")
            .iter()
            .map(|m| m["module"].as_str().expect("module").to_string())
            .collect()
    };
    assert_eq!(modules_of("p.a.fa"), ["p.a", "p.b"]);
    assert_eq!(modules_of("p.b.fb"), ["p.b"]);
    // CheckOutput::verification names each by its (provisional) identity.
    let output = index.check_output(&checked.check, false);
    assert_eq!(
        to_json(&output.verification),
        json!([
            { "symbol": "p.a.fa", "qualified_name": "p.a.fa", "status": "unknown", "reason": "verifier_unavailable" },
            { "symbol": "p.b.fb", "qualified_name": "p.b.fb", "status": "unknown", "reason": "verifier_unavailable" },
        ])
    );
}

/// `ProveOutput` (7.2): `symbol`, `result`, `obligations`, `receipt` in that
/// order, and `symbol` "is the target's `SymbolVersion`, not a symbol
/// block": its five keys and nothing else.
#[test]
fn prove_output_symbol_is_a_version_not_a_block() {
    let checked = Checked::clean(&[("src/m/a.cpl", CONTRACTS)]);
    let index = checked.index();
    let proof = index.prove(&Target::parse("step")).expect("a function");
    assert_eq!(
        keys_of(&proof.output),
        ["symbol", "result", "obligations", "receipt"]
    );
    assert_eq!(
        keys_of(&proof.output.symbol),
        ["id", "path", "provisional", "facets", "definition"]
    );
    assert_eq!(to_json(&proof.output)["result"], json!("unknown"));
    let ensures: Vec<&str> = proof
        .output
        .obligations
        .iter()
        .filter(|o| o.kind == "ensures")
        .map(|o| o.id.as_str())
        .collect();
    assert_eq!(ensures, ["m.a.step#ensures/1"]);
}

/// `CheckOutput` (7.2): keys `release`, `summary`, `holes`, `verification`
/// in that order; the summary counts the project's definitions, modules,
/// and files (std left out) and the check's holes.
#[test]
fn check_output_counts_project_definitions_modules_and_files() {
    let checked = multi_file();
    let index = checked.index();
    let output = index.check_output(&checked.check, false);
    assert_eq!(
        keys_of(&output),
        ["release", "summary", "holes", "verification"]
    );
    assert_eq!(
        keys_of(&output.summary),
        [
            "errors",
            "warnings",
            "holes",
            "definitions",
            "modules",
            "files"
        ]
    );
    assert_eq!(
        to_json(&output.summary),
        json!({ "errors": 0, "warnings": 0, "holes": 1, "definitions": 4, "modules": 2, "files": 3 })
    );
    assert_eq!(output.holes.len(), 1);
    assert_eq!(output.holes[0].hole, "todo");
    assert!(output.verification.is_empty());
}

/// The static call closure of a selection follows calls only (S59, 8.2):
/// a test that only builds a struct of another module neither lists that
/// module in its receipt nor the struct in `tested`; a run stopped by
/// `--fail-fast` still covers every selected test in `core` and `tested`
/// (8: "the union over the selected tests"), and is `failed` because not
/// every selected test is `tested`.
#[test]
fn receipts_follow_calls_and_the_whole_selection() {
    let checked = Checked::clean(&[
        (
            "src/x/x.cpl",
            "module x.x\n\npub struct Box {\n    v: Int\n}\n\npub fn fx(n: Int) -> Int {\n    n + 1\n}\n",
        ),
        ("src/y/y.cpl", TESTS_Y),
        (
            "src/tests/t.cpl",
            "module tests.t\n\nuse x.x.{Box, fx}\nuse y.y.{fy}\n\ntest \"box\" {\n    let b = Box { v: 1 }\n    assert b.v == 1\n}\n\ntest \"fx works\" {\n    assert fx(1) == 2\n}\n\ntest \"fy works\" {\n    assert fy(2) == 4\n}\n",
        ),
    ]);
    let index = checked.index();
    let names = [
        "tests.t.\"box\"",
        "tests.t.\"fx works\"",
        "tests.t.\"fy works\"",
    ];
    let selected: Vec<DefId> = names.iter().map(|n| def(&index, n)).collect();
    let box_core = index.test_receipt_core(&selected[..1], json!({}));
    assert_eq!(
        box_core
            .modules
            .iter()
            .map(|m| m.module.as_str())
            .collect::<Vec<_>>(),
        ["tests.t"]
    );
    // Only the first record ran: --fail-fast stopped after it (no failure
    // is needed for the selection to stay the receipt's subject).
    let mut report = report_of(vec![record(names[0], TestKind::Test)]);
    report.selected = 3;
    let mut config = run_config();
    config.fail_fast = true;
    let receipt = index.complete_test_report(&mut report, &selected, &config, 10_000);
    assert_eq!(
        receipt
            .core
            .modules
            .iter()
            .map(|m| m.module.as_str())
            .collect::<Vec<_>>(),
        ["tests.t", "x.x", "y.y"]
    );
    let tested: Vec<&str> = receipt.tested.iter().map(|v| v.path.as_str()).collect();
    assert!(tested.contains(&"x.x.fx"), "{tested:?}");
    assert!(tested.contains(&"y.y.fy"), "{tested:?}");
    assert!(!tested.contains(&"x.x.Box"), "{tested:?}");
    assert_eq!(receipt.status, TestStatus::Failed);
    assert_eq!(receipt.tests.len(), 1);
    assert_eq!(receipt.configuration, config);
}

/// A std stub has no body (5.1), so every reference it makes is in its
/// signature or its contract: all its dependencies are interface ones.
#[test]
fn a_bodiless_std_function_has_only_interface_dependencies() {
    let checked = Checked::example("reader");
    let index = checked.index();
    for path in ["std.prelude.List.take", "std.text.trim", "std.gen.range"] {
        let d = def(&index, path);
        let all = index.dependencies(d);
        assert!(!all.is_empty(), "{path}");
        assert_eq!(index.interface_dependencies(d), all, "{path}");
    }
}

/// A move of an annotated declaration to another module is `moved` about
/// its identity (9.1), which `changes` reports for it (6.2).
#[test]
fn changes_report_a_move_under_the_kept_identity() {
    use cpl_diag::json::DiffStatementKind as K;
    let base = changes_base_of(&[(
        "src/m/c.cpl",
        "module m.c\n\n@id(\"fn_bbbbbbbbbbbbbbbb\")\npub fn wanderer() -> Int {\n    1\n}\n",
    )]);
    let checked = Checked::clean(&[(
        "src/m/d.cpl",
        "module m.d\n\n@id(\"fn_bbbbbbbbbbbbbbbb\")\npub fn wanderer() -> Int {\n    1\n}\n",
    )]);
    let index = checked.index().with_changes_base(base);
    let out = inspect(&index, "fn_bbbbbbbbbbbbbbbb", &inc("changes"));
    let statements: Vec<(K, Option<Facet>)> = out
        .changes
        .expect("included")
        .statements
        .iter()
        .map(|s| (s.statement, s.facet))
        .collect();
    assert_eq!(statements, [(K::Moved, None)]);
}

/// `relation` in a dependency list is the `RefKind` of the first reference
/// (6.4, 6.5), spelled `call`, `type_use`, `ctor`, `field_access`: a field
/// read on a call's result and a constructor are the first references of
/// their types there.
#[test]
fn relations_spell_the_kind_of_the_first_reference() {
    let checked = referencing();
    let index = checked.index();
    assert_eq!(
        deps(&index, "lib.uses.count_made"),
        [
            dep("lib.core.Item", RefKind::FieldAccess, false),
            dep("lib.uses.make", RefKind::Call, false),
            dep("std.prelude.Int", RefKind::TypeUse, true),
        ]
    );
    assert_eq!(
        deps(&index, "lib.uses.circle_area"),
        [
            dep("lib.core.Shape", RefKind::Ctor, false),
            dep("lib.uses.area", RefKind::Call, false),
            dep("std.prelude.Int", RefKind::TypeUse, true),
        ]
    );
    let relations = |arg: &str| -> Vec<Value> {
        to_json(&inspect(&index, arg, &inc("dependencies")))["dependencies"]
            .as_array()
            .expect("dependencies")
            .iter()
            .map(|b| b["relation"].clone())
            .collect()
    };
    assert_eq!(
        relations("lib.uses.count_made"),
        [json!("field_access"), json!("call"), json!("type_use")]
    );
    assert_eq!(
        relations("lib.uses.circle_area"),
        [json!("ctor"), json!("call"), json!("type_use")]
    );
}

/// A malformed `@id` identifies nothing (4.3: the declaration has its
/// provisional identity, and the checker reports
/// `invalid_annotation_argument`); a generated-format id of another kind is
/// still the identity (T21: the prefix is informative, a warning only).
#[test]
fn malformed_and_mismatched_ids_as_targets() {
    let bad = Checked::files(&[(
        "src/m/bad.cpl",
        "module m.bad\n\n@id(\"Bad\")\npub fn f() -> Int {\n    1\n}\n",
    )]);
    assert!(bad
        .check
        .diagnostics
        .iter()
        .any(|d| d.code == "invalid_annotation_argument"));
    let index = bad.index();
    let version = index.version(def(&index, "m.bad.f"));
    assert_eq!(version.id.as_str(), "m.bad.f");
    assert!(version.provisional);
    assert_unknown(&resolve_error(&index, "Bad"));
    assert!(index.partial().is_some(), "the project has an error");

    let mismatched = Checked::files(&[(
        "src/m/mis.cpl",
        "module m.mis\n\n@id(\"ty_aaaaaaaaaaaaaaaa\")\npub fn f() -> Int {\n    1\n}\n",
    )]);
    assert!(mismatched
        .check
        .diagnostics
        .iter()
        .any(|d| d.code == "id_kind_mismatch" && d.severity == Severity::Warning));
    assert!(!mismatched.check.has_errors());
    let index = mismatched.index();
    assert_eq!(path_of(&index, "ty_aaaaaaaaaaaaaaaa"), "m.mis.f");
    let out = inspect(&index, "ty_aaaaaaaaaaaaaaaa", &inc("signature"));
    assert_eq!(out.symbol.version.id.as_str(), "ty_aaaaaaaaaaaaaaaa");
    assert!(!out.symbol.version.provisional);
    assert_eq!(out.symbol.kind, "fn", "the declaration decides the kind");
    assert_eq!(index.partial(), None, "a warning leaves the index complete");
}

/// Semantics S36 (b), which owns the status vocabulary and takes
/// precedence over toolchain 8.1's status table (spec/README.md): in
/// Milestone 1 `cpl prove` reports `unknown` (`verifier_unavailable`), also
/// for a function with no obligations and for one whose only obligation
/// the type checker proved, and so exits 1 (T29).
#[test]
fn milestone_one_proofs_are_unknown_even_with_nothing_open() {
    let checked = Checked::clean(&[(
        "src/m/l.cpl",
        "module m.l\n\npub type Small = Int where self < 10\n\npub fn one() -> Int {\n    1\n}\n\npub fn three() -> Int {\n    let n: Small = 3\n    n\n}\n",
    )]);
    let index = checked.index();
    for arg in ["m.l.one", "m.l.three"] {
        let proof = index.prove(&Target::parse(arg)).expect("a function");
        assert_eq!(
            proof.output.result,
            cpl_diag::json::ProofStatus::Unknown,
            "{arg}"
        );
        assert_eq!(
            proof.output.receipt.status,
            cpl_diag::json::ProofStatus::Unknown
        );
        assert_eq!(proof.status(), cpl_diag::json::Status::Unsuccessful);
        assert_eq!(proof.diagnostic.data["status"], json!("unknown"));
        assert_eq!(
            proof.diagnostic.data["reason"],
            json!("verifier_unavailable")
        );
        assert_eq!(
            proof.diagnostic.facet,
            Some(Facet::VerificationPolicy),
            "{arg}"
        );
    }
    let one = index.prove(&Target::parse("m.l.one")).expect("a function");
    assert!(one.output.obligations.is_empty());
    let three = index
        .prove(&Target::parse("m.l.three"))
        .expect("a function");
    assert!(three
        .output
        .obligations
        .iter()
        .all(|o| o.status == ObligationStatus::Proved));
}

/// A bodiless std function has all eight facets with an empty `body` text
/// (5.1), whose hash is the empty-string vector of 5.3; a std opaque type
/// has the four facets of a wrapper.
#[test]
fn std_stub_versions_hash_an_empty_body() {
    let checked = Checked::example("reader");
    let index = checked.index();
    let trim = index.version(def(&index, "std.text.trim"));
    assert_eq!(trim.facets.len(), 8);
    assert_eq!(
        trim.facets[&Facet::Body].as_str(),
        "af1349b9f5f9a1a6a0404dea36dcc949"
    );
    let list = index.version(def(&index, "std.prelude.List"));
    assert_eq!(
        list.facets.keys().copied().collect::<Vec<_>>(),
        [Facet::Identity, Facet::Name, Facet::Signature, Facet::Docs]
    );
}

/// The formulas of 6.5 over every definition of both corpus projects, std
/// included: `x` is a dependency of `d` exactly when `d` is a caller of
/// `x`; the interface dependencies are the dependencies flagged
/// `interface`, in the same order and with the same relations; and `t` is
/// in `tests_for(d)` exactly when `t` is a test or property whose call
/// closure reaches `d` (`call_closure`, without `t` itself).
#[test]
fn the_formulas_of_6_5_agree_over_every_definition() {
    for name in ["reader", "holes"] {
        let checked = Checked::example(name);
        let index = checked.index();
        let program = index.program();
        let all: Vec<DefId> = program.defs().map(|d| d.id).collect();
        let tests: Vec<DefId> = program
            .defs()
            .filter(|d| d.kind.is_test())
            .map(|d| d.id)
            .collect();
        let closures: Vec<(DefId, std::collections::BTreeSet<DefId>)> = tests
            .iter()
            .map(|&t| (t, index.call_closure(&[t])))
            .collect();
        for &d in &all {
            let path = &index.version(d).path;
            let dependencies = index.dependencies(d);
            for dep in &dependencies {
                assert!(
                    index.callers(dep.def).contains(&d),
                    "{name}: {path} depends on {} but is not its caller",
                    index.version(dep.def).path
                );
            }
            for caller in index.callers(d) {
                assert!(
                    index.dependencies(caller).iter().any(|x| x.def == d),
                    "{name}: {} calls {path} without depending on it",
                    index.version(caller).path
                );
            }
            let flagged: Vec<_> = dependencies
                .iter()
                .copied()
                .filter(|x| x.interface)
                .collect();
            assert_eq!(index.interface_dependencies(d), flagged, "{name}: {path}");
            let expected: Vec<DefId> = closures
                .iter()
                .filter(|(t, closure)| *t != d && closure.contains(&d))
                .map(|(t, _)| *t)
                .collect();
            let mut found = index.tests_for(d);
            found.sort();
            let mut expected = expected;
            expected.sort();
            assert_eq!(found, expected, "{name}: tests of {path}");
        }
    }
}

/// Every list of a response is "sorted by display path" (6.5): callers,
/// dependencies, interface dependencies, and tests, for every project
/// definition of the corpus, in byte order and without repetition.
#[test]
fn every_list_of_every_response_is_sorted_by_display_path() {
    for name in ["reader", "holes"] {
        let checked = Checked::example(name);
        let index = checked.index();
        let program = index.program();
        for d in program.defs() {
            if program.module(d.module).origin == cpl_diag::FileOrigin::Std {
                continue;
            }
            let path = index.version(d.id).path.clone();
            let out = inspect(&index, &path, &everything());
            for (list, blocks) in [
                ("callers", out.callers.as_deref()),
                ("dependencies", out.dependencies.as_deref()),
                (
                    "interface_dependencies",
                    out.interface_dependencies.as_deref(),
                ),
                ("tests", out.tests.as_deref()),
            ] {
                let paths = block_paths(blocks.expect("included"));
                let mut sorted = paths.clone();
                sorted.sort();
                sorted.dedup();
                assert_eq!(paths, sorted, "{name}: {list} of {path}");
            }
        }
    }
}

/// `changes` keeps the diff's statement order for the target: by path,
/// then the table order of 9.1 (signature before body before docs); a
/// renamed provisional declaration is a new identity, so only its
/// `identity_added` is about it.
#[test]
fn changes_keep_the_table_order_of_statements() {
    use cpl_diag::json::DiffStatementKind as K;
    let base = changes_base_of(&[(
        "src/m/c.cpl",
        "module m.c\n\npub fn f(n: Int) -> Int {\n    n\n}\n\npub fn before() -> Int {\n    1\n}\n",
    )]);
    let checked = Checked::clean(&[(
        "src/m/c.cpl",
        "module m.c\n\n/// Now documented.\npub fn f(n: Int, m: Int) -> Int {\n    n + m\n}\n\npub fn after() -> Int {\n    1\n}\n",
    )]);
    let index = checked.index().with_changes_base(base);
    let kinds = |arg: &str| -> Vec<K> {
        inspect(&index, arg, &inc("changes"))
            .changes
            .expect("included")
            .statements
            .iter()
            .map(|s| s.statement)
            .collect()
    };
    assert_eq!(
        kinds("m.c.f"),
        [K::SignatureChanged, K::BodyChanged, K::DocsChanged]
    );
    assert_eq!(kinds("m.c.after"), [K::IdentityAdded]);
}

/// The `holes` of an implementation are the holes inside that body, in
/// source order (6.4; the order every other list of the response follows
/// for one declaration), each once.
#[test]
fn the_holes_of_a_body_come_in_source_order() {
    let checked = Checked::clean(&[(
        "src/m/hh.cpl",
        "module m.hh\n\npub fn two() -> Int {\n    let x: Int = ?first\n    let y: Int = ?second\n    x + y\n}\n\npub fn other() -> Int {\n    ?third\n}\n",
    )]);
    let index = checked.index();
    let names = |arg: &str| -> Vec<String> {
        inspect(&index, arg, &inc("implementation"))
            .symbol
            .holes
            .expect("included")
            .iter()
            .map(|h| h.hole.clone())
            .collect()
    };
    assert_eq!(names("two"), ["first", "second"]);
    assert_eq!(names("other"), ["third"]);
}

/// Std functions are provable targets like any function (6.1, 7.2): their
/// contracts are trusted-runtime axioms (stdlib S41), so there is no
/// obligation to discharge, the status is still `unknown` in Milestone 1,
/// no project module is in the closure, and the std declarations the
/// specification uses are `dependencies` with their versions (S41).
#[test]
fn a_std_function_is_provable_with_axioms_only() {
    let checked = Checked::example("reader");
    let index = checked.index();
    let proof = index
        .prove(&Target::parse("std.text.trim"))
        .expect("a function");
    assert_eq!(proof.output.result, cpl_diag::json::ProofStatus::Unknown);
    assert!(proof.output.obligations.is_empty());
    let receipt = &proof.output.receipt;
    assert_eq!(receipt.target.id.as_str(), "std.text.trim");
    assert!(receipt.target.provisional);
    assert!(
        receipt.core.modules.is_empty(),
        "{:?}",
        receipt.core.modules
    );
    assert!(receipt
        .dependencies
        .iter()
        .any(|v| v.path == "std.prelude.Text"));
    for v in &receipt.dependencies {
        assert_eq!(*v, *index.version(def(&index, &v.path)), "{}", v.path);
    }
    assert_eq!(
        proof
            .diagnostic
            .span
            .map(|s| checked.project.source_map.location(s)),
        Some(loc("std/text.cpl", 6, 8, 6, 12)),
        "the function's name token (semantics S36)"
    );
}

/// Semantics S69: a hole whose expected type is or contains the error type
/// is still reported once, its `expected_type` rendering the error type as
/// `?`, as does every rendering of a type in the report (a binding of an
/// unresolved type); the index answers with `index_partial` (7.2).
#[test]
fn erroneous_types_render_as_question_marks_in_hole_reports() {
    let checked = Checked::files(&[(
        "src/m/err.cpl",
        "module m.err\n\npub fn takes(x: Nope) -> Int {\n    1\n}\n\npub fn caller() -> Int {\n    takes(?h)\n}\n\npub fn holder(y: Nope) -> Int {\n    ?g\n}\n",
    )]);
    assert!(checked.check.has_errors());
    let index = checked.index();
    assert!(index.partial().is_some());
    let report = |name: &str| {
        let r = checked
            .check
            .holes
            .iter()
            .find(|h| h.name == name)
            .unwrap_or_else(|| panic!("?{name} is reported (S69)"));
        to_json(&index.hole_report_json(r))
    };
    assert_eq!(report("h")["expected_type"], json!("?"));
    let g = report("g");
    assert_eq!(g["expected_type"], json!("Int"));
    assert_eq!(
        g["bindings"],
        json!([{ "name": "y", "type": "?", "kind": "parameter" }])
    );
    let out = inspect(&index, "m.err.caller", &inc("implementation"));
    let holes = out.symbol.holes.expect("included");
    assert_eq!(holes.len(), 1);
    assert_eq!(holes[0].expected_type, "?");
}

/// 6.1's rows "resolve identically": the response is a function of the
/// resolved definition and the include set alone, whichever form named
/// the target (stable id, provisional id, qualified path, bare name).
#[test]
fn every_target_form_of_one_definition_gives_one_response() {
    let checked = Checked::example("reader");
    let index = checked.index();
    let forms = [
        Target::parse("pgn_7c2d"),
        Target::Id(StableId::parse("reader.pages.paginate").expect("an id")),
        Target::parse("reader.pages.paginate"),
        Target::parse("paginate"),
    ];
    for include in [everything(), IncludeSet::default_set()] {
        let responses: Vec<String> = forms
            .iter()
            .map(|t| {
                serde_json::to_string(&index.inspect(t, &include).expect("resolves"))
                    .expect("serializes")
            })
            .collect();
        assert!(
            responses.windows(2).all(|w| w[0] == w[1]),
            "the forms answer differently"
        );
    }
    let proofs: Vec<_> = forms
        .iter()
        .map(|t| {
            let mut p = index.prove(t).expect("a function");
            p.output.receipt.produced_at.clear();
            p
        })
        .collect();
    assert!(proofs.windows(2).all(|w| w[0] == w[1]));
}

/// A test name may contain `.`: its qualified path quotes it (4.3, T22),
/// and so must a bare name; unquoted, the argument is a qualified path of
/// another module and names nothing (6.1 row 3).
#[test]
fn test_names_with_dots_need_their_quotes() {
    let checked = Checked::clean(&[(
        "src/t/d.cpl",
        "module t.d\n\ntest \"a.b\" {\n    assert true\n}\n",
    )]);
    let index = checked.index();
    assert_eq!(path_of(&index, r#"t.d."a.b""#), r#"t.d."a.b""#);
    assert_eq!(path_of(&index, r#""a.b""#), r#"t.d."a.b""#);
    assert_unknown(&resolve_error(&index, "a.b"));
    assert_unknown(&resolve_error(&index, "t.d.a.b"));
}

/// Obligations are those "of a function `f`" (8.1): a test's or a
/// property's `assert` statements are no obligations, so `obligations` is
/// an empty list for them, as for every other kind.
#[test]
fn tests_and_properties_have_no_obligations() {
    let checked = Checked::clean(&[("src/m/kinds.cpl", KINDS)]);
    let index = checked.index();
    for arg in [
        "\"origin is zero\"",
        "\"positive ints are positive\"",
        "m.kinds.Point",
        "m.kinds.Pos",
        "m.kinds.Clock",
    ] {
        assert_eq!(
            inspect(&index, arg, &inc("obligations"))
                .obligations
                .as_deref(),
            Some(&[][..]),
            "{arg}"
        );
    }
}

/// `cpl-cli` "fills each record's `receipt` (S59)" (7.2): a record that
/// arrives with a stale receipt leaves with the one of its own test.
#[test]
fn test_report_completion_replaces_stale_record_receipts() {
    let checked = Checked::clean(&[
        ("src/x/x.cpl", TESTS_X),
        ("src/y/y.cpl", TESTS_Y),
        ("src/tests/t.cpl", TESTS_T),
    ]);
    let index = checked.index();
    let name = "tests.t.\"fx works\"";
    let test = def(&index, name);
    let configuration = cpl_index::test_check_configuration(&run_config(), 10_000);
    let mut stale = record(name, TestKind::Test);
    let mut wrong = index.test_receipt_core(&[def(&index, "tests.t.\"fy works\"")], json!({}));
    wrong.source_revision = "f".repeat(40);
    stale.receipt = Some(wrong);
    let mut report = report_of(vec![stale]);
    index.complete_test_report(&mut report, &[test], &run_config(), 10_000);
    assert_eq!(
        report.results[0].receipt,
        Some(index.test_receipt_core(&[test], configuration))
    );
}

/// A generator in a `for` clause is "an ordinary call" (3.5: `gen.list(g)`
/// is an ordinary call of the `@builtin`, `gen.int` a `@property` member),
/// so its call site is the whole call expression, as for every other call
/// (6.4's `paginate(items, checked_size)`; `xs.len` for a property member),
/// and a call nested in its arguments is a site of its own callee.
#[test]
fn generator_calls_in_for_clauses_are_whole_call_sites() {
    let local = Checked::clean(&[("src/m/p.cpl", PROPERTY)]);
    let reader = Checked::example("reader");
    let squares = "m.p.\"squares are nonnegative\"";
    let preserves = "tests.pagination.\"pagination preserves input\"";
    let (p, t) = ("src/m/p.cpl", "src/tests/pagination.cpl");
    // `    for n in gen.range(0, limit())`, and in the reader
    // `    for items in gen.list(gen.int)` and `    for size in gen.positive_int`.
    let cases = [
        (&local, squares, "std.gen.range", loc(p, 12, 14, 12, 35)),
        (&local, squares, "m.p.limit", loc(p, 12, 27, 12, 34)),
        (&reader, preserves, "std.gen.list", loc(t, 17, 18, 17, 35)),
        (&reader, preserves, "std.gen.int", loc(t, 17, 27, 17, 34)),
        (
            &reader,
            preserves,
            "std.gen.positive_int",
            loc(t, 18, 17, 18, 33),
        ),
    ];
    let mut wrong = Vec::new();
    for (checked, caller, callee, expected) in cases {
        let index = checked.index();
        let out = inspect(&index, callee, &inc("callers"));
        let sites = block(out.callers.as_ref().expect("callers"), caller)
            .call_sites
            .clone()
            .expect("call sites");
        if sites != [expected.clone()] {
            wrong.push(format!("{callee}: expected {expected:?}, found {sites:?}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
