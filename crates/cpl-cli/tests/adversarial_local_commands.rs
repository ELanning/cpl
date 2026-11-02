//! Adversarial tests of the local `cpl` commands (`check`, `fmt`, `run`,
//! `test`, `id assign`) against `spec/toolchain.md`: 7.1 (project
//! discovery, `--json` and `--jsonl` framing, exit codes, determinism), 7.2
//! (the commands and their flags), 7.3 (constant-expression and value-JSON
//! arguments, value text), 7.4 (exit codes), 1.4.1 (the stable human lines),
//! 1.4.2 and Appendix C (the wire types: key order, `null` versus omitted
//! keys), with the semantics rules they cite (S39, S40, S42, S44, S46, S49,
//! S63) and Sections 4.1-4.2 for `id assign`.
//!
//! Every expectation is the specification's answer, not the current
//! behaviour; where the specification is silent the assertion's comment says
//! which reading of `design-doc.md` it takes. None of these tests needs git.

mod common;

use common::*;
use ordered::J;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

// ===========================================================================
// Fixtures
// ===========================================================================

/// `examples/reader`'s source-set files other than `cpl.toml`, in path order.
const READER_FILES: [&str; 5] = [
    "src/reader/images.cpl",
    "src/reader/pages.cpl",
    "src/tests/pagination.cpl",
    "work/pagination.patch",
    "work/pagination.work",
];

/// The tests and properties of `examples/reader`, in run order.
const READER_TESTS: [&str; 3] = [
    "tests.pagination.\"pagination handles empty input\"",
    "tests.pagination.\"pagination preserves a short tail\"",
    "tests.pagination.\"pagination preserves input\"",
];

const CATEGORIES: [&str; 16] = [
    "parse",
    "resolve",
    "type",
    "effect",
    "contract",
    "hole",
    "test",
    "release_policy",
    "runtime",
    "internal",
    "project",
    "index",
    "git",
    "work",
    "patch",
    "usage",
];

/// `Facet::ALL`, the canonical order (1.4).
const FACETS: [&str; 8] = [
    "identity",
    "name",
    "signature",
    "contract",
    "effects",
    "body",
    "docs",
    "verification_policy",
];

/// The fixed keys of a diagnostic object, in `DiagnosticJson` order (1.4.2, T5).
const FIXED_KEYS: [&str; 10] = [
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
];

const ENVELOPE_KEYS: [&str; 5] = ["command", "toolchain", "revision", "status", "diagnostics"];
const LOCATION_KEYS: [&str; 5] = ["file", "line", "col", "end_line", "end_col"];

const TIDY: &str = "module m\n\nfn f(a: Int) -> Int {\n    a\n}\n";
const MESSY: &str = "module m\n\nfn f( a:Int )->Int{\n  a\n}\n";
const BROKEN_SYNTAX: &str = "module reader.broken\n\nfn f( {\n";

/// One module with a function for every value form of Section 7.3, the
/// refinements of semantics S39, a generic and an effectful function, and
/// an `@id`-named function.
const VALS: &str = r#"module vals.main

use std.foreign.{ForeignError, ForeignErrorKind}

pub type Positive = Int where self > 0
pub type Small = Positive where self < 10
pub type NonEmpty = Text where self.len > 0
pub type Tag(Text)
pub type PageId(Text)
pub type Wrapped(Positive)

pub struct Pair {
    left: Int
    right: Text
}

pub struct Page {
    id: PageId
    text: Text
}

pub struct Boxed {
    n: Positive
    label: Text
}

pub struct Tree {
    children: List<Tree>
}

pub enum Shape {
    Dot
    Circle(Positive)
    Both(Int, Text)
}

pub capability Clock {
    fn now() -> Int
}

pub fn int(n: Int) -> Int {
    n
}

pub fn add(a: Int, b: Int) -> Int {
    a + b
}

pub fn answer() -> Int {
    42
}

pub fn flag(b: Bool) -> Bool {
    b
}

pub fn text(t: Text) -> Text {
    t
}

pub fn unit(u: Unit) -> Unit {
    u
}

pub fn bytes(b: Bytes) -> Bytes {
    b
}

pub fn ints(xs: List<Int>) -> List<Int> {
    xs
}

pub fn texts(xs: List<List<Text>>) -> List<List<Text>> {
    xs
}

pub fn tag(t: Tag) -> Tag {
    t
}

pub fn pair(p: Pair) -> Pair {
    p
}

pub fn pages(ps: List<Page>) -> Int {
    ps.len
}

pub fn shape(s: Shape) -> Shape {
    s
}

pub fn nested(o: Option<Option<Int>>) -> Option<Option<Int>> {
    o
}

pub fn res(r: Result<Int, Text>) -> Result<Int, Text> {
    r
}

pub fn tree(t: Tree) -> Tree {
    t
}

pub fn ce(e: ConstraintError) -> ConstraintError {
    e
}

pub fn fe(e: ForeignError) -> ForeignErrorKind {
    e.kind
}

pub fn pos(n: Positive) -> Int {
    n
}

pub fn small(n: Small) -> Int {
    n
}

pub fn non_empty(t: NonEmpty) -> Int {
    t.len
}

pub fn two(a: Positive, b: Positive) -> Int {
    a + b
}

pub fn boxed(b: Boxed) -> Int {
    b.n
}

pub fn many(xs: List<Positive>) -> Int {
    xs.len
}

pub fn maybe(o: Option<Positive>) -> Bool {
    o.is_some()
}

pub fn wrapped(w: Wrapped) -> Wrapped {
    w
}

pub fn head<T>(xs: List<T>) -> Option<T> {
    xs.first()
}

pub fn head_int(xs: List<Int>) -> Option<Int> {
    head(xs)
}

pub fn read_clock(using clock: Clock) -> Int
    effects { clock.now }
{
    clock.now()
}

@id("fn_aaaaaaaaaaaaaaaa")
pub fn by_id(n: Int) -> Int {
    n + 1
}
"#;

/// Tests whose qualified names order differently from their identities.
const ORD_MAIN: &str = r#"module ord.main

test "a" {
    assert true
}

test "a!" {
    assert true
}

@id("test_zzzzzzzzzzzzzzzz")
test "a b" {
    assert true
}

property "p"
    for n in gen.int
{
    assert n == n
}

@id("prop_aaaaaaaaaaaaaaaa")
property "q"
    for n in gen.int
{
    assert n == n
}
"#;

const ORD_INNER: &str = "module ord.deep.inner\n\ntest \"inner\" {\n    assert 1 == 1\n}\n";
const ORD_OTHER: &str = "module ordx.other\n\ntest \"other\" {\n    assert 2 == 2\n}\n";

/// The run order of the `ord` project: the byte order of the qualified
/// names (1.7.2), so `"a b"` and `"a!"` run before `"a"`.
const ORD_ORDER: [&str; 7] = [
    "ord.deep.inner.\"inner\"",
    "ord.main.\"a b\"",
    "ord.main.\"a!\"",
    "ord.main.\"a\"",
    "ord.main.\"p\"",
    "ord.main.\"q\"",
    "ordx.other.\"other\"",
];

/// Test names that need escapes.
const ESC: &str = r#"module esc.main

test "say \"hi\"" {
    assert true
}

test "tab\there" {
    assert true
}

test "caf\u{e9}" {
    assert true
}
"#;

const ERR: &str = "module err.main\n\npub fn f() -> Int {\n    \"not an int\"\n}\n";

/// A project whose check reports one warning and nothing else.
const WARN: &str = r#"module warn.main

use std.text.{trim}

pub fn f() -> Int {
    1
}

test "f is one" {
    assert f() == 1
}
"#;

/// A hole in a test body (a warning under `--release`, 7.2).
const TH: &str = r#"module th.main

pub fn double(n: Int) -> Int {
    n * 2
}

test "unfinished" {
    let expected: Int = ?expected_value
    assert double(2) == expected
}
"#;

/// One definition with one error, one warning, and one hole.
const ONE: &str = r#"module one.main

fn f() -> Int {
    let x: Text = 1
    ?todo
}
"#;

/// Diagnostics in three files emitted out of path order, and span-less
/// manifest diagnostics whose keys are not in sorted order.
const SORT_MANIFEST: &str =
    "[project]\nname = \"sorted\"\nedition = \"0.1\"\nzeta = 1\nalpha = 2\n\n[test]\ncases = 5\nbogus = 3\n";
const SORT_A: &str = "module a\n\nfn f() -> Int {\n    let v = 2\n    true\n}\n";
const SORT_AZ: &str = "module a.z\n\nfn h() -> Int {\n    \"z\"\n}\n";
const SORT_BY: &str = "module b.y\n\nfn g() -> Int {\n    let u = 1\n    \"x\"\n}\n";

/// A warning and an info at one position: `missing_decreases` and
/// `verification_result` both span the function's name (S63 (3)).
const SAME_SPOT: &str = r#"module s.main

@prove
fn spin(n: Int) -> Int {
    spin(n)
}
"#;

/// A hole with a `var`, a `let`, and a parameter in scope.
const MUT: &str = r#"module mut.main

fn f(p: Int) -> Int {
    var x = 1
    let y = 2
    x = x + y
    ?h
}
"#;

fn manifest(name: &str) -> String {
    format!("[project]\nname = \"{name}\"\nedition = \"0.1\"\n")
}

/// Write `text` to `dir/path`, creating directories.
fn put(dir: &Path, path: &str, text: &str) {
    let full = dir.join(path);
    std::fs::create_dir_all(full.parent().expect("a parent directory")).expect("create directory");
    std::fs::write(&full, text).unwrap_or_else(|e| panic!("write {path}: {e}"));
}

/// The text of `dir/path`.
fn get(dir: &Path, path: &str) -> String {
    std::fs::read_to_string(dir.join(path)).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

/// A project of its own in a fresh directory: `files`, plus a manifest
/// unless `files` has one.
fn project(dir_name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = temp_dir(dir_name);
    if !files.iter().any(|(path, _)| *path == "cpl.toml") {
        put(&dir, "cpl.toml", &manifest("adv"));
    }
    for (path, text) in files {
        put(&dir, path, text);
    }
    dir
}

fn vals_project(name: &str) -> PathBuf {
    project(name, &[("src/vals/main.cpl", VALS)])
}

fn ord_project(name: &str) -> PathBuf {
    project(
        name,
        &[
            ("src/ord/main.cpl", ORD_MAIN),
            ("src/ord/deep/inner.cpl", ORD_INNER),
            ("src/ordx/other.cpl", ORD_OTHER),
        ],
    )
}

fn sorted_project(name: &str) -> PathBuf {
    project(
        name,
        &[
            ("cpl.toml", SORT_MANIFEST),
            ("src/b/y.cpl", SORT_BY),
            ("src/a.cpl", SORT_A),
            ("src/a/z.cpl", SORT_AZ),
        ],
    )
}

/// The reader fixture with a `paginate` that drops the first element of a
/// short tail, which violates its first `ensures`.
fn broken_reader(name: &str) -> Fixture {
    let fx = Fixture::new(name);
    let path = "reader/src/reader/pages.cpl";
    let text = fx.read(path);
    let broken = text.replace(
        "        pages = pages.push(rest)\n",
        "        pages = pages.push(rest.drop(1))\n",
    );
    assert_ne!(broken, text);
    fx.write(path, &broken);
    fx
}

/// A `vals.main.Tree` value nested `levels` deep, as value JSON: each level
/// is an object and an array, so the JSON nesting is `2 * levels`.
fn tree_json(levels: usize) -> String {
    let mut text = String::new();
    for _ in 0..levels {
        text.push_str("{\"$type\": \"vals.main.Tree\", \"children\": [");
    }
    for _ in 0..levels {
        text.push_str("]}");
    }
    text
}

// ===========================================================================
// Helpers: the rules every `--json` output obeys
// ===========================================================================

/// `cpl --json <args>` in `dir`: the exit code, the envelope, and its
/// key-ordered tree, after the checks of [`assert_envelope`].
#[track_caller]
fn jtree(dir: &Path, args: &[&str]) -> (i32, Value, J) {
    let mut all = vec!["--json"];
    all.extend_from_slice(args);
    let run = cpl(dir, &all);
    let env = run.json();
    assert_envelope(&env, run.code, &run.stdout);
    let tree = ordered::parse(&run.stdout);
    (run.code, env, tree)
}

/// [`jtree`] without the tree.
#[track_caller]
fn jrun(dir: &Path, args: &[&str]) -> (i32, Value) {
    let (code, env, _) = jtree(dir, args);
    (code, env)
}

/// Section 7.1 and 1.4.2: the status is the exit code's, the toolchain and
/// revision objects have their shapes, the output is pretty-printed with
/// two-space indentation, every diagnostic is an S49 object, and every key
/// is in declaration order (a payload sorted, T4).
#[track_caller]
fn assert_envelope(env: &Value, code: i32, raw: &str) {
    let status = match code {
        0 => "ok",
        1 => "unsuccessful",
        2 => "error",
        other => panic!("exit code {other} is none of 0, 1, 2 (7.1):\n{raw}"),
    };
    assert_eq!(
        env["status"], status,
        "the status of exit code {code} (7.1, T44)"
    );
    assert!(env["command"].is_string(), "{env}");
    assert_eq!(
        env["toolchain"],
        json!({ "version": "0.1.0", "edition": "0.1", "json_schema": 1 })
    );
    let revision = env
        .get("revision")
        .expect("revision is always present (Appendix C)");
    if !revision.is_null() {
        assert_revision(revision);
    }
    assert!(
        raw.starts_with("{\n  \"command\": \""),
        "pretty-printed with two-space indentation (7.1):\n{raw}"
    );
    for d in env["diagnostics"].as_array().expect("diagnostics") {
        assert_s49(d);
    }
    for d in nested_diagnostics(env) {
        assert_s49(d);
    }
    assert_envelope_order(&ordered::parse(raw));
}

/// Section 6.3: `{ commit, state, digest }`.
#[track_caller]
fn assert_revision(revision: &Value) {
    let object = revision.as_object().expect("a revision is an object");
    assert_eq!(object.len(), 3, "{revision}");
    let commit = &revision["commit"];
    assert!(
        commit.is_null() || commit.as_str().is_some_and(|c| is_hex(c, 40)),
        "{revision}"
    );
    assert!(
        matches!(revision["state"].as_str(), Some("clean" | "worktree")),
        "{revision}"
    );
    assert!(
        revision["digest"].as_str().is_some_and(|d| is_hex(d, 64)),
        "{revision}"
    );
}

/// Semantics S49: the fixed keys, optional keys omitted and never `null`,
/// `notes` and `backtrace` omitted when empty, `qualified_name` exactly
/// with `symbol`, `facet` only with `symbol`.
#[track_caller]
fn assert_s49(d: &Value) {
    let object = d
        .as_object()
        .unwrap_or_else(|| panic!("a diagnostic is an object: {d}"));
    let code = object.get("code").and_then(Value::as_str).expect("code");
    assert!(
        !code.is_empty()
            && code
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
        "a snake_case code: {code}"
    );
    let category = object
        .get("category")
        .and_then(Value::as_str)
        .expect("category");
    assert!(CATEGORIES.contains(&category), "category {category}");
    let severity = object
        .get("severity")
        .and_then(Value::as_str)
        .expect("severity");
    assert!(
        ["error", "warning", "info"].contains(&severity),
        "severity {severity}"
    );
    assert!(
        object
            .get("message")
            .and_then(Value::as_str)
            .is_some_and(|m| !m.is_empty()),
        "a message: {d}"
    );
    for key in [
        "span",
        "symbol",
        "qualified_name",
        "facet",
        "notes",
        "backtrace",
    ] {
        if let Some(value) = object.get(key) {
            assert!(
                !value.is_null(),
                "{key} is omitted when absent, never null (S49): {d}"
            );
        }
    }
    if let Some(span) = object.get("span") {
        assert_location(span);
    }
    assert_eq!(
        object.contains_key("symbol"),
        object.contains_key("qualified_name"),
        "qualified_name accompanies symbol (S49): {d}"
    );
    if let Some(symbol) = object.get("symbol") {
        assert!(symbol.is_string(), "{d}");
    }
    if let Some(facet) = object.get("facet") {
        assert!(object.contains_key("symbol"), "facet only with symbol: {d}");
        assert!(facet.as_str().is_some_and(|f| FACETS.contains(&f)), "{d}");
    }
    if let Some(notes) = object.get("notes") {
        let notes = notes.as_array().expect("notes");
        assert!(!notes.is_empty(), "notes are omitted when empty: {d}");
        for note in notes {
            let note = note.as_object().expect("a note");
            assert!(note.get("message").is_some_and(Value::is_string), "{d}");
            assert!(note.keys().all(|k| k == "message" || k == "span"), "{d}");
            if let Some(span) = note.get("span") {
                assert_location(span);
            }
        }
    }
    if let Some(frames) = object.get("backtrace") {
        let frames = frames.as_array().expect("backtrace");
        assert!(!frames.is_empty(), "backtrace is omitted when empty: {d}");
        for frame in frames {
            let frame = frame.as_object().expect("a frame");
            assert!(
                frame.get("qualified_name").is_some_and(Value::is_string),
                "{d}"
            );
            assert!(
                frame.keys().all(|k| k == "qualified_name" || k == "span"),
                "{d}"
            );
            if let Some(span) = frame.get("span") {
                assert_location(span);
            }
        }
    }
}

/// A `Location`: project-relative with `/` separators, 1-based, the end not
/// before the start.
#[track_caller]
fn assert_location(location: &Value) {
    let object = location.as_object().expect("a location is an object");
    assert_eq!(object.len(), 5, "{location}");
    let file = object["file"].as_str().expect("file");
    assert!(
        !file.contains('\\') && !file.starts_with('/'),
        "project-relative with / separators (7.1): {file}"
    );
    let n = |key: &str| {
        object[key]
            .as_u64()
            .unwrap_or_else(|| panic!("{key} in {location}"))
    };
    let (line, col, end_line, end_col) = (n("line"), n("col"), n("end_line"), n("end_col"));
    assert!(line >= 1 && col >= 1, "1-based: {location}");
    assert!((end_line, end_col) >= (line, col), "{location}");
}

/// The diagnostics nested in a result: `RunOutput::fault`, and the `fault`
/// and `counterexamples` of each test record.
fn nested_diagnostics(env: &Value) -> Vec<&Value> {
    let mut found = Vec::new();
    if env["fault"].is_object() {
        found.push(&env["fault"]);
    }
    if let Some(results) = env["results"].as_array() {
        for record in results {
            if record["fault"].is_object() {
                found.push(&record["fault"]);
            }
            if let Some(list) = record["counterexamples"].as_array() {
                found.extend(list);
            }
        }
    }
    found
}

/// Section 7.1: the envelope's keys first and in declaration order.
#[track_caller]
fn assert_envelope_order(tree: &J) {
    let keys = tree.keys();
    assert!(
        keys.len() >= 5 && keys[..5] == ENVELOPE_KEYS,
        "the envelope keys come first, in declaration order: {keys:?}"
    );
    assert_eq!(
        tree.get("toolchain").keys(),
        ["version", "edition", "json_schema"]
    );
    if let J::Obj(_) = tree.get("revision") {
        assert_eq!(tree.get("revision").keys(), ["commit", "state", "digest"]);
    }
    for d in tree.get("diagnostics").items() {
        assert_diagnostic_order(d);
    }
}

/// A diagnostic object written as a `DiagnosticJson`: the fixed keys
/// present, in declaration order, then the flattened payload, a sorted map
/// whose nested objects are sorted maps too (7.1, T4).
#[track_caller]
fn assert_diagnostic_order(d: &J) {
    let keys = d.keys();
    let fixed_count = keys.iter().take_while(|k| FIXED_KEYS.contains(k)).count();
    let fixed = &keys[..fixed_count];
    let expected: Vec<&str> = FIXED_KEYS
        .iter()
        .copied()
        .filter(|k| fixed.contains(k))
        .collect();
    assert_eq!(
        fixed,
        &expected[..],
        "fixed keys in declaration order: {keys:?}"
    );
    let payload = &keys[fixed_count..];
    assert!(
        payload.iter().all(|k| !FIXED_KEYS.contains(k)),
        "the fixed keys precede the payload: {keys:?}"
    );
    assert!(
        payload.windows(2).all(|w| w[0] < w[1]),
        "the flattened payload is a sorted map (7.1, T4): {payload:?}"
    );
    if let Some(span) = d.opt("span") {
        assert_eq!(span.keys(), LOCATION_KEYS);
    }
    if let Some(notes) = d.opt("notes") {
        for note in notes.items() {
            let k = note.keys();
            assert!(k == ["message"] || k == ["message", "span"], "{k:?}");
        }
    }
    if let Some(frames) = d.opt("backtrace") {
        for frame in frames.items() {
            let k = frame.keys();
            assert!(
                k == ["qualified_name"] || k == ["qualified_name", "span"],
                "{k:?}"
            );
        }
    }
    for key in payload {
        assert_sorted_deep(d.get(key));
    }
}

/// Every object inside a `serde_json` map has sorted keys (T4).
#[track_caller]
fn assert_sorted_deep(value: &J) {
    match value {
        J::Obj(members) => {
            let keys: Vec<&str> = members.iter().map(|(k, _)| k.as_str()).collect();
            assert!(
                keys.windows(2).all(|w| w[0] < w[1]),
                "a serde_json map is sorted (T4): {keys:?}"
            );
            for (_, v) in members {
                assert_sorted_deep(v);
            }
        }
        J::Arr(items) => items.iter().for_each(assert_sorted_deep),
        _ => {}
    }
}

/// A `SymbolVersion`'s `facets`: only facet names, in `Facet::ALL` order
/// (1.4.2), each a 32-hex `FacetHash`.
#[track_caller]
fn assert_canonical_facets(facets: &J) {
    let keys = facets.keys();
    let canonical: Vec<&str> = FACETS
        .iter()
        .copied()
        .filter(|f| keys.contains(f))
        .collect();
    assert_eq!(
        keys, canonical,
        "facets in Facet::ALL order (1.4.2), not sorted"
    );
    for key in keys {
        match facets.get(key) {
            J::Str(hash) => assert!(is_hex(hash, 32), "{hash}"),
            other => panic!("a facet hash is a string: {other:?}"),
        }
    }
}

fn envelope_and(extra: &[&'static str]) -> Vec<&'static str> {
    ENVELOPE_KEYS.iter().chain(extra).copied().collect()
}

/// The codes of an envelope's diagnostics, in array order.
fn codes(env: &Value) -> Vec<String> {
    env["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .map(|d| d["code"].as_str().expect("code").to_string())
        .collect()
}

/// The one diagnostic with `code`.
#[track_caller]
fn only<'a>(env: &'a Value, code: &str) -> &'a Value {
    let found = with_code(env, code);
    assert_eq!(found.len(), 1, "exactly one {code} in {env}");
    found[0]
}

/// The qualified names of a test report's records, in order.
fn record_names(env: &Value) -> Vec<String> {
    env["results"]
        .as_array()
        .expect("results")
        .iter()
        .map(|r| r["qualified_name"].as_str().expect("name").to_string())
        .collect()
}

/// The record of the test or property `name`.
#[track_caller]
fn record<'a>(env: &'a Value, name: &str) -> &'a Value {
    env["results"]
        .as_array()
        .expect("results")
        .iter()
        .find(|r| r["qualified_name"] == name)
        .unwrap_or_else(|| panic!("no record of {name} in {env}"))
}

fn is_hex(text: &str, len: usize) -> bool {
    text.len() == len
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The generated id format of Section 4.1 with this kind prefix.
fn generated_id(id: &str, kind: &str) -> bool {
    id.strip_prefix(kind)
        .and_then(|rest| rest.strip_prefix('_'))
        .is_some_and(|rest| {
            rest.len() == 16
                && rest
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
        })
}

fn severity_rank(d: &Value) -> u8 {
    match d["severity"].as_str() {
        Some("error") => 0,
        Some("warning") => 1,
        _ => 2,
    }
}

/// An output with its `duration_ms` values (the one volatile field of a
/// seeded run, 7.1 Determinism) replaced.
fn no_durations(text: &str) -> String {
    normalize(text, &Volatile::default())
}

/// The two stable lines of every diagnostic rendered in a human stderr
/// (Section 1.4.1): line 1, and line 2 when the next line is a `-->` line.
fn stable_lines(stderr: &str) -> Vec<(String, Option<String>)> {
    let lines: Vec<&str> = stderr.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let head = ["error[", "warning[", "info["]
            .iter()
            .any(|p| line.starts_with(p))
            && line.contains("]: ");
        if head {
            let next = lines
                .get(i + 1)
                .filter(|l| l.starts_with("  --> "))
                .map(|l| l.to_string());
            out.push((line.to_string(), next));
        }
    }
    out
}

/// The stable lines the diagnostic objects render to (Section 1.4.1).
fn expected_lines(diagnostics: &[Value]) -> Vec<(String, Option<String>)> {
    diagnostics
        .iter()
        .map(|d| {
            let first = format!(
                "{}[{}]: {}",
                d["severity"].as_str().expect("severity"),
                d["code"].as_str().expect("code"),
                d["message"].as_str().expect("message")
            );
            let second = d.get("span").map(|s| {
                format!(
                    "  --> {}:{}:{}",
                    s["file"].as_str().expect("file"),
                    s["line"],
                    s["col"]
                )
            });
            (first, second)
        })
        .collect()
}

/// The 1-based line and column of the first occurrence of `needle` (ASCII).
fn locate(text: &str, needle: &str) -> (u64, u64) {
    let at = text
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} is not in the fixture"));
    let line = text[..at].matches('\n').count() as u64 + 1;
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    (line, (at - line_start) as u64 + 1)
}

/// Where parameter `param` of the function whose header starts with
/// `header` is in `VALS`.
fn param_location(header: &str, param: &str) -> (u64, u64) {
    let (line, _) = locate(VALS, header);
    let text = VALS
        .lines()
        .nth(line as usize - 1)
        .expect("the header line");
    let col = text
        .find(param)
        .unwrap_or_else(|| panic!("{param} in {text}")) as u64
        + 1;
    (line, col)
}

/// The human summary line of `cpl check` for a `CheckSummary` (7.2):
/// `check: 0 errors, 2 warnings, 1 hole, 14 definitions in 3 modules`.
fn summary_line(summary: &Value) -> String {
    let count = |key: &str| summary[key].as_u64().expect(key);
    let words = |n: u64, one: &str, many: &str| {
        if n == 1 {
            format!("1 {one}")
        } else {
            format!("{n} {many}")
        }
    };
    format!(
        "check: {}, {}, {}, {} in {}\n",
        words(count("errors"), "error", "errors"),
        words(count("warnings"), "warning", "warnings"),
        words(count("holes"), "hole", "holes"),
        words(count("definitions"), "definition", "definitions"),
        words(count("modules"), "module", "modules"),
    )
}

// ===========================================================================
// 7.1 Project discovery
// ===========================================================================

#[test]
fn every_local_command_finds_the_project_from_a_nested_directory() {
    let fx = Fixture::new("adv-discovery-nested");
    let deep = fx.reader.join("src/tests");
    let (code, env) = jrun(&deep, &["check"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(env["summary"]["definitions"], 21);
    // Paths stay project-relative whatever the current directory (7.1).
    assert_eq!(
        only(&env, "verification_result")["span"]["file"],
        "src/reader/pages.cpl"
    );
    let (code, env) = jrun(&deep, &["test", "--seed", "1"]);
    assert_eq!((code, env["selected"].clone()), (0, json!(3)));
    let (code, env) = jrun(&deep, &["run", "reader.pages.divide_exactly", "6", "3"]);
    assert_eq!((code, env["result_text"].clone()), (0, json!("2")));
    let (code, env) = jrun(&deep, &["fmt", "--check"]);
    assert_eq!(code, 0, "{env}");
    let paths: Vec<&str> = env["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|f| f["path"].as_str().expect("path"))
        .collect();
    assert_eq!(paths, READER_FILES);
    let (code, env) = jrun(&deep, &["id", "assign", "--check"]);
    assert_eq!(code, 1);
    let assignments = env["assignments"].as_array().expect("assignments");
    assert_eq!(assignments.len(), 20, "the whole source set, not the cwd");
    assert!(assignments
        .iter()
        .all(|a| a["path"].as_str().is_some_and(|p| p.starts_with("src/"))));
}

#[test]
fn the_nearest_ancestor_with_cpl_toml_is_the_project() {
    let fx = Fixture::new("adv-discovery-nearest");
    // holes, copied inside reader's root (outside reader's sources).
    let inner = fx.reader.join("inner");
    copy_dir(&fx.holes, &inner);
    let (code, env) = jrun(&inner.join("src/holes"), &["check"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(env["summary"]["holes"], 2, "the nested holes project");
    assert_eq!(env["summary"]["definitions"], 5);
    let (code, env) = jrun(&inner, &["test"]);
    assert_eq!(code, 1, "holes has no tests: an empty selection");
    assert_eq!(codes(&env), ["empty_test_selection"]);
    let (code, env) = jrun(&fx.reader.join("src/reader"), &["check"]);
    assert_eq!(
        (code, env["summary"]["definitions"].clone()),
        (0, json!(21))
    );
}

#[test]
fn project_flag_is_resolved_against_the_current_directory_and_wins() {
    let fx = Fixture::new("adv-project-flag");
    // From inside holes, `--project` names reader, which wins.
    let inside = fx.holes.join("src/holes");
    let reader = "../../../reader";
    let (code, env) = jrun(&inside, &["check", "--project", reader]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(env["summary"]["definitions"], 21, "reader, not holes");
    let (code, env) = jrun(&inside, &["--project", reader, "test", "--seed", "1"]);
    assert_eq!((code, env["selected"].clone()), (0, json!(3)));
    let (code, env) = jrun(
        &inside,
        &[
            "run",
            "reader.pages.divide_exactly",
            "6",
            "3",
            "--project",
            reader,
        ],
    );
    assert_eq!((code, env["result_text"].clone()), (0, json!("2")));
    let (code, env) = jrun(&inside, &["fmt", "--check", "--project", reader]);
    assert_eq!(code, 0);
    assert_eq!(env["files"].as_array().map(Vec::len), Some(5));
    let (code, env) = jrun(&inside, &["id", "assign", "--check", "--project", reader]);
    assert_eq!(
        (code, env["assignments"].as_array().map(Vec::len)),
        (1, Some(20))
    );
    // An absolute path works from a directory outside any project.
    let elsewhere = temp_dir("adv-project-flag-elsewhere");
    let holes = fx.holes.to_str().expect("a UTF-8 path");
    let (code, env) = jrun(&elsewhere, &["check", "--project", holes]);
    assert_eq!((code, env["summary"]["holes"].clone()), (0, json!(2)));
}

#[test]
fn project_flag_without_cpl_toml_is_usage_no_project() {
    let fx = Fixture::new("adv-project-flag-missing");
    let commands: [&[&str]; 5] = [
        &["check"],
        &["test"],
        &["run", "reader.pages.divide_exactly", "6", "3"],
        &["fmt"],
        &["id", "assign"],
    ];
    // `src` lies inside the project, but `--project DIR` names the root
    // itself, which must contain cpl.toml (7.1): no search upward. A file
    // and a missing path name no root either.
    for bad in ["src", "src/reader", "cpl.toml", "no_such_directory"] {
        for command in commands {
            let mut args = vec!["--project", bad];
            args.extend_from_slice(command);
            let (code, env) = jrun(&fx.reader, &args);
            assert_eq!(code, 2, "{args:?}: {env}");
            assert_eq!(codes(&env), ["usage_no_project"], "{args:?}");
            assert_eq!(env["revision"], Value::Null, "{args:?}");
            assert_eq!(env["diagnostics"][0]["category"], "usage");
        }
    }
}

#[test]
fn usage_no_project_is_one_spanless_error_in_every_format() {
    let dir = temp_dir("adv-no-project-formats");
    let commands: [(&[&str], &str); 5] = [
        (&["check"], "check"),
        (&["test"], "test"),
        (&["run", "a.f"], "run"),
        (&["fmt"], "fmt"),
        (&["id", "assign"], "id assign"),
    ];
    for (command, name) in commands {
        let run = cpl(&dir, command);
        run.expect_code(2);
        assert_eq!(run.stdout, "", "{command:?}: nothing on stdout");
        let lines = stable_lines(&run.stderr);
        assert_eq!(lines.len(), 1, "{command:?}:\n{}", run.stderr);
        assert!(
            lines[0].0.starts_with("error[usage_no_project]: "),
            "{lines:?}"
        );
        assert_eq!(lines[0].1, None, "a spanless diagnostic has no --> line");
        let (code, env, tree) = jtree(&dir, command);
        assert_eq!(code, 2);
        assert_eq!(env["command"], name);
        assert_eq!(env["revision"], Value::Null);
        assert_eq!(codes(&env), ["usage_no_project"]);
        assert!(env["diagnostics"][0].get("span").is_none());
        assert_eq!(
            tree.keys(),
            ENVELOPE_KEYS,
            "a usage error's envelope carries no keys of its own (NoOutput, 1.4.2)"
        );
    }
    for name in ["check", "test"] {
        let run = cpl(&dir, &[name, "--jsonl"]);
        run.expect_code(2);
        let lines = run.json_lines();
        assert_eq!(
            lines.len(),
            2,
            "one diagnostic, then the envelope:\n{}",
            run.stdout
        );
        assert_eq!(lines[0]["code"], "usage_no_project");
        assert_eq!(
            lines[1],
            json!({
                "command": name,
                "toolchain": { "version": "0.1.0", "edition": "0.1", "json_schema": 1 },
                "revision": null,
                "status": "error"
            })
        );
    }
}

#[test]
fn fmt_with_explicit_paths_needs_no_project() {
    let dir = temp_dir("adv-fmt-loose");
    put(&dir, "a.cpl", TIDY);
    put(&dir, "sub/b.cpl", MESSY);
    put(&dir, "sub/notes.txt", "not CPL\n");
    let (code, env) = jrun(&dir, &["fmt", "--check", "."]);
    assert_eq!(code, 1, "{env}");
    assert_eq!(
        env["revision"],
        Value::Null,
        "no project is involved (1.4.2)"
    );
    // The specification fixes project-relative paths only; outside a project
    // the reading taken is relative to the current directory with `/`
    // separators (determinism, 7.1).
    assert_eq!(
        env["files"],
        json!([{ "path": "a.cpl", "result": "unchanged" },
               { "path": "sub/b.cpl", "result": "would_format" }])
    );
    let run = cpl(&dir, &["fmt", "--check", "a.cpl", "sub"]);
    run.expect_code(1);
    assert_eq!(run.stdout, "sub/b.cpl\n", "--check names the file (7.2)");
    assert_eq!(get(&dir, "sub/b.cpl"), MESSY, "--check writes nothing");
    cpl(&dir, &["fmt", "sub/b.cpl"]).expect_code(0);
    assert_eq!(get(&dir, "sub/b.cpl"), TIDY);
    cpl(&dir, &["fmt", "--check", "."]).expect_code(0);
}

#[test]
fn id_assign_with_explicit_paths_needs_no_project() {
    let dir = temp_dir("adv-id-loose");
    let a = "module a\n\npub fn f() -> Int {\n    1\n}\n\npub struct S {\n    v: Int\n}\n";
    let b = "module b\n\ntest \"t\" {\n    assert true\n}\n";
    put(&dir, "lib/a.cpl", a);
    put(&dir, "lib/b.cpl", b);
    let (code, env) = jrun(&dir, &["id", "assign", "--check", "lib"]);
    assert_eq!(code, 1, "--check found work: {env}");
    assert_eq!(env["revision"], Value::Null, "no project is involved");
    let found: Vec<(&str, &str)> = env["assignments"]
        .as_array()
        .expect("assignments")
        .iter()
        .map(|a| (a["path"].as_str().unwrap(), a["symbol"].as_str().unwrap()))
        .collect();
    assert_eq!(
        found,
        [
            ("lib/a.cpl", "a.f"),
            ("lib/a.cpl", "a.S"),
            ("lib/b.cpl", "b.\"t\"")
        ]
    );
    assert_eq!(get(&dir, "lib/a.cpl"), a, "--check writes nothing");
    let (code, env) = jrun(&dir, &["id", "assign", "lib/a.cpl", "lib/b.cpl"]);
    assert_eq!(code, 0, "{env}");
    let assignments = env["assignments"].as_array().expect("assignments");
    let mut ids: Vec<&str> = assignments
        .iter()
        .map(|a| a["id"].as_str().unwrap())
        .collect();
    for (id, kind) in ids.iter().zip(["fn", "ty", "test"]) {
        assert!(
            generated_id(id, kind),
            "{id} is a generated {kind} id (4.1)"
        );
    }
    assert!(get(&dir, "lib/a.cpl").contains(&format!("@id(\"{}\")\npub fn f()", ids[0])));
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 3, "never the same id twice in one run");
    let (code, env) = jrun(&dir, &["id", "assign", "--check", "lib"]);
    assert_eq!((code, env["assignments"].clone()), (0, json!([])));
}

// ===========================================================================
// 7.1 --json: exactly one envelope
// ===========================================================================

#[test]
fn json_is_one_envelope_for_every_outcome_of_the_local_commands() {
    let fx = Fixture::new("adv-envelopes");
    let vals = vals_project("adv-envelopes-vals");
    let err = project("adv-envelopes-err", &[("src/err/main.cpl", ERR)]);
    let bad = project("adv-envelopes-bad", &[("cpl.toml", "[project\n")]);
    let messy = project("adv-envelopes-messy", &[("src/m.cpl", MESSY)]);
    let ids = project(
        "adv-envelopes-ids",
        &[(
            "src/i.cpl",
            "module i\n\n@id(\"fn_bbbbbbbbbbbbbbbb\")\npub fn f() -> Int {\n    1\n}\n",
        )],
    );
    let broken = broken_reader("adv-envelopes-broken");
    let duplicate = r#"{"a": 1, "a": 2}"#;
    let cases: Vec<(&Path, Vec<&str>, i32)> = vec![
        (fx.reader.as_path(), vec!["check"], 0),
        (fx.holes.as_path(), vec!["check"], 0),
        (fx.holes.as_path(), vec!["check", "--release"], 1),
        (err.as_path(), vec!["check"], 1),
        (bad.as_path(), vec!["check"], 2),
        (fx.reader.as_path(), vec!["fmt", "--check"], 0),
        (messy.as_path(), vec!["fmt", "--check"], 1),
        (messy.as_path(), vec!["fmt", "--check", "missing.cpl"], 2),
        (vals.as_path(), vec!["run", "vals.main.int", "1"], 0),
        (vals.as_path(), vec!["run", "vals.main.pos", "0"], 1),
        (
            fx.reader.as_path(),
            vec!["run", "reader.pages.divide_exactly", "7", "0"],
            1,
        ),
        (err.as_path(), vec!["run", "err.main.f"], 1),
        (vals.as_path(), vec!["run", "vals.main.head", "[1]"], 2),
        (vals.as_path(), vec!["run", "vals.main.int", "1 + 1"], 2),
        (
            vals.as_path(),
            vec!["run", "vals.main.int", "--arg", duplicate],
            2,
        ),
        (bad.as_path(), vec!["run", "x.f"], 2),
        (fx.reader.as_path(), vec!["test", "--seed", "1"], 0),
        (broken.reader.as_path(), vec!["test", "--seed", "1"], 1),
        (fx.holes.as_path(), vec!["test"], 1),
        (err.as_path(), vec!["test"], 1),
        (fx.reader.as_path(), vec!["test", "not..a.selector"], 2),
        (bad.as_path(), vec!["test"], 2),
        (ids.as_path(), vec!["id", "assign", "--check"], 0),
        (fx.reader.as_path(), vec!["id", "assign", "--check"], 1),
        (
            fx.reader.as_path(),
            vec!["id", "assign", "--check", "missing.cpl"],
            2,
        ),
    ];
    for (dir, args, expected) in cases {
        let (code, env) = jrun(dir, &args);
        assert_eq!(code, expected, "{args:?}: {env}");
    }
}

#[test]
fn json_flag_may_be_written_anywhere_on_the_command_line() {
    let fx = Fixture::new("adv-json-anywhere");
    let forms: [&[&str]; 4] = [
        &["--json", "run", "reader.pages.divide_exactly", "6", "3"],
        &["run", "--json", "reader.pages.divide_exactly", "6", "3"],
        &["run", "reader.pages.divide_exactly", "--json", "6", "3"],
        &["run", "reader.pages.divide_exactly", "6", "3", "--json"],
    ];
    let first = cpl(&fx.reader, forms[0]);
    first.expect_code(0);
    first.json();
    for form in &forms[1..] {
        let run = cpl(&fx.reader, form);
        run.expect_code(0);
        assert_eq!(run.stdout, first.stdout, "{form:?}");
    }
    let a = cpl(&fx.reader, &["--json", "check"]);
    let b = cpl(&fx.reader, &["check", "--json"]);
    assert_eq!(a.stdout, b.stdout);
    let a = cpl(&fx.reader, &["--json", "test", "--seed", "2"]);
    let b = cpl(&fx.reader, &["test", "--seed", "2", "--json"]);
    assert_eq!(no_durations(&a.stdout), no_durations(&b.stdout));
}

// ===========================================================================
// Appendix C: key order
// ===========================================================================

#[test]
fn check_output_keys_follow_the_declaration_order() {
    let fx = Fixture::new("adv-order-check");
    let (code, _, tree) = jtree(&fx.holes, &["check"]);
    assert_eq!(code, 0);
    assert_eq!(
        tree.keys(),
        envelope_and(&["release", "summary", "holes", "verification"])
    );
    assert_eq!(
        tree.get("summary").keys(),
        [
            "errors",
            "warnings",
            "holes",
            "definitions",
            "modules",
            "files"
        ]
    );
    for entry in tree.get("verification").items() {
        assert_eq!(
            entry.keys(),
            ["symbol", "qualified_name", "status", "reason"]
        );
    }
    let holes = tree.get("holes").items();
    assert_eq!(holes.len(), 2);
    for hole in holes {
        assert_eq!(
            hole.keys(),
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
        for binding in hole.get("bindings").items() {
            assert_eq!(binding.keys(), ["name", "type", "kind"]);
        }
        let deps = hole.get("dependencies");
        assert_eq!(
            deps.keys(),
            ["source_revision", "toolchain", "modules", "symbols"]
        );
        assert_eq!(deps.get("toolchain").keys(), ["cpl", "edition", "std"]);
        for module in deps.get("modules").items() {
            assert_eq!(module.keys(), ["module", "content_hash"]);
        }
        for symbol in deps.get("symbols").items() {
            assert_eq!(
                symbol.keys(),
                ["id", "path", "provisional", "facets", "definition"]
            );
            assert_canonical_facets(symbol.get("facets"));
        }
    }
    // Inside the `hole` diagnostic the same report is flattened payload, a
    // sorted map all the way down (T4).
    let in_diagnostic = tree
        .get("diagnostics")
        .items()
        .iter()
        .find(|d| d.get("code") == &J::Str("hole".into()))
        .expect("a hole diagnostic");
    assert_eq!(
        in_diagnostic.get("dependencies").keys(),
        ["modules", "source_revision", "symbols", "toolchain"]
    );
}

#[test]
fn run_output_keys_follow_the_declaration_order() {
    let vals = vals_project("adv-order-run");
    let written = r#"{"right": "r", "left": 1, "$type": "vals.main.Pair"}"#;
    let (code, _, tree) = jtree(&vals, &["run", "vals.main.pair", "--arg", written]);
    assert_eq!(code, 0);
    assert_eq!(
        tree.keys(),
        envelope_and(&["function", "arguments", "result", "result_text", "fault"])
    );
    let function = tree.get("function");
    assert_eq!(
        function.keys(),
        ["id", "path", "provisional", "facets", "definition"]
    );
    assert_eq!(
        function.get("facets").keys(),
        FACETS,
        "a fn has all eight facets, in Facet::ALL order (1.4.2)"
    );
    assert_canonical_facets(function.get("facets"));
    assert_eq!(
        tree.get("result").keys(),
        ["$type", "left", "right"],
        "value JSON is a sorted map (7.3, T4)"
    );
    assert_eq!(
        tree.get("arguments").items()[0].keys(),
        ["$type", "left", "right"]
    );
    let (_, _, tree) = jtree(&vals, &["run", "vals.main.shape", "Both(1, \"x\")"]);
    assert_eq!(tree.get("result").keys(), ["$payload", "$type", "$variant"]);
    let (_, _, tree) = jtree(&vals, &["run", "vals.main.tag", "Tag(\"p\")"]);
    assert_eq!(tree.get("result").keys(), ["$payload", "$type"]);
    // A fault is a `DiagnosticJson` in its own field order.
    let fx = Fixture::new("adv-order-run-fault");
    let (code, _, tree) = jtree(
        &fx.reader,
        &["run", "reader.pages.divide_exactly", "7", "0"],
    );
    assert_eq!(code, 1);
    let fault = tree.get("fault");
    assert_diagnostic_order(fault);
    assert_eq!(
        &fault.keys()[..9],
        [
            "code",
            "category",
            "severity",
            "message",
            "span",
            "symbol",
            "qualified_name",
            "facet",
            "backtrace"
        ]
    );
    assert_eq!(fault.get("input").keys(), ["a", "b"]);
}

#[test]
fn test_output_keys_follow_the_declaration_order() {
    let fx = broken_reader("adv-order-test");
    let (code, _, tree) = jtree(&fx.reader, &["test", "--seed", "1"]);
    assert_eq!(code, 1);
    assert_eq!(
        tree.keys(),
        envelope_and(&["seed", "cases", "selected", "tested", "failed", "results", "receipt"])
    );
    let results = tree.get("results").items();
    assert_eq!(results.len(), 3);
    assert_eq!(
        results[0].keys(),
        [
            "symbol",
            "qualified_name",
            "kind",
            "status",
            "duration_ms",
            "receipt"
        ]
    );
    assert_eq!(
        results[1].keys(),
        [
            "symbol",
            "qualified_name",
            "kind",
            "status",
            "duration_ms",
            "fault",
            "counterexamples",
            "receipt"
        ]
    );
    assert_eq!(
        results[2].keys(),
        [
            "symbol",
            "qualified_name",
            "kind",
            "status",
            "duration_ms",
            "seed",
            "property_seed",
            "cases",
            "failing_case",
            "shrunk",
            "fault",
            "counterexamples",
            "receipt"
        ]
    );
    assert_eq!(results[2].get("failing_case").keys(), ["index", "input"]);
    assert_eq!(
        results[2].get("shrunk").keys(),
        ["input", "shrink_steps", "shrink_exhausted"]
    );
    for record in results {
        let receipt = record.get("receipt");
        assert_eq!(
            receipt.keys(),
            [
                "source_revision",
                "toolchain",
                "modules",
                "check_configuration",
                "assumptions",
                "fixtures",
                "environment"
            ],
            "ReceiptCore order; `dirty` omitted when not true"
        );
        assert_eq!(receipt.get("toolchain").keys(), ["cpl", "edition", "std"]);
        for module in receipt.get("modules").items() {
            assert_eq!(module.keys(), ["module", "content_hash"]);
        }
        assert_sorted_deep(receipt.get("check_configuration"));
        assert_sorted_deep(receipt.get("environment"));
        if let Some(fault) = record.opt("fault") {
            assert_diagnostic_order(fault);
        }
        if let Some(list) = record.opt("counterexamples") {
            for counterexample in list.items() {
                assert_diagnostic_order(counterexample);
            }
        }
    }
    let fx = Fixture::new("adv-order-test-pass");
    let (code, _, tree) = jtree(&fx.reader, &["test", "--seed", "1"]);
    assert_eq!(code, 0);
    assert_eq!(
        tree.get("results").items()[2].keys(),
        [
            "symbol",
            "qualified_name",
            "kind",
            "status",
            "duration_ms",
            "seed",
            "property_seed",
            "cases",
            "receipt"
        ]
    );
}

#[test]
fn fmt_and_id_assign_output_keys_follow_the_declaration_order() {
    let fx = Fixture::new("adv-order-fmt-id");
    let (code, _, tree) = jtree(&fx.reader, &["fmt", "--check"]);
    assert_eq!(code, 0);
    assert_eq!(tree.keys(), envelope_and(&["check", "files"]));
    for file in tree.get("files").items() {
        assert_eq!(file.keys(), ["path", "result"]);
    }
    put(&fx.reader, "src/reader/broken.cpl", BROKEN_SYNTAX);
    let (code, _, tree) = jtree(&fx.reader, &["id", "assign", "--check"]);
    assert_eq!(code, 1);
    assert_eq!(
        tree.keys(),
        envelope_and(&["check", "assignments", "skipped_files"])
    );
    for assignment in tree.get("assignments").items() {
        assert_eq!(assignment.keys(), ["path", "line", "symbol", "kind", "id"]);
    }
    let skipped = tree.get("skipped_files").items();
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].keys(), ["path", "reason"]);
}

// ===========================================================================
// Appendix C: null versus omitted
// ===========================================================================

#[test]
fn run_output_writes_an_absent_result_or_fault_as_null() {
    let vals = vals_project("adv-null-run");
    let (code, env, tree) = jtree(&vals, &["run", "vals.main.int", "5"]);
    assert_eq!(code, 0);
    assert_eq!(tree.get("fault"), &J::Null, "present and null (Appendix C)");
    assert_eq!(env["result"], json!(5));
    assert_eq!(env["result_text"], "5");
    let (code, env, tree) = jtree(&vals, &["run", "vals.main.pos", "0"]);
    assert_eq!(code, 1);
    assert_eq!(tree.get("result"), &J::Null);
    assert_eq!(tree.get("result_text"), &J::Null);
    assert!(env["fault"].is_object());
    assert!(
        env["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .contains(&env["fault"]),
        "the fault is also in diagnostics (7.2)"
    );
}

#[test]
fn revision_is_null_exactly_when_no_project_is_involved() {
    let fx = Fixture::new("adv-null-revision");
    let loose = temp_dir("adv-null-revision-loose");
    put(&loose, "a.cpl", TIDY);
    let (_, env) = jrun(&loose, &["fmt", "--check", "a.cpl"]);
    assert_eq!(env["revision"], Value::Null, "fmt on loose files (1.4.2)");
    let run = cpl_with_stdin(
        &loose,
        &["--json", "fmt", "--stdin", "cpl", "--check"],
        Some(TIDY),
    );
    run.expect_code(0);
    let env = run.json();
    assert_envelope(&env, 0, &run.stdout);
    assert_eq!(env["revision"], Value::Null, "fmt --stdin");
    let (_, env) = jrun(&loose, &["id", "assign", "--check", "a.cpl"]);
    assert_eq!(env["revision"], Value::Null, "id assign on loose files");
    let (code, env) = jrun(&fx.reader, &["check", "--frobnicate"]);
    assert_eq!(code, 2);
    assert_eq!(env["revision"], Value::Null, "a refused command line");
    // In a project without a repository: commit null, state worktree.
    let commands: [&[&str]; 6] = [
        &["check"],
        &["fmt", "--check"],
        &["fmt", "--check", "src"],
        &["test", "--seed", "1"],
        &["run", "reader.pages.divide_exactly", "6", "3"],
        &["id", "assign", "--check"],
    ];
    for command in commands {
        let (_, env) = jrun(&fx.reader, command);
        assert_eq!(env["revision"]["commit"], Value::Null, "{command:?}");
        assert_eq!(env["revision"]["state"], "worktree", "{command:?}");
    }
}

#[test]
fn passing_test_records_omit_the_failure_and_property_keys() {
    let fx = Fixture::new("adv-omitted-keys");
    let (code, env) = jrun(&fx.reader, &["test", "--seed", "1"]);
    assert_eq!(code, 0);
    let results = env["results"].as_array().expect("results");
    for record in &results[..2] {
        assert_eq!(record["kind"], "test");
        for key in [
            "seed",
            "property_seed",
            "cases",
            "failing_case",
            "shrunk",
            "fault",
            "counterexamples",
        ] {
            assert!(
                record.get(key).is_none(),
                "a passing test omits {key}: {record}"
            );
        }
    }
    let property = &results[2];
    for key in ["failing_case", "shrunk", "fault", "counterexamples"] {
        assert!(
            property.get(key).is_none(),
            "a passing property omits {key}"
        );
    }
    for key in ["seed", "property_seed", "cases"] {
        assert!(property.get(key).is_some(), "a property carries {key}");
    }
    for d in with_code(&env, "test_result") {
        for key in ["failing_case", "shrunk", "fault", "counterexamples"] {
            assert!(d.get(key).is_none(), "{key} in {d}");
        }
    }
    for record in results {
        let receipt = &record["receipt"];
        assert_eq!(receipt["source_revision"], "unversioned");
        assert!(receipt.get("dirty").is_none(), "dirty only when true");
        assert_eq!(receipt["fixtures"], json!([]));
        assert!(receipt["environment"].is_object());
    }
}

#[test]
fn hole_bindings_carry_mutable_only_when_true() {
    let dir = project("adv-mutable", &[("src/mut/main.cpl", MUT)]);
    let (code, env) = jrun(&dir, &["check"]);
    assert_eq!(code, 0, "{env}");
    // Innermost scope first; within a scope the most recent binding first.
    let expected = json!([
        { "name": "y", "type": "Int", "kind": "let" },
        { "name": "x", "type": "Int", "kind": "var", "mutable": true },
        { "name": "p", "type": "Int", "kind": "parameter" }
    ]);
    assert_eq!(env["holes"][0]["bindings"], expected);
    assert_eq!(only(&env, "hole")["bindings"], expected);
}

// ===========================================================================
// 7.1 and T46: Diagnostics::sorted order in JSON, JSONL, and text
// ===========================================================================

#[test]
fn diagnostics_are_sorted_by_file_then_position_and_spanless_ones_come_last() {
    let dir = sorted_project("adv-sorted");
    let (code, env) = jrun(&dir, &["check"]);
    assert_eq!(code, 1, "{env}");
    let diagnostics = env["diagnostics"].as_array().expect("diagnostics");
    let spanned = diagnostics
        .iter()
        .take_while(|d| d.get("span").is_some())
        .count();
    assert!(
        diagnostics[spanned..]
            .iter()
            .all(|d| d.get("span").is_none()),
        "spanless diagnostics come last: {env}"
    );
    let key = |d: &Value| {
        let span = &d["span"];
        (
            span["file"].as_str().expect("file").to_string(),
            span["line"].as_u64().expect("line"),
            span["col"].as_u64().expect("col"),
            severity_rank(d),
            d["code"].as_str().expect("code").to_string(),
        )
    };
    for pair in diagnostics[..spanned].windows(2) {
        assert!(
            key(&pair[0]) <= key(&pair[1]),
            "out of order:\n{}\n{}",
            pair[0],
            pair[1]
        );
    }
    let mut files: Vec<&str> = diagnostics[..spanned]
        .iter()
        .map(|d| d["span"]["file"].as_str().expect("file"))
        .collect();
    files.dedup();
    assert_eq!(files, ["src/a.cpl", "src/a/z.cpl", "src/b/y.cpl"]);
    let unknown: Vec<&str> = diagnostics[spanned..]
        .iter()
        .filter(|d| d["code"] == "project_manifest_unknown_key")
        .map(|d| d["key"].as_str().expect("key"))
        .collect();
    assert_eq!(
        unknown,
        ["project.zeta", "project.alpha", "test.bogus"],
        "spanless diagnostics keep their emission (document) order"
    );
    let run = cpl(&dir, &["check", "--jsonl"]);
    run.expect_code(1);
    let lines = run.json_lines();
    assert_eq!(
        &lines[..lines.len() - 1],
        &diagnostics[..],
        "--jsonl streams check's sorted order (7.1)"
    );
}

#[test]
fn diagnostics_at_one_position_are_ordered_by_severity() {
    let dir = project("adv-same-spot", &[("src/s/main.cpl", SAME_SPOT)]);
    let (code, env) = jrun(&dir, &["check"]);
    assert_eq!(code, 0, "{env}");
    let at_name: Vec<(&str, &str)> = env["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .filter(|d| d["span"]["line"] == 4 && d["span"]["col"] == 4)
        .map(|d| (d["severity"].as_str().unwrap(), d["code"].as_str().unwrap()))
        .collect();
    assert_eq!(
        at_name,
        [
            ("warning", "missing_decreases"),
            ("info", "verification_result")
        ]
    );
}

#[test]
fn human_stderr_lists_the_diagnostics_in_the_json_order() {
    let fx = Fixture::new("adv-human-order");
    let sorted = sorted_project("adv-human-order-sorted");
    let same = project("adv-human-order-same", &[("src/s/main.cpl", SAME_SPOT)]);
    let one = project("adv-human-order-one", &[("src/one/main.cpl", ONE)]);
    let cases: [(&Path, &[&str]); 6] = [
        (sorted.as_path(), &["check"]),
        (same.as_path(), &["check"]),
        (one.as_path(), &["check"]),
        (fx.holes.as_path(), &["check"]),
        (fx.holes.as_path(), &["check", "--release"]),
        (fx.reader.as_path(), &["check"]),
    ];
    for (dir, args) in cases {
        let (code, env) = jrun(dir, args);
        let run = cpl(dir, args);
        run.expect_code(code);
        assert_eq!(
            stable_lines(&run.stderr),
            expected_lines(env["diagnostics"].as_array().expect("diagnostics")),
            "{} {args:?}:\n{}",
            dir.display(),
            run.stderr
        );
    }
    // cpl fmt's parse diagnostics of two files.
    let loose = temp_dir("adv-human-order-fmt");
    put(&loose, "b.cpl", "module b\n\nfn f( {\n");
    put(&loose, "a.cpl", "module a\n\nfn g( ) -> {\n");
    let (code, env) = jrun(&loose, &["fmt", "--check", "."]);
    assert_eq!(code, 1);
    let files: Vec<&str> = env["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .map(|d| d["span"]["file"].as_str().expect("file"))
        .collect();
    assert_eq!(files, ["a.cpl", "b.cpl"]);
    let run = cpl(&loose, &["fmt", "--check", "."]);
    run.expect_code(1);
    assert_eq!(
        stable_lines(&run.stderr),
        expected_lines(env["diagnostics"].as_array().expect("diagnostics"))
    );
}

// ===========================================================================
// 7.1 --jsonl framing
// ===========================================================================

#[test]
fn jsonl_lines_are_compact_objects_and_the_tail_is_the_envelope_without_diagnostics() {
    let fx = Fixture::new("adv-jsonl-frames");
    let run = cpl(&fx.reader, &["test", "--seed", "1", "--jsonl"]);
    run.expect_code(0);
    let lines = run.json_lines();
    assert_eq!(lines.len(), 4);
    assert!(run.stdout.ends_with('\n') && !run.stdout.contains("\n\n"));
    let raw: Vec<&str> = run.stdout.lines().collect();
    for line in &raw[..3] {
        assert!(
            line.starts_with("{\"code\":\"test_result\",\"category\":\"test\","),
            "one compact object per line: {line}"
        );
    }
    let tail = raw[3];
    assert!(
        tail.starts_with(
            "{\"command\":\"test\",\"toolchain\":{\"version\":\"0.1.0\",\"edition\":\"0.1\",\"json_schema\":1},\"revision\":{\"commit\":null,"
        ),
        "{tail}"
    );
    assert_eq!(
        ordered::parse(tail).keys(),
        [
            "command",
            "toolchain",
            "revision",
            "status",
            "seed",
            "cases",
            "selected",
            "tested",
            "failed",
            "results",
            "receipt"
        ]
    );
    let json_run = cpl(&fx.reader, &["test", "--seed", "1", "--json"]);
    json_run.expect_code(0);
    let mut env: Value = serde_json::from_str(&no_durations(&json_run.stdout)).expect("JSON");
    let diagnostics = env
        .as_object_mut()
        .expect("an object")
        .remove("diagnostics")
        .expect("diagnostics");
    let tail_value: Value = serde_json::from_str(&no_durations(tail)).expect("JSON");
    assert_eq!(
        tail_value, env,
        "the tail is the envelope without diagnostics"
    );
    let streamed: Vec<Value> = raw[..3]
        .iter()
        .map(|line| serde_json::from_str(&no_durations(line)).expect("JSON"))
        .collect();
    assert_eq!(Value::Array(streamed), diagnostics);
}

#[test]
fn jsonl_test_streams_in_completion_order_while_json_sorts() {
    let fx = broken_reader("adv-jsonl-order");
    let run = cpl(&fx.reader, &["test", "--seed", "1", "--jsonl"]);
    run.expect_code(1);
    let lines = run.json_lines();
    let streamed = &lines[..lines.len() - 1];
    assert_eq!(
        streamed[0]["code"], "test_result",
        "the first selected test completes first (7.1)"
    );
    assert_eq!(streamed[0]["qualified_name"], READER_TESTS[0]);
    let results: Vec<&str> = streamed
        .iter()
        .filter(|d| d["code"] == "test_result")
        .map(|d| d["qualified_name"].as_str().expect("name"))
        .collect();
    assert_eq!(results, READER_TESTS);
    // A counterexample is streamed while its own test runs: after the
    // results of the tests before it, before those of the tests after it.
    let position = |name: &Value| READER_TESTS.iter().position(|t| name == *t);
    for (i, d) in streamed.iter().enumerate() {
        if d["code"] != "contract_counterexample" {
            continue;
        }
        let own = position(&d["found_by"]).expect("found_by names a selected test");
        for (j, other) in streamed.iter().enumerate() {
            if other["code"] == "test_result" {
                let k = position(&other["qualified_name"]).expect("a selected test");
                assert!(
                    (k < own && j < i) || (k > own && j > i) || k == own,
                    "line {i} ({d}) against line {j} ({other})"
                );
            }
        }
    }
    let (_, env) = jrun(&fx.reader, &["test", "--seed", "1"]);
    assert_eq!(
        env["diagnostics"][0]["code"], "contract_counterexample",
        "--json is in sorted order: the spanned counterexamples first (T46)"
    );
    let mut a: Vec<String> = streamed
        .iter()
        .map(|v| no_durations(&v.to_string()))
        .collect();
    let mut b: Vec<String> = env["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .map(|v| no_durations(&v.to_string()))
        .collect();
    a.sort();
    b.sort();
    assert_eq!(a, b, "the same diagnostics in both forms");
}

#[test]
fn jsonl_is_refused_outside_check_and_test() {
    let fx = Fixture::new("adv-jsonl-refused");
    let commands: [&[&str]; 3] = [
        &["run", "reader.pages.divide_exactly", "6", "3", "--jsonl"],
        &["fmt", "--check", "--jsonl"],
        &["id", "assign", "--check", "--jsonl"],
    ];
    for args in commands {
        let run = cpl(&fx.reader, args);
        run.expect_code(2);
        assert!(
            run.stdout.contains("usage_bad_argument") || run.stderr.contains("usage_bad_argument"),
            "{args:?}: --jsonl is check's and test's only (7.1)\n{}{}",
            run.stdout,
            run.stderr
        );
    }
}

// ===========================================================================
// 7.4 Exit codes
// ===========================================================================

#[test]
fn check_exit_codes() {
    let fx = Fixture::new("adv-exit-check");
    let warn = project("adv-exit-check-warn", &[("src/warn/main.cpl", WARN)]);
    let err = project("adv-exit-check-err", &[("src/err/main.cpl", ERR)]);
    let bad = project("adv-exit-check-bad", &[("cpl.toml", "[project\n")]);
    let (code, env) = jrun(&fx.reader, &["check"]);
    assert_eq!(code, 0);
    assert_eq!(kinds(&env), pairs(&[("info", "verification_result")]));
    let (code, env) = jrun(&warn, &["check"]);
    assert_eq!(code, 0, "warnings alone exit 0 (7.1): {env}");
    assert_eq!(kinds(&env), pairs(&[("warning", "unused_import")]));
    let (code, env) = jrun(&warn, &["check", "--deny-warnings"]);
    assert_eq!(code, 1, "--deny-warnings: exit 1 on a warning");
    assert_eq!(only(&env, "unused_import")["severity"], "warning");
    let (code, _) = jrun(&fx.holes, &["check", "--deny-warnings"]);
    assert_eq!(code, 0, "infos are not warnings");
    let (code, _) = jrun(&fx.reader, &["check", "--deny-warnings"]);
    assert_eq!(code, 0);
    let (code, env) = jrun(&err, &["check"]);
    assert_eq!(code, 1);
    assert!(kinds(&env).contains(&("error".to_string(), "type_mismatch".to_string())));
    let (code, env) = jrun(&bad, &["check"]);
    assert_eq!(code, 2);
    assert_eq!(codes(&env), ["project_manifest_invalid"]);
    for (dir, args, expected) in [
        (warn.as_path(), &["check"][..], 0),
        (warn.as_path(), &["check", "--deny-warnings"][..], 1),
        (err.as_path(), &["check"][..], 1),
        (bad.as_path(), &["check"][..], 2),
    ] {
        cpl(dir, args).expect_code(expected);
    }
}

#[test]
fn release_checks_with_only_warnings_exit_0() {
    let vals = vals_project("adv-release-vals");
    let (code, env) = jrun(&vals, &["check", "--release"]);
    assert_eq!(code, 0, "warnings alone exit 0 (7.1): {env}");
    assert_eq!(kinds(&env), pairs(&[("warning", "project_lock_missing")]));
    assert_eq!(env["release"], true);
    let (code, _) = jrun(&vals, &["check", "--release", "--deny-warnings"]);
    assert_eq!(code, 1);
    // `--lock` writes after a successful check: a warning does not stop
    // it, `--deny-warnings` does (7.2).
    let (code, _) = jrun(&vals, &["check", "--release", "--lock", "--deny-warnings"]);
    assert_eq!(code, 1);
    assert!(
        !vals.join("cpl.lock").exists(),
        "a failed check writes no lock"
    );
    let (code, _) = jrun(&vals, &["check", "--release", "--lock"]);
    assert_eq!(code, 0);
    assert!(vals.join("cpl.lock").is_file());
    let (code, env) = jrun(&vals, &["check", "--release"]);
    assert_eq!(
        (code, codes(&env)),
        (0, Vec::<String>::new()),
        "a fresh lock leaves nothing to report"
    );
    let th = project("adv-release-th", &[("src/th/main.cpl", TH)]);
    let (code, env) = jrun(&th, &["check"]);
    assert_eq!(code, 0);
    assert_eq!(kinds(&env), pairs(&[("info", "hole")]));
    let (code, env) = jrun(&th, &["check", "--release"]);
    assert_eq!(
        code, 0,
        "a hole in a test body stays `hole`, raised to a warning (7.2): {env}"
    );
    assert_eq!(
        kinds(&env),
        pairs(&[("warning", "hole"), ("warning", "project_lock_missing")])
    );
    let (code, _) = jrun(&th, &["check", "--release", "--deny-warnings"]);
    assert_eq!(code, 1);
}

#[test]
fn fmt_exit_codes() {
    let fx = Fixture::new("adv-exit-fmt");
    let loose = temp_dir("adv-exit-fmt-loose");
    put(&loose, "tidy.cpl", TIDY);
    put(&loose, "messy.cpl", MESSY);
    put(&loose, "broken.cpl", "module m\n\nfn f( {\n");
    let cases: Vec<(&Path, Vec<&str>, i32)> = vec![
        (fx.reader.as_path(), vec!["fmt", "--check"], 0),
        (fx.reader.as_path(), vec!["fmt"], 0),
        (loose.as_path(), vec!["fmt", "--check", "tidy.cpl"], 0),
        (loose.as_path(), vec!["fmt", "--check", "messy.cpl"], 1),
        (loose.as_path(), vec!["fmt", "--check", "broken.cpl"], 1),
        (loose.as_path(), vec!["fmt", "broken.cpl"], 1),
        (loose.as_path(), vec!["fmt", "--check", "missing.cpl"], 2),
        (loose.as_path(), vec!["fmt", "--check", "missing_dir"], 2),
        (fx.reader.as_path(), vec!["fmt", "cpl.toml"], 2),
        (
            loose.as_path(),
            vec!["fmt", "--stdin", "cpl", "tidy.cpl"],
            2,
        ),
    ];
    for (dir, args, expected) in cases {
        let (code, env) = jrun(dir, &args);
        assert_eq!(code, expected, "{args:?}: {env}");
    }
    assert_eq!(
        get(&loose, "broken.cpl"),
        "module m\n\nfn f( {\n",
        "left untouched"
    );
    // Formatting a file that needs it succeeds: "all files formatted".
    let (code, env) = jrun(&loose, &["fmt", "messy.cpl"]);
    assert_eq!(code, 0);
    assert_eq!(
        env["files"],
        json!([{ "path": "messy.cpl", "result": "formatted" }])
    );
    assert_eq!(get(&loose, "messy.cpl"), TIDY);
}

#[test]
fn run_exit_codes() {
    let fx = Fixture::new("adv-exit-run");
    let vals = vals_project("adv-exit-run-vals");
    let err = project("adv-exit-run-err", &[("src/err/main.cpl", ERR)]);
    let cases: Vec<(&Path, Vec<&str>, i32, &str)> = vec![
        (vals.as_path(), vec!["run", "vals.main.int", "1"], 0, ""),
        // An `Err(...)` is a value (7.2).
        (
            fx.reader.as_path(),
            vec!["run", "reader.pages.paginate_ints", "[1]", "0"],
            0,
            "",
        ),
        (
            fx.reader.as_path(),
            vec!["run", "reader.pages.divide_exactly", "7", "0"],
            1,
            "contract_violation",
        ),
        (
            vals.as_path(),
            vec!["run", "vals.main.pos", "0"],
            1,
            "refinement_violation",
        ),
        (err.as_path(), vec!["run", "err.main.f"], 1, "type_mismatch"),
        (
            fx.reader.as_path(),
            vec!["run", "reader.pages.nope"],
            2,
            "index_unknown_symbol",
        ),
        (
            fx.reader.as_path(),
            vec!["run", "reader.pages.Page"],
            2,
            "usage_not_a_function",
        ),
        (
            vals.as_path(),
            vec!["run", "vals.main.head", "[1]"],
            2,
            "cannot_run_generic",
        ),
        (
            vals.as_path(),
            vec!["run", "vals.main.read_clock"],
            2,
            "cannot_run_effectful",
        ),
        (
            vals.as_path(),
            vec!["run", "vals.main.int", "\"x\""],
            2,
            "usage_bad_argument",
        ),
        (
            vals.as_path(),
            vec!["run", "vals.main.int"],
            2,
            "usage_bad_argument",
        ),
        (
            vals.as_path(),
            vec!["run", "vals.main.int", "1 +"],
            2,
            "usage_bad_argument",
        ),
        (
            vals.as_path(),
            vec!["run", "vals.main.int", "--arg", "["],
            2,
            "usage_bad_argument",
        ),
    ];
    for (dir, args, expected, code) in cases {
        let (exit, env) = jrun(dir, &args);
        assert_eq!(exit, expected, "{args:?}: {env}");
        if !code.is_empty() {
            assert!(codes(&env).iter().any(|c| c == code), "{code} in {env}");
        }
    }
}

#[test]
fn test_exit_codes() {
    let fx = Fixture::new("adv-exit-test");
    let broken = broken_reader("adv-exit-test-broken");
    let err = project("adv-exit-test-err", &[("src/err/main.cpl", ERR)]);
    let th = project("adv-exit-test-th", &[("src/th/main.cpl", TH)]);
    let cases: Vec<(&Path, Vec<&str>, i32)> = vec![
        (fx.reader.as_path(), vec!["test", "--seed", "1"], 0),
        (broken.reader.as_path(), vec!["test", "--seed", "1"], 1),
        (fx.holes.as_path(), vec!["test"], 1),
        (
            fx.reader.as_path(),
            vec!["test", "--name", "no such test"],
            1,
        ),
        (err.as_path(), vec!["test"], 1),
        (th.as_path(), vec!["test", "--seed", "1"], 1),
        (fx.reader.as_path(), vec!["test", "Not.A.Module"], 2),
        (fx.reader.as_path(), vec!["test", "--seed", "x"], 2),
        (fx.reader.as_path(), vec!["test", "--frobnicate"], 2),
    ];
    for (dir, args, expected) in cases {
        let (code, env) = jrun(dir, &args);
        assert_eq!(code, expected, "{args:?}: {env}");
    }
    let (_, env) = jrun(&th, &["test", "--seed", "1"]);
    let record = &env["results"][0];
    assert_eq!(record["status"], "failed");
    assert_eq!(record["fault"]["code"], "unresolved_hole");
    assert_eq!(record["fault"]["hole"], "expected_value");
}

#[test]
fn id_assign_exit_codes() {
    let fx = Fixture::new("adv-exit-id");
    let (code, _) = jrun(&fx.reader, &["id", "assign", "--check"]);
    assert_eq!(code, 1, "--check found work");
    let (code, _) = jrun(&fx.reader, &["id", "assign"]);
    assert_eq!(code, 0, "every declaration has an id afterwards");
    let (code, env) = jrun(&fx.reader, &["id", "assign", "--check"]);
    assert_eq!((code, env["assignments"].clone()), (0, json!([])));
    put(&fx.reader, "src/reader/broken.cpl", BROKEN_SYNTAX);
    let (code, env) = jrun(&fx.reader, &["id", "assign"]);
    assert_eq!(code, 1, "a file skipped for syntax errors (4.2, 7.2)");
    assert_eq!(
        env["skipped_files"],
        json!([{ "path": "src/reader/broken.cpl", "reason": "syntax errors" }])
    );
    let (code, _) = jrun(&fx.reader, &["id", "assign", "--check"]);
    assert_eq!(code, 1);
    let (code, env) = jrun(&fx.reader, &["id", "assign", "no/such/file.cpl"]);
    assert_eq!(code, 2, "bad paths (7.4)");
    assert_eq!(codes(&env), ["usage_bad_argument"]);
}

// ===========================================================================
// Flags and their usage errors (7.2)
// ===========================================================================

#[test]
fn test_seed_takes_every_u64_and_nothing_else() {
    let dir = ord_project("adv-seed-range");
    for seed in ["0", "18446744073709551615"] {
        let run = cpl(&dir, &["test", "--seed", seed, "--json"]);
        run.expect_code(0);
        let env = run.json();
        assert_eq!(env["seed"].to_string(), seed, "reported exactly");
        assert!(
            run.stdout.contains(&format!("\n  \"seed\": {seed},\n")),
            "the run seed is a decimal number (1.7.2)"
        );
        let human = cpl(&dir, &["test", "--seed", seed]);
        human.expect_code(0);
        assert!(
            human.stdout.ends_with(&format!("; seed {seed}\n")),
            "{}",
            human.stdout
        );
    }
    for bad in [
        "18446744073709551616",
        "-1",
        "0x10",
        "1e3",
        "1.0",
        "",
        "one",
    ] {
        let (code, env) = jrun(&dir, &["test", "--seed", bad]);
        assert_eq!(code, 2, "--seed {bad:?}");
        assert_eq!(codes(&env), ["usage_bad_argument"], "--seed {bad:?}");
    }
}

#[test]
fn numeric_flags_reject_values_outside_their_types() {
    let fx = Fixture::new("adv-numeric-flags");
    let bad: [&[&str]; 9] = [
        &["test", "--cases", "4294967296"],
        &["test", "--cases", "-1"],
        &["test", "--max-shrinks", "4294967296"],
        &["test", "--max-shrinks", "-3"],
        &["test", "--timeout-ms", "18446744073709551616"],
        &["test", "--timeout-ms", "soon"],
        &[
            "run",
            "reader.pages.divide_exactly",
            "6",
            "3",
            "--max-call-depth",
            "-1",
        ],
        &[
            "run",
            "reader.pages.divide_exactly",
            "6",
            "3",
            "--max-call-depth",
            "4294967296",
        ],
        &[
            "run",
            "reader.pages.divide_exactly",
            "6",
            "3",
            "--max-call-depth",
            "deep",
        ],
    ];
    for args in bad {
        let (code, env) = jrun(&fx.reader, args);
        assert_eq!(code, 2, "{args:?}: {env}");
        assert_eq!(codes(&env), ["usage_bad_argument"], "{args:?}");
    }
}

#[test]
fn flags_without_values_and_values_without_flags_are_refused() {
    let fx = Fixture::new("adv-flag-values");
    let bad: [&[&str]; 15] = [
        &["check", "--release=yes"],
        &["check", "--deny-warnings=1"],
        &["check", "--lint"],
        &["check", "stray"],
        &["test", "--name"],
        &["test", "--id"],
        &["test", "--receipt"],
        &["test", "--fail-fast=true"],
        &["test", "tests", "extra"],
        &["run", "reader.pages.divide_exactly", "6", "--arg"],
        &["fmt", "--stdin"],
        &["fmt", "--check=no"],
        &["id", "assign", "--check=yes"],
        &["id"],
        &["id", "unassign"],
    ];
    for args in bad {
        let (code, env) = jrun(&fx.reader, args);
        assert_eq!(code, 2, "{args:?}: {env}");
        assert_eq!(codes(&env), ["usage_bad_argument"], "{args:?}");
    }
}

#[test]
fn fmt_stdin_kind_is_cpl_work_or_patch() {
    let dir = temp_dir("adv-stdin-kind");
    let read = |path: &str| {
        std::fs::read_to_string(workspace().join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
    };
    let work = read("examples/reader/work/pagination.work");
    let patch = read("examples/reader/work/pagination.patch");
    for (kind, text) in [
        ("cpl", TIDY),
        ("work", work.as_str()),
        ("patch", patch.as_str()),
    ] {
        let run = cpl_with_stdin(&dir, &["fmt", "--stdin", kind], Some(text));
        run.expect_code(0);
        assert_eq!(
            run.stdout, text,
            "{kind}: a formatted file comes back unchanged"
        );
    }
    for bad in ["CPL", "toml", "", "cpl,work"] {
        let run = cpl_with_stdin(
            &dir,
            &["--json", "fmt", "--stdin", bad, "--check"],
            Some(TIDY),
        );
        run.expect_code(2);
        let env = run.json();
        assert_envelope(&env, 2, &run.stdout);
        assert_eq!(codes(&env), ["usage_bad_argument"], "--stdin {bad:?}");
    }
    // A kind the text is not is a syntax error (exit 1), not a usage error.
    let run = cpl_with_stdin(&dir, &["fmt", "--stdin", "work"], Some(TIDY));
    run.expect_code(1);
    assert_eq!(run.stdout, "");
}

// ===========================================================================
// cpl fmt (7.2)
// ===========================================================================

#[test]
fn fmt_check_lists_every_file_that_would_change_and_writes_nothing() {
    let fx = Fixture::new("adv-fmt-list");
    let edits = [
        (
            "reader/src/reader/images.cpl",
            "pub struct Dimensions {",
            "pub struct Dimensions{",
        ),
        (
            "reader/src/reader/pages.cpl",
            "fn clean(page: Page) -> Page {",
            "fn clean( page:Page )->Page{",
        ),
        (
            "reader/work/pagination.work",
            "    workers 3\n",
            "    workers  3\n",
        ),
    ];
    let mut originals = Vec::new();
    for (path, from, to) in edits {
        let text = fx.read(path);
        assert!(text.contains(from), "{path}");
        fx.write(path, &text.replace(from, to));
        originals.push(text);
    }
    let deformatted: Vec<String> = edits.iter().map(|(p, _, _)| fx.read(p)).collect();
    let run = cpl(&fx.reader, &["fmt", "--check"]);
    run.expect_code(1);
    assert_eq!(
        run.stdout, "src/reader/images.cpl\nsrc/reader/pages.cpl\nwork/pagination.work\n",
        "one file per line on stdout (7.2)"
    );
    let now: Vec<String> = edits.iter().map(|(p, _, _)| fx.read(p)).collect();
    assert_eq!(now, deformatted, "--check writes nothing");
    let (code, env) = jrun(&fx.reader, &["fmt", "--check"]);
    assert_eq!(code, 1);
    let results: Vec<&str> = env["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|f| f["result"].as_str().expect("result"))
        .collect();
    assert_eq!(
        results,
        [
            "would_format",
            "would_format",
            "unchanged",
            "unchanged",
            "would_format"
        ]
    );
    let (code, env) = jrun(&fx.reader, &["fmt"]);
    assert_eq!(code, 0, "{env}");
    let results: Vec<&str> = env["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|f| f["result"].as_str().expect("result"))
        .collect();
    assert_eq!(
        results,
        [
            "formatted",
            "formatted",
            "unchanged",
            "unchanged",
            "formatted"
        ]
    );
    let now: Vec<String> = edits.iter().map(|(p, _, _)| fx.read(p)).collect();
    assert_eq!(
        now, originals,
        "formatting restores the formatted originals"
    );
}

#[test]
fn fmt_formats_the_other_files_when_one_does_not_parse() {
    let fx = Fixture::new("adv-fmt-partial");
    let pages = fx.read("reader/src/reader/pages.cpl");
    fx.write(
        "reader/src/reader/pages.cpl",
        &pages.replace(
            "fn clean(page: Page) -> Page {",
            "fn clean( page:Page )->Page{",
        ),
    );
    fx.write("reader/src/reader/broken.cpl", BROKEN_SYNTAX);
    let (code, env) = jrun(&fx.reader, &["fmt"]);
    assert_eq!(code, 1, "a syntax error makes the exit code 1 (7.2)");
    let result = |path: &str| {
        env["files"]
            .as_array()
            .expect("files")
            .iter()
            .find(|f| f["path"] == path)
            .unwrap_or_else(|| panic!("{path} in {env}"))["result"]
            .clone()
    };
    assert_eq!(result("src/reader/broken.cpl"), "error");
    assert_eq!(result("src/reader/pages.cpl"), "formatted");
    assert_eq!(fx.read("reader/src/reader/pages.cpl"), pages);
    assert_eq!(
        fx.read("reader/src/reader/broken.cpl"),
        BROKEN_SYNTAX,
        "left untouched"
    );
    let diagnostics = env["diagnostics"].as_array().expect("diagnostics");
    assert_eq!(
        diagnostics.len(),
        1,
        "the file's one parse diagnostic: {env}"
    );
    assert_eq!(diagnostics[0]["category"], "parse");
    assert_eq!(diagnostics[0]["span"]["file"], "src/reader/broken.cpl");
}

#[test]
fn fmt_and_id_assign_cover_exactly_the_source_set() {
    // Section 0: the source set is cpl.toml, the .cpl files under the
    // sources, and the .work and .patch files directly under work/; a file
    // named just `.cpl` is no source file.
    let fx = Fixture::new("adv-source-set");
    let deep_work = "change c {\n    base git(\"main\")\n    target git(\"main\")\n    workers  2\n    task t {\n        write {}\n    }\n}\n";
    put(&fx.reader, "work/deep/x.work", deep_work);
    put(&fx.reader, "scripts/x.cpl", MESSY);
    put(&fx.reader, "src/.cpl", MESSY);
    let paths = |env: &Value| -> Vec<String> {
        env["files"]
            .as_array()
            .expect("files")
            .iter()
            .map(|f| f["path"].as_str().expect("path").to_string())
            .collect()
    };
    let (code, env) = jrun(&fx.reader, &["fmt", "--check"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(paths(&env), READER_FILES);
    let (code, env) = jrun(&fx.reader, &["id", "assign", "--check"]);
    assert_eq!(code, 1);
    let assignments = env["assignments"].as_array().expect("assignments");
    assert_eq!(assignments.len(), 20, "{env}");
    // Named directories are walked recursively (7.2).
    let run = cpl(&fx.reader, &["fmt", "--check", "work"]);
    run.expect_code(1);
    assert_eq!(run.stdout, "work/deep/x.work\n");
    let (code, env) = jrun(&fx.reader, &["fmt", "--check", "src"]);
    assert_eq!(code, 0, "src/.cpl is no source file: {env}");
    assert_eq!(
        paths(&env),
        [
            "src/reader/images.cpl",
            "src/reader/pages.cpl",
            "src/tests/pagination.cpl"
        ]
    );
}

#[cfg(unix)]
#[test]
fn fmt_rewrites_only_files_whose_content_changes() {
    use std::os::unix::fs::PermissionsExt;
    let fx = Fixture::new("adv-fmt-readonly");
    let set = |mode: u32| {
        for path in READER_FILES {
            std::fs::set_permissions(fx.reader.join(path), std::fs::Permissions::from_mode(mode))
                .expect("set permissions");
        }
    };
    set(0o444);
    let (code, env) = jrun(&fx.reader, &["fmt"]);
    set(0o644);
    assert_eq!(code, 0, "an unchanged file is not rewritten (7.2): {env}");
    assert!(env["files"]
        .as_array()
        .expect("files")
        .iter()
        .all(|f| f["result"] == "unchanged"));
    assert_eq!(env["diagnostics"], json!([]));
}

#[test]
fn fmt_is_idempotent_and_never_touches_ids() {
    let messy = "module m\n\n@id(\"fn_cccccccccccccccc\")\nfn f( a:Int )->Int{\n  a\n}\n\nfn g()->Int{\n  1\n}\n";
    let tidy = "module m\n\n@id(\"fn_cccccccccccccccc\")\nfn f(a: Int) -> Int {\n    a\n}\n\nfn g() -> Int {\n    1\n}\n";
    let dir = project("adv-fmt-ids", &[("src/m.cpl", messy)]);
    let (code, env) = jrun(&dir, &["fmt"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(
        env["files"],
        json!([{ "path": "src/m.cpl", "result": "formatted" }])
    );
    assert_eq!(
        get(&dir, "src/m.cpl"),
        tidy,
        "fmt keeps the id and assigns none (T14)"
    );
    let (code, env) = jrun(&dir, &["fmt"]);
    assert_eq!(code, 0);
    assert_eq!(
        env["files"],
        json!([{ "path": "src/m.cpl", "result": "unchanged" }])
    );
    assert_eq!(get(&dir, "src/m.cpl"), tidy, "fmt(fmt(x)) == fmt(x)");
}

#[test]
fn fmt_stdin_formats_each_file_kind() {
    let dir = temp_dir("adv-stdin-kinds");
    let read = |path: &str| {
        std::fs::read_to_string(workspace().join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
    };
    let cases = [
        (
            "cpl",
            read("examples/reader/src/reader/pages.cpl"),
            "fn clean(page: Page) -> Page {",
            "fn clean( page:Page )->Page{",
        ),
        (
            "work",
            read("examples/reader/work/pagination.work"),
            "    workers 3\n",
            "    workers  3\n",
        ),
        (
            "patch",
            read("examples/reader/work/pagination.patch"),
            "        rest = rest.drop(size)\n",
            "        rest  =  rest.drop( size )\n",
        ),
    ];
    for (kind, original, from, to) in cases {
        let messy = original.replace(from, to);
        assert_ne!(messy, original);
        let run = cpl_with_stdin(&dir, &["fmt", "--stdin", kind], Some(&messy));
        run.expect_code(0);
        assert_eq!(run.stdout, original, "{kind}: the formatted text on stdout");
        let run = cpl_with_stdin(&dir, &["fmt", "--stdin", kind, "--check"], Some(&messy));
        run.expect_code(1);
        let first = original.lines().next().expect("a first line");
        assert!(
            !run.stdout.contains(first),
            "{kind}: --check writes no formatted text:\n{}",
            run.stdout
        );
        let run = cpl_with_stdin(&dir, &["fmt", "--stdin", kind, "--check"], Some(&original));
        run.expect_code(0);
        assert_eq!(run.stdout, "");
    }
}

#[test]
fn fmt_reports_invalid_utf8_and_leaves_the_file() {
    let dir = temp_dir("adv-fmt-utf8");
    let bytes: &[u8] = b"module m\n\xff\n";
    std::fs::write(dir.join("bad.cpl"), bytes).expect("write");
    let (code, env) = jrun(&dir, &["fmt", "bad.cpl"]);
    assert_eq!(code, 1, "{env}");
    assert_eq!(
        env["files"],
        json!([{ "path": "bad.cpl", "result": "error" }])
    );
    assert_eq!(codes(&env), ["invalid_utf8"]);
    let d = &env["diagnostics"][0];
    assert_eq!(d["category"], "parse");
    assert_eq!(
        d["message"], "invalid UTF-8 at byte 9",
        "1.4 SourceMap::add_bytes"
    );
    assert_eq!(
        d["span"],
        json!({ "file": "bad.cpl", "line": 2, "col": 1, "end_line": 2, "end_col": 2 })
    );
    assert_eq!(std::fs::read(dir.join("bad.cpl")).expect("read"), bytes);
}

// ===========================================================================
// cpl run: constant-expression arguments (7.2, semantics S39)
// ===========================================================================

#[test]
fn run_reports_constant_expression_arguments_as_value_json() {
    let fx = Fixture::new("adv-run-arguments");
    let (code, env) = jrun(
        &fx.reader,
        &["run", "reader.pages.paginate_ints", "[1, 2, 3]", "2"],
    );
    assert_eq!(code, 0, "{env}");
    assert_eq!(
        env["arguments"],
        json!([[1, 2, 3], 2]),
        "the RunOutput example of 7.2, which runs `paginate_ints \"[1, 2, 3]\" 2`"
    );
}

#[test]
fn run_arguments_are_the_same_whichever_form_spells_them() {
    let vals = vals_project("adv-run-two-spellings");
    let pairs: [(&str, &str, &str); 9] = [
        ("vals.main.int", "-5", "-5"),
        ("vals.main.ints", "[1, -2]", "[1, -2]"),
        ("vals.main.text", r#""a\tb""#, r#""a\tb""#),
        ("vals.main.unit", "Unit", r#"{"$type": "Unit"}"#),
        (
            "vals.main.tag",
            r#"Tag("p")"#,
            r#"{"$type": "vals.main.Tag", "$payload": ["p"]}"#,
        ),
        (
            "vals.main.pair",
            r#"Pair { left: 1, right: "r" }"#,
            r#"{"$type": "vals.main.Pair", "left": 1, "right": "r"}"#,
        ),
        (
            "vals.main.shape",
            r#"Both(2, "x")"#,
            r#"{"$type": "vals.main.Shape", "$variant": "Both", "$payload": [2, "x"]}"#,
        ),
        (
            "vals.main.nested",
            "Some(None)",
            r#"{"$type": "Option", "$variant": "Some", "$payload": [{"$type": "Option", "$variant": "None"}]}"#,
        ),
        (
            "vals.main.res",
            r#"Err("e")"#,
            r#"{"$type": "Result", "$variant": "Err", "$payload": ["e"]}"#,
        ),
    ];
    for (function, constant, value_json) in pairs {
        let expected: Value = serde_json::from_str(value_json).expect("JSON");
        let (code, by_json) = jrun(&vals, &["run", function, "--arg", value_json]);
        assert_eq!(code, 0, "{function} --arg {value_json}: {by_json}");
        assert_eq!(by_json["arguments"], json!([expected.clone()]));
        let (code, by_constant) = jrun(&vals, &["run", function, constant]);
        assert_eq!(code, 0, "{function} {constant}: {by_constant}");
        assert_eq!(by_constant["result"], by_json["result"]);
        assert_eq!(
            by_constant["arguments"],
            json!([expected]),
            "{function} {constant}: the same value spelled two ways (7.2)"
        );
    }
}

#[test]
fn run_reports_json_arguments_in_canonical_value_json() {
    let vals = vals_project("adv-run-canonical");
    let (code, env) = jrun(&vals, &["run", "vals.main.ints", "--arg", "[-0, 0, 7]"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(env["result"], json!([0, 0, 7]));
    assert_eq!(
        env["arguments"],
        json!([[0, 0, 7]]),
        "`-0` decodes to the Int 0, whose value JSON is `0` (7.3, S40)"
    );
    let (_, env) = jrun(&vals, &["run", "vals.main.int", "--arg", "  12  "]);
    assert_eq!(env["arguments"], json!([12]));
}

#[test]
fn run_accepts_every_constant_expression_form() {
    let vals = vals_project("adv-run-forms");
    let fx = Fixture::new("adv-run-forms-reader");
    let v = vals.as_path();
    let r = fx.reader.as_path();
    let cases: Vec<(&Path, &str, &str, &str)> = vec![
        (v, "vals.main.int", "42", "42"),
        (v, "vals.main.int", "-42", "-42"),
        (v, "vals.main.int", "0", "0"),
        (v, "vals.main.int", "1_000_000", "1000000"),
        (
            v,
            "vals.main.int",
            "123456789012345678901234567890",
            "123456789012345678901234567890",
        ),
        (v, "vals.main.int", "(((7)))", "7"),
        (v, "vals.main.flag", "true", "true"),
        (v, "vals.main.flag", "not true", "false"),
        (v, "vals.main.flag", "not (not false)", "false"),
        (v, "vals.main.text", r#""""#, r#""""#),
        (
            v,
            "vals.main.text",
            r#""tab\there \u{1F600}""#,
            "\"tab\\there 😀\"",
        ),
        (v, "vals.main.unit", "Unit", "Unit"),
        (v, "vals.main.ints", "[]", "[]"),
        (v, "vals.main.ints", "[1, -2, (3)]", "[1, -2, 3]"),
        (v, "vals.main.texts", r#"[["a"], []]"#, r#"[["a"], []]"#),
        (v, "vals.main.tag", r#"Tag("p")"#, r#"Tag("p")"#),
        (v, "vals.main.tag", r#"vals.main.Tag("p")"#, r#"Tag("p")"#),
        // The written path is a suffix of the declaration's (7.2).
        (v, "vals.main.tag", r#"main.Tag("p")"#, r#"Tag("p")"#),
        (
            v,
            "vals.main.pair",
            r#"Pair { left: 1, right: "r" }"#,
            r#"Pair { left: 1, right: "r" }"#,
        ),
        (
            v,
            "vals.main.pair",
            r#"Pair { right: "r", left: 1 }"#,
            r#"Pair { left: 1, right: "r" }"#,
        ),
        (
            v,
            "vals.main.pair",
            r#"vals.main.Pair { left: -1, right: "" }"#,
            r#"Pair { left: -1, right: "" }"#,
        ),
        (v, "vals.main.shape", "Dot", "Dot"),
        (v, "vals.main.shape", "Shape.Dot", "Dot"),
        (v, "vals.main.shape", "vals.main.Shape.Dot", "Dot"),
        (v, "vals.main.shape", r#"Both(-1, "x")"#, r#"Both(-1, "x")"#),
        (v, "vals.main.shape", "Circle(3)", "Circle(3)"),
        (v, "vals.main.nested", "None", "None"),
        (v, "vals.main.nested", "Some(None)", "Some(None)"),
        (v, "vals.main.nested", "Some(Some(-1))", "Some(Some(-1))"),
        (v, "vals.main.res", "Ok(1)", "Ok(1)"),
        (v, "vals.main.res", r#"Err("e")"#, r#"Err("e")"#),
        (
            v,
            "vals.main.ce",
            r#"ConstraintError { type_name: "T", message: "m" }"#,
            r#"ConstraintError { type_name: "T", message: "m" }"#,
        ),
        (
            v,
            "vals.main.fe",
            r#"ForeignError { kind: Timeout, message: "m", details: [ForeignDetail { key: "k", value: "v" }] }"#,
            "Timeout",
        ),
        (v, "vals.main.wrapped", "Wrapped(5)", "Wrapped(5)"),
        (
            r,
            "reader.pages.label",
            r#"Err(Missing(PageId("p")))"#,
            r#""Missing page""#,
        ),
        (
            r,
            "reader.pages.label",
            r#"Err(StoreError.Missing(PageId("p")))"#,
            r#""Missing page""#,
        ),
        (
            r,
            "reader.pages.label",
            "Err(pages.StoreError.Unavailable)",
            r#""Storage unavailable""#,
        ),
        (
            r,
            "reader.pages.label",
            "Err(reader.pages.StoreError.Unavailable)",
            r#""Storage unavailable""#,
        ),
        (
            r,
            "reader.pages.label",
            r#"Ok(Page { text: "hi", id: PageId("p") })"#,
            r#""hi""#,
        ),
        (
            r,
            "reader.pages.label",
            r#"Ok(reader.pages.Page { id: reader.pages.PageId("p"), text: "x" })"#,
            r#""x""#,
        ),
    ];
    for (dir, function, argument, text) in cases {
        let (code, env) = jrun(dir, &["run", function, argument]);
        assert_eq!(code, 0, "{function} {argument}: {env}");
        assert_eq!(env["result_text"], text, "{function} {argument}");
    }
    // Unary minus with a space and around parentheses (grammar D48),
    // passed after `--` so that they are not read as flags.
    for (argument, text) in [("- 5", "-5"), ("-(7)", "-7"), ("-(-5)", "5"), ("- -5", "5")] {
        let (code, env) = jrun(&vals, &["run", "vals.main.int", "--", argument]);
        assert_eq!(
            (code, env["result_text"].clone()),
            (0, json!(text)),
            "{argument}"
        );
    }
}

#[test]
fn run_constructor_qualifiers_must_be_suffixes_of_the_declaration_path() {
    let fx = Fixture::new("adv-run-qualifiers");
    let vals = vals_project("adv-run-qualifiers-vals");
    let cases: [(&Path, &str, &str); 8] = [
        (
            fx.reader.as_path(),
            "reader.pages.label",
            "Err(reader.pages.Unavailable)",
        ),
        (
            fx.reader.as_path(),
            "reader.pages.label",
            "Err(pages.Unavailable)",
        ),
        (
            fx.reader.as_path(),
            "reader.pages.label",
            "Err(Other.Unavailable)",
        ),
        (
            fx.reader.as_path(),
            "reader.pages.label",
            "Err(StoreErrors.Unavailable)",
        ),
        (
            fx.reader.as_path(),
            "reader.pages.label",
            r#"Ok(reader.Page { id: PageId("p"), text: "x" })"#,
        ),
        (vals.as_path(), "vals.main.tag", r#"other.Tag("p")"#),
        (vals.as_path(), "vals.main.tag", r#"vals.Tag("p")"#),
        (vals.as_path(), "vals.main.shape", "Main.Shape.Dot"),
    ];
    for (dir, function, argument) in cases {
        let (code, env) = jrun(dir, &["run", function, argument]);
        assert_eq!(code, 2, "{argument}: {env}");
        assert_eq!(codes(&env), ["usage_bad_argument"], "{argument}");
        assert_eq!(env["diagnostics"][0]["argument"], 0, "{argument}");
    }
}

#[test]
fn run_refuses_what_is_not_a_constant_expression() {
    let vals = vals_project("adv-run-not-constant");
    let cases: [(&str, &str); 29] = [
        ("vals.main.int", "1 + 2"),
        ("vals.main.int", "3 - 1"),
        ("vals.main.int", "2 * 3"),
        ("vals.main.int", "6 / 3"),
        ("vals.main.int", "7 % 2"),
        ("vals.main.flag", "1 == 1"),
        ("vals.main.flag", "1 != 2"),
        ("vals.main.flag", "1 < 2"),
        ("vals.main.flag", "1 <= 2"),
        ("vals.main.flag", "2 > 1"),
        ("vals.main.flag", "2 >= 1"),
        ("vals.main.flag", "true and false"),
        ("vals.main.flag", "true or false"),
        ("vals.main.ints", "[1] ++ [2]"),
        ("vals.main.text", r#""a" ++ "b""#),
        ("vals.main.int", "n"),
        ("vals.main.int", "answer()"),
        ("vals.main.int", "int(1)"),
        ("vals.main.int", "[1, 2].len"),
        ("vals.main.flag", "[1].is_empty()"),
        ("vals.main.int", "if true { 1 } else { 2 }"),
        ("vals.main.int", "?h"),
        ("vals.main.res", "Ok(1)?"),
        ("vals.main.pair", r#"Pair { ..p, right: "r" }"#),
        ("vals.main.flag", "forall x in [1]: x > 0"),
        ("vals.main.int", "{ 1 }"),
        ("vals.main.res", "check<Positive>(1)"),
        ("vals.main.ints", "[1, x]"),
        ("vals.main.pair", r#"Pair { left: 1 + 1, right: "r" }"#),
    ];
    for (function, argument) in cases {
        let (code, env) = jrun(&vals, &["run", function, argument]);
        assert_eq!(code, 2, "{argument}: {env}");
        assert_eq!(codes(&env), ["usage_bad_argument"], "{argument}");
        let d = &env["diagnostics"][0];
        assert_eq!(d["argument"], 0, "{argument}");
        assert_eq!(
            d["span"]["file"], "<virtual:argument 0>",
            "{argument}: spans the argument's own text (7.2): {d}"
        );
    }
}

#[test]
fn run_argument_diagnostics_span_the_argument_text() {
    let vals = vals_project("adv-run-arg-span");
    // `007` is a lexical error at columns 5-7 of the second argument's text.
    let (code, env) = jrun(&vals, &["run", "vals.main.add", "1", "[1, 007]"]);
    assert_eq!(code, 2, "{env}");
    let d = only(&env, "usage_bad_argument");
    assert_eq!(d["argument"], 1);
    assert_eq!(
        d["span"],
        json!({ "file": "<virtual:argument 1>", "line": 1, "col": 5, "end_line": 1, "end_col": 8 })
    );
    let run = cpl(&vals, &["run", "vals.main.add", "1", "[1, 007]"]);
    run.expect_code(2);
    assert_eq!(run.stdout, "");
    let lines = stable_lines(&run.stderr);
    assert_eq!(lines.len(), 1, "{}", run.stderr);
    assert!(
        lines[0].0.starts_with("error[usage_bad_argument]: "),
        "{lines:?}"
    );
    assert_eq!(
        lines[0].1.as_deref(),
        Some("  --> <virtual:argument 1>:1:5")
    );
}

#[test]
fn run_argument_indices_are_zero_based_across_both_forms() {
    let vals = vals_project("adv-run-arg-index");
    let cases: [(&[&str], u64); 4] = [
        (&["--arg", "1", "\"x\""], 1),
        (&["\"x\"", "--arg", "1"], 0),
        (&["1", "--arg", "\"x\""], 1),
        (&["--arg", "\"x\"", "2"], 0),
    ];
    for (args, index) in cases {
        let mut all = vec!["run", "vals.main.add"];
        all.extend_from_slice(args);
        let (code, env) = jrun(&vals, &all);
        assert_eq!(code, 2, "{args:?}: {env}");
        assert_eq!(
            only(&env, "usage_bad_argument")["argument"],
            index,
            "{args:?}"
        );
    }
    // For JSON, the pointer too (7.2).
    let (_, env) = jrun(&vals, &["run", "vals.main.add", "1", "--arg", "\"x\""]);
    assert_eq!(only(&env, "usage_bad_argument")["pointer"], "");
    // A refinement violation names the parameter of the failing argument.
    let (code, env) = jrun(&vals, &["run", "vals.main.two", "--arg", "1", "0"]);
    assert_eq!(code, 1, "{env}");
    assert_eq!(env["fault"]["parameter"], "b");
}

#[test]
fn run_refuses_ill_typed_constant_expressions() {
    let vals = vals_project("adv-run-ill-typed");
    let cases: [(&str, &str); 21] = [
        ("vals.main.int", r#""1""#),
        ("vals.main.int", "true"),
        ("vals.main.int", "Unit"),
        ("vals.main.int", "[1]"),
        ("vals.main.flag", "1"),
        ("vals.main.text", "1"),
        ("vals.main.ints", r#"[1, "a"]"#),
        ("vals.main.ints", "1"),
        ("vals.main.tag", r#""p""#),
        ("vals.main.tag", "Tag(1)"),
        ("vals.main.shape", "Circle"),
        ("vals.main.shape", "Dot(1)"),
        ("vals.main.shape", "Both(1)"),
        ("vals.main.shape", "Gone"),
        ("vals.main.pair", "Pair { left: 1 }"),
        (
            "vals.main.pair",
            r#"Pair { left: 1, right: "r", extra: 2 }"#,
        ),
        ("vals.main.pair", r#"Pair { left: 1, left: 2, right: "r" }"#),
        ("vals.main.nested", "Some(1)"),
        ("vals.main.res", r#"Ok("x")"#),
        ("vals.main.unit", "Unit(1)"),
        ("vals.main.pair", r#"Tag("p")"#),
    ];
    for (function, argument) in cases {
        let (code, env) = jrun(&vals, &["run", function, argument]);
        assert_eq!(code, 2, "{function} {argument}: {env}");
        assert_eq!(codes(&env), ["usage_bad_argument"], "{function} {argument}");
        assert_eq!(env["diagnostics"][0]["argument"], 0, "{argument}");
    }
}

#[test]
fn run_bytes_have_only_the_json_form() {
    let vals = vals_project("adv-run-bytes");
    for argument in [r#"Bytes("00")"#, r#""00""#, "[0]"] {
        let (code, env) = jrun(&vals, &["run", "vals.main.bytes", argument]);
        assert_eq!(code, 2, "{argument}: no constant-expression form (D8, 7.2)");
        assert_eq!(codes(&env), ["usage_bad_argument"]);
    }
    let bytes = r#"{"$type": "Bytes", "hex": "00ff"}"#;
    let (code, env) = jrun(&vals, &["run", "vals.main.bytes", "--arg", bytes]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(env["result"], json!({ "$type": "Bytes", "hex": "00ff" }));
    assert_eq!(env["result_text"], r#"Bytes("00ff")"#);
    let to_list = r#"{"$type": "Bytes", "hex": "00ff10"}"#;
    let (code, env) = jrun(&vals, &["run", "std.bytes.to_list", "--arg", to_list]);
    assert_eq!((code, env["result"].clone()), (0, json!([0, 255, 16])));
    let (code, env) = jrun(&vals, &["run", "std.bytes.from_list", "[0, 255, 16]"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(env["result"], json!({ "$type": "Bytes", "hex": "00ff10" }));
    assert_eq!(env["result_text"], r#"Bytes("00ff10")"#);
}

#[test]
fn run_refuses_nesting_beyond_the_limits_without_crashing() {
    let vals = vals_project("adv-run-deep");
    let n = 50_000;
    let lists = format!("{}{}", "[".repeat(n), "]".repeat(n));
    let parens = format!("{}1{}", "(".repeat(n), ")".repeat(n));
    let minus = format!("{}1", "-".repeat(n));
    let nots = format!("{}true", "not ".repeat(n / 4));
    let constants = [
        ("vals.main.ints", lists.as_str()),
        ("vals.main.int", parens.as_str()),
        ("vals.main.int", minus.as_str()),
        ("vals.main.flag", nots.as_str()),
    ];
    for (function, argument) in constants {
        let (code, env) = jrun(&vals, &["run", function, "--", argument]);
        assert_eq!(
            code, 2,
            "{function}: a nesting limit (grammar D73), not a crash"
        );
        assert_eq!(codes(&env), ["usage_bad_argument"], "{function}");
    }
    let deep_200 = format!("{}{}", "[".repeat(200), "]".repeat(200));
    let tree_140 = tree_json(70);
    let values = [
        ("vals.main.ints", lists.as_str()),
        ("vals.main.ints", deep_200.as_str()),
        ("vals.main.tree", tree_140.as_str()),
    ];
    for (function, argument) in values {
        let (code, env) = jrun(&vals, &["run", function, "--arg", argument]);
        assert_eq!(
            code, 2,
            "{function}: deeper than the JSON reader's 128 levels (7.2): {env}"
        );
        assert_eq!(codes(&env), ["usage_bad_argument"], "{function}");
        assert_eq!(env["diagnostics"][0]["argument"], 0);
    }
}

#[test]
fn run_decodes_json_nested_within_the_reader_limit() {
    let vals = vals_project("adv-run-deep-ok");
    // 60 trees are 120 levels of objects and arrays: within 128.
    let run = cpl(&vals, &["run", "vals.main.tree", "--arg", &tree_json(60)]);
    run.expect_code(0);
    assert!(
        run.stdout
            .starts_with("Tree { children: [Tree { children: ["),
        "{}",
        &run.stdout[..run.stdout.len().min(120)]
    );
    assert_eq!(run.stdout.matches("Tree {").count(), 60);
    assert_eq!(run.stdout.lines().count(), 1);
    // 100 nested arrays are within the limit: the refusal is the decoding
    // one, at the offending value.
    let nested = format!("{}{}", "[".repeat(100), "]".repeat(100));
    let (code, env) = jrun(&vals, &["run", "vals.main.ints", "--arg", &nested]);
    assert_eq!(code, 2);
    assert_eq!(
        only(&env, "usage_bad_argument")["pointer"],
        "/0",
        "an ill-typed element, not a nesting refusal"
    );
}

// ===========================================================================
// cpl run: value JSON (7.3, semantics S40)
// ===========================================================================

#[test]
fn run_value_json_round_trips_through_identity_functions() {
    let vals = vals_project("adv-run-value-json");
    let cases: [(&str, &str, &str); 26] = [
        ("vals.main.int", "0", "0"),
        ("vals.main.int", "-7", "-7"),
        (
            "vals.main.int",
            "123456789012345678901234567890123456789",
            "123456789012345678901234567890123456789",
        ),
        ("vals.main.flag", "true", "true"),
        ("vals.main.flag", "false", "false"),
        ("vals.main.text", r#""""#, r#""""#),
        (
            "vals.main.text",
            r#""a\"b\\c\n\r\t\u0001\u001f\u007f\u0080é😀""#,
            "\"a\\\"b\\\\c\\n\\r\\t\\u{1}\\u{1f}\\u{7f}\u{80}é😀\"",
        ),
        ("vals.main.text", r#""😀""#, "\"😀\""),
        ("vals.main.unit", r#"{"$type": "Unit"}"#, "Unit"),
        (
            "vals.main.bytes",
            r#"{"$type": "Bytes", "hex": ""}"#,
            r#"Bytes("")"#,
        ),
        (
            "vals.main.bytes",
            r#"{"$type": "Bytes", "hex": "00ff7f"}"#,
            r#"Bytes("00ff7f")"#,
        ),
        ("vals.main.ints", "[]", "[]"),
        ("vals.main.ints", "[1, -2, 3]", "[1, -2, 3]"),
        (
            "vals.main.texts",
            r#"[["a", "b"], []]"#,
            r#"[["a", "b"], []]"#,
        ),
        (
            "vals.main.tag",
            r#"{"$type": "vals.main.Tag", "$payload": ["p"]}"#,
            r#"Tag("p")"#,
        ),
        (
            "vals.main.pair",
            r#"{"$type": "vals.main.Pair", "left": 1, "right": "r"}"#,
            r#"Pair { left: 1, right: "r" }"#,
        ),
        (
            "vals.main.shape",
            r#"{"$type": "vals.main.Shape", "$variant": "Dot"}"#,
            "Dot",
        ),
        (
            "vals.main.shape",
            r#"{"$type": "vals.main.Shape", "$variant": "Circle", "$payload": [3]}"#,
            "Circle(3)",
        ),
        (
            "vals.main.shape",
            r#"{"$type": "vals.main.Shape", "$variant": "Both", "$payload": [-1, "x"]}"#,
            r#"Both(-1, "x")"#,
        ),
        (
            "vals.main.nested",
            r#"{"$type": "Option", "$variant": "None"}"#,
            "None",
        ),
        (
            "vals.main.nested",
            r#"{"$type": "Option", "$variant": "Some", "$payload": [{"$type": "Option", "$variant": "None"}]}"#,
            "Some(None)",
        ),
        (
            "vals.main.nested",
            r#"{"$type": "Option", "$variant": "Some", "$payload": [{"$type": "Option", "$variant": "Some", "$payload": [1]}]}"#,
            "Some(Some(1))",
        ),
        (
            "vals.main.res",
            r#"{"$type": "Result", "$variant": "Ok", "$payload": [1]}"#,
            "Ok(1)",
        ),
        (
            "vals.main.res",
            r#"{"$type": "Result", "$variant": "Err", "$payload": ["e"]}"#,
            r#"Err("e")"#,
        ),
        (
            "vals.main.ce",
            r#"{"$type": "ConstraintError", "type_name": "T", "message": "m"}"#,
            r#"ConstraintError { type_name: "T", message: "m" }"#,
        ),
        (
            "vals.main.wrapped",
            r#"{"$type": "vals.main.Wrapped", "$payload": [5]}"#,
            "Wrapped(5)",
        ),
    ];
    for (function, argument, text) in cases {
        let (code, env) = jrun(&vals, &["run", function, "--arg", argument]);
        assert_eq!(code, 0, "{function} {argument}: {env}");
        let expected: Value = serde_json::from_str(argument).expect("JSON");
        assert_eq!(env["result"], expected, "{function} {argument}");
        assert_eq!(env["result_text"], text, "{function} {argument}");
    }
    // A surrogate-pair escape is one scalar value (7.3).
    let pair = format!("\"\\{}d83d\\{}de00\"", 'u', 'u');
    let (code, env) = jrun(&vals, &["run", "vals.main.text", "--arg", &pair]);
    assert_eq!((code, env["result"].clone()), (0, json!("😀")), "{env}");
    assert_eq!(env["result_text"], "\"😀\"");
    // U+0000 is a minimal-digit braced escape in value text (S40).
    let nul = format!("\"\\{}0000x\"", 'u');
    let (code, env) = jrun(&vals, &["run", "vals.main.text", "--arg", &nul]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(env["result_text"], "\"\\u{0}x\"");
    // A std type's `$type` is its qualified name (T30).
    let error = r#"{"$type": "std.foreign.ForeignError", "kind": {"$type": "std.foreign.ForeignErrorKind", "$variant": "Timeout"}, "message": "m", "details": []}"#;
    let (code, env) = jrun(&vals, &["run", "vals.main.fe", "--arg", error]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(
        env["result"],
        json!({ "$type": "std.foreign.ForeignErrorKind", "$variant": "Timeout" })
    );
    assert_eq!(env["result_text"], "Timeout");
}

#[test]
fn run_value_json_decoding_is_strict() {
    let vals = vals_project("adv-run-strict");
    let bad_page = r#"[{"$type": "vals.main.Page", "id": {"$type": "vals.main.PageId", "$payload": ["a"]}, "text": "t"}, {"$type": "vals.main.Page", "id": {"$type": "vals.main.PageId", "$payload": [7]}, "text": "t"}]"#;
    let cases: [(&str, &str, Option<&str>); 46] = [
        ("vals.main.int", "1.0", Some("")),
        ("vals.main.int", "1e3", Some("")),
        ("vals.main.int", "-0.0", Some("")),
        ("vals.main.int", r#""1""#, Some("")),
        ("vals.main.int", "true", Some("")),
        ("vals.main.int", "null", Some("")),
        ("vals.main.ints", "[1, 2.5]", Some("/1")),
        ("vals.main.ints", r#"{"$type": "List"}"#, Some("")),
        ("vals.main.flag", "0", Some("")),
        ("vals.main.text", "7", Some("")),
        ("vals.main.text", r#""\ud800""#, None),
        ("vals.main.unit", "{}", None),
        ("vals.main.unit", r#""Unit""#, None),
        (
            "vals.main.unit",
            r#"{"$type": "Unit", "$variant": "Unit"}"#,
            None,
        ),
        ("vals.main.unit", r#"{"$type": "Unit", "extra": 1}"#, None),
        (
            "vals.main.bytes",
            r#"{"$type": "Bytes", "hex": "0F"}"#,
            None,
        ),
        (
            "vals.main.bytes",
            r#"{"$type": "Bytes", "hex": "abc"}"#,
            None,
        ),
        (
            "vals.main.bytes",
            r#"{"$type": "Bytes", "hex": "zz"}"#,
            None,
        ),
        ("vals.main.bytes", r#"{"$type": "Bytes"}"#, None),
        (
            "vals.main.bytes",
            r#"{"$type": "Bytes", "hex": "00", "extra": 1}"#,
            None,
        ),
        ("vals.main.bytes", r#"{"hex": "00"}"#, None),
        ("vals.main.bytes", r#""00""#, None),
        ("vals.main.tag", r#""p""#, Some("")),
        (
            "vals.main.tag",
            r#"{"$type": "vals.main.Tag", "$payload": "p"}"#,
            None,
        ),
        (
            "vals.main.tag",
            r#"{"$type": "vals.main.Tag", "$payload": []}"#,
            None,
        ),
        (
            "vals.main.tag",
            r#"{"$type": "vals.main.Tag", "$payload": ["p", "q"]}"#,
            None,
        ),
        (
            "vals.main.tag",
            r#"{"$type": "vals.main.Tag", "$payload": [1]}"#,
            Some("/$payload/0"),
        ),
        (
            "vals.main.pair",
            r#"{"$type": "vals.main.Pair", "left": 1}"#,
            None,
        ),
        (
            "vals.main.pair",
            r#"{"$type": "vals.main.Pair", "left": 1, "right": "r", "extra": 0}"#,
            None,
        ),
        (
            "vals.main.pair",
            r#"{"$type": "other.Pair", "left": 1, "right": "r"}"#,
            None,
        ),
        ("vals.main.pair", r#"{"left": 1, "right": "r"}"#, None),
        (
            "vals.main.pair",
            r#"{"$type": "vals.main.Pair", "left": "1", "right": "r"}"#,
            Some("/left"),
        ),
        (
            "vals.main.shape",
            r#"{"$type": "vals.main.Shape", "$variant": "Dot", "$payload": []}"#,
            None,
        ),
        (
            "vals.main.shape",
            r#"{"$type": "vals.main.Shape", "$variant": "Gone"}"#,
            None,
        ),
        ("vals.main.shape", r#"{"$type": "vals.main.Shape"}"#, None),
        (
            "vals.main.shape",
            r#"{"$type": "vals.main.Shape", "$variant": "Both", "$payload": [1]}"#,
            None,
        ),
        (
            "vals.main.shape",
            r#"{"$type": "vals.main.Shape", "$variant": "Both", "$payload": [1, 2]}"#,
            Some("/$payload/1"),
        ),
        (
            "vals.main.shape",
            r#"{"$type": "vals.main.Shape", "$variant": "Circle"}"#,
            None,
        ),
        ("vals.main.nested", "null", Some("")),
        (
            "vals.main.nested",
            r#"{"$type": "Option", "$variant": "Some", "$payload": [1]}"#,
            Some("/$payload/0"),
        ),
        (
            "vals.main.res",
            r#"{"$type": "Result", "$variant": "Ok"}"#,
            None,
        ),
        ("vals.main.pages", bad_page, Some("/1/id/$payload/0")),
        // T30: a project or std type is spelled by its qualified name, a
        // prelude type by its bare name; decoding takes no other spelling.
        (
            "vals.main.tag",
            r#"{"$type": "Tag", "$payload": ["p"]}"#,
            None,
        ),
        (
            "vals.main.fe",
            r#"{"$type": "ForeignError", "kind": {"$type": "std.foreign.ForeignErrorKind", "$variant": "Timeout"}, "message": "m", "details": []}"#,
            None,
        ),
        (
            "vals.main.ce",
            r#"{"$type": "std.prelude.ConstraintError", "type_name": "T", "message": "m"}"#,
            None,
        ),
        (
            "vals.main.nested",
            r#"{"$type": "Option", "$variant": "None", "$payload": null}"#,
            None,
        ),
    ];
    for (function, argument, pointer) in cases {
        let (code, env) = jrun(&vals, &["run", function, "--arg", argument]);
        assert_eq!(code, 2, "{function} --arg {argument}: {env}");
        assert_eq!(
            codes(&env),
            ["usage_bad_argument"],
            "{function} --arg {argument}"
        );
        let d = &env["diagnostics"][0];
        assert_eq!(d["argument"], 0, "{argument}");
        if let Some(pointer) = pointer {
            assert_eq!(
                d["pointer"], pointer,
                "{function} --arg {argument} (7.3 RFC 6901)"
            );
        }
    }
}

#[test]
fn run_refuses_duplicate_keys_at_any_depth() {
    let vals = vals_project("adv-run-duplicates");
    let page = |id: &str| {
        format!(
            r#"{{"$type": "vals.main.Page", "id": {{"$type": "vals.main.PageId", "$payload": ["{id}"]}}, "text": "t"}}"#
        )
    };
    let nested = format!(
        r#"[{}, {{"$type": "vals.main.Page", "id": {{"$type": "vals.main.PageId", "$payload": ["b"], "$type": "vals.main.PageId"}}, "text": "t"}}]"#,
        page("a")
    );
    // The key `$type` once more, its dollar sign written as a JSON escape.
    let escaped_key = format!(r#"{{"$type": "Unit", "\{}0024type": "Unit"}}"#, 'u');
    let cases: [(&str, &str); 5] = [
        ("vals.main.unit", r#"{"$type": "Unit", "$type": "Unit"}"#),
        ("vals.main.unit", escaped_key.as_str()),
        (
            "vals.main.tag",
            r#"{"$type": "vals.main.Tag", "$payload": ["p"], "$payload": ["p"]}"#,
        ),
        ("vals.main.pages", nested.as_str()),
        (
            "vals.main.pair",
            r#"{"$type": "vals.main.Pair", "left": 1, "left": 1, "right": "r"}"#,
        ),
    ];
    for (function, argument) in cases {
        let (code, env) = jrun(&vals, &["run", function, "--arg", argument]);
        assert_eq!(
            code, 2,
            "{argument}: a duplicate key is refused even with equal values (7.2)"
        );
        assert_eq!(codes(&env), ["usage_bad_argument"], "{argument}");
    }
    // Repeated keys in sibling objects are not duplicates.
    let siblings = format!("[{}, {}]", page("a"), page("b"));
    let (code, env) = jrun(&vals, &["run", "vals.main.pages", "--arg", &siblings]);
    assert_eq!((code, env["result"].clone()), (0, json!(2)));
}

// ===========================================================================
// cpl run: refinements, refusals, targets, human output
// ===========================================================================

/// One refinement-violation case: the function (in `vals.main`), its
/// arguments, and the expected `parameter`, `pointer`, `clause`,
/// `expected_type`, `actual`, and the header and parameter text that locate
/// the expected span.
struct Violation {
    function: &'static str,
    args: &'static [&'static str],
    parameter: &'static str,
    pointer: &'static str,
    clause: &'static str,
    expected_type: &'static str,
    actual: Value,
    header: &'static str,
    param: &'static str,
}

#[test]
fn run_refinement_violations_are_faults_naming_the_failing_position() {
    let vals = vals_project("adv-run-refinements");
    let positive = |function, args, parameter, pointer, actual, header, param| Violation {
        function,
        args,
        parameter,
        pointer,
        clause: "self > 0",
        expected_type: "Positive",
        actual,
        header,
        param,
    };
    let cases = vec![
        positive(
            "pos",
            &["0"],
            "n",
            "",
            json!(0),
            "pub fn pos(",
            "n: Positive",
        ),
        positive(
            "pos",
            &["--arg", "-3"],
            "n",
            "",
            json!(-3),
            "pub fn pos(",
            "n: Positive",
        ),
        // The first false predicate of the chain, root first (S39).
        Violation {
            function: "small",
            args: &["0"],
            parameter: "n",
            pointer: "",
            clause: "self > 0",
            expected_type: "Small",
            actual: json!(0),
            header: "pub fn small(",
            param: "n: Small",
        },
        Violation {
            function: "small",
            args: &["12"],
            parameter: "n",
            pointer: "",
            clause: "self < 10",
            expected_type: "Small",
            actual: json!(12),
            header: "pub fn small(",
            param: "n: Small",
        },
        Violation {
            function: "non_empty",
            args: &[r#""""#],
            parameter: "t",
            pointer: "",
            clause: "self.len > 0",
            expected_type: "NonEmpty",
            actual: json!(""),
            header: "pub fn non_empty(",
            param: "t: NonEmpty",
        },
        positive(
            "many",
            &["[1, 0, 2]"],
            "xs",
            "/1",
            json!(0),
            "pub fn many(",
            "xs: List<Positive>",
        ),
        positive(
            "boxed",
            &[r#"Boxed { n: 0, label: "x" }"#],
            "b",
            "/n",
            json!(0),
            "pub fn boxed(",
            "b: Boxed",
        ),
        positive(
            "boxed",
            &[
                "--arg",
                r#"{"$type": "vals.main.Boxed", "label": "x", "n": -1}"#,
            ],
            "b",
            "/n",
            json!(-1),
            "pub fn boxed(",
            "b: Boxed",
        ),
        positive(
            "shape",
            &["Circle(0)"],
            "s",
            "/$payload/0",
            json!(0),
            "pub fn shape(",
            "s: Shape",
        ),
        positive(
            "maybe",
            &["Some(0)"],
            "o",
            "/$payload/0",
            json!(0),
            "pub fn maybe(",
            "o: Option<Positive>",
        ),
        positive(
            "wrapped",
            &["Wrapped(0)"],
            "w",
            "/$payload/0",
            json!(0),
            "pub fn wrapped(",
            "w: Wrapped",
        ),
        positive(
            "two",
            &["1", "0"],
            "b",
            "",
            json!(0),
            "pub fn two(",
            "b: Positive",
        ),
    ];
    for case in cases {
        let target = format!("vals.main.{}", case.function);
        let mut all = vec!["run", target.as_str()];
        all.extend_from_slice(case.args);
        let (code, env) = jrun(&vals, &all);
        assert_eq!(
            code, 1,
            "{all:?}: a fault of the call (S39 (a), 7.2): {env}"
        );
        assert_eq!(env["result"], Value::Null);
        assert_eq!(env["result_text"], Value::Null);
        let fault = &env["fault"];
        assert_eq!(fault["code"], "refinement_violation", "{all:?}");
        assert_eq!(fault["category"], "runtime");
        assert_eq!(fault["severity"], "error");
        assert_eq!(fault["symbol"], target.as_str());
        assert_eq!(fault["qualified_name"], target.as_str());
        assert_eq!(fault["parameter"], case.parameter, "{all:?}");
        assert_eq!(fault["pointer"], case.pointer, "{all:?}");
        assert_eq!(fault["clause"], case.clause, "{all:?}");
        assert_eq!(fault["expected_type"], case.expected_type, "{all:?}");
        assert_eq!(fault["actual"], case.actual, "{all:?}");
        assert!(
            fault.get("backtrace").is_none(),
            "raised before any activation: no backtrace (1.7.3)"
        );
        let (line, col) = param_location(case.header, case.param);
        assert_eq!(fault["span"]["file"], "src/vals/main.cpl");
        assert_eq!(
            (
                fault["span"]["line"].as_u64(),
                fault["span"]["col"].as_u64()
            ),
            (Some(line), Some(col)),
            "{all:?}: spans the parameter (S39)"
        );
        assert!(env["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .contains(fault));
    }
}

#[test]
fn run_refined_arguments_that_hold_run() {
    let vals = vals_project("adv-run-refined-ok");
    let cases: [(&str, &[&str], &str); 9] = [
        ("vals.main.small", &["5"], "5"),
        ("vals.main.small", &["9"], "9"),
        ("vals.main.pos", &["1"], "1"),
        ("vals.main.many", &["[1, 2]"], "2"),
        ("vals.main.boxed", &[r#"Boxed { n: 1, label: "" }"#], "1"),
        ("vals.main.maybe", &["None"], "false"),
        ("vals.main.maybe", &["Some(4)"], "true"),
        ("vals.main.non_empty", &[r#""x""#], "1"),
        ("vals.main.two", &["--arg", "1", "--arg", "2"], "3"),
    ];
    for (function, args, text) in cases {
        let mut all = vec!["run", function];
        all.extend_from_slice(args);
        let (code, env) = jrun(&vals, &all);
        assert_eq!(
            (code, env["result_text"].clone()),
            (0, json!(text)),
            "{all:?}: {env}"
        );
    }
}

#[test]
fn run_a_refinement_predicate_that_faults_reports_that_fault() {
    // S39: "a predicate that faults is reported per S34": the fault itself,
    // with facet `contract`, the clause text, and the declaration whose
    // clause was being evaluated as its symbol.
    let source = "module pf.main\n\npub type Weird = Int where 10 / self > 1\n\npub fn weird(n: Weird) -> Int {\n    n\n}\n";
    let dir = project("adv-run-faulting-predicate", &[("src/pf/main.cpl", source)]);
    let (code, env) = jrun(&dir, &["run", "pf.main.weird", "0"]);
    assert_eq!(code, 1, "a fault of the run: {env}");
    let fault = &env["fault"];
    assert_eq!(fault["code"], "division_by_zero");
    assert_eq!(fault["facet"], "contract");
    assert_eq!(fault["clause"], "10 / self > 1");
    assert_eq!(fault["symbol"], "pf.main.Weird");
    assert_eq!(fault["qualified_name"], "pf.main.Weird");
    assert_eq!(env["result"], Value::Null);
    // A predicate that is merely false is the refinement_violation.
    let (code, env) = jrun(&dir, &["run", "pf.main.weird", "20"]);
    assert_eq!(code, 1);
    assert_eq!(env["fault"]["code"], "refinement_violation");
    assert_eq!(env["fault"]["clause"], "10 / self > 1");
    let (code, env) = jrun(&dir, &["run", "pf.main.weird", "3"]);
    assert_eq!((code, env["result_text"].clone()), (0, json!("3")));
}

#[test]
fn run_refuses_generic_and_effectful_functions_before_counting_arguments() {
    let vals = vals_project("adv-run-refusal-order");
    let arguments: [&[&str]; 4] = [&[], &["[1]"], &["1", "2", "3"], &["--arg", r#"{"x": 1}"#]];
    for args in arguments {
        let mut all = vec!["run", "vals.main.head"];
        all.extend_from_slice(args);
        let (code, env) = jrun(&vals, &all);
        assert_eq!(code, 2, "{all:?}");
        assert_eq!(
            codes(&env),
            ["cannot_run_generic"],
            "{all:?}: Generic is refused before ArgCount (1.7)"
        );
    }
    let (_, env) = jrun(&vals, &["run", "vals.main.head"]);
    let d = &env["diagnostics"][0];
    assert_eq!(d["category"], "resolve");
    assert_eq!(d["symbol"], "vals.main.head");
    assert_eq!(d["qualified_name"], "vals.main.head");
    let (line, col) = locate(VALS, "head<T>");
    assert_eq!(
        (d["span"]["line"].as_u64(), d["span"]["col"].as_u64()),
        (Some(line), Some(col)),
        "the declaration's name token (S63 (3))"
    );
    let notes: Vec<&str> = d["notes"]
        .as_array()
        .map(|n| n.iter().filter_map(|n| n["message"].as_str()).collect())
        .unwrap_or_default();
    assert!(
        notes.iter().any(|n| n.contains("vals.main.head_int")),
        "the note names a monomorphic wrapper (7.2): {notes:?}"
    );
    for args in [&[][..], &["1", "2"][..]] {
        let mut all = vec!["run", "vals.main.read_clock"];
        all.extend_from_slice(args);
        let (code, env) = jrun(&vals, &all);
        assert_eq!(code, 2, "{all:?}");
        assert_eq!(codes(&env), ["cannot_run_effectful"], "{all:?}");
        let d = &env["diagnostics"][0];
        assert_eq!(d["category"], "resolve");
        assert_eq!(d["symbol"], "vals.main.read_clock");
        let (line, col) = locate(VALS, "read_clock(");
        assert_eq!(
            (d["span"]["line"].as_u64(), d["span"]["col"].as_u64()),
            (Some(line), Some(col))
        );
    }
}

#[test]
fn run_refuses_declarations_that_are_not_functions() {
    let fx = Fixture::new("adv-run-not-fn");
    for target in [
        READER_TESTS[0],
        READER_TESTS[2],
        "reader.pages.PageRead",
        "reader.pages.Page",
        "reader.pages.StoreError",
        "reader.pages.Positive",
        "reader.pages.PageId",
        "reader.images.Dimensions",
    ] {
        let (code, env) = jrun(&fx.reader, &["run", target]);
        assert_eq!(code, 2, "{target}");
        assert_eq!(codes(&env), ["usage_not_a_function"], "{target}");
        assert_eq!(env["diagnostics"][0]["category"], "usage");
    }
    for target in [
        "reader.pages.nope",
        "nope.nope.nope",
        "reader.pages.Missing",
    ] {
        let (code, env) = jrun(&fx.reader, &["run", target]);
        assert_eq!(code, 2, "{target}");
        assert_eq!(codes(&env), ["index_unknown_symbol"], "{target}");
        assert_eq!(env["diagnostics"][0]["category"], "index");
    }
}

#[test]
fn run_resolves_an_at_id_target() {
    let vals = vals_project("adv-run-by-id");
    let (code, env) = jrun(&vals, &["run", "fn_aaaaaaaaaaaaaaaa", "41"]);
    assert_eq!(
        (code, env["result_text"].clone()),
        (0, json!("42")),
        "{env}"
    );
    assert_eq!(env["function"]["id"], "fn_aaaaaaaaaaaaaaaa");
    assert_eq!(env["function"]["path"], "vals.main.by_id");
    assert_eq!(env["function"]["provisional"], false);
    let (code, by_name) = jrun(&vals, &["run", "vals.main.by_id", "41"]);
    assert_eq!(code, 0);
    assert_eq!(by_name["function"], env["function"], "one SymbolVersion");
    let (_, env) = jrun(&vals, &["run", "vals.main.int", "1"]);
    assert_eq!(
        env["function"]["id"], "vals.main.int",
        "a provisional id (4.3)"
    );
    assert_eq!(env["function"]["provisional"], true);
}

#[test]
fn run_human_output_is_one_line_of_value_text() {
    let vals = vals_project("adv-run-human");
    let cases: [(&str, &str, &str); 5] = [
        ("vals.main.text", r#""a\nb""#, "\"a\\nb\""),
        (
            "vals.main.pair",
            r#"Pair { right: "r", left: 1 }"#,
            r#"Pair { left: 1, right: "r" }"#,
        ),
        ("vals.main.res", r#"Err("e")"#, r#"Err("e")"#),
        ("vals.main.unit", "Unit", "Unit"),
        ("vals.main.nested", "Some(Some(1))", "Some(Some(1))"),
    ];
    for (function, argument, text) in cases {
        let run = cpl(&vals, &["run", function, argument]);
        run.expect_code(0);
        assert_eq!(
            run.stdout,
            format!("{text}\n"),
            "one line of value text (7.2)"
        );
        assert_eq!(run.stderr, "");
    }
    let bytes = r#"{"$type": "Bytes", "hex": "0a"}"#;
    let run = cpl(&vals, &["run", "vals.main.bytes", "--arg", bytes]);
    run.expect_code(0);
    assert_eq!(run.stdout, "Bytes(\"0a\")\n");
    // A fault prints nothing on stdout; its stable lines go to stderr.
    let run = cpl(&vals, &["run", "vals.main.pos", "0"]);
    run.expect_code(1);
    assert_eq!(run.stdout, "");
    let lines = stable_lines(&run.stderr);
    assert_eq!(lines.len(), 1, "{}", run.stderr);
    assert!(lines[0].0.starts_with("error[refinement_violation]: "));
    let (line, col) = param_location("pub fn pos(", "n: Positive");
    assert_eq!(
        lines[0].1,
        Some(format!("  --> src/vals/main.cpl:{line}:{col}"))
    );
}

#[test]
fn run_and_test_still_run_when_the_check_has_only_warnings() {
    let dir = project("adv-warnings-only", &[("src/warn/main.cpl", WARN)]);
    let (code, env) = jrun(&dir, &["run", "warn.main.f"]);
    assert_eq!((code, env["result_text"].clone()), (0, json!("1")), "{env}");
    // 9.1: a warning is "reported" by cpl run and cpl test.
    assert_eq!(codes(&env), ["unused_import"]);
    let (code, env) = jrun(&dir, &["test", "--seed", "1"]);
    assert_eq!((code, env["tested"].clone()), (0, json!(1)), "{env}");
    assert_eq!(
        codes(&env),
        ["unused_import", "test_result"],
        "spanned first, then spanless (T46)"
    );
    let run = cpl(&dir, &["run", "warn.main.f"]);
    run.expect_code(0);
    assert_eq!(run.stdout, "1\n");
    assert!(
        run.stderr.starts_with("warning[unused_import]: "),
        "{}",
        run.stderr
    );
}

// ===========================================================================
// cpl test (7.2, 1.7.2, semantics S42-S46)
// ===========================================================================

#[test]
fn tests_run_in_the_byte_order_of_their_qualified_names() {
    let dir = ord_project("adv-test-order");
    let (code, env) = jrun(&dir, &["test", "--seed", "1"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(record_names(&env), ORD_ORDER);
    assert_eq!(
        env["results"][1]["symbol"], "test_zzzzzzzzzzzzzzzz",
        "ordered by qualified name, not by identity"
    );
    let diagnostic_names: Vec<&str> = with_code(&env, "test_result")
        .iter()
        .map(|d| d["qualified_name"].as_str().expect("name"))
        .collect();
    assert_eq!(
        diagnostic_names, ORD_ORDER,
        "spanless test_result diagnostics keep completion order (T46)"
    );
    let run = cpl(&dir, &["test", "--seed", "1"]);
    run.expect_code(0);
    let expected: String = ORD_ORDER
        .iter()
        .map(|name| format!("ok       {name}\n"))
        .chain(["test: 7 tested, 0 failed; seed 1\n".to_string()])
        .collect();
    assert_eq!(run.stdout, expected);
    let run = cpl(&dir, &["test", "--seed", "1", "--jsonl"]);
    run.expect_code(0);
    let lines = run.json_lines();
    let streamed: Vec<&str> = lines[..lines.len() - 1]
        .iter()
        .map(|d| d["qualified_name"].as_str().expect("name"))
        .collect();
    assert_eq!(streamed, ORD_ORDER);
}

#[test]
fn selectors_select_namespaces_segment_by_segment() {
    let dir = ord_project("adv-test-selectors");
    for (selector, expected) in [
        ("ord", 6),
        ("ord.*", 6),
        ("ord.main", 5),
        ("ord.main.*", 5),
        ("ord.deep", 1),
        ("ord.deep.*", 1),
        ("ord.deep.inner", 1),
        ("ordx", 1),
        ("ordx.*", 1),
    ] {
        let (code, env) = jrun(&dir, &["test", selector, "--seed", "1"]);
        assert_eq!(
            (code, env["selected"].clone()),
            (0, json!(expected)),
            "{selector}"
        );
    }
    for selector in ["ord.dee", "ord.main.a", "ordx.other.deeper", "orde"] {
        let (code, env) = jrun(&dir, &["test", selector, "--seed", "1"]);
        assert_eq!(
            (code, env["selected"].clone()),
            (1, json!(0)),
            "{selector}: namespaces match segment by segment (T43)"
        );
        assert_eq!(codes(&env), ["empty_test_selection"]);
    }
}

#[test]
fn malformed_selectors_are_usage_errors() {
    let dir = ord_project("adv-test-bad-selectors");
    for selector in [
        "ord.",
        ".ord",
        "ord..main",
        "ord.*.*",
        "*",
        "ord.main.\"a\"",
        "Ord",
        "ord.Main",
        "ord.test",
        "ord-main",
        "ord main",
        "",
    ] {
        let (code, env) = jrun(&dir, &["test", selector, "--seed", "1"]);
        assert_eq!(code, 2, "{selector:?}: {env}");
        assert_eq!(codes(&env), ["usage_bad_argument"], "{selector:?}");
    }
}

#[test]
fn name_filter_matches_the_decoded_name_exactly() {
    let dir = project("adv-test-names", &[("src/esc/main.cpl", ESC)]);
    let selected = |name: &str| -> (i32, Vec<String>) {
        let (code, env) = jrun(&dir, &["test", "--seed", "1", "--name", name]);
        (code, record_names(&env))
    };
    assert_eq!(
        selected("say \"hi\""),
        (0, vec!["esc.main.\"say \\\"hi\\\"\"".to_string()])
    );
    assert_eq!(
        selected("tab\there"),
        (0, vec!["esc.main.\"tab\\there\"".to_string()])
    );
    assert_eq!(selected("café"), (0, vec!["esc.main.\"café\"".to_string()]));
    for name in [
        "tab\\there",
        "caf\\u{e9}",
        "say",
        "SAY \"HI\"",
        "say \"hi\" ",
    ] {
        assert_eq!(
            selected(name),
            (1, vec![]),
            "{name:?}: exactly that name text"
        );
    }
    let run = cpl(&dir, &["test", "--seed", "1"]);
    run.expect_code(0);
    assert_eq!(
        run.stdout,
        "ok       esc.main.\"café\"\nok       esc.main.\"say \\\"hi\\\"\"\nok       esc.main.\"tab\\there\"\ntest: 3 tested, 0 failed; seed 1\n"
    );
}

#[test]
fn name_and_id_filters_apply_within_the_selector() {
    let dir = ord_project("adv-test-filters");
    for (selector, flag, value, expected) in [
        ("ord.main", "--name", "a", 1),
        ("ord.deep", "--name", "a", 0),
        ("ord", "--name", "inner", 1),
        ("ordx", "--name", "inner", 0),
        ("ord.main", "--id", "test_zzzzzzzzzzzzzzzz", 1),
        ("ord.deep", "--id", "test_zzzzzzzzzzzzzzzz", 0),
    ] {
        let (code, env) = jrun(&dir, &["test", selector, flag, value, "--seed", "1"]);
        assert_eq!(
            (code, env["selected"].clone()),
            (if expected == 0 { 1 } else { 0 }, json!(expected)),
            "{selector} {flag} {value}"
        );
    }
    // A name is unique within a module only: `--name` selects it in each.
    let same = |module: &str| format!("module {module}\n\ntest \"same\" {{\n    assert true\n}}\n");
    let dir = project(
        "adv-test-same-name",
        &[
            ("src/m/one.cpl", &same("m.one")),
            ("src/m/two.cpl", &same("m.two")),
        ],
    );
    let (code, env) = jrun(&dir, &["test", "--seed", "1", "--name", "same"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(record_names(&env), ["m.one.\"same\"", "m.two.\"same\""]);
    let (code, env) = jrun(&dir, &["test", "m.two", "--seed", "1", "--name", "same"]);
    assert_eq!(code, 0);
    assert_eq!(record_names(&env), ["m.two.\"same\""]);
}

#[test]
fn id_filter_selects_by_stable_identity() {
    let dir = ord_project("adv-test-ids");
    for (id, expected) in [
        ("test_zzzzzzzzzzzzzzzz", vec!["ord.main.\"a b\""]),
        ("prop_aaaaaaaaaaaaaaaa", vec!["ord.main.\"q\""]),
        ("ord.main.\"a\"", vec!["ord.main.\"a\""]),
        // An `@id`-annotated test's stable id is its `@id`, not its name.
        ("ord.main.\"a b\"", vec![]),
        ("fn_aaaaaaaaaaaaaaaa", vec![]),
    ] {
        let (code, env) = jrun(&dir, &["test", "--seed", "1", "--id", id]);
        assert_eq!(record_names(&env), expected, "--id {id}");
        assert_eq!(code, if expected.is_empty() { 1 } else { 0 }, "--id {id}");
    }
    for bad in ["Not An Id", "", "x"] {
        let (code, env) = jrun(&dir, &["test", "--id", bad]);
        assert_eq!(code, 2, "--id {bad:?}");
        assert_eq!(codes(&env), ["usage_bad_argument"]);
    }
}

#[test]
fn empty_selections_are_a_spanless_test_error() {
    let dir = ord_project("adv-test-empty");
    let filters: [&[&str]; 3] = [
        &["ord.dee"],
        &["--name", "zzz"],
        &["--id", "fn_aaaaaaaaaaaaaaaa"],
    ];
    for filter in filters {
        let mut args = vec!["test", "--seed", "1"];
        args.extend_from_slice(filter);
        let (code, env) = jrun(&dir, &args);
        assert_eq!(code, 1, "{filter:?}");
        let d = only(&env, "empty_test_selection");
        assert_eq!(d["category"], "test");
        assert_eq!(d["severity"], "error");
        assert!(d.get("span").is_none(), "spanless (S63 (6))");
        assert!(d.get("selector").is_some(), "the selector field (9.4)");
        assert_eq!(env["selected"], 0);
        assert_eq!(env["results"], json!([]));
        let run = cpl(&dir, &args);
        run.expect_code(1);
        assert_eq!(run.stdout, "test: 0 tested, 0 failed; seed 1\n");
    }
}

#[test]
fn property_seeds_derive_from_the_run_seed_and_the_property_identity() {
    let dir = ord_project("adv-test-property-seed");
    let mut seen = Vec::new();
    for seed in [0u64, 1, 2, u64::MAX] {
        let (code, env) = jrun(&dir, &["test", "--seed", &seed.to_string()]);
        assert_eq!(code, 0, "{env}");
        // The identity is the `@id` when present, else the qualified name.
        for (name, identity) in [
            ("ord.main.\"p\"", "ord.main.\"p\""),
            ("ord.main.\"q\"", "prop_aaaaaaaaaaaaaaaa"),
        ] {
            let property = record(&env, name);
            assert_eq!(property["seed"].as_u64(), Some(seed));
            let expected = cpl_eval::seed_hex(&cpl_eval::property_seed(seed, identity));
            assert_eq!(
                property["property_seed"],
                expected.as_str(),
                "blake3(run seed LE ++ identity) (S44)"
            );
            let d = with_code(&env, "test_result")
                .into_iter()
                .find(|d| d["qualified_name"] == name)
                .expect("a test_result");
            assert_eq!(d["property_seed"], property["property_seed"]);
            seen.push(expected);
        }
    }
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 8, "every seed and identity has its own stream");
}

#[test]
fn unseeded_runs_draw_and_report_an_os_seed() {
    let dir = ord_project("adv-test-os-seed");
    let (code, a) = jrun(&dir, &["test"]);
    assert_eq!(code, 0);
    let (_, b) = jrun(&dir, &["test"]);
    let seed_a = a["seed"].as_u64().expect("a decimal u64 seed");
    let seed_b = b["seed"].as_u64().expect("a decimal u64 seed");
    assert_ne!(
        seed_a, seed_b,
        "64 random bits per run, not a fixed default (S44)"
    );
    for (env, seed) in [(&a, seed_a), (&b, seed_b)] {
        let property = record(env, "ord.main.\"p\"");
        assert_eq!(property["seed"].as_u64(), Some(seed));
        let expected = cpl_eval::seed_hex(&cpl_eval::property_seed(seed, "ord.main.\"p\""));
        assert_eq!(property["property_seed"], expected.as_str());
    }
    let run = cpl(&dir, &["test"]);
    run.expect_code(0);
    let last = run.stdout.lines().last().expect("a summary line");
    let prefix = "test: 7 tested, 0 failed; seed ";
    assert!(last.starts_with(prefix), "{last}");
    assert!(last[prefix.len()..].parse::<u64>().is_ok(), "{last}");
}

#[test]
fn property_cases_do_not_depend_on_unrelated_tests() {
    let fx = broken_reader("adv-test-unrelated");
    let property = READER_TESTS[2];
    let (_, before) = jrun(&fx.reader, &["test", "--seed", "9"]);
    let before = record(&before, property).clone();
    assert_eq!(before["status"], "failed");
    let tests = fx.read("reader/src/tests/pagination.cpl");
    fx.write(
        "reader/src/tests/pagination.cpl",
        &format!("{tests}\ntest \"aaa added first\" {{\n    assert true\n}}\n"),
    );
    fx.write(
        "reader/src/tests/added.cpl",
        "module tests.added\n\ntest \"another\" {\n    assert 1 == 1\n}\n",
    );
    let (_, after) = jrun(&fx.reader, &["test", "--seed", "9"]);
    assert_eq!(after["selected"], 5);
    let after = record(&after, property);
    for key in ["property_seed", "cases", "failing_case", "shrunk"] {
        assert_eq!(after[key], before[key], "{key} (S44)");
    }
}

#[test]
fn more_cases_extend_the_case_sequence_of_fewer() {
    let fx = broken_reader("adv-test-cases-prefix");
    let property = |cases: &str| -> Value {
        let (_, env) = jrun(
            &fx.reader,
            &[
                "test",
                "--seed",
                "1",
                "--name",
                "pagination preserves input",
                "--cases",
                cases,
            ],
        );
        env["results"][0].clone()
    };
    let full = property("100");
    assert_eq!(full["status"], "failed");
    let index = full["failing_case"]["index"].as_u64().expect("index");
    let exact = property(&(index + 1).to_string());
    for key in ["status", "cases", "failing_case", "shrunk", "property_seed"] {
        assert_eq!(
            exact[key], full[key],
            "{key}: --cases changes no earlier case (1.7.2)"
        );
    }
    if index > 0 {
        let fewer = property(&index.to_string());
        assert_eq!(
            fewer["status"], "tested",
            "the first {index} cases passed before"
        );
        assert_eq!(fewer["cases"], index);
    }
}

#[test]
fn cases_flag_sets_the_case_count_of_every_property() {
    let dir = ord_project("adv-test-cases");
    for cases in [7u64, 1] {
        let (code, env) = jrun(
            &dir,
            &["test", "--seed", "1", "--cases", &cases.to_string()],
        );
        assert_eq!(code, 0, "{env}");
        assert_eq!(env["cases"], cases);
        for record in env["results"].as_array().expect("results") {
            if record["kind"] == "property" {
                assert_eq!(record["cases"], cases);
                assert_eq!(record["receipt"]["check_configuration"]["cases"], cases);
            } else {
                assert!(record.get("cases").is_none());
            }
        }
    }
}

#[test]
fn test_human_output_has_the_record_lines_and_the_summary() {
    let th = project("adv-test-human", &[("src/th/main.cpl", TH)]);
    let run = cpl(&th, &["test", "--seed", "1"]);
    run.expect_code(1);
    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(
        lines[0], "FAILED   th.main.\"unfinished\"",
        "{}",
        run.stdout
    );
    assert!(
        lines[1]
            .trim_start()
            .starts_with("error[unresolved_hole]: "),
        "the fault's line 1 (1.4.1): {}",
        run.stdout
    );
    assert!(
        lines[2].trim_start().starts_with("--> src/th/main.cpl:"),
        "the fault's line 2: {}",
        run.stdout
    );
    assert_eq!(
        lines.last().copied(),
        Some("test: 0 tested, 1 failed; seed 1")
    );
    assert!(!run.stderr.contains("[test_result]"));
}

// ===========================================================================
// cpl id assign (4.1, 4.2, 7.2)
// ===========================================================================

#[test]
fn id_assign_draws_ids_of_the_declaration_kind() {
    let fx = Fixture::new("adv-id-kinds");
    let (code, env) = jrun(&fx.reader, &["id", "assign", "--check"]);
    assert_eq!(code, 1);
    // Path order, then source order (4.2 step 2); the kind prefix of 4.1.
    let expected: [(&str, &str); 20] = [
        ("reader.images.Dimensions", "ty"),
        ("reader.images.ImageCodec", "cap"),
        ("reader.images.image_width", "fn"),
        ("reader.pages.PageId", "ty"),
        ("reader.pages.Positive", "ty"),
        ("reader.pages.Page", "ty"),
        ("reader.pages.StoreError", "ty"),
        ("reader.pages.clean", "fn"),
        ("reader.pages.label", "fn"),
        ("reader.pages.parse_size", "fn"),
        ("reader.pages.positive_size", "fn"),
        ("reader.pages.paginate_ints", "fn"),
        ("reader.pages.PageRead", "cap"),
        ("reader.pages.PageWrite", "cap"),
        ("reader.pages.load_page", "fn"),
        ("reader.pages.load_text", "fn"),
        ("reader.pages.divide_exactly", "fn"),
        (
            "tests.pagination.\"pagination preserves a short tail\"",
            "test",
        ),
        (
            "tests.pagination.\"pagination handles empty input\"",
            "test",
        ),
        ("tests.pagination.\"pagination preserves input\"", "prop"),
    ];
    let assignments = env["assignments"].as_array().expect("assignments");
    let found: Vec<(&str, &str)> = assignments
        .iter()
        .map(|a| {
            let id = a["id"].as_str().expect("id");
            (
                a["symbol"].as_str().expect("symbol"),
                id.split('_').next().unwrap_or(""),
            )
        })
        .collect();
    assert_eq!(found, expected);
    for a in assignments {
        let id = a["id"].as_str().expect("id");
        let kind = a["kind"].as_str().expect("kind");
        assert!(
            generated_id(id, kind),
            "{a}: the generated format with its kind"
        );
    }
    let mut ids: Vec<&str> = assignments
        .iter()
        .map(|a| a["id"].as_str().unwrap())
        .collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 20, "never an id twice");
    assert!(!ids.contains(&"pgn_7c2d"));
    let line_of = |symbol: &str| {
        assignments
            .iter()
            .find(|a| a["symbol"] == symbol)
            .and_then(|a| a["line"].as_u64())
    };
    assert_eq!(line_of("reader.pages.clean"), Some(21), "the 7.2 example");
    assert_eq!(line_of("reader.images.Dimensions"), Some(6));
    assert_eq!(line_of("reader.pages.divide_exactly"), Some(105));
    assert_eq!(line_of(READER_TESTS[2]), Some(16));
}

#[test]
fn id_assign_inserts_each_line_where_section_4_2_says() {
    let original = "module m.a\r\n\r\n/// Docs line.\r\n@prove\r\npub fn f(x: Int) -> Int\r\n    ensures result == x\r\n{\r\n    x\r\n}\r\n\r\n    fn g() -> Int {\r\n    1\r\n}\r\n\r\n@id(\"Foo\")\r\nfn h() -> Int {\r\n    2\r\n}\r\n\r\n/// A struct.\r\npub struct S {\r\n    v: Int\r\n}\r\n";
    let dir = project("adv-id-insert", &[("src/m/a.cpl", original)]);
    let (code, env) = jrun(&dir, &["id", "assign"]);
    assert_eq!(code, 0, "{env}");
    let assignments = env["assignments"].as_array().expect("assignments");
    let summary: Vec<(&str, u64, &str)> = assignments
        .iter()
        .map(|a| {
            (
                a["symbol"].as_str().unwrap(),
                a["line"].as_u64().unwrap(),
                a["kind"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [("m.a.f", 4, "fn"), ("m.a.g", 11, "fn"), ("m.a.S", 21, "ty")],
        "a malformed @id counts as one (4.2 step 2); `line` is the first annotation's, else the keyword's"
    );
    let id = |i: usize| assignments[i]["id"].as_str().expect("id");
    // At column 0 of the first annotation's line, else the keyword's, after
    // the doc comments; LF-terminated in a CRLF file; no other byte changes.
    let expected = original
        .replace(
            "/// Docs line.\r\n@prove\r\n",
            &format!("/// Docs line.\r\n@id(\"{}\")\n@prove\r\n", id(0)),
        )
        .replace(
            "\r\n    fn g()",
            &format!("\r\n@id(\"{}\")\n    fn g()", id(1)),
        )
        .replace(
            "/// A struct.\r\npub struct S",
            &format!("/// A struct.\r\n@id(\"{}\")\npub struct S", id(2)),
        );
    assert_eq!(get(&dir, "src/m/a.cpl"), expected);
    let (code, env) = jrun(&dir, &["id", "assign"]);
    assert_eq!(
        (code, env["assignments"].clone()),
        (0, json!([])),
        "idempotent"
    );
    assert_eq!(get(&dir, "src/m/a.cpl"), expected);
}

#[test]
fn id_assign_on_a_directory_covers_only_the_files_under_it() {
    let fx = Fixture::new("adv-id-dir");
    let tests_before = fx.read("reader/src/tests/pagination.cpl");
    let (code, env) = jrun(&fx.reader, &["id", "assign", "src/reader"]);
    assert_eq!(code, 0, "{env}");
    let assignments = env["assignments"].as_array().expect("assignments");
    assert_eq!(assignments.len(), 17);
    assert!(assignments.iter().all(|a| a["path"]
        .as_str()
        .is_some_and(|p| p.starts_with("src/reader/"))));
    assert_eq!(fx.read("reader/src/tests/pagination.cpl"), tests_before);
    let (code, env) = jrun(&fx.reader, &["id", "assign", "--check"]);
    assert_eq!(code, 1);
    let symbols: Vec<&str> = env["assignments"]
        .as_array()
        .expect("assignments")
        .iter()
        .map(|a| a["symbol"].as_str().unwrap())
        .collect();
    assert_eq!(
        symbols,
        [
            "tests.pagination.\"pagination preserves a short tail\"",
            "tests.pagination.\"pagination handles empty input\"",
            "tests.pagination.\"pagination preserves input\""
        ]
    );
    // A file named from a nested directory: just that file, project-relative.
    let (code, env) = jrun(
        &fx.reader.join("src/tests"),
        &["id", "assign", "pagination.cpl"],
    );
    assert_eq!(code, 0, "{env}");
    let paths: Vec<&str> = env["assignments"]
        .as_array()
        .expect("assignments")
        .iter()
        .map(|a| a["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["src/tests/pagination.cpl"; 3]);
    let (code, _) = jrun(&fx.reader, &["id", "assign", "--check"]);
    assert_eq!(code, 0);
}

#[test]
fn id_assign_human_lines_follow_section_7_2() {
    let fx = Fixture::new("adv-id-human");
    let (_, env) = jrun(&fx.reader, &["id", "assign", "--check"]);
    let expected: Vec<(String, u64, String)> = env["assignments"]
        .as_array()
        .expect("assignments")
        .iter()
        .map(|a| {
            (
                a["path"].as_str().unwrap().to_string(),
                a["line"].as_u64().unwrap(),
                a["symbol"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let run = cpl(&fx.reader, &["id", "assign", "--check"]);
    run.expect_code(1);
    assert_eq!(run.stderr, "");
    let mut seen = Vec::new();
    for line in run.stdout.lines() {
        // `src/reader/pages.cpl:21  @id("fn_k7w3qz2mb6ptx4ve")  reader.pages.clean`
        let mut parts = line.splitn(3, "  ");
        let place = parts.next().expect("place");
        let id = parts.next().unwrap_or_else(|| panic!("{line}"));
        let symbol = parts.next().unwrap_or_else(|| panic!("{line}"));
        let (path, number) = place.rsplit_once(':').unwrap_or_else(|| panic!("{line}"));
        let id = id
            .strip_prefix("@id(\"")
            .and_then(|rest| rest.strip_suffix("\")"))
            .unwrap_or_else(|| panic!("{line}"));
        let kind = id.split('_').next().unwrap_or("");
        assert!(generated_id(id, kind), "{line}");
        seen.push((
            path.to_string(),
            number
                .parse::<u64>()
                .unwrap_or_else(|e| panic!("{line}: {e}")),
            symbol.to_string(),
        ));
    }
    assert_eq!(seen, expected);
}

#[test]
fn id_assign_never_reuses_or_changes_an_id() {
    let dir = project(
        "adv-id-reuse",
        &[
            (
                "src/a.cpl",
                "module a\n\n@id(\"fn_aaaaaaaaaaaaaaaa\")\npub fn f() -> Int {\n    1\n}\n\npub fn g() -> Int {\n    2\n}\n",
            ),
            (
                "src/b.cpl",
                "module b\n\npub fn h() -> Int {\n    3\n}\n\npub fn k() -> Int {\n    4\n}\n",
            ),
        ],
    );
    let (code, env) = jrun(&dir, &["id", "assign"]);
    assert_eq!(code, 0, "{env}");
    let mut ids: Vec<&str> = env["assignments"]
        .as_array()
        .expect("assignments")
        .iter()
        .map(|a| a["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 3);
    assert!(!ids.contains(&"fn_aaaaaaaaaaaaaaaa"));
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 3);
    assert!(
        get(&dir, "src/a.cpl").starts_with("module a\n\n@id(\"fn_aaaaaaaaaaaaaaaa\")\npub fn f()"),
        "an existing id is never changed"
    );
    let (code, env) = jrun(&dir, &["check", "--lint", "id_missing"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(
        codes(&env),
        Vec::<String>::new(),
        "no duplicate and no missing id"
    );
}

// ===========================================================================
// 1.4.1 Human output
// ===========================================================================

#[test]
fn human_diagnostics_have_the_two_stable_lines() {
    let manifest_text = format!("{}colour = \"blue\"\n", manifest("one"));
    let dir = project(
        "adv-human-lines",
        &[
            ("cpl.toml", manifest_text.as_str()),
            ("src/one/main.cpl", ONE),
        ],
    );
    let (code, env) = jrun(&dir, &["check"]);
    assert_eq!(code, 1);
    let run = cpl(&dir, &["check"]);
    run.expect_code(1);
    let lines = stable_lines(&run.stderr);
    assert_eq!(
        lines,
        expected_lines(env["diagnostics"].as_array().expect("diagnostics")),
        "{}",
        run.stderr
    );
    for prefix in [
        "error[type_mismatch]: ",
        "warning[unused_binding]: ",
        "info[hole]: ",
    ] {
        let found = lines
            .iter()
            .find(|(first, _)| first.starts_with(prefix))
            .unwrap_or_else(|| panic!("{prefix} in {lines:?}"));
        assert!(
            found
                .1
                .as_ref()
                .is_some_and(|l| l.starts_with("  --> src/one/main.cpl:")),
            "{found:?}"
        );
    }
    let unknown = lines
        .iter()
        .find(|(first, _)| first.starts_with("error[project_manifest_unknown_key]: "))
        .expect("the unknown key");
    assert_eq!(unknown.1, None, "a spanless diagnostic has no line 2");
}

#[test]
fn human_check_summary_agrees_with_the_json_summary() {
    let fx = Fixture::new("adv-human-summary");
    let one = project("adv-human-summary-one", &[("src/one/main.cpl", ONE)]);
    let sorted = sorted_project("adv-human-summary-sorted");
    let vals = vals_project("adv-human-summary-vals");
    let cases: [(&Path, &[&str]); 7] = [
        (fx.reader.as_path(), &["check"]),
        (fx.holes.as_path(), &["check"]),
        (fx.holes.as_path(), &["check", "--release"]),
        (one.as_path(), &["check"]),
        (sorted.as_path(), &["check"]),
        (vals.as_path(), &["check"]),
        (vals.as_path(), &["check", "--release"]),
    ];
    for (dir, args) in cases {
        let (code, env) = jrun(dir, args);
        let run = cpl(dir, args);
        run.expect_code(code);
        assert_eq!(
            run.stdout,
            summary_line(&env["summary"]),
            "{} {args:?}",
            dir.display()
        );
    }
}

// ===========================================================================
// 7.1 Determinism
// ===========================================================================

#[test]
fn repeated_runs_print_byte_identical_output_apart_from_durations() {
    let fx = Fixture::new("adv-repeat");
    let broken = broken_reader("adv-repeat-broken");
    let vals = vals_project("adv-repeat-vals");
    let cases: Vec<(&Path, Vec<&str>)> = vec![
        (fx.reader.as_path(), vec!["check"]),
        (fx.reader.as_path(), vec!["check", "--json"]),
        (fx.holes.as_path(), vec!["check", "--jsonl"]),
        (fx.holes.as_path(), vec!["check", "--release"]),
        (fx.reader.as_path(), vec!["test", "--seed", "11"]),
        (broken.reader.as_path(), vec!["test", "--seed", "11"]),
        (
            broken.reader.as_path(),
            vec!["test", "--seed", "11", "--json"],
        ),
        (
            broken.reader.as_path(),
            vec!["test", "--seed", "11", "--jsonl"],
        ),
        (
            fx.reader.as_path(),
            vec!["run", "reader.pages.paginate_ints", "[1, 2, 3, 4, 5]", "2"],
        ),
        (
            fx.reader.as_path(),
            vec!["run", "reader.pages.divide_exactly", "7", "0"],
        ),
        (vals.as_path(), vec!["run", "vals.main.pos", "0", "--json"]),
        (vals.as_path(), vec!["run", "vals.main.int", "1 + 1"]),
        (fx.reader.as_path(), vec!["fmt", "--check", "--json"]),
    ];
    for (dir, args) in cases {
        let a = cpl(dir, &args);
        let b = cpl(dir, &args);
        assert_eq!(a.code, b.code, "{args:?}");
        assert_eq!(no_durations(&a.stdout), no_durations(&b.stdout), "{args:?}");
        assert_eq!(a.stderr, b.stderr, "{args:?}");
    }
}

#[test]
fn outputs_do_not_depend_on_where_the_project_lives() {
    let a = Fixture::new("adv-where-a");
    let b = Fixture::new("adv-where-b-elsewhere");
    let broken_a = broken_reader("adv-where-broken-a");
    let broken_b = broken_reader("adv-where-broken-b-elsewhere");
    let page = "Page { id: PageId(\"p\"), text: \"t\" }";
    let cases: Vec<(&Path, &Path, Vec<&str>)> = vec![
        (
            a.reader.as_path(),
            b.reader.as_path(),
            vec!["check", "--json"],
        ),
        (a.reader.as_path(), b.reader.as_path(), vec!["check"]),
        (
            a.holes.as_path(),
            b.holes.as_path(),
            vec!["check", "--json"],
        ),
        (
            a.holes.as_path(),
            b.holes.as_path(),
            vec!["check", "--release", "--jsonl"],
        ),
        (
            a.reader.as_path(),
            b.reader.as_path(),
            vec!["test", "--seed", "3", "--json"],
        ),
        (
            a.reader.as_path(),
            b.reader.as_path(),
            vec!["test", "--seed", "3"],
        ),
        (
            broken_a.reader.as_path(),
            broken_b.reader.as_path(),
            vec!["test", "--seed", "3", "--json"],
        ),
        (
            broken_a.reader.as_path(),
            broken_b.reader.as_path(),
            vec!["test", "--seed", "3"],
        ),
        (
            a.reader.as_path(),
            b.reader.as_path(),
            vec![
                "run",
                "reader.pages.paginate_ints",
                "[1, 2, 3]",
                "2",
                "--json",
            ],
        ),
        (
            a.reader.as_path(),
            b.reader.as_path(),
            vec!["run", "reader.pages.divide_exactly", "7", "0"],
        ),
        (
            a.reader.as_path(),
            b.reader.as_path(),
            vec!["fmt", "--check", "--json"],
        ),
        (
            a.holes.as_path(),
            b.holes.as_path(),
            vec!["run", "holes.pages.summarize", page, "--json"],
        ),
    ];
    for (here, there, args) in cases {
        let x = cpl(here, &args);
        let y = cpl(there, &args);
        assert_eq!(x.code, y.code, "{args:?}");
        assert_eq!(
            no_durations(&x.stdout),
            no_durations(&y.stdout),
            "{args:?}: the same source set prints the same bytes (7.1)"
        );
        assert_eq!(x.stderr, y.stderr, "{args:?}");
    }
}

// ===========================================================================
// Failures outside the command line: revisions, unreadable input, I/O
// ===========================================================================

#[test]
fn argument_errors_in_a_project_identify_its_revision() {
    // 1.4.2: the envelope's `revision` is None only "when no project is
    // involved"; design line 245: every response identifies the source
    // revision. An argument the command itself refuses (a command line clap
    // parsed) comes from a command running in a project, so the envelope
    // names the project's revision whichever argument check refused it.
    let vals = vals_project("adv-revision-usage");
    let cases: [&[&str]; 7] = [
        &["run", "vals.main.int", "\"x\""],
        &["run", "vals.main.int", "1", "2"],
        &["run", "vals.main.int", "--arg", r#"{"a": 1, "a": 1}"#],
        &["run", "vals.main.int", "--arg", "["],
        &["test", "Not.A.Module"],
        &["test", "--id", "Not An Id"],
        &["check", "--lint", "bogus"],
    ];
    for args in cases {
        let (code, env) = jrun(&vals, args);
        assert_eq!(code, 2, "{args:?}");
        assert_eq!(codes(&env), ["usage_bad_argument"], "{args:?}");
        assert!(
            env["revision"].is_object(),
            "{args:?}: the project's revision, as for every response in a project: {env}"
        );
    }
}

#[cfg(unix)]
#[test]
fn an_unreadable_source_file_exits_2() {
    use std::os::unix::fs::PermissionsExt;
    let fx = Fixture::new("adv-unreadable");
    let file = fx.reader.join("src/reader/images.cpl");
    let mode = |m: u32| {
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(m)).expect("chmod")
    };
    mode(0o000);
    if std::fs::read(&file).is_ok() {
        mode(0o644);
        eprintln!("skipped: permissions do not stop this user from reading");
        return;
    }
    let commands: [&[&str]; 5] = [
        &["check"],
        &["test", "--seed", "1"],
        &["run", "reader.pages.divide_exactly", "6", "3"],
        &["fmt", "--check"],
        &["id", "assign", "--check"],
    ];
    let outcomes: Vec<(i32, Value)> = commands.iter().map(|args| jrun(&fx.reader, args)).collect();
    mode(0o644);
    for (args, (code, env)) in commands.iter().zip(outcomes) {
        assert_eq!(code, 2, "{args:?}: unreadable input (7.1, 7.4): {env}");
    }
}

#[test]
fn a_test_receipt_that_cannot_be_written_exits_2() {
    let fx = Fixture::new("adv-receipt-unwritable");
    let missing = fx.root.join("no/such/directory/receipt.json");
    let path = missing.to_str().expect("a UTF-8 path");
    let (code, env) = jrun(&fx.reader, &["test", "--seed", "1", "--receipt", path]);
    assert_eq!(
        code, 2,
        "the receipt the flag asks for cannot be written (7.1): {env}"
    );
    assert!(
        env["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .iter()
            .any(|d| d["severity"] == "error"),
        "{env}"
    );
    assert!(!missing.exists());
}

#[test]
fn check_reports_a_source_that_is_not_utf8_as_a_parse_error() {
    let fx = Fixture::new("adv-check-utf8");
    std::fs::write(
        fx.reader.join("src/reader/bad.cpl"),
        b"module reader.bad\n\xff\n",
    )
    .expect("write");
    let (code, env) = jrun(&fx.reader, &["check"]);
    assert_eq!(
        code, 1,
        "a parse diagnostic of the file, not a load failure (1.4, S52): {env}"
    );
    let d = only(&env, "invalid_utf8");
    assert_eq!(d["category"], "parse");
    assert_eq!(d["message"], "invalid UTF-8 at byte 18");
    assert_eq!(
        d["span"],
        json!({ "file": "src/reader/bad.cpl", "line": 2, "col": 1, "end_line": 2, "end_col": 2 })
    );
    let run = cpl(&fx.reader, &["check"]);
    run.expect_code(1);
    assert!(
        run.stderr.starts_with(
            "error[invalid_utf8]: invalid UTF-8 at byte 18\n  --> src/reader/bad.cpl:2:1\n"
        ),
        "{}",
        run.stderr
    );
}

#[test]
fn jsonl_frames_a_load_failure_as_one_diagnostic_then_the_tail() {
    let bad = project("adv-jsonl-load-failure", &[("cpl.toml", "[project\n")]);
    for command in ["check", "test"] {
        let run = cpl(&bad, &[command, "--jsonl"]);
        run.expect_code(2);
        let lines = run.json_lines();
        assert_eq!(lines.len(), 2, "{command}:\n{}", run.stdout);
        assert_eq!(lines[0]["code"], "project_manifest_invalid");
        assert_eq!(lines[1]["command"], command);
        assert_eq!(lines[1]["status"], "error");
        assert_eq!(lines[1]["revision"], Value::Null);
        assert!(lines[1].get("diagnostics").is_none());
    }
}

// ===========================================================================
// An order-preserving JSON reader
// ===========================================================================

/// A JSON reader that keeps object keys in written order, for the key-order
/// rules of Section 7.1 and Appendix C (`serde_json::Value` sorts them).
mod ordered {
    #[derive(Clone, Debug, PartialEq)]
    pub enum J {
        Null,
        Bool(bool),
        Num(String),
        Str(String),
        Arr(Vec<J>),
        Obj(Vec<(String, J)>),
    }

    impl J {
        /// The keys of an object, in written order.
        #[track_caller]
        pub fn keys(&self) -> Vec<&str> {
            match self {
                J::Obj(members) => members.iter().map(|(k, _)| k.as_str()).collect(),
                other => panic!("not an object: {other:?}"),
            }
        }

        /// The member `key` of an object.
        #[track_caller]
        pub fn get(&self, key: &str) -> &J {
            self.opt(key)
                .unwrap_or_else(|| panic!("no key {key:?} in {self:?}"))
        }

        /// The member `key` of an object, if present.
        #[track_caller]
        pub fn opt(&self, key: &str) -> Option<&J> {
            match self {
                J::Obj(members) => members.iter().find(|(k, _)| k == key).map(|(_, v)| v),
                other => panic!("not an object: {other:?}"),
            }
        }

        /// The elements of an array.
        #[track_caller]
        pub fn items(&self) -> &[J] {
            match self {
                J::Arr(items) => items,
                other => panic!("not an array: {other:?}"),
            }
        }
    }

    /// Parse one JSON value (the whole text).
    #[track_caller]
    pub fn parse(text: &str) -> J {
        let mut parser = Parser {
            bytes: text.as_bytes(),
            at: 0,
        };
        let value = parser.value();
        parser.space();
        assert_eq!(
            parser.at,
            parser.bytes.len(),
            "trailing text after the JSON value"
        );
        value
    }

    struct Parser<'a> {
        bytes: &'a [u8],
        at: usize,
    }

    impl Parser<'_> {
        fn peek(&self) -> u8 {
            self.bytes[self.at]
        }

        fn space(&mut self) {
            while self.at < self.bytes.len()
                && matches!(self.bytes[self.at], b' ' | b'\t' | b'\n' | b'\r')
            {
                self.at += 1;
            }
        }

        fn eat(&mut self, byte: u8) {
            assert_eq!(self.peek(), byte, "at byte {}", self.at);
            self.at += 1;
        }

        fn word(&mut self, word: &str) {
            assert!(
                self.bytes[self.at..].starts_with(word.as_bytes()),
                "at byte {}",
                self.at
            );
            self.at += word.len();
        }

        fn value(&mut self) -> J {
            self.space();
            match self.peek() {
                b'{' => {
                    self.at += 1;
                    let mut members = Vec::new();
                    self.space();
                    if self.peek() == b'}' {
                        self.at += 1;
                        return J::Obj(members);
                    }
                    loop {
                        self.space();
                        let key = self.string();
                        self.space();
                        self.eat(b':');
                        let value = self.value();
                        members.push((key, value));
                        self.space();
                        if self.peek() == b',' {
                            self.at += 1;
                        } else {
                            self.eat(b'}');
                            return J::Obj(members);
                        }
                    }
                }
                b'[' => {
                    self.at += 1;
                    let mut items = Vec::new();
                    self.space();
                    if self.peek() == b']' {
                        self.at += 1;
                        return J::Arr(items);
                    }
                    loop {
                        items.push(self.value());
                        self.space();
                        if self.peek() == b',' {
                            self.at += 1;
                        } else {
                            self.eat(b']');
                            return J::Arr(items);
                        }
                    }
                }
                b'"' => J::Str(self.string()),
                b't' => {
                    self.word("true");
                    J::Bool(true)
                }
                b'f' => {
                    self.word("false");
                    J::Bool(false)
                }
                b'n' => {
                    self.word("null");
                    J::Null
                }
                _ => {
                    let start = self.at;
                    while self.at < self.bytes.len()
                        && matches!(
                            self.bytes[self.at],
                            b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'
                        )
                    {
                        self.at += 1;
                    }
                    assert!(self.at > start, "unexpected byte at {start}");
                    J::Num(String::from_utf8(self.bytes[start..self.at].to_vec()).expect("ASCII"))
                }
            }
        }

        fn string(&mut self) -> String {
            self.eat(b'"');
            let mut out = String::new();
            loop {
                let start = self.at;
                while !matches!(self.bytes[self.at], b'"' | b'\\') {
                    self.at += 1;
                }
                out.push_str(std::str::from_utf8(&self.bytes[start..self.at]).expect("UTF-8"));
                if self.peek() == b'"' {
                    self.at += 1;
                    return out;
                }
                self.at += 1;
                let escape = self.peek();
                self.at += 1;
                match escape {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\u{8}'),
                    b'f' => out.push('\u{c}'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        let high = self.hex4();
                        let scalar = if (0xD800..0xDC00).contains(&high) {
                            self.eat(b'\\');
                            self.eat(b'u');
                            let low = self.hex4();
                            0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00)
                        } else {
                            high
                        };
                        out.push(char::from_u32(scalar).expect("a scalar value"));
                    }
                    other => panic!("bad escape {other}"),
                }
            }
        }

        fn hex4(&mut self) -> u32 {
            let digits = std::str::from_utf8(&self.bytes[self.at..self.at + 4]).expect("ASCII");
            self.at += 4;
            u32::from_str_radix(digits, 16).expect("hex digits")
        }
    }
}
