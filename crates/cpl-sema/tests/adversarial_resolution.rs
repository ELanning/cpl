//! Adversarial tests of name resolution and identity
//! (`spec/semantics.md` Sections 1 and 2; `spec/toolchain.md` 3.6, 3.7,
//! and 4.3-4.5): file-scoped imports (S5, T20), the prelude and its
//! shadowing rules (1.7, S6, stdlib S3 and S11), the `gen` alias (S54),
//! module-alias and namespace imports (1.6, D15), modules spanning several
//! files (1.2, toolchain 3.7), visibility and exportability (1.5, S4),
//! qualified chains through modules versus values (2.4), the std-only
//! annotations in project files (1.8, stdlib 10.2), stable and provisional
//! identities (S2, toolchain 4.1, 4.3, 4.5, T21), reserved module paths
//! (1.1), and the determinism of `FileId`s and `DefId`s (toolchain 3.2,
//! T15).
//!
//! Every expectation is the specification's answer, not the checker's
//! current behaviour; where a rule needs a reading, the comment at the test
//! names the reading and the sections it rests on.

mod common;

use common::*;
use cpl_diag::{DeclName, FileId, FileOrigin, StableId, SymbolPath};
use cpl_hir::{DefId, DefStatus, ImportTarget, ModuleDef, Program, Ty};
use cpl_sema::project::{load_project_from_tree, LoadOptions, ParsedFile, StdSource};
use cpl_sema::{check, CheckResult};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

// ---- helpers ---------------------------------------------------------------

/// `(file, line, code)` of every diagnostic in the result's order; a
/// span-less diagnostic has the file `""` and the line 0.
fn located(result: &CheckResult) -> Vec<(String, u32, String)> {
    json_values(result)
        .iter()
        .map(|d| {
            (
                d["span"]["file"].as_str().unwrap_or("").to_string(),
                d["span"]["line"].as_u64().unwrap_or(0) as u32,
                d["code"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

/// `(line, code)` of every diagnostic in the result's order.
fn lines(result: &CheckResult) -> Vec<(u32, String)> {
    located(result)
        .into_iter()
        .map(|(_, line, code)| (line, code))
        .collect()
}

/// An expected `(file, line, code)`.
fn at(file: &str, line: u32, code: &str) -> (String, u32, String) {
    (file.to_string(), line, code.to_string())
}

/// An expected `(line, code)`.
fn lc(line: u32, code: &str) -> (u32, String) {
    (line, code.to_string())
}

/// The S49 objects of the diagnostics with `code`, in the result's order.
fn with_code(result: &CheckResult, code: &str) -> Vec<Value> {
    json_values(result)
        .into_iter()
        .filter(|d| d["code"] == json!(code))
        .collect()
}

/// The `name` fields of diagnostic objects, in order.
fn names(ds: &[Value]) -> Vec<String> {
    ds.iter()
        .map(|d| d["name"].as_str().unwrap_or("").to_string())
        .collect()
}

/// The note messages of a diagnostic object.
fn note_messages(d: &Value) -> Vec<String> {
    d["notes"]
        .as_array()
        .map(|notes| {
            notes
                .iter()
                .map(|n| n["message"].as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The `(file, line)` of a diagnostic object's `n`th note.
fn note_at(d: &Value, n: usize) -> (String, u32) {
    let span = &d["notes"][n]["span"];
    (
        span["file"].as_str().unwrap_or("").to_string(),
        span["line"].as_u64().unwrap_or(0) as u32,
    )
}

/// The primary `(line, col)` of a diagnostic object.
fn span_of(d: &Value) -> (u32, u32) {
    (
        d["span"]["line"].as_u64().unwrap_or(0) as u32,
        d["span"]["col"].as_u64().unwrap_or(0) as u32,
    )
}

/// The 1-based line and column of the first occurrence of `needle`.
fn pos(source: &str, needle: &str) -> (u32, u32) {
    let offset = source
        .find(needle)
        .unwrap_or_else(|| panic!("`{needle}` is not in the source"));
    let before = &source[..offset];
    let line = before.matches('\n').count() as u32 + 1;
    let col = before.rsplit('\n').next().unwrap_or("").chars().count() as u32 + 1;
    (line, col)
}

/// The line of the first occurrence of `needle`.
fn line_of(source: &str, needle: &str) -> u32 {
    pos(source, needle).0
}

/// The line and column of `token` on the first line that contains
/// `line_needle`.
fn pos_in(source: &str, line_needle: &str, token: &str) -> (u32, u32) {
    let line = line_of(source, line_needle);
    let text = source
        .lines()
        .nth(line as usize - 1)
        .expect("the line exists");
    let offset = text
        .find(token)
        .unwrap_or_else(|| panic!("`{token}` is not on line {line}"));
    (line, text[..offset].chars().count() as u32 + 1)
}

fn sorted<T: Ord>(mut v: Vec<T>) -> Vec<T> {
    v.sort();
    v
}

/// The definition with this qualified name.
fn def_id(p: &Program, path: &str) -> DefId {
    p.lookup(&SymbolPath::parse(path).expect("a symbol path"))
        .unwrap_or_else(|| panic!("no definition `{path}`"))
}

/// The module with this dotted path.
fn module_def<'p>(p: &'p Program, path: &str) -> &'p ModuleDef {
    let segments: Vec<String> = path.split('.').map(str::to_string).collect();
    p.module(
        p.module_by_path(&segments)
            .unwrap_or_else(|| panic!("no module `{path}`")),
    )
}

/// The project-relative paths of some files.
fn paths(p: &Program, files: &[FileId]) -> Vec<String> {
    files
        .iter()
        .map(|f| p.source_map().file(*f).path.clone())
        .collect()
}

/// The import targets of one file, in the program's order.
fn imports_of<'p>(p: &'p Program, file: &str) -> Vec<&'p ImportTarget> {
    p.imports
        .iter()
        .filter(|i| p.source_map().file(i.file).path == file)
        .map(|i| &i.target)
        .collect()
}

/// What the ids of a checked program determine: the source map in `FileId`
/// order, every module with its files and definitions, every definition's
/// identity and location, the references and imports, and every
/// diagnostic.
fn fingerprint(result: &CheckResult) -> Vec<String> {
    let p = &result.program;
    let mut out: Vec<String> = p
        .source_map()
        .files()
        .map(|f| format!("file {} {}", f.id.0, f.path))
        .collect();
    for m in p.modules() {
        let files: Vec<u32> = m.files.iter().map(|f| f.0).collect();
        let defs: Vec<u32> = m.defs.iter().map(|d| d.0).collect();
        out.push(format!(
            "module {} {} {files:?} {defs:?}",
            m.id.0,
            m.path.join(".")
        ));
    }
    for d in p.defs() {
        out.push(format!(
            "def {} {} {} {} {}",
            d.id.0,
            d.stable_id,
            p.display_path(d.id),
            d.file.0,
            d.decl_index
        ));
    }
    out.extend(p.references.iter().map(|r| format!("{r:?}")));
    out.extend(p.imports.iter().map(|i| format!("{i:?}")));
    out.extend(json_values(result).iter().map(Value::to_string));
    out
}

/// A fresh directory under `CARGO_TARGET_TMPDIR` for one test of this file.
fn scratch_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("cpl-sema-adversarial-resolution")
        .join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).expect("remove an old scratch directory");
    }
    fs::create_dir_all(&dir).expect("create a scratch directory");
    dir
}

/// `lib.a`: a public wrapper and two public functions.
const LIB_A: &str = "module lib.a\n\npub type Id(Text)\n\npub fn make(t: Text) -> Id {\n    Id(t)\n}\n\npub fn f() -> Int {\n    1\n}\n";

// ---- file-scoped imports (S5, T20, toolchain 3.7 rule 3) -------------------

#[test]
fn a_type_import_is_not_visible_in_a_sibling_file() {
    // The sibling file of `app.m` must import `Id` itself; in a type
    // position the miss is `unresolved_type` with `name` (9.4).
    let first = "module app.m\n\nuse lib.a.{Id}\n\npub fn first(id: Id) -> Id {\n    id\n}\n";
    let second = "module app.m\n\npub fn second(id: Id) -> Int {\n    1\n}\n";
    let result = check_files(&[
        ("src/lib/a.cpl", LIB_A),
        ("src/app/m.cpl", first),
        ("src/app/m/second.cpl", second),
    ]);
    assert_eq!(
        located(&result),
        [at("src/app/m/second.cpl", 3, "unresolved_type")],
        "{}",
        render(&result)
    );
    assert_eq!(json_values(&result)[0]["name"], json!("Id"));
}

#[test]
fn the_same_import_in_two_files_of_one_module_is_not_a_duplicate() {
    // 1.6 forbids importing a name twice in *one file*; every file of a
    // module has its own imports (S5).
    let a = "module app.m\n\nuse std.text.{trim}\n\nfn a(t: Text) -> Text {\n    trim(t)\n}\n";
    let b = "module app.m\n\nuse std.text.{trim}\n\nfn b(t: Text) -> Text {\n    trim(t)\n}\n";
    let result = check_files(&[("src/app/m.cpl", a), ("src/app/m/b.cpl", b)]);
    assert!(result.diagnostics.is_empty(), "{}", render(&result));
}

#[test]
fn a_use_in_a_sibling_file_does_not_go_through_this_files_import() {
    // S5: `trim` in b.cpl is not explained by a.cpl's `use`, so the call in
    // b.cpl is unresolved and the import of a.cpl is unused (1.6).
    let a = "module app.m\n\nuse std.text.{trim}\n\nfn a() -> Int {\n    1\n}\n";
    let b = "module app.m\n\nfn b(t: Text) -> Text {\n    trim(t)\n}\n";
    let result = check_files(&[("src/app/m.cpl", a), ("src/app/m/b.cpl", b)]);
    assert_eq!(
        sorted(located(&result)),
        [
            at("src/app/m.cpl", 3, "unused_import"),
            at("src/app/m/b.cpl", 4, "unresolved_name"),
        ],
        "{}",
        render(&result)
    );
    assert!(json_values(&result)
        .iter()
        .all(|d| d["name"] == json!("trim")));
    assert_eq!(
        with_code(&result, "unused_import")[0]["severity"],
        json!("warning")
    );
}

#[test]
fn an_import_conflicts_with_a_declaration_in_a_sibling_file() {
    // 1.6: an imported name may not equal a declaration of the importing
    // module in *any* file; S63 (4): the import is the primary span and the
    // declaration (in the other file) is the note.
    let a = "module app.m\n\nuse std.text.{trim}\n\nfn a(t: Text) -> Text {\n    trim(t)\n}\n";
    let b = "module app.m\n\nfn trim(t: Text) -> Text {\n    t\n}\n";
    let result = check_files(&[("src/app/m.cpl", a), ("src/app/m/b.cpl", b)]);
    assert_eq!(
        located(&result),
        [at("src/app/m.cpl", 3, "import_conflicts_with_declaration")],
        "{}",
        render(&result)
    );
    let d = &json_values(&result)[0];
    assert_eq!(d["name"], json!("trim"));
    assert_eq!(d["category"], json!("resolve"));
    assert_eq!(note_at(d, 0), ("src/app/m/b.cpl".to_string(), 3));
}

#[test]
fn a_module_alias_import_is_file_scoped() {
    // S5 for the submodule-alias form of 1.6: `fmt` is bound only in the
    // file with the `use`; elsewhere it is no binding, alias, or root module
    // name, so the chain is `unresolved_name` (2.4).
    let tools = "module tools.fmt\n\npub fn width() -> Int {\n    80\n}\n";
    let a = "module app.m\n\nuse tools.{fmt}\n\nfn a() -> Int {\n    fmt.width()\n}\n";
    let b = "module app.m\n\nfn b() -> Int {\n    fmt.width()\n}\n";
    let result = check_files(&[
        ("src/tools/fmt.cpl", tools),
        ("src/app/m.cpl", a),
        ("src/app/m/b.cpl", b),
    ]);
    assert_eq!(
        located(&result),
        [at("src/app/m/b.cpl", 4, "unresolved_name")],
        "{}",
        render(&result)
    );
    assert_eq!(json_values(&result)[0]["name"], json!("fmt"));
}

#[test]
fn an_unused_module_alias_is_an_unused_import() {
    // 1.6: every kind of `use` item, a submodule alias included, warns when
    // nothing in its file goes through it.
    let user = "module app.u\n\nuse lib.{a}\n\nfn g() -> Int {\n    lib.a.f()\n}\n";
    let result = check_files(&[("src/lib/a.cpl", LIB_A), ("src/app/u.cpl", user)]);
    assert_eq!(
        located(&result),
        [at("src/app/u.cpl", 3, "unused_import")],
        "{}",
        render(&result)
    );
    assert_eq!(json_values(&result)[0]["name"], json!("a"));
}

// ---- the prelude and its shadowing rules (1.7, S6, stdlib S3, S11) ---------

const PRELUDE_DECLS: &str = r#"module app.p

pub struct Option {
    v: Int
}

pub enum Result {
    Fine
}

pub type Text(Int)

pub type Bytes = Int where self > 0

pub capability ConstraintError {
    fn get(k: Int) -> Int
}

fn flatten(n: Int) -> Int {
    n
}

fn check(n: Int) -> Int {
    n
}

fn gen() -> Int {
    1
}

pub type Some(Int)

pub enum UInt8 {
    Small
}

pub capability Int32 {
    fn get(k: Int) -> Int
}

pub struct Gen {
    v: Int
}
"#;

/// Check `source` and assert that its diagnostics are exactly one
/// `prelude_name_redefined` (error, resolve) at the name token of each
/// `(name, needle)` (each needle starts at the name), with `name` and
/// `notes`, and for the names in `reserved` the note `reserved for a later
/// edition`.
fn assert_prelude_redefinitions(source: &str, expected: &[(&str, &str)], reserved: &[&str]) {
    let result = check_source(source);
    let want: Vec<(u32, u32, String)> = expected
        .iter()
        .map(|(name, needle)| {
            let (line, col) = pos(source, needle);
            (line, col, name.to_string())
        })
        .collect();
    let j = json_values(&result);
    assert!(
        j.iter()
            .all(|d| d["code"] == json!("prelude_name_redefined")),
        "{}",
        render(&result)
    );
    let got: Vec<(u32, u32, String)> = j
        .iter()
        .map(|d| {
            let (line, col) = span_of(d);
            (line, col, d["name"].as_str().unwrap_or("").to_string())
        })
        .collect();
    assert_eq!(sorted(got), sorted(want), "{}", render(&result));
    for d in &j {
        assert_eq!(d["severity"], json!("error"));
        assert_eq!(d["category"], json!("resolve"));
        assert!(!note_messages(d).is_empty(), "9.4 lists `notes`: {d}");
    }
    for name in reserved {
        let d = j
            .iter()
            .find(|d| d["name"] == json!(name))
            .unwrap_or_else(|| panic!("no diagnostic for {name}"));
        assert!(
            note_messages(d)
                .iter()
                .any(|m| m.contains("reserved for a later edition")),
            "{d}"
        );
    }
}

#[test]
fn declarations_named_like_prelude_names_are_rejected_whatever_their_kind() {
    // S6, stdlib S3 and S11: a module-level declaration of any kind whose
    // name is a prelude name (a prelude type, a prelude constructor, the
    // intrinsic `check`, `flatten`, the alias `gen`, `Gen`, a reserved
    // numeric name) is `prelude_name_redefined` at its name token (S63
    // (3)) with `name` and `notes` (9.4); stdlib S11 gives the reserved
    // numeric names the note `reserved for a later edition`.
    assert_prelude_redefinitions(
        PRELUDE_DECLS,
        &[
            ("Option", "Option {"),
            ("Result", "Result {"),
            ("Text", "Text(Int)"),
            ("Bytes", "Bytes ="),
            ("ConstraintError", "ConstraintError {"),
            ("flatten", "flatten("),
            ("check", "check("),
            ("gen", "gen()"),
            ("Some", "Some("),
            ("UInt8", "UInt8 {"),
            ("Int32", "Int32 {"),
            ("Gen", "Gen {"),
        ],
        &["UInt8", "Int32"],
    );
}

const PRELUDE_DECLS_REST: &str = r#"module app.q

pub type Int(Text)

pub struct Bool {
    v: Text
}

pub enum Unit {
    Only
}

pub capability List {
    fn get(k: Text) -> Text
}

pub type None(Text)

pub type Ok(Text)

pub type Err(Text)

pub type Int8(Text)

pub struct Int16 {
    v: Text
}

pub enum Int64 {
    Wide
}

pub type UInt16 = Text where true

pub capability UInt32 {
    fn get(k: Text) -> Text
}

pub type UInt64(Text)

pub struct Float32 {
    v: Text
}

pub enum Float64 {
    Double
}
"#;

#[test]
fn every_other_prelude_name_is_rejected_too() {
    // The rest of 1.7's table (the remaining prelude types and
    // constructors and the remaining reserved numeric names of stdlib S11),
    // one declaration each, over every declaration kind.
    assert_prelude_redefinitions(
        PRELUDE_DECLS_REST,
        &[
            ("Int", "Int(Text)"),
            ("Bool", "Bool {"),
            ("Unit", "Unit {"),
            ("List", "List {"),
            ("None", "None("),
            ("Ok", "Ok("),
            ("Err", "Err("),
            ("Int8", "Int8("),
            ("Int16", "Int16 {"),
            ("Int64", "Int64 {"),
            ("UInt16", "UInt16 ="),
            ("UInt32", "UInt32 {"),
            ("UInt64", "UInt64("),
            ("Float32", "Float32 {"),
            ("Float64", "Float64 {"),
        ],
        &[
            "Int8", "Int16", "Int64", "UInt16", "UInt32", "UInt64", "Float32", "Float64",
        ],
    );
}

#[test]
fn a_redefined_prelude_name_keeps_its_prelude_meaning() {
    // S6's rationale: a reader may assume the prelude meaning of `Option`
    // everywhere, so the rejected redefinition does not replace it: the
    // prelude's `Option<Int>` and `Some` resolve with no further
    // diagnostic (S62: one diagnostic per mistake).
    let source = "module app.keep\n\npub struct Option {\n    v: Int\n}\n\nfn f() -> Option<Int> {\n    Some(1)\n}\n";
    let result = check_source(source);
    assert_eq!(
        lines(&result),
        [lc(3, "prelude_name_redefined")],
        "{}",
        render(&result)
    );
}

#[test]
fn importing_a_submodule_alias_named_like_a_prelude_name_is_rejected() {
    // 1.6, S6: the rule is on the imported *name*, so a submodule alias
    // `check` or `gen` redefines a prelude name. (`lib.gen` is itself a
    // legal module path: only a first segment `std` or `gen` is reserved,
    // 1.1.) A diagnosed use item is never also `unused_import` (S62), and
    // the redefinition is the primary span (S63 (4)).
    let check_mod = "module lib.check\n\npub fn one() -> Int {\n    1\n}\n";
    let gen_mod = "module lib.gen\n\npub fn two() -> Int {\n    2\n}\n";
    let user = "module app.u\n\nuse lib.{check}\nuse lib.gen\n\nfn f() -> Int {\n    1\n}\n";
    let result = check_files(&[
        ("src/lib/check.cpl", check_mod),
        ("src/lib/gen.cpl", gen_mod),
        ("src/app/u.cpl", user),
    ]);
    assert_eq!(
        located(&result),
        [
            at("src/app/u.cpl", 3, "prelude_name_redefined"),
            at("src/app/u.cpl", 4, "prelude_name_redefined"),
        ],
        "{}",
        render(&result)
    );
    assert_eq!(names(&json_values(&result)), ["check", "gen"]);
}

#[test]
fn std_prelude_imports_name_its_declarations_only() {
    // 1.6, stdlib S3: `use std.prelude.{Int, Option, List}` names the
    // prelude's own declarations, redundantly and without a diagnostic; but
    // a variant (`Some`, 1.6: "use m.{V} finds no item"), the intrinsic
    // `check` (stdlib 1.4: it has no stub, so it is no item of
    // std.prelude), and a member stub (`take`, stdlib S37: not a free
    // name) are not items of the module: `unresolved_name` with `module`.
    let source = "module app.r\n\nuse std.prelude.{Int, Option, List}\nuse std.prelude.{Some}\nuse std.prelude.{check}\nuse std.prelude.{take}\n\nfn f(xs: List<Int>) -> Option<Int> {\n    xs.first()\n}\n";
    let result = check_source(source);
    assert_eq!(
        lines(&result),
        [
            lc(4, "unresolved_name"),
            lc(5, "unresolved_name"),
            lc(6, "unresolved_name"),
        ],
        "{}",
        render(&result)
    );
    let j = json_values(&result);
    assert_eq!(names(&j), ["Some", "check", "take"]);
    assert!(j.iter().all(|d| d["module"] == json!("std.prelude")));
}

#[test]
fn locals_and_parameters_may_shadow_prelude_names() {
    // S6 restricts module-level declarations and imports; local bindings,
    // parameters included, may shadow anything (2.3, stdlib S3).
    let result = check_source(
        "module app.s\n\nfn f(flatten: Int, check: Int) -> Int {\n    let gen = 2\n    flatten + gen + check\n}\n",
    );
    assert!(result.diagnostics.is_empty(), "{}", render(&result));
}

#[test]
fn calling_a_local_that_hides_a_prelude_function_is_not_callable() {
    // 2.3: `let flatten = 3` hides the prelude function with no diagnostic;
    // a call through the local is `not_callable` with a note naming the
    // hidden item (9.4: `name`, `notes`); the error type stops any cascade
    // (S62).
    let result = check_source(
        "module app.c\n\nfn f(xs: List<List<Int>>) -> List<Int> {\n    let flatten = 3\n    flatten(xs)\n}\n",
    );
    assert_eq!(
        lines(&result),
        [lc(5, "not_callable")],
        "{}",
        render(&result)
    );
    let d = &json_values(&result)[0];
    assert_eq!(d["name"], json!("flatten"));
    assert!(
        note_messages(d).iter().any(|m| m.contains("flatten")),
        "{d}"
    );
}

#[test]
fn members_and_type_parameters_named_like_prelude_names_are_allowed() {
    // S6 and stdlib S3 cover module-level declarations and imports only: a
    // variant, a field, an operation, a parameter, and a type parameter are
    // not module-level names (1.4: members belong to their type). A type
    // parameter is scope 3, which a type position consults before the
    // prelude (2.1, 2.2), so `Text` in `same` is the parameter; it is
    // exportable (1.5). Tests are keyed by their TEXT name in the test
    // namespace (S3) and never meet the prelude's names.
    let source = r#"module app.m

pub enum Tag {
    Int64
    Gen
}

pub struct Box {
    gen: Int
    check: Int
}

pub capability Store {
    fn flatten(k: Int) -> Int
}

pub fn same<Text>(x: Text) -> Text {
    x
}

fn read(b: Box, using db: Store) -> Int
    effects { db.flatten }
{
    db.flatten(b.gen + b.check)
}

test "Option" {
    assert same(1) == 1
}
"#;
    let result = check_source(source);
    assert!(result.diagnostics.is_empty(), "{}", render(&result));
}

#[test]
fn a_user_variant_named_like_a_prelude_constructor_is_ambiguous_only_without_an_expected_type() {
    // stdlib S3 and S10: the prelude's constructors (`Unit` included) are
    // lexical candidates beside the module's variants, so with no expected
    // type `Unit`, `None`, and `Some` are `ambiguous_constructor` (with
    // `name` and `candidates`);
    // an expected type (step 1) or a qualified path decides.
    let source = r#"module app.n

pub enum Reply {
    None
    Some(Int)
    Unit
}

fn f() -> Int {
    let _u = Unit
    let _a = None
    let _b: Option<Int> = None
    let _c: Reply = None
    let _d: Option<Int> = Option.None
    let _e = Reply.None
    let _f = Some(1)
    let _g: Option<Int> = Some(1)
    let _h = std.prelude.Option.Some(2)
    1
}
"#;
    let result = check_source(source);
    assert_eq!(
        lines(&result),
        [
            lc(line_of(source, "_u = Unit"), "ambiguous_constructor"),
            lc(line_of(source, "_a = None"), "ambiguous_constructor"),
            lc(line_of(source, "_f = Some"), "ambiguous_constructor"),
        ],
        "{}",
        render(&result)
    );
    let j = json_values(&result);
    let candidates = |d: &Value| -> Vec<String> {
        sorted(
            d["candidates"]
                .as_array()
                .map(|c| {
                    c.iter()
                        .map(|n| n.as_str().unwrap_or("").to_string())
                        .collect()
                })
                .unwrap_or_default(),
        )
    };
    assert_eq!(names(&j), ["Unit", "None", "Some"]);
    assert_eq!(
        candidates(&j[0]),
        ["app.n.Reply.Unit", "std.prelude.Unit.Unit"]
    );
    assert_eq!(
        candidates(&j[1]),
        ["app.n.Reply.None", "std.prelude.Option.None"]
    );
    assert_eq!(
        candidates(&j[2]),
        ["app.n.Reply.Some", "std.prelude.Option.Some"]
    );
}

#[test]
fn reserved_type_names_in_type_positions_are_unresolved_types() {
    // 1.7 and stdlib S11: `unresolved_type` (with `name`) and the note
    // `reserved for a later edition`; the body is not checked against the
    // error-typed return type (S62).
    let result = check_source("module app.t\n\nfn f(a: Int64, b: Float32) -> UInt16 {\n    1\n}\n");
    assert_eq!(
        lines(&result),
        [
            lc(3, "unresolved_type"),
            lc(3, "unresolved_type"),
            lc(3, "unresolved_type"),
        ],
        "{}",
        render(&result)
    );
    let j = json_values(&result);
    assert_eq!(names(&j), ["Int64", "Float32", "UInt16"]);
    for d in &j {
        assert!(
            note_messages(d)
                .iter()
                .any(|m| m.contains("reserved for a later edition")),
            "{d}"
        );
    }
}

// ---- the `gen` alias (S54, stdlib S1, S28) ---------------------------------

#[test]
fn gen_and_std_gen_are_one_module() {
    // S54: a chain beginning with `gen` resolves exactly as the same chain
    // beginning with `std.gen`; the two spellings mix freely.
    let source = r#"module app.g

property "both spellings"
    for a in gen.int
    for b in std.gen.int
    for xs in gen.list(std.gen.positive_int)
    for ys in std.gen.option(gen.text)
{
    assert a + b == b + a
    assert xs.len >= 0
    assert ys.is_some() or ys.is_none()
}
"#;
    let result = check_source(source);
    assert!(result.diagnostics.is_empty(), "{}", render(&result));
}

#[test]
fn no_item_of_std_gen_can_be_imported_in_either_use_form() {
    // 1.6, stdlib S28: every item of std.gen, the refinement `Positive`, the
    // functions, the generator properties, and `Gen`, in the list and the
    // single form (D15), is `generator_import` with `name`; never also
    // `unused_import` (S62).
    let source = "module app.gi\n\nuse std.gen.{Positive}\nuse std.gen.{range, option}\nuse std.gen.text\nuse std.gen.{Gen}\n\nfn f() -> Int {\n    1\n}\n";
    let result = check_source(source);
    assert_eq!(
        lines(&result),
        [
            lc(3, "generator_import"),
            lc(4, "generator_import"),
            lc(4, "generator_import"),
            lc(5, "generator_import"),
            lc(6, "generator_import"),
        ],
        "{}",
        render(&result)
    );
    assert_eq!(
        names(&json_values(&result)),
        ["Positive", "range", "option", "text", "Gen"]
    );
}

#[test]
fn a_rejected_generator_import_binds_an_erroneous_name() {
    // S62: the diagnosed use item binds `int` to an erroneous item, which is
    // not reported again where it is used (no `not_a_value`, no
    // `generator_outside_for`).
    let result = check_source(
        "module app.ge\n\nuse std.gen.{int}\n\nfn f() -> Int {\n    let _g = int\n    1\n}\n",
    );
    assert_eq!(codes(&result), ["generator_import"], "{}", render(&result));
}

#[test]
fn a_missing_generator_is_unresolved_in_std_gen() {
    // S54 and 2.4: `gen.nope` looks `nope` up in the module std.gen:
    // `unresolved_name` with `name` and `module`; the binder is error-typed
    // and nothing cascades (S62).
    let result = check_source(
        "module app.gn\n\nproperty \"p\"\n    for n in gen.nope\n{\n    assert n == n\n}\n",
    );
    assert_eq!(
        lines(&result),
        [lc(4, "unresolved_name")],
        "{}",
        render(&result)
    );
    let d = &json_values(&result)[0];
    assert_eq!(d["name"], json!("nope"));
    assert_eq!(d["module"], json!("std.gen"));
}

#[test]
fn a_local_named_gen_hides_the_alias_in_value_chains() {
    // S54: "a local binding named gen hides the alias for value chains like
    // any root module name": `gen.abs()` is a method call on the local.
    let result =
        check_source("module app.gl\n\nfn f() -> Int {\n    let gen = -3\n    gen.abs()\n}\n");
    assert!(result.diagnostics.is_empty(), "{}", render(&result));
}

#[test]
fn a_parameter_named_gen_does_not_affect_type_positions() {
    // 2.4: type positions never consult local scopes, so `gen.Positive` is
    // still std.gen's refinement with a parameter named `gen` in scope; the
    // literal 1 is proved (3.5).
    let result = check_source("module app.gp\n\nfn f(gen: Int) -> gen.Positive {\n    1\n}\n");
    assert!(result.diagnostics.is_empty(), "{}", render(&result));
    let p = &result.program;
    let ret = &p.def(def_id(p, "app.gp.f")).kind.as_fn().expect("a fn").ret;
    assert!(
        matches!(ret, Ty::Refined(_, d) if *d == p.std_defs().positive),
        "{}",
        p.display_ty(ret)
    );
}

// ---- reserved module paths (1.1, S54, toolchain 3.6) -----------------------

#[test]
fn module_paths_whose_first_segment_is_std_or_gen_are_reserved() {
    // 1.1: a project may not declare a module whose path begins with `std`
    // or `gen`; only the first segment counts. The diagnostic spans the
    // header's module path (S63 (3)) and carries `module` (9.4).
    let files = [
        ("src/std.cpl", "module std\n"),
        ("src/std/extra.cpl", "module std.extra\n"),
        ("src/gen.cpl", "module gen\n"),
        ("src/stdx/y.cpl", "module stdx.y\n"),
        ("src/generator.cpl", "module generator\n"),
        ("src/my/std.cpl", "module my.std\n"),
        ("src/app/gen.cpl", "module app.gen\n"),
    ];
    let result = check_files(&files);
    assert_eq!(
        sorted(located(&result)),
        [
            at("src/gen.cpl", 1, "reserved_module_path"),
            at("src/std.cpl", 1, "reserved_module_path"),
            at("src/std/extra.cpl", 1, "reserved_module_path"),
        ],
        "{}",
        render(&result)
    );
    let j = json_values(&result);
    for d in &j {
        let module = d["module"].as_str().expect("a `module` field");
        assert_eq!(d["severity"], json!("error"));
        assert_eq!(d["category"], json!("resolve"));
        assert_eq!(span_of(d), (1, 8), "{d}");
        assert_eq!(d["span"]["end_col"], json!(8 + module.len()), "{d}");
    }
    let mut modules: Vec<&str> = j.iter().filter_map(|d| d["module"].as_str()).collect();
    modules.sort();
    assert_eq!(modules, ["gen", "std", "std.extra"]);
}

#[test]
fn a_project_file_cannot_extend_the_standard_library() {
    // 1.1 and toolchain 3.6 (stdlib S34: "a user project cannot override
    // or extend them"): a project header `module std.prelude` is reserved
    // and contributes nothing to the std module of that path; the prelude
    // keeps working everywhere else.
    let result = check_files(&[
        (
            "src/std/prelude.cpl",
            "module std.prelude\n\npub fn extra() -> Int {\n    1\n}\n",
        ),
        (
            "src/app/u.cpl",
            "module app.u\n\nfn f(xs: List<List<Int>>) -> List<Int> {\n    flatten(xs)\n}\n",
        ),
    ]);
    assert_eq!(
        located(&result),
        [at("src/std/prelude.cpl", 1, "reserved_module_path")],
        "{}",
        render(&result)
    );
    let p = &result.program;
    assert!(p
        .lookup(&SymbolPath::parse("std.prelude.extra").expect("a path"))
        .is_none());
    assert_eq!(
        p.modules().filter(|m| m.path == ["std", "prelude"]).count(),
        1
    );
    let prelude = module_def(p, "std.prelude");
    assert_eq!(prelude.origin, FileOrigin::Std);
    assert_eq!(paths(p, &prelude.files), ["std/prelude.cpl"]);
}

// ---- module-alias and namespace imports (1.6, D15, 2.4) --------------------

#[test]
fn a_submodule_alias_resolves_in_type_positions_and_counts_as_used() {
    // 1.6, 2.2: a type path's module qualification resolves through a
    // submodule alias (2.4), which uses the import.
    let user = "module app.u\n\nuse lib.{a}\n\nfn f(x: a.Id) -> a.Id {\n    x\n}\n";
    let result = check_files(&[("src/lib/a.cpl", LIB_A), ("src/app/u.cpl", user)]);
    assert!(result.diagnostics.is_empty(), "{}", render(&result));
}

#[test]
fn the_single_use_form_imports_a_submodule_or_an_item() {
    // D15: `use a.b.c` imports the item or submodule `c` of `a.b` under the
    // name `c`, exactly like `use a.b.{c}`.
    let user =
        "module app.u\n\nuse lib.a\nuse lib.a.make\n\nfn f(t: Text) -> a.Id {\n    make(t)\n}\n";
    let result = check_files(&[("src/lib/a.cpl", LIB_A), ("src/app/u.cpl", user)]);
    assert!(result.diagnostics.is_empty(), "{}", render(&result));
    let p = &result.program;
    let targets = imports_of(p, "src/app/u.cpl");
    let lib_a = p
        .module_by_path(&["lib".to_string(), "a".to_string()])
        .expect("lib.a");
    assert_eq!(targets.len(), 2, "{targets:?}");
    assert!(
        targets.contains(&&ImportTarget::Module(lib_a)),
        "{targets:?}"
    );
    assert!(
        targets.contains(&&ImportTarget::Def(def_id(p, "lib.a.make"))),
        "{targets:?}"
    );
}

#[test]
fn an_import_alias_shadows_a_root_module_of_the_same_name() {
    // 2.1: the file's imports are scope 2 and the root module names scope 1,
    // so after `use other.{lib}` every chain `lib.x` is looked up in
    // `other.lib`: `root_only` is `unresolved_name` with `module`.
    let user = "module app.u\n\nuse other.{lib}\n\nfn a() -> Int {\n    lib.nested_only()\n}\n\nfn b() -> Int {\n    lib.root_only()\n}\n";
    let result = check_files(&[
        (
            "src/lib.cpl",
            "module lib\n\npub fn root_only() -> Int {\n    1\n}\n",
        ),
        (
            "src/other/lib.cpl",
            "module other.lib\n\npub fn nested_only() -> Int {\n    2\n}\n",
        ),
        ("src/app/u.cpl", user),
    ]);
    assert_eq!(
        located(&result),
        [at("src/app/u.cpl", 10, "unresolved_name")],
        "{}",
        render(&result)
    );
    let d = &json_values(&result)[0];
    assert_eq!(d["name"], json!("root_only"));
    assert_eq!(d["module"], json!("other.lib"));
}

/// Module `x` declares the function `y`; the module `x.y` exists as well.
const X: &str = "module x\n\npub fn y() -> Int {\n    1\n}\n";
const X_Y: &str = "module x.y\n\npub fn z() -> Int {\n    2\n}\n";

#[test]
fn a_use_item_prefers_the_item_to_a_submodule_of_the_same_name() {
    // 1.6: a public item of the module is imported; only "a LOWER_IDENT
    // that is not an item" imports a submodule alias. 2.4: a chain takes
    // the longest module path, so `x.y.z` is the function of module `x.y`.
    let user = "module app.u\n\nuse x.{y}\n\nfn a() -> Int {\n    y()\n}\n\nfn b() -> Int {\n    x.y.z()\n}\n";
    let result = check_files(&[
        ("src/x.cpl", X),
        ("src/x/y.cpl", X_Y),
        ("src/app/u.cpl", user),
    ]);
    assert!(result.diagnostics.is_empty(), "{}", render(&result));
    let p = &result.program;
    assert_eq!(
        imports_of(p, "src/app/u.cpl"),
        [&ImportTarget::Def(def_id(p, "x.y"))]
    );
}

#[test]
fn a_chain_that_ends_at_a_module_path_is_not_the_function_of_that_name() {
    // 2.4 takes the *longest* prefix that is a module path, so the chain
    // `x.y` is the module `x.y` even though module `x` declares a function
    // `y`; a chain that ends inside the module path is no function, and in
    // callee position that is `not_callable` (3.8: "callee is a ...
    // module"). 1.6 resolves `use x.{y}` the other way, item first (see the
    // previous test and the spec issues of this report).
    let user = "module app.u\n\nfn c() -> Int {\n    x.y()\n}\n";
    let result = check_files(&[
        ("src/x.cpl", X),
        ("src/x/y.cpl", X_Y),
        ("src/app/u.cpl", user),
    ]);
    assert_eq!(
        located(&result),
        [at("src/app/u.cpl", 4, "not_callable")],
        "{}",
        render(&result)
    );
}

/// The only module under the namespace `ns`.
const NS: &str = "module ns.deep.x\n\npub fn f() -> Int {\n    1\n}\n";

#[test]
fn a_use_path_that_is_only_a_prefix_imports_only_submodules() {
    // 1.6: the module of a `use` must exist, or be a proper prefix of one
    // "for the submodule form". `Thing` cannot name a submodule and
    // `ns.deep.f` is no module, so both items need the modules `ns` and
    // `ns.deep`, which do not exist: `unresolved_module` with `module`.
    let user = "module app.u\n\nuse ns.{Thing}\nuse ns.deep.{f}\n\nfn g() -> Int {\n    1\n}\n";
    let result = check_files(&[("src/ns/deep/x.cpl", NS), ("src/app/u.cpl", user)]);
    assert_eq!(
        lines(&result),
        [lc(3, "unresolved_module"), lc(4, "unresolved_module")],
        "{}",
        render(&result)
    );
    let j = json_values(&result);
    assert_eq!(j[0]["module"], json!("ns"));
    assert_eq!(j[1]["module"], json!("ns.deep"));
}

#[test]
fn a_namespace_alias_reaches_only_the_modules_below_it() {
    // 2.4: after `use ns.{deep}`, `deep.x.f` reaches the module `ns.deep.x`;
    // `deep.f` asks for an item of `ns.deep`, which is only a prefix of a
    // module path: `unresolved_module`.
    // A type position resolves its qualification the same way (2.2).
    let user = "module app.u\n\nuse ns.{deep}\n\nfn g() -> Int {\n    deep.x.f()\n}\n\nfn h() -> Int {\n    deep.f()\n}\n\nfn k(t: deep.T) -> Int {\n    1\n}\n";
    let result = check_files(&[("src/ns/deep/x.cpl", NS), ("src/app/u.cpl", user)]);
    assert_eq!(
        located(&result),
        [
            at("src/app/u.cpl", 10, "unresolved_module"),
            at("src/app/u.cpl", 13, "unresolved_module"),
        ],
        "{}",
        render(&result)
    );
}

#[test]
fn modules_aliases_and_functions_in_value_position_are_not_values() {
    // 2.2, 2.4, S11, 3.8: an alias alone, a root module name, a chain that
    // ends inside a module path, and a function or `check` outside callee
    // position are `not_a_value` (with `name`); going through the alias
    // still uses it, so there is no `unused_import` (1.6).
    let user = "module app.u\n\nuse lib.{a}\n\nfn apply(n: Int) -> Int {\n    n\n}\n\nfn g() -> Int {\n    let _m = a\n    let _n = lib\n    let _o = lib.a\n    let _p = lib.a.f\n    let _c = check\n    apply(apply)\n}\n";
    let result = check_files(&[("src/lib/a.cpl", LIB_A), ("src/app/u.cpl", user)]);
    let expected: Vec<(u32, String)> = [
        "_m = a",
        "_n = lib",
        "_o = lib.a",
        "_p = lib.a.f",
        "_c = check",
        "apply(apply)",
    ]
    .iter()
    .map(|needle| lc(line_of(user, needle), "not_a_value"))
    .collect();
    assert_eq!(lines(&result), expected, "{}", render(&result));
    let j = json_values(&result);
    assert_eq!(j[0]["name"], json!("a"));
    assert_eq!(j[1]["name"], json!("lib"));
    assert_eq!(j[4]["name"], json!("check"));
    assert_eq!(j[5]["name"], json!("apply"));
}

#[test]
fn std_modules_import_as_submodule_aliases() {
    // 1.6: "the standard library is imported exactly like project modules":
    // `std` is a proper prefix of std.text and std.prelude, so `text` and
    // `prelude` are submodule aliases.
    let source = "module app.sa\n\nuse std.{text}\nuse std.{prelude}\n\nfn f(t: Text) -> Text {\n    text.trim(t)\n}\n\nfn g(xs: List<List<Int>>) -> List<Int> {\n    prelude.flatten(xs)\n}\n";
    let result = check_source(source);
    assert!(result.diagnostics.is_empty(), "{}", render(&result));
}

#[test]
fn variants_and_operations_are_not_module_items() {
    // 1.4: variants and operations are members of their type; 1.6 ("use
    // m.{V} finds no item") and 2.4 (the segment after a module path must
    // be an item of the module): `unresolved_name` with `name` and
    // `module`.
    let lib = "module lib.v\n\npub enum E {\n    V\n}\n\npub capability Store {\n    fn get(k: Text) -> Int\n}\n";
    let user = "module app.u\n\nuse lib.v.{V}\nuse lib.v.{get}\n\nfn f() -> Int {\n    let _v = lib.v.V\n    1\n}\n";
    let result = check_files(&[("src/lib/v.cpl", lib), ("src/app/u.cpl", user)]);
    assert_eq!(
        lines(&result),
        [
            lc(3, "unresolved_name"),
            lc(4, "unresolved_name"),
            lc(7, "unresolved_name"),
        ],
        "{}",
        render(&result)
    );
    let j = json_values(&result);
    assert_eq!(names(&j), ["V", "get", "V"]);
    assert!(j.iter().all(|d| d["module"] == json!("lib.v")));
}

#[test]
fn a_duplicate_import_leaves_an_unused_first_import_unused() {
    // 1.6: the second `trim` is `duplicate_import` (diagnosed, so never
    // `unused_import`, S62); the first is an ordinary import that nothing
    // in the file uses, an independent mistake.
    let source = "module app.d\n\nuse std.text.{trim}\nuse std.text.{trim, to_upper}\n\nfn f(t: Text) -> Text {\n    to_upper(t)\n}\n";
    let result = check_source(source);
    assert_eq!(
        lines(&result),
        [lc(3, "unused_import"), lc(4, "duplicate_import")],
        "{}",
        render(&result)
    );
}

#[test]
fn imports_used_only_in_clauses_predicates_ranges_and_patterns_are_used() {
    // 1.6 counts every use in the file: a `where` predicate, a `requires`
    // clause, a property's `for` range, and a qualified pattern constructor
    // are all uses.
    let lib = "module lib.a\n\npub fn positive(n: Int) -> Bool {\n    n > 0\n}\n\npub fn small(n: Int) -> Bool {\n    n < 100\n}\n\npub fn limit() -> Int {\n    10\n}\n\npub enum Mode {\n    On\n    Off\n}\n";
    let user = r#"module app.u

use lib.a.{positive, small, limit, Mode}

type Ranged = Int where small(self)

fn f(n: Int, m: lib.a.Mode) -> Int
    requires positive(n)
{
    match m {
        Mode.On => 1
        Mode.Off => 0
    }
}

property "bounded"
    for k in gen.range(0, limit())
{
    assert k >= 0
}
"#;
    let result = check_files(&[("src/lib/a.cpl", lib), ("src/app/u.cpl", user)]);
    assert!(result.diagnostics.is_empty(), "{}", render(&result));
}

#[test]
fn only_enums_declared_or_imported_supply_lexical_constructors() {
    // S10 step 2: the lexical candidates are the variants of the enums
    // declared in the module or imported into the file; step 1 still finds
    // a variant of an enum that is not imported through the expected type.
    let lib = "module lib.r\n\npub enum Remote {\n    Same\n}\n\npub fn take(r: Remote) -> Int {\n    1\n}\n";
    let u = "module app.u\n\nenum Local {\n    Same\n}\n\nfn f() -> Int {\n    let _x = Same\n    lib.r.take(Same)\n}\n";
    let v = "module app.v\n\nuse lib.r.{Remote}\n\nenum Local {\n    Same\n}\n\nfn g() -> Remote {\n    let _y = Same\n    Same\n}\n";
    let result = check_files(&[
        ("src/lib/r.cpl", lib),
        ("src/app/u.cpl", u),
        ("src/app/v.cpl", v),
    ]);
    assert_eq!(
        located(&result),
        [at("src/app/v.cpl", 10, "ambiguous_constructor")],
        "{}",
        render(&result)
    );
    let d = &json_values(&result)[0];
    let candidates: Vec<String> = d["candidates"]
        .as_array()
        .map(|c| {
            c.iter()
                .map(|n| n.as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        sorted(candidates),
        ["app.v.Local.Same", "lib.r.Remote.Same"]
    );
}

#[test]
fn importing_an_item_of_the_importing_module_conflicts_with_it() {
    // 1.6: the imported name `f` equals a declaration of the importing
    // module; the import is the primary span, the declaration the note (S63
    // (4)).
    let source = "module app.me\n\nuse app.me.{f}\n\npub fn f() -> Int {\n    1\n}\n";
    let result = check_source(source);
    assert_eq!(
        lines(&result),
        [lc(3, "import_conflicts_with_declaration")],
        "{}",
        render(&result)
    );
    let d = &json_values(&result)[0];
    assert_eq!(d["name"], json!("f"));
    assert_eq!(note_at(d, 0).1, 5);
}

// ---- modules spanning several files (1.2, 1.4, toolchain 3.7) --------------

#[test]
fn every_declaration_kind_is_visible_across_the_files_of_a_module() {
    // 1.2, S1, 3.7 rule 3: a module's own declarations, private ones
    // included, are visible in all of its files without import: types,
    // struct literals, unqualified and qualified constructors, a capability
    // in a `using` parameter, functions plain and qualified.
    let a = r#"module m.a

struct Point {
    x: Int
    y: Int
}

enum Shape {
    Dot(Point)
    Empty
}

type Label(Text)

type Small = Int where self < 10

capability Store {
    fn get(k: Text) -> Int
}

fn helper(n: Int) -> Int {
    n + 1
}
"#;
    let b = r#"module m.a

fn use_all(p: Point, s: Small, using db: Store) -> Int
    effects { db.get }
{
    let _shape = Dot(Point { x: p.x, y: p.y })
    let _empty = Shape.Empty
    let _label = Label("x")
    let _q = m.a.helper(s)
    helper(db.get("k"))
}
"#;
    let result = check_files(&[("src/m/a.cpl", a), ("src/m/a/b.cpl", b)]);
    assert!(result.diagnostics.is_empty(), "{}", render(&result));
}

#[test]
fn private_items_reach_sibling_files_but_not_other_modules() {
    // 1.5: private declarations are visible in every file of their module
    // and nowhere else; a qualified path to one from another module is
    // `private_item` with `symbol` (the `@id` when present, S2) and
    // `qualified_name` (9.4), in an expression, a type position, a variant
    // chain, and a struct literal alike. A public struct's literal crosses
    // modules (fields have no visibility of their own).
    let a = r#"module m.a

@id("fn_secret_id")
fn secret() -> Int {
    1
}

type Hidden(Int)

pub enum Mode {
    On
}

enum Inner {
    Deep
}

struct Closed {
    v: Int
}

pub struct Open {
    v: Int
}
"#;
    let b = "module m.a\n\nfn sibling() -> Int {\n    secret() + m.a.secret()\n}\n\nfn sibling_types(h: m.a.Hidden, i: Inner) -> Int {\n    let _c = m.a.Closed { v: 1 }\n    1\n}\n";
    let u = r#"module app.u

fn outside() -> Int {
    m.a.secret()
}

fn typed(h: m.a.Hidden) -> Int {
    1
}

fn variant() -> Int {
    let _v = m.a.Inner.Deep
    let _w = m.a.Mode.On
    let _x = m.a.Closed { v: 1 }
    let _y = m.a.Open { v: 2 }
    1
}
"#;
    let result = check_files(&[
        ("src/m/a.cpl", a),
        ("src/m/a/b.cpl", b),
        ("src/app/u.cpl", u),
    ]);
    assert_eq!(
        located(&result),
        [
            at("src/app/u.cpl", 4, "private_item"),
            at("src/app/u.cpl", 7, "private_item"),
            at("src/app/u.cpl", 12, "private_item"),
            at("src/app/u.cpl", 14, "private_item"),
        ],
        "{}",
        render(&result)
    );
    let symbols: Vec<(String, String)> = json_values(&result)
        .iter()
        .map(|d| {
            (
                d["symbol"].as_str().unwrap_or("").to_string(),
                d["qualified_name"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    assert_eq!(
        symbols,
        [
            ("fn_secret_id".to_string(), "m.a.secret".to_string()),
            ("m.a.Hidden".to_string(), "m.a.Hidden".to_string()),
            ("m.a.Inner".to_string(), "m.a.Inner".to_string()),
            ("m.a.Closed".to_string(), "m.a.Closed".to_string()),
        ]
    );
}

#[test]
fn members_of_public_types_are_reachable_wherever_the_type_is() {
    // 1.5: fields, variants, and operations have no visibility of their
    // own: they are reachable wherever their type is.
    let lib = "module lib.t\n\npub struct Point {\n    x: Int\n}\n\npub enum Shape {\n    Dot(Point)\n}\n\npub capability Store {\n    fn get(k: Text) -> Int\n}\n";
    let user = r#"module app.u

use lib.t.{Point, Shape, Store}

fn f(p: Point, using db: Store) -> Int
    effects { db.get }
{
    let _s = Shape.Dot(Point { x: p.x })
    let _t = lib.t.Shape.Dot(lib.t.Point { x: 1 })
    db.get("k") + p.x
}
"#;
    let result = check_files(&[("src/lib/t.cpl", lib), ("src/app/u.cpl", user)]);
    assert!(result.diagnostics.is_empty(), "{}", render(&result));
}

#[test]
fn duplicate_declarations_across_files_are_one_diagnostic_at_the_later() {
    // S3 and 3.7 rule 2: one item namespace per module over all files,
    // whatever the kinds (a struct and an enum collide) and visibilities;
    // one `duplicate_declaration` at the later declaration in (file path,
    // declaration order), spanning its name (S63 (3), (4)), with `name` and
    // a note at the earlier one in the other file. `src/m/a.cpl` sorts
    // before `src/m/a/aa.cpl`, which sorts before `src/m/a/zz.cpl`.
    let a = "module m.a\n\npub fn width() -> Int {\n    1\n}\n\nstruct Shape {\n    v: Int\n}\n";
    let zz = "module m.a\n\nfn width() -> Int {\n    2\n}\n";
    let aa = "module m.a\n\nenum Shape {\n    Round\n}\n";
    let result = check_files(&[
        ("src/m/a.cpl", a),
        ("src/m/a/zz.cpl", zz),
        ("src/m/a/aa.cpl", aa),
    ]);
    assert_eq!(
        located(&result),
        [
            at("src/m/a/aa.cpl", 3, "duplicate_declaration"),
            at("src/m/a/zz.cpl", 3, "duplicate_declaration"),
        ],
        "{}",
        render(&result)
    );
    let j = json_values(&result);
    assert_eq!(names(&j), ["Shape", "width"]);
    assert_eq!(span_of(&j[0]), (3, 6));
    assert_eq!(span_of(&j[1]), (3, 4));
    assert_eq!(note_at(&j[0], 0), ("src/m/a.cpl".to_string(), 7));
    assert_eq!(note_at(&j[1], 0), ("src/m/a.cpl".to_string(), 3));
}

#[test]
fn a_name_declared_three_times_is_two_duplicates() {
    // S3 and 3.7 rule 2: every declaration after the first in (file path,
    // declaration order) is a `duplicate_declaration` at its name, with a
    // note at an earlier declaration of the name.
    let result = check_files(&[
        (
            "src/m/d.cpl",
            "module m.d\n\nfn twice() -> Int {\n    1\n}\n\nfn twice() -> Int {\n    2\n}\n",
        ),
        (
            "src/m/d/more.cpl",
            "module m.d\n\nfn twice() -> Int {\n    3\n}\n",
        ),
    ]);
    assert_eq!(
        located(&result),
        [
            at("src/m/d.cpl", 7, "duplicate_declaration"),
            at("src/m/d/more.cpl", 3, "duplicate_declaration"),
        ],
        "{}",
        render(&result)
    );
    for d in json_values(&result) {
        let primary = (
            d["span"]["file"].as_str().unwrap_or("").to_string(),
            span_of(&d).0,
        );
        assert!(note_at(&d, 0) < primary, "{d}");
    }
}

#[test]
fn tests_and_properties_share_one_namespace_keyed_by_the_decoded_name() {
    // S3: tests and properties share the module's test namespace, keyed by
    // the TEXT name, which is the token's decoded string (grammar 1.5), so
    // `"a\u{62}"` and `"ab"` are one name. One `duplicate_test_name`
    // (category `test`, 9.4) at the later TEXT name with a note at the
    // earlier (S63 (3), (4)); another module's namespace is separate.
    let t = "module m.t\n\ntest \"same name\" {\n    assert true\n}\n";
    let more = r#"module m.t

property "same name"
    for n in gen.int
{
    assert n == n
}

test "a\u{62}" {
    assert true
}
"#;
    let other = "module m.t\n\ntest \"ab\" {\n    assert true\n}\n";
    let n = "module n\n\ntest \"same name\" {\n    assert true\n}\n";
    let result = check_files(&[
        ("src/m/t.cpl", t),
        ("src/m/t/more.cpl", more),
        ("src/m/t/other.cpl", other),
        ("src/n.cpl", n),
    ]);
    assert_eq!(
        located(&result),
        [
            at("src/m/t/more.cpl", 3, "duplicate_test_name"),
            at("src/m/t/other.cpl", 3, "duplicate_test_name"),
        ],
        "{}",
        render(&result)
    );
    let j = json_values(&result);
    assert_eq!(names(&j), ["same name", "ab"]);
    assert!(j.iter().all(|d| d["category"] == json!("test")));
    assert_eq!(span_of(&j[0]), (3, 10));
    assert_eq!(span_of(&j[1]), (3, 6));
    assert_eq!(note_at(&j[0], 0), ("src/m/t.cpl".to_string(), 3));
    assert_eq!(note_at(&j[1], 0), ("src/m/t/more.cpl".to_string(), 9));
}

#[test]
fn items_and_tests_never_collide() {
    // S3: the item and test namespaces are separate.
    let result = check_source(
        "module m.n\n\nfn f() -> Int {\n    1\n}\n\ntest \"f\" {\n    assert f() == 1\n}\n",
    );
    assert!(result.diagnostics.is_empty(), "{}", render(&result));
}

#[test]
fn module_files_off_the_conventional_paths_warn_with_the_expected_path() {
    // 1.2 and 3.7 rule 5: `src/m/a.cpl` or `src/m/a/<name>.cpl`, nothing
    // deeper and nothing elsewhere; elsewhere is the warning
    // `module_file_misplaced` on the header's module path (S63 (3)) with
    // `module` and `expected` (9.4). The files still form the module.
    let result = check_files(&[
        (
            "src/elsewhere/file.cpl",
            "module m.a\n\npub fn one() -> Int {\n    1\n}\n",
        ),
        (
            "src/m/a/x/deep.cpl",
            "module m.a\n\npub fn two() -> Int {\n    2\n}\n",
        ),
        (
            "src/m/a/fine.cpl",
            "module m.a\n\npub fn three() -> Int {\n    3\n}\n",
        ),
        ("src/m.cpl", "module m.a\n"),
    ]);
    assert_eq!(
        located(&result),
        [
            at("src/elsewhere/file.cpl", 1, "module_file_misplaced"),
            at("src/m.cpl", 1, "module_file_misplaced"),
            at("src/m/a/x/deep.cpl", 1, "module_file_misplaced"),
        ],
        "{}",
        render(&result)
    );
    for d in json_values(&result) {
        assert_eq!(d["severity"], json!("warning"));
        assert_eq!(d["category"], json!("resolve"));
        assert_eq!(d["module"], json!("m.a"));
        assert_eq!(d["expected"], json!("src/m/a.cpl"));
        assert_eq!(span_of(&d), (1, 8));
        assert_eq!(d["span"]["end_col"], json!(11));
    }
    assert!(!result.has_errors());
    let p = &result.program;
    let m = module_def(p, "m.a");
    assert_eq!(m.files.len(), 4);
    assert_eq!(m.defs.len(), 3);
}

#[test]
fn module_docs_concatenate_in_file_path_order() {
    // 1.2 and 3.7 rule 4: the module docs of the files, in file (path)
    // order, separated by one blank line; a file without module docs adds
    // nothing.
    let result = check_files(&[
        ("src/d/b.cpl", "/// Two.\nmodule d\n"),
        ("src/d/a0.cpl", "module d\n"),
        ("src/d/a.cpl", "/// Three.\nmodule d\n"),
        ("src/d.cpl", "/// One.\n/// More of one.\nmodule d\n"),
    ]);
    assert!(result.diagnostics.is_empty(), "{}", render(&result));
    let p = &result.program;
    let d = module_def(p, "d");
    assert_eq!(
        paths(p, &d.files),
        ["src/d.cpl", "src/d/a.cpl", "src/d/a0.cpl", "src/d/b.cpl"]
    );
    assert_eq!(
        d.docs.as_deref(),
        Some("One.\nMore of one.\n\nThree.\n\nTwo.")
    );
}

// ---- visibility and exportability (1.5, S4) --------------------------------

const EXPORTS: &str = r#"module m.x

type Secret(Text)

capability Vault {
    fn open(k: Text) -> Int
}

type Small = Int where self < 10

pub fn param(s: Secret) -> Int {
    1
}

pub fn ret(n: Int) -> Option<List<Secret>> {
    None
}

pub fn handle(n: Int, using v: Vault) -> Int
    effects { v.open }
{
    v.open("k")
}

pub capability Door {
    fn knock(s: Secret) -> Small
}

pub struct Holder<T> {
    first: T
    second: Result<T, Secret>
}

pub enum Carrier {
    Wrapped(Int, Secret)
}

pub type Outer(Small)

pub type Tiny = Small where self > 0

pub type Many = List<Secret> where self.len > 0
"#;

#[test]
fn private_types_in_every_exported_position_are_reported() {
    // 1.5: every parameter type, `using` capability type, and return type of
    // a `pub fn`; every operation parameter and return type of a `pub
    // capability`; every field, payload, wrapper payload, and refinement
    // base, nested type arguments included. The diagnostic spans the
    // offending type (S63 (3)) and carries `symbol` (the public
    // declaration), `facet: "signature"`, and `type` (the private type's
    // qualified name).
    let result = check_source(EXPORTS);
    let expect = |line_needle: &str, token: &str, symbol: &str, ty: &str| {
        let (line, col) = pos_in(EXPORTS, line_needle, token);
        (line, col, symbol.to_string(), ty.to_string())
    };
    let expected = vec![
        expect("pub fn param", "Secret", "m.x.param", "m.x.Secret"),
        expect("pub fn ret", "Secret", "m.x.ret", "m.x.Secret"),
        expect("pub fn handle", "Vault", "m.x.handle", "m.x.Vault"),
        expect("fn knock", "Secret", "m.x.Door", "m.x.Secret"),
        expect("fn knock", "Small", "m.x.Door", "m.x.Small"),
        expect("second:", "Secret", "m.x.Holder", "m.x.Secret"),
        expect("Wrapped(", "Secret", "m.x.Carrier", "m.x.Secret"),
        expect("pub type Outer", "Small", "m.x.Outer", "m.x.Small"),
        expect("pub type Tiny", "Small", "m.x.Tiny", "m.x.Small"),
        expect("pub type Many", "Secret", "m.x.Many", "m.x.Secret"),
    ];
    let j = json_values(&result);
    assert!(
        j.iter()
            .all(|d| d["code"] == json!("private_type_in_public_signature")),
        "{}",
        render(&result)
    );
    let got: Vec<(u32, u32, String, String)> = j
        .iter()
        .map(|d| {
            assert_eq!(d["facet"], json!("signature"), "{d}");
            assert_eq!(d["qualified_name"], d["symbol"], "{d}");
            let (line, col) = span_of(d);
            (
                line,
                col,
                d["symbol"].as_str().unwrap_or("").to_string(),
                d["type"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    assert_eq!(sorted(got), sorted(expected), "{}", render(&result));
}

#[test]
fn exportable_signatures_are_accepted() {
    // 1.5: prelude types, library value types, type parameters (even one
    // named like a private type of the module, scope 3 first), and public
    // declarations are exportable; private declarations, and bodies of
    // public ones, may mention anything visible.
    let source = r#"module m.ok

pub type Id(Text)

pub type Positive = Int where self > 0

pub capability Store {
    fn get(k: Id) -> Positive
}

pub struct Box<T> {
    v: T
    id: Option<Id>
}

struct T {
    v: Int
}

type Secret(Int)

pub fn wrap<T>(x: T) -> Box<T> {
    Box { v: x, id: None }
}

pub fn library_types(e: std.foreign.ForeignError, c: ConstraintError) -> Int {
    let _s: Option<Secret> = None
    1
}

fn private_may_mention(s: Secret) -> Secret {
    s
}
"#;
    let result = check_source(source);
    assert!(result.diagnostics.is_empty(), "{}", render(&result));
}

#[test]
fn a_private_type_from_a_sibling_file_is_not_exportable() {
    // 1.5: being visible in every file of the module does not make a
    // private type exportable.
    let result = check_files(&[
        (
            "src/m/s.cpl",
            "module m.s\n\nstruct Secret {\n    v: Int\n}\n",
        ),
        (
            "src/m/s/api.cpl",
            "module m.s\n\npub fn leak() -> Secret {\n    Secret { v: 1 }\n}\n",
        ),
    ]);
    assert_eq!(
        located(&result),
        [at("src/m/s/api.cpl", 3, "private_type_in_public_signature")],
        "{}",
        render(&result)
    );
    let d = &json_values(&result)[0];
    assert_eq!(d["symbol"], json!("m.s.leak"));
    assert_eq!(d["type"], json!("m.s.Secret"));
}

#[test]
fn importing_a_private_type_is_one_diagnostic() {
    // 1.5, 1.6: `private_item` on the use item with `symbol` and
    // `qualified_name`; every use of the name is then error-typed silently
    // (S62), so the signature has no `unresolved_type`.
    let result = check_files(&[
        ("src/lib/s.cpl", "module lib.s\n\ntype Secret(Int)\n"),
        (
            "src/app/u.cpl",
            "module app.u\n\nuse lib.s.{Secret}\n\nfn f(s: Secret) -> Secret {\n    s\n}\n",
        ),
    ]);
    assert_eq!(
        located(&result),
        [at("src/app/u.cpl", 3, "private_item")],
        "{}",
        render(&result)
    );
    let d = &json_values(&result)[0];
    assert_eq!(d["symbol"], json!("lib.s.Secret"));
    assert_eq!(d["qualified_name"], json!("lib.s.Secret"));
}

const CONTRACTS: &str = r#"module m.c

fn ok(n: Int) -> Bool {
    n >= 0
}

fn measure(n: Int) -> Int {
    n
}

pub fn ensured(n: Int) -> Int
    ensures ok(result)
{
    n
}

pub fn counted(n: Int) -> Int
    requires n >= 0
    decreases measure(n)
{
    if n == 0 { 0 } else { counted(n - 1) }
}

pub type Checked = Int where ok(self)

type Quiet = Int where ok(self)

fn private_contract(n: Int) -> Int
    requires ok(n)
{
    n
}

pub fn public_helpers(xs: List<List<Int>>) -> Int
    requires flatten(xs).len >= 0
    requires public_check(xs.len)
{
    xs.len
}

pub fn public_check(n: Int) -> Bool {
    n >= 0
}
"#;

#[test]
fn public_contracts_may_not_call_private_functions_in_any_clause() {
    // S4: `ensures`, `decreases`, and a public refinement's `where` are part
    // of a public declaration's contract facet; each call of a private
    // function there is `private_in_public_contract` spanning the clause
    // from its keyword (S63 (2)) with `symbol`, `facet: "contract"`,
    // `clause`, and `callee`. Private declarations' contracts and calls of
    // public or prelude functions are fine.
    let result = check_source(CONTRACTS);
    let (where_line, where_col) = pos(CONTRACTS, "where ok(self)");
    let expected = vec![
        (
            line_of(CONTRACTS, "ensures ok(result)"),
            5,
            "m.c.ensured".to_string(),
            "ok(result)".to_string(),
            "m.c.ok".to_string(),
        ),
        (
            line_of(CONTRACTS, "decreases measure(n)"),
            5,
            "m.c.counted".to_string(),
            "measure(n)".to_string(),
            "m.c.measure".to_string(),
        ),
        (
            where_line,
            where_col,
            "m.c.Checked".to_string(),
            "ok(self)".to_string(),
            "m.c.ok".to_string(),
        ),
    ];
    let j = json_values(&result);
    assert!(
        j.iter()
            .all(|d| d["code"] == json!("private_in_public_contract")),
        "{}",
        render(&result)
    );
    let got: Vec<(u32, u32, String, String, String)> = j
        .iter()
        .map(|d| {
            assert_eq!(d["facet"], json!("contract"), "{d}");
            let (line, col) = span_of(d);
            (
                line,
                col,
                d["symbol"].as_str().unwrap_or("").to_string(),
                d["clause"].as_str().unwrap_or("").to_string(),
                d["callee"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    assert_eq!(sorted(got), sorted(expected), "{}", render(&result));
}

#[test]
fn a_public_capabilitys_operation_contracts_are_public_contracts() {
    // 1.3: the contract facet of a capability is every operation's
    // `requires`/`ensures`; S4 applies to it when the capability is public.
    let source = "module m.k\n\nfn valid(k: Text) -> Bool {\n    k.len > 0\n}\n\npub capability Store {\n    fn get(k: Text) -> Int\n        requires valid(k)\n}\n\ncapability Quiet {\n    fn get(k: Text) -> Int\n        requires valid(k)\n}\n";
    let result = check_source(source);
    assert_eq!(
        lines(&result),
        [lc(9, "private_in_public_contract")],
        "{}",
        render(&result)
    );
    let d = &json_values(&result)[0];
    assert_eq!(d["facet"], json!("contract"));
    assert_eq!(d["clause"], json!("valid(k)"));
    assert_eq!(d["callee"], json!("m.k.valid"));
}

// ---- qualified chains through modules versus values (2.4) ------------------

#[test]
fn locals_and_parameters_hide_root_modules_only_in_value_chains() {
    // 2.4: "A local binding named like a root module hides the module for
    // value chains; type positions and use paths are unaffected." A chain
    // on the local is a member chain typed by 3.9: `unknown_member` (with
    // `name` and `type`) on the `Int`, nothing after it (S62).
    let user = r#"module app.u

fn by_param(lib: Text, id: lib.a.Id) -> lib.a.Id {
    let _n = lib.len
    id
}

fn by_local() -> Int {
    let lib = "abc"
    lib.len
}

fn hidden() -> Int {
    let lib = 1
    lib.a.f()
}
"#;
    let result = check_files(&[("src/lib/a.cpl", LIB_A), ("src/app/u.cpl", user)]);
    assert_eq!(
        located(&result),
        [at("src/app/u.cpl", 15, "unknown_member")],
        "{}",
        render(&result)
    );
    let d = &json_values(&result)[0];
    assert_eq!(d["name"], json!("a"));
    assert_eq!(d["type"], json!("Int"));
}

#[test]
fn a_module_function_named_like_a_root_module_hides_it_in_that_module() {
    // 2.4 step 1: a root segment that resolves, scopes 10 to 1, to a
    // function makes a value chain; the module's own `lib` (scope 2) is
    // found before the root module name `lib` (scope 1), so `lib.a` is no
    // member of a function: `unresolved_name`. Other modules are unaffected.
    let u = "module app.u\n\nfn lib() -> Int {\n    0\n}\n\nfn g() -> Int {\n    lib.a.f()\n}\n\nfn h() -> Int {\n    lib()\n}\n";
    let v = "module app.v\n\nfn g() -> Int {\n    lib.a.f()\n}\n";
    let result = check_files(&[
        ("src/lib/a.cpl", LIB_A),
        ("src/app/u.cpl", u),
        ("src/app/v.cpl", v),
    ]);
    assert_eq!(
        located(&result),
        [at("src/app/u.cpl", 8, "unresolved_name")],
        "{}",
        render(&result)
    );
    assert_eq!(json_values(&result)[0]["name"], json!("a"));
}

#[test]
fn members_are_not_reached_through_type_names() {
    // 2.4 step 2: after a type that is not an enum, a further segment is
    // `unresolved_name` ("not a member of ... type"); members are reached
    // through receiver values only (stdlib S37, S38: no UFCS), so neither
    // `Int.max` nor `List.len` nor the stub's qualified name
    // `std.prelude.List.take` is a callee.
    let source = "module app.ty\n\nfn f(xs: List<Int>) -> Int {\n    Int.max(1, 2) + List.len + xs.len\n}\n\nfn g(xs: List<Int>) -> List<Int> {\n    std.prelude.List.take(xs, 1)\n}\n";
    let result = check_source(source);
    assert_eq!(
        lines(&result),
        [
            lc(4, "unresolved_name"),
            lc(4, "unresolved_name"),
            lc(8, "unresolved_name"),
        ],
        "{}",
        render(&result)
    );
    assert_eq!(names(&json_values(&result)), ["max", "len", "take"]);
}

#[test]
fn std_paths_need_no_import_and_report_missing_items_and_modules() {
    // 2.1 (stdlib S1): `std` is a root module name, so fully qualified std
    // paths resolve with no `use`; an item the module lacks is
    // `unresolved_name` with `module`; `std.list` does not exist in 0.1
    // (stdlib S33): `std` is only a prefix, so `unresolved_module`.
    // The `module` of both `std.list` diagnostics is the missing module.
    let source = "module app.std\n\nuse std.list.{take}\n\nfn f(t: Text) -> Text {\n    std.text.trim(t)\n}\n\nfn g(t: Text) -> Text {\n    std.text.nope(t)\n}\n\nfn h(xs: List<Int>) -> List<Int> {\n    std.list.take(xs, 1)\n}\n";
    let result = check_source(source);
    assert_eq!(
        lines(&result),
        [
            lc(3, "unresolved_module"),
            lc(line_of(source, "std.text.nope"), "unresolved_name"),
            lc(line_of(source, "std.list.take"), "unresolved_module"),
        ],
        "{}",
        render(&result)
    );
    let j = json_values(&result);
    assert_eq!(j[0]["module"], json!("std.list"));
    assert_eq!(j[1]["name"], json!("nope"));
    assert_eq!(j[1]["module"], json!("std.text"));
    assert_eq!(j[2]["module"], json!("std.list"));
}

// ---- std-only annotations in project files (1.8, stdlib 10.2) --------------

const ANNOTATIONS: &str = r#"module app.ann

@builtin("list_len")
pub fn length(xs: List<Int>) -> Int

@builtin("int")
pub type Number

@method
fn count(xs: List<Int>) -> Int {
    xs.len
}

@property
fn size(xs: List<Int>) -> Int {
    xs.len
}

@builtin("list_len")
@method
fn both(xs: List<Int>) -> Int {
    xs.len
}

@method(1)
fn with_argument(xs: List<Int>) -> Int {
    xs.len
}
"#;

#[test]
fn std_only_annotations_are_unknown_in_project_files() {
    // 1.8, stdlib 10.2, toolchain 3.6: `@builtin`, `@method`, and
    // `@property` are `unknown_annotation` in a project file ("builtin forms
    // are permitted only in the standard library"), spanning the annotation
    // (S63 (3)) with `name`, never `annotation_misplaced` there; a bodiless
    // `fn` or definition-less `type` outside the standard library is also
    // `missing_body` at its name, whatever its annotations, and the rest of
    // the file is checked. An unknown annotation's arguments are not
    // validated (`@method(1)` is `unknown_annotation` alone).
    let result = check_source(ANNOTATIONS);
    let both = line_of(ANNOTATIONS, "fn both");
    let expected = vec![
        (
            line_of(ANNOTATIONS, "pub fn length") - 1,
            1,
            "unknown_annotation",
            "builtin",
        ),
        (
            pos(ANNOTATIONS, "length(").0,
            pos(ANNOTATIONS, "length(").1,
            "missing_body",
            "length",
        ),
        (
            line_of(ANNOTATIONS, "@builtin(\"int\")"),
            1,
            "unknown_annotation",
            "builtin",
        ),
        (
            pos(ANNOTATIONS, "Number").0,
            pos(ANNOTATIONS, "Number").1,
            "missing_body",
            "Number",
        ),
        (
            line_of(ANNOTATIONS, "@method"),
            1,
            "unknown_annotation",
            "method",
        ),
        (
            line_of(ANNOTATIONS, "@property"),
            1,
            "unknown_annotation",
            "property",
        ),
        (both - 2, 1, "unknown_annotation", "builtin"),
        (both - 1, 1, "unknown_annotation", "method"),
        (
            line_of(ANNOTATIONS, "fn with_argument") - 1,
            1,
            "unknown_annotation",
            "method",
        ),
    ];
    let expected: Vec<(u32, u32, String, String)> = expected
        .into_iter()
        .map(|(l, c, code, name)| (l, c, code.to_string(), name.to_string()))
        .collect();
    let j = json_values(&result);
    let got: Vec<(u32, u32, String, String)> = j
        .iter()
        .map(|d| {
            let (line, col) = span_of(d);
            (
                line,
                col,
                d["code"].as_str().unwrap_or("").to_string(),
                d["name"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    assert_eq!(sorted(got), sorted(expected), "{}", render(&result));
    for d in j
        .iter()
        .filter(|d| d["code"] == json!("unknown_annotation"))
    {
        assert!(
            d["message"].as_str().is_some_and(
                |m| m.contains("builtin forms are permitted only in the standard library")
            ),
            "{d}"
        );
    }
}

// ---- stable and provisional identities (S2, toolchain 4.1, 4.3, 4.5) -------

#[test]
fn provisional_identities_are_qualified_names_with_quoted_test_names() {
    // S2, toolchain 4.3: without `@id` the identity is the qualified name,
    // flagged provisional; a test or property name is the final segment in
    // double quotes, canonically escaped (4.3, T22). `by_stable_id` covers
    // the project and std (4.5).
    let source = r#"module app.ids

pub fn f() -> Int {
    1
}

pub type W(Int)

pub type R = Int where self > 0

pub struct S {
    v: Int
}

pub enum E {
    A
}

pub capability C {
    fn op() -> Int
}

test "say \"hi\"\tnow" {
    assert true
}

property "p\\q"
    for n in gen.int
{
    assert n == n
}

test "\u{1}\u{E9}\u{7F}" {
    assert true
}

test "x.y" {
    assert true
}
"#;
    let result = check_source(source);
    assert!(result.diagnostics.is_empty(), "{}", render(&result));
    let p = &result.program;
    for name in ["f", "W", "R", "S", "E", "C"] {
        let id = def_id(p, &format!("app.ids.{name}"));
        let d = p.def(id);
        assert!(d.provisional, "{name}");
        assert_eq!(d.stable_id.as_str(), format!("app.ids.{name}"));
        assert_eq!(p.by_stable_id(&d.stable_id), Some(id));
    }
    let module = vec!["app".to_string(), "ids".to_string()];
    let test = p
        .lookup(&SymbolPath::new(
            module.clone(),
            DeclName::Text("say \"hi\"\tnow".to_string()),
        ))
        .expect("the test");
    assert!(p.def(test).provisional);
    assert_eq!(
        p.def(test).stable_id.as_str(),
        "app.ids.\"say \\\"hi\\\"\\tnow\""
    );
    // Toolchain 2.4 rule 10: the other control characters and DEL as
    // `\u{H}` with minimal lowercase digits, every other scalar literally.
    let escaped = p
        .lookup(&SymbolPath::new(
            module.clone(),
            DeclName::Text("\u{1}\u{e9}\u{7f}".to_string()),
        ))
        .expect("the escaped test");
    assert_eq!(
        p.def(escaped).stable_id.as_str(),
        "app.ids.\"\\u{1}\u{e9}\\u{7f}\""
    );
    let dotted = p
        .lookup(&SymbolPath::new(
            module.clone(),
            DeclName::Text("x.y".to_string()),
        ))
        .expect("the dotted test");
    assert_eq!(p.def(dotted).stable_id.as_str(), "app.ids.\"x.y\"");
    assert_eq!(
        p.by_stable_id(&StableId::parse("app.ids.\"x.y\"").expect("an id")),
        Some(dotted)
    );
    let property = p
        .lookup(&SymbolPath::new(module, DeclName::Text("p\\q".to_string())))
        .expect("the property");
    assert_eq!(p.def(property).stable_id.as_str(), "app.ids.\"p\\\\q\"");
    assert_eq!(
        p.by_stable_id(&StableId::parse("app.ids.\"p\\\\q\"").expect("an id")),
        Some(property)
    );
    assert_eq!(
        p.by_stable_id(&StableId::parse("std.prelude.flatten").expect("an id")),
        Some(def_id(p, "std.prelude.flatten"))
    );
}

#[test]
fn id_arguments_follow_the_accepted_syntax_and_length() {
    // Toolchain 4.1: `^[a-z][a-z0-9]*(_[a-z0-9]+)+$`, 4 to 64 characters;
    // anything else is `invalid_annotation_argument` (S2, 1.8) with `name`,
    // and the declaration keeps its provisional identity. An invalid value
    // is no identity, so two equal invalid values are no
    // `duplicate_identity`.
    let long64 = format!("ab_{}", "c".repeat(61));
    let long65 = format!("ab_{}", "c".repeat(62));
    let valid = ["ab_c", long64.as_str(), "pgn_7c2d", "a1_2b", "x_1_2"];
    let invalid = [
        "a_b",
        long65.as_str(),
        "a__b",
        "ab_",
        "1a_b",
        "abcd",
        "ab_Cd",
        "app.idv.f",
        "_ab_c",
        "ab-cd",
        "ab_\u{e9}",
        "Same_Bad",
        "Same_Bad",
    ];
    let mut source = String::from("module app.idv\n");
    for (i, v) in valid.iter().enumerate() {
        source.push_str(&format!(
            "\n@id(\"{v}\")\nfn valid_{i}() -> Int {{\n    1\n}}\n"
        ));
    }
    for (i, v) in invalid.iter().enumerate() {
        source.push_str(&format!(
            "\n@id(\"{v}\")\nfn invalid_{i}() -> Int {{\n    1\n}}\n"
        ));
    }
    let result = check_source(&source);
    let expected: Vec<(u32, String)> = (0..invalid.len())
        .map(|i| {
            lc(
                line_of(&source, &format!("fn invalid_{i}(")) - 1,
                "invalid_annotation_argument",
            )
        })
        .collect();
    assert_eq!(lines(&result), expected, "{}", render(&result));
    assert!(json_values(&result)
        .iter()
        .all(|d| d["name"] == json!("id")));
    let p = &result.program;
    for (i, v) in valid.iter().enumerate() {
        let d = p.def(def_id(p, &format!("app.idv.valid_{i}")));
        assert_eq!(d.stable_id.as_str(), *v);
        assert!(!d.provisional, "{v}");
    }
    for i in 0..invalid.len() {
        let d = p.def(def_id(p, &format!("app.idv.invalid_{i}")));
        assert_eq!(d.stable_id.as_str(), format!("app.idv.invalid_{i}"));
        assert!(d.provisional);
    }
}

const DUPLICATE_ID: &str = r#"module app.dup

@id("same_id")
fn first() -> Int {
    1
}

@id("same_id")
pub struct Second {
    v: Int
}
"#;

#[test]
fn duplicate_identity_spans_the_later_declarations_name() {
    // S2, toolchain 4.5: two equal `@id` arguments are one
    // `duplicate_identity` (S63 (4): one diagnostic, at the later
    // occurrence). S63 (3) lists `duplicate_identity` among the diagnostics
    // about a declaration as a whole, which span the declaration's *name
    // token*, not the annotation: here `Second`.
    let result = check_source(DUPLICATE_ID);
    let j = json_values(&result);
    assert_eq!(
        codes(&result),
        ["duplicate_identity"],
        "{}",
        render(&result)
    );
    let d = &j[0];
    assert_eq!(d["severity"], json!("error"));
    assert_eq!(d["category"], json!("resolve"));
    assert_eq!(d["qualified_name"], json!("app.dup.Second"));
    assert_eq!(span_of(d), pos(DUPLICATE_ID, "Second"), "{d}");
}

#[test]
fn duplicate_identity_notes_the_earlier_declaration() {
    // S63 (4): the notes carry the earlier occurrence; for a diagnostic
    // about a declaration as a whole (S63 (3)) that is the earlier
    // declaration, located by its name token (`first`).
    let result = check_source(DUPLICATE_ID);
    let d = &with_code(&result, "duplicate_identity")[0];
    let notes = d["notes"].as_array().cloned().unwrap_or_default();
    assert_eq!(notes.len(), 1, "{d}");
    let (line, col) = pos(DUPLICATE_ID, "first");
    assert_eq!(notes[0]["span"]["line"], json!(line), "{d}");
    assert_eq!(notes[0]["span"]["col"], json!(col), "{d}");
}

#[test]
fn duplicate_identities_across_files_and_kinds() {
    // Toolchain 4.5: identities are unique across the program whatever the
    // modules, files, and kinds; every later occurrence is a
    // `duplicate_identity` whose note names another file; the first keeps
    // the identity and every later definition is given its provisional
    // identity and `HasErrors`.
    let result = check_files(&[
        (
            "src/a/one.cpl",
            "module a.one\n\n@id(\"shared_id\")\npub fn f() -> Int {\n    1\n}\n",
        ),
        (
            "src/a/two.cpl",
            "module a.two\n\n@id(\"shared_id\")\ntest \"t\" {\n    assert true\n}\n",
        ),
        (
            "src/b.cpl",
            "module b\n\n@id(\"shared_id\")\npub enum E {\n    X\n}\n",
        ),
    ]);
    let j = json_values(&result);
    assert_eq!(
        codes(&result),
        ["duplicate_identity", "duplicate_identity"],
        "{}",
        render(&result)
    );
    let files: Vec<&str> = j
        .iter()
        .filter_map(|d| d["span"]["file"].as_str())
        .collect();
    assert_eq!(files, ["src/a/two.cpl", "src/b.cpl"]);
    for d in &j {
        let (note_file, _) = note_at(d, 0);
        assert!(!note_file.is_empty(), "{d}");
        assert_ne!(
            note_file.as_str(),
            d["span"]["file"].as_str().unwrap_or(""),
            "{d}"
        );
    }
    let p = &result.program;
    let f = def_id(p, "a.one.f");
    assert_eq!(
        p.by_stable_id(&StableId::parse("shared_id").expect("an id")),
        Some(f)
    );
    assert!(!p.def(f).provisional);
    let test = p
        .lookup(&SymbolPath::new(
            vec!["a".to_string(), "two".to_string()],
            DeclName::Text("t".to_string()),
        ))
        .expect("the test");
    let e = def_id(p, "b.E");
    for (id, provisional) in [(test, "a.two.\"t\""), (e, "b.E")] {
        let d = p.def(id);
        assert_eq!(d.stable_id.as_str(), provisional);
        assert!(d.provisional);
        assert_eq!(d.status, DefStatus::HasErrors);
    }
}

const KINDS: &str = r#"module app.k

@id("test_abcdefghijklmnop")
test "a test" {
    assert true
}

@id("prop_abcdefghijklmnop")
property "a property"
    for n in gen.int
{
    assert n == n
}

@id("cap_abcdefghijklmnop")
pub capability C {
    fn op() -> Int
}

@id("ty_abcdefghijklmnop")
pub type R = Int where self > 0

@id("ty_bcdefghijklmnopq")
pub enum E {
    A
}

@id("prop_bcdefghijklmnopq")
test "mismatched test" {
    assert true
}

@id("test_bcdefghijklmnopq")
property "mismatched property"
    for n in gen.int
{
    assert n == n
}

@id("fn_bcdefghijklmnopq")
pub struct S {
    v: Int
}

@id("ty_k7w3qz2mb9ptx4ve")
fn nine() -> Int {
    1
}

@id("fn_abcdefghijklmno")
pub struct Short {
    v: Int
}

@id("cap_abcdefghijklmnopq")
fn long() -> Int {
    1
}

@id("ty_abcdefghijklmn01")
fn digits() -> Int {
    1
}

@id("ty_cdefghijklmnopqr")
pub type Label(Text)

@id("ty_defghijklmnopqrs")
pub capability Wrong {
    fn op() -> Int
}
"#;

#[test]
fn id_kind_mismatch_warns_only_for_generated_ids_of_another_kind() {
    // T21: `id_kind_mismatch` is a warning, and only for an id in the
    // generated format `^(fn|ty|cap|test|prop)_[a-z2-7]{16}$` (toolchain
    // 4.1) whose prefix is not the declaration's kind (`test` for a test,
    // `prop` for a property, `cap`, `ty` for every type form). Ids with a
    // digit outside 2-7, or 15 or 17 characters after the prefix, are not
    // in the generated format and never warn. The identity holds either way.
    let result = check_source(KINDS);
    let got = with_code(&result, "id_kind_mismatch");
    assert_eq!(result.diagnostics.len(), 4, "{}", render(&result));
    assert_eq!(got.len(), 4, "{}", render(&result));
    for d in &got {
        assert_eq!(d["severity"], json!("warning"));
        assert_eq!(d["category"], json!("resolve"));
    }
    for id in [
        "prop_bcdefghijklmnopq",
        "test_bcdefghijklmnopq",
        "fn_bcdefghijklmnopq",
        "ty_defghijklmnopqrs",
    ] {
        let annotation = line_of(KINDS, &format!("@id(\"{id}\")"));
        let hits = got
            .iter()
            .filter(|d| {
                let line = span_of(d).0;
                line == annotation || line == annotation + 1
            })
            .count();
        assert_eq!(hits, 1, "{id}: {}", render(&result));
    }
    let p = &result.program;
    for id in [
        "test_abcdefghijklmnop",
        "prop_abcdefghijklmnop",
        "cap_abcdefghijklmnop",
        "ty_abcdefghijklmnop",
        "ty_bcdefghijklmnopq",
        "prop_bcdefghijklmnopq",
        "test_bcdefghijklmnopq",
        "fn_bcdefghijklmnopq",
        "ty_k7w3qz2mb9ptx4ve",
        "fn_abcdefghijklmno",
        "cap_abcdefghijklmnopq",
        "ty_abcdefghijklmn01",
        "ty_cdefghijklmnopqr",
        "ty_defghijklmnopqrs",
    ] {
        let def = p
            .by_stable_id(&StableId::parse(id).expect("an id"))
            .unwrap_or_else(|| panic!("no definition has the identity {id}"));
        assert!(!p.def(def).provisional, "{id}");
    }
}

// ---- FileId and DefId determinism (toolchain 3.2, 3.3, T15) ----------------

/// A project with several modules, a module over two files, a misplaced
/// file, a work file, an error, and a warning.
const DETERMINISM: &[(&str, &str)] = &[
    ("cpl.toml", "[project]\nname = \"t\"\nedition = \"0.1\"\n"),
    (
        "src/zeta.cpl",
        "module zeta\n\npub fn z() -> Int {\n    alpha.a() + 1\n}\n",
    ),
    ("src/alpha.cpl", "module alpha\n\npub fn a() -> Int {\n    1\n}\n"),
    (
        "src/alpha/more.cpl",
        "module alpha\n\nfn b() -> Int {\n    a()\n}\n\ntest \"alpha works\" {\n    assert a() == 1\n}\n",
    ),
    (
        "src/misplaced.cpl",
        "module mid.way\n\npub fn m() -> Int {\n    nowhere()\n}\n",
    ),
    ("work/plan.work", "change plan {\n    workers 1\n}\n"),
];

#[test]
fn two_loads_of_one_tree_assign_identical_ids() {
    // T15: `FileId`s, and with them `DefId`s in (file, declaration) order,
    // are a deterministic function of the source set and std.
    let t = tree(DETERMINISM);
    let first = check(&load_tree(&t));
    let second = check(&load_tree(&t));
    assert!(first.has_errors(), "{}", render(&first));
    assert_eq!(fingerprint(&first), fingerprint(&second));
}

#[test]
fn a_directory_and_its_tree_load_with_identical_ids() {
    // Toolchain 1.6: "Loading from a tree and from a directory produce
    // identical Projects", whatever order the file system lists the
    // directory in (the files are written in reverse path order here).
    let dir = scratch_dir("dir_and_tree");
    for (path, text) in DETERMINISM.iter().rev() {
        let full = dir.join(path);
        fs::create_dir_all(full.parent().expect("a parent directory")).expect("create a directory");
        fs::write(&full, text).expect("write a file");
    }
    let from_dir = check(&load_dir(&dir));
    let from_tree = check(&load_tree(&tree_of_dir(&dir)));
    assert_eq!(fingerprint(&from_dir), fingerprint(&from_tree));
    let project: Vec<String> = from_dir
        .program
        .source_map()
        .files()
        .filter(|f| f.origin != FileOrigin::Std)
        .map(|f| f.path.clone())
        .collect();
    assert_eq!(project, sorted(project.clone()));
}

#[test]
fn file_ids_put_std_first_then_project_paths_in_lexicographic_order() {
    // Toolchain 3.2 and T15: std files in their sorted order, then the
    // project's source-set files in lexicographic order of project-relative
    // path, across every `[build] sources` directory and `work/` (not in
    // the order of the `sources` list); `files[i].file` is `FileId(i)`.
    let manifest =
        "[project]\nname = \"t\"\nedition = \"0.1\"\n\n[build]\nsources = [\"src\", \"lib\"]\n";
    let project = load_tree(&tree(&[
        ("cpl.toml", manifest),
        ("work/w.work", "change c {\n    workers 1\n}\n"),
        ("src/a_b.cpl", "module a_b\n"),
        ("src/a/b.cpl", "module a.b\n"),
        ("src/a.cpl", "module a\n"),
        ("lib/z.cpl", "module z\n"),
    ]));
    let paths: Vec<&str> = project
        .source_map
        .files()
        .map(|f| f.path.as_str())
        .collect();
    assert_eq!(
        paths,
        [
            "std/bytes.cpl",
            "std/foreign.cpl",
            "std/gen.cpl",
            "std/prelude.cpl",
            "std/text.cpl",
            "lib/z.cpl",
            "src/a.cpl",
            "src/a/b.cpl",
            "src/a_b.cpl",
            "work/w.work",
        ]
    );
    for (i, entry) in project.files.iter().enumerate() {
        assert_eq!(entry.file, FileId(i as u32));
    }
    let result = check(&project);
    assert!(result.diagnostics.is_empty(), "{}", render(&result));
}

#[test]
fn work_files_take_file_ids_whatever_the_load_options() {
    // Toolchain 0 and 3.2: `.work` files are members of the source set, so
    // they hold their `FileId` whether or not they are parsed (`cpl run`
    // and `cpl test` do not parse them, `cpl check` does): every later file
    // and definition keeps its id.
    let manifest =
        "[project]\nname = \"t\"\nedition = \"0.1\"\n\n[build]\nsources = [\"src\", \"zz\"]\n";
    let t = tree(&[
        ("cpl.toml", manifest),
        ("src/a.cpl", "module a\n\npub fn f() -> Int {\n    1\n}\n"),
        ("work/plan.work", "change plan {\n    workers 1\n}\n"),
        (
            "zz/late.cpl",
            "module late\n\npub fn g() -> Int {\n    2\n}\n",
        ),
    ]);
    let parsed = load_project_from_tree(
        &t,
        FileOrigin::Virtual,
        &LoadOptions {
            include_work_files: true,
            ..LoadOptions::default()
        },
    )
    .expect("loads");
    let skipped =
        load_project_from_tree(&t, FileOrigin::Virtual, &LoadOptions::default()).expect("loads");
    let work = skipped
        .source_map
        .find("work/plan.work")
        .expect("the work file is registered");
    assert!(matches!(
        skipped.parsed[work.0 as usize],
        ParsedFile::Skipped
    ));
    let a = check(&parsed);
    let b = check(&skipped);
    assert!(a.diagnostics.is_empty(), "{}", render(&a));
    assert!(b.diagnostics.is_empty(), "{}", render(&b));
    assert_eq!(fingerprint(&a), fingerprint(&b));
}

#[test]
fn definitions_follow_file_order_then_declaration_order() {
    // Toolchain 3.3 and 3.7 rules 1-2: std definitions first, then every
    // project definition in (file path, declaration order), whatever the
    // module paths; `ModuleDef::files` in path order and `ModuleDef::defs`
    // in (file, declaration) order.
    let result = check_files(&[
        (
            "src/k/zz.cpl",
            "module k\n\nfn zz_first() -> Int {\n    1\n}\n\nfn zz_second() -> Int {\n    2\n}\n",
        ),
        (
            "src/k/aa.cpl",
            "module k\n\nfn aa_only() -> Int {\n    3\n}\n",
        ),
        (
            "src/a/late.cpl",
            "module a.late\n\nfn late() -> Int {\n    4\n}\n",
        ),
    ]);
    assert!(result.diagnostics.is_empty(), "{}", render(&result));
    let p = &result.program;
    let is_std = |d: &cpl_hir::Def| p.module(d.module).origin == FileOrigin::Std;
    let first_project = p
        .defs()
        .position(|d| !is_std(d))
        .expect("a project definition");
    assert!(
        p.defs().skip(first_project).all(|d| !is_std(d)),
        "std definitions come first"
    );
    let project: Vec<String> = p
        .defs()
        .filter(|d| !is_std(d))
        .map(|d| p.display_path(d.id))
        .collect();
    assert_eq!(
        project,
        ["a.late.late", "k.aa_only", "k.zz_first", "k.zz_second"]
    );
    let k = module_def(p, "k");
    assert_eq!(paths(p, &k.files), ["src/k/aa.cpl", "src/k/zz.cpl"]);
    let defs: Vec<String> = k.defs.iter().map(|d| p.display_path(*d)).collect();
    assert_eq!(defs, ["k.aa_only", "k.zz_first", "k.zz_second"]);
}

// ---- further probes of the same rules --------------------------------------

#[test]
fn the_same_alias_name_names_different_modules_in_different_files() {
    // S5: each file's imports are its own, so `fmt` is `tools.fmt` in one
    // file of `app.m` and `other.fmt` in the other.
    let result = check_files(&[
        (
            "src/tools/fmt.cpl",
            "module tools.fmt\n\npub fn width() -> Int {\n    80\n}\n",
        ),
        (
            "src/other/fmt.cpl",
            "module other.fmt\n\npub fn height() -> Int {\n    24\n}\n",
        ),
        (
            "src/app/m.cpl",
            "module app.m\n\nuse tools.{fmt}\n\nfn a() -> Int {\n    fmt.width()\n}\n",
        ),
        (
            "src/app/m/b.cpl",
            "module app.m\n\nuse other.{fmt}\n\nfn b() -> Int {\n    fmt.height()\n}\n",
        ),
    ]);
    assert!(result.diagnostics.is_empty(), "{}", render(&result));
}

#[test]
fn imports_are_not_items_of_the_importing_module() {
    // 1.6 imports a public item *declared* in the named module; a `use`
    // binds a name in its own file only (S5) and CPL has no re-exports
    // (stdlib S1 rejects them even for the prelude), so `app.u`'s import of
    // `Id` is no item of `app.u`: not for a `use`, a type path, or a chain.
    let u = "module app.u\n\nuse lib.a.{Id}\n\npub fn f(x: Id) -> Id {\n    x\n}\n";
    let v = "module app.v\n\nuse app.u.{Id}\n\nfn g(y: app.u.Id) -> Int {\n    let _i = app.u.Id(\"x\")\n    1\n}\n";
    let result = check_files(&[
        ("src/lib/a.cpl", LIB_A),
        ("src/app/u.cpl", u),
        ("src/app/v.cpl", v),
    ]);
    assert_eq!(
        located(&result),
        [
            at("src/app/v.cpl", 3, "unresolved_name"),
            at("src/app/v.cpl", 5, "unresolved_type"),
            at("src/app/v.cpl", 6, "unresolved_name"),
        ],
        "{}",
        render(&result)
    );
    let j = json_values(&result);
    assert!(j.iter().all(|d| d["name"] == json!("Id")));
    assert_eq!(j[0]["module"], json!("app.u"));
    assert_eq!(j[2]["module"], json!("app.u"));
}

/// `lib.k`: one public type of each kind that is not a constructor.
const LIB_K: &str = "module lib.k\n\npub struct Point {\n    x: Int\n}\n\npub type Positive = Int where self > 0\n\npub enum Remote {\n    Same\n}\n\npub capability Store {\n    fn get(k: Text) -> Int\n}\n";

#[test]
fn a_name_that_resolves_through_an_import_uses_it_even_where_no_constructor_is_possible() {
    // 1.6: an import is used when a name of the file resolves through it.
    // `Point`, `Positive`, `Remote`, and `Store` below resolve through the
    // `use` to types, which S10 step 3 rejects as constructors
    // (`not_a_constructor` with `name`) in expression and pattern
    // positions alike. The import did its job (without it the names would
    // be `unresolved_name`), so it is no `unused_import`; reporting it as
    // well would be a cascade whose fix is the `not_a_constructor` fix
    // (S62). The checker already treats an alias or a function named in
    // value position this way (`not_a_value` alone, tested above).
    let user = r#"module app.u

use lib.k.{Point, Positive, Remote, Store}

fn f() -> Int {
    let _p = Point
    let _q = Positive
    let _r = Remote
    let _s = Store
    1
}
"#;
    let pattern = r#"module app.w

use lib.k.{Point}

fn g(p: lib.k.Point) -> Int {
    match p {
        Point => 1
    }
}
"#;
    let result = check_files(&[
        ("src/lib/k.cpl", LIB_K),
        ("src/app/u.cpl", user),
        ("src/app/w.cpl", pattern),
    ]);
    assert_eq!(
        located(&result),
        [
            at("src/app/u.cpl", 6, "not_a_constructor"),
            at("src/app/u.cpl", 7, "not_a_constructor"),
            at("src/app/u.cpl", 8, "not_a_constructor"),
            at("src/app/u.cpl", 9, "not_a_constructor"),
            at("src/app/w.cpl", 7, "not_a_constructor"),
        ],
        "{}",
        render(&result)
    );
    assert_eq!(
        names(&json_values(&result)),
        ["Point", "Positive", "Remote", "Store", "Point"]
    );
}

#[test]
fn a_public_signature_may_name_a_public_refinement_over_a_private_base() {
    // 1.5: "a refinement must itself be public; its base is checked by the
    // rule for its own declaration": the private base `Hidden` makes
    // `pub type R` the one offender, and `pub fn f(r: R)` names a public
    // declaration.
    let source = "module app.r\n\ntype Hidden(Int)\n\npub type R = Hidden where true\n\npub fn f(r: R) -> Int {\n    1\n}\n";
    let result = check_source(source);
    assert_eq!(
        lines(&result),
        [lc(5, "private_type_in_public_signature")],
        "{}",
        render(&result)
    );
    let d = &json_values(&result)[0];
    assert_eq!(d["symbol"], json!("app.r.R"));
    assert_eq!(d["type"], json!("app.r.Hidden"));
}

#[test]
fn contract_calls_are_judged_per_callee_through_any_path() {
    // S4: one `private_in_public_contract` per private callee of a clause
    // (the field is the singular `callee`), whether the call is written
    // bare or as a qualified path to the module's own function; a private
    // function of another module is `private_item` alone (1.5), not also
    // `private_in_public_contract`.
    let c = "module m.c\n\nfn ok(n: Int) -> Bool {\n    n >= 0\n}\n\nfn small(n: Int) -> Bool {\n    n < 10\n}\n\npub fn q(n: Int) -> Int\n    requires m.c.ok(n)\n    requires ok(n) and small(n)\n{\n    n\n}\n";
    let h = "module lib.h\n\nfn secret(n: Int) -> Bool {\n    n > 0\n}\n";
    let u = "module app.u\n\npub fn f(n: Int) -> Int\n    requires lib.h.secret(n)\n{\n    n\n}\n";
    let result = check_files(&[
        ("src/m/c.cpl", c),
        ("src/lib/h.cpl", h),
        ("src/app/u.cpl", u),
    ]);
    assert_eq!(
        sorted(located(&result)),
        [
            at("src/app/u.cpl", 4, "private_item"),
            at("src/m/c.cpl", 12, "private_in_public_contract"),
            at("src/m/c.cpl", 13, "private_in_public_contract"),
            at("src/m/c.cpl", 13, "private_in_public_contract"),
        ],
        "{}",
        render(&result)
    );
    let calls: Vec<(u32, String, String)> = with_code(&result, "private_in_public_contract")
        .iter()
        .map(|d| {
            (
                span_of(d).0,
                d["clause"].as_str().unwrap_or("").to_string(),
                d["callee"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    let expected = [
        (12, "m.c.ok(n)", "m.c.ok"),
        (13, "ok(n) and small(n)", "m.c.ok"),
        (13, "ok(n) and small(n)", "m.c.small"),
    ]
    .map(|(line, clause, callee)| (line, clause.to_string(), callee.to_string()));
    assert_eq!(sorted(calls), expected);
}

/// A copy of the embedded std whose `prelude.cpl` ends with `extra`.
fn std_dir_with_prelude_extra(name: &str, extra: &str) -> PathBuf {
    let dir = scratch_dir(name);
    for (file, text) in cpl_sema::project::STD_FILES {
        let text = if *file == "prelude.cpl" {
            format!("{text}\n{extra}")
        } else {
            text.to_string()
        };
        fs::write(dir.join(file), text).expect("write a std file");
    }
    dir
}

#[test]
fn the_prelude_scope_is_the_public_declarations_of_std_prelude() {
    // stdlib S1 and toolchain 3.6: the prelude is data-driven ("the stub
    // file is the prelude"): a further public stub is in scope everywhere
    // without `use`; a private one is not (`unresolved_name`), and a
    // qualified path to it is `private_item` (1.5).
    let dir = std_dir_with_prelude_extra(
        "prelude_scope",
        "\n@builtin(\"list_len\")\npub fn extra_len(xs: List<Int>) -> Int\n\n@builtin(\"list_len\")\nfn hidden_len(xs: List<Int>) -> Int\n",
    );
    let user = "module app.u\n\nfn a(xs: List<Int>) -> Int {\n    extra_len(xs)\n}\n\nfn b(xs: List<Int>) -> Int {\n    hidden_len(xs)\n}\n\nfn c(xs: List<Int>) -> Int {\n    std.prelude.hidden_len(xs)\n}\n";
    let toml = manifest("t");
    let t = tree(&[("cpl.toml", toml.as_str()), ("src/app/u.cpl", user)]);
    let project = load_project_from_tree(
        &t,
        FileOrigin::Virtual,
        &LoadOptions {
            std: StdSource::Dir(dir),
            include_work_files: true,
        },
    )
    .expect("loads");
    let result = check(&project);
    assert_eq!(
        located(&result),
        [
            at("src/app/u.cpl", 8, "unresolved_name"),
            at("src/app/u.cpl", 12, "private_item"),
        ],
        "{}",
        render(&result)
    );
    let j = json_values(&result);
    assert_eq!(j[0]["name"], json!("hidden_len"));
    assert_eq!(j[1]["symbol"], json!("std.prelude.hidden_len"));
    assert_eq!(j[1]["qualified_name"], json!("std.prelude.hidden_len"));
}

#[test]
fn annotation_names_may_be_keywords_and_are_still_unknown() {
    // D71: an annotation name may be any keyword token; D17 and 1.8: the
    // project vocabulary is `@id` and `@prove`, so `@test`, `@fn`, and
    // `@pub` are `unknown_annotation` with `name`, each spanning its
    // annotation (S63 (3)).
    let result = check_source("module app.kw\n\n@test\n@fn\n@pub\nfn g() -> Int {\n    1\n}\n");
    assert_eq!(
        lines(&result),
        [
            lc(3, "unknown_annotation"),
            lc(4, "unknown_annotation"),
            lc(5, "unknown_annotation"),
        ],
        "{}",
        render(&result)
    );
    let j = json_values(&result);
    assert_eq!(names(&j), ["test", "fn", "pub"]);
    assert!(j.iter().all(|d| span_of(d).1 == 1));
}

#[test]
fn the_check_intrinsic_has_no_qualified_path() {
    // stdlib 1.4: `check` is a typechecker intrinsic with no stub, so it is
    // no item of std.prelude (2.4: `unresolved_name` with `module`); the
    // diagnosed callee stops the chain (S62).
    let source = "module app.ck\n\npub type Positive = Int where self > 0\n\nfn f(n: Int) -> Bool {\n    std.prelude.check<Positive>(n).is_ok()\n}\n";
    let result = check_source(source);
    assert_eq!(
        lines(&result),
        [lc(6, "unresolved_name")],
        "{}",
        render(&result)
    );
    let d = &json_values(&result)[0];
    assert_eq!(d["name"], json!("check"));
    assert_eq!(d["module"], json!("std.prelude"));
}

#[test]
fn constructors_of_a_module_reached_only_through_an_alias_need_a_qualified_path() {
    // S10 step 2: the lexical candidates come from the enums and wrappers
    // declared in the module or *imported into the file*; a submodule alias
    // imports neither, so with no expected type `Same` and `Tag` are
    // `unresolved_name`, while the qualified forms resolve (2.4). Two
    // imported enums with one variant name are `ambiguous_constructor`.
    let r = "module lib.r\n\npub enum Remote {\n    Same\n}\n\npub type Tag(Text)\n";
    let s = "module lib.s\n\npub enum Other {\n    Same\n}\n";
    let u = "module app.u\n\nuse lib.{r}\n\nfn f() -> Int {\n    let _x = Same\n    let _t = Tag(\"x\")\n    let _y = r.Remote.Same\n    let _z = r.Tag(\"y\")\n    1\n}\n";
    let w = "module app.w\n\nuse lib.r.{Remote}\nuse lib.s.{Other}\n\nfn g() -> Int {\n    let _x = Same\n    1\n}\n";
    let result = check_files(&[
        ("src/lib/r.cpl", r),
        ("src/lib/s.cpl", s),
        ("src/app/u.cpl", u),
        ("src/app/w.cpl", w),
    ]);
    assert_eq!(
        located(&result),
        [
            at("src/app/u.cpl", 6, "unresolved_name"),
            at("src/app/u.cpl", 7, "unresolved_name"),
            at("src/app/w.cpl", 7, "ambiguous_constructor"),
        ],
        "{}",
        render(&result)
    );
    let j = json_values(&result);
    assert_eq!(names(&j), ["Same", "Tag", "Same"]);
    let candidates: Vec<String> = j[2]["candidates"]
        .as_array()
        .map(|c| {
            c.iter()
                .map(|n| n.as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        sorted(candidates),
        ["lib.r.Remote.Same", "lib.s.Other.Same"]
    );
}

#[test]
fn methods_fields_locals_and_type_parameters_do_not_use_imports() {
    // 1.6: an import is used only by a name that resolves through it. A
    // method or field name is looked up in its receiver's type (3.9), a
    // local hides the import for the rest of its block (2.3), and a type
    // parameter of the same name hides it in its declaration (scope 3
    // before scope 2, 2.2): each import below is `unused_import`.
    let user = r#"module app.u

use std.text.{contains}
use std.text.{trim}
use lib.a.{Id}

pub struct Holder {
    trim: Int
}

fn f(xs: List<Int>, h: Holder) -> Bool {
    let trim = 3
    xs.contains(trim + h.trim)
}

fn g<Id>(x: Id) -> Id {
    x
}
"#;
    let result = check_files(&[("src/lib/a.cpl", LIB_A), ("src/app/u.cpl", user)]);
    assert_eq!(
        lines(&result),
        [
            lc(3, "unused_import"),
            lc(4, "unused_import"),
            lc(5, "unused_import"),
        ],
        "{}",
        render(&result)
    );
    assert_eq!(names(&json_values(&result)), ["contains", "trim", "Id"]);
}

#[test]
fn files_outside_the_source_set_take_no_file_id() {
    // 1.1 and toolchain 0: the source set is `cpl.toml`, the `.cpl` files
    // under the source directories, and the `.work`/`.patch` files directly
    // in `work/`; nothing else is registered, parsed, or diagnosed.
    let project = load_tree(&tree(&[
        ("cpl.toml", &manifest("t")),
        ("src/a.cpl", "module a\n"),
        ("src/notes.txt", "not a source file"),
        ("tools/broken.cpl", "this is not cpl ((("),
        ("work/nested/deep.work", "garbage {{{"),
        ("work/readme.md", "x"),
    ]));
    let project_paths: Vec<&str> = project
        .source_map
        .files()
        .filter(|f| f.origin != FileOrigin::Std)
        .map(|f| f.path.as_str())
        .collect();
    assert_eq!(project_paths, ["src/a.cpl"]);
    let result = check(&project);
    assert!(result.diagnostics.is_empty(), "{}", render(&result));
}

#[test]
fn adding_a_file_later_in_path_order_keeps_every_earlier_id() {
    // T15: ids are assigned in path order, so a new file that sorts after
    // the others leaves every earlier `FileId` and `DefId` as it was.
    let base = [
        ("src/a.cpl", "module a\n\npub fn one() -> Int {\n    1\n}\n"),
        ("src/b.cpl", "module b\n\npub fn two() -> Int {\n    2\n}\n"),
    ];
    let before = check_files(&base);
    let mut more = base.to_vec();
    more.push((
        "src/c.cpl",
        "module c\n\npub fn three() -> Int {\n    3\n}\n",
    ));
    let after = check_files(&more);
    let ids = |r: &CheckResult| -> Vec<(u32, String, u32)> {
        r.program
            .defs()
            .map(|d| (d.id.0, r.program.display_path(d.id), d.file.0))
            .collect()
    };
    let (old, new) = (ids(&before), ids(&after));
    assert_eq!(new.len(), old.len() + 1);
    assert_eq!(new[..old.len()], old[..]);
    assert_eq!(
        new.last().map(|(_, path, _)| path.as_str()),
        Some("c.three")
    );
}

#[test]
fn std_enums_outside_the_prelude_supply_constructors_only_when_imported() {
    // S10 step 2: only the prelude's enums are candidates everywhere;
    // `std.foreign.ForeignErrorKind` has a variant `Unavailable`, which
    // competes with a module's own `Unavailable` only in a file that imports
    // the enum (1.6: "the standard library is imported exactly like project
    // modules"); the expected type still decides (step 1).
    let s = "module app.s\n\nenum StoreError {\n    Unavailable\n}\n\nfn f() -> Int {\n    let _x = Unavailable\n    1\n}\n";
    let t = "module app.t\n\nuse std.foreign.{ForeignErrorKind}\n\nenum StoreError {\n    Unavailable\n}\n\nfn g() -> ForeignErrorKind {\n    let _y = Unavailable\n    Unavailable\n}\n";
    let result = check_files(&[("src/app/s.cpl", s), ("src/app/t.cpl", t)]);
    assert_eq!(
        located(&result),
        [at("src/app/t.cpl", 10, "ambiguous_constructor")],
        "{}",
        render(&result)
    );
    let candidates: Vec<String> = json_values(&result)[0]["candidates"]
        .as_array()
        .map(|c| {
            c.iter()
                .map(|n| n.as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        sorted(candidates),
        [
            "app.t.StoreError.Unavailable",
            "std.foreign.ForeignErrorKind.Unavailable"
        ]
    );
}

#[test]
fn private_items_stay_private_through_a_submodule_alias() {
    // 1.5 and 2.4: a chain through an alias is a qualified path like any
    // other, so a private item of the aliased module is `private_item` in a
    // type position, in value position, and as a callee (one diagnostic
    // each: no `not_a_value` on top, S62); the alias itself is used.
    let h = "module lib.h\n\nfn secret() -> Int {\n    1\n}\n\ntype Hidden(Int)\n";
    let u = "module app.u\n\nuse lib.{h}\n\nfn f(x: h.Hidden) -> Int {\n    let _g = h.secret\n    h.secret()\n}\n";
    let result = check_files(&[("src/lib/h.cpl", h), ("src/app/u.cpl", u)]);
    assert_eq!(
        located(&result),
        [
            at("src/app/u.cpl", 5, "private_item"),
            at("src/app/u.cpl", 6, "private_item"),
            at("src/app/u.cpl", 7, "private_item"),
        ],
        "{}",
        render(&result)
    );
    let qualified: Vec<String> = json_values(&result)
        .iter()
        .map(|d| d["qualified_name"].as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(qualified, ["lib.h.Hidden", "lib.h.secret", "lib.h.secret"]);
}

#[test]
fn every_definition_is_found_again_by_its_display_path() {
    // Toolchain 4.3 and 4.4: `Program::display_path` is the qualified name
    // in `SymbolPath::display` syntax (a quoted, canonically escaped final
    // segment for tests and properties; `Receiver.name` for std member
    // stubs), it parses back to the same path, `Program::lookup` finds the
    // definition by it, and an unannotated definition's identity is exactly
    // that string; over the whole program, std included.
    let source = r#"module app.paths

pub fn f() -> Int {
    1
}

pub struct S {
    v: Int
}

test "a.b" {
    assert true
}

test "say \"q\" \\ done" {
    assert true
}

property "p"
    for n in gen.int
{
    assert n == n
}
"#;
    let result = check_source(source);
    assert!(result.diagnostics.is_empty(), "{}", render(&result));
    let p = &result.program;
    let mut members = 0;
    for d in p.defs() {
        let shown = p.display_path(d.id);
        let path = SymbolPath::parse(&shown).unwrap_or_else(|e| panic!("{shown}: {e}"));
        assert_eq!(path.display(), shown);
        assert_eq!(p.lookup(&path), Some(d.id), "{shown}");
        if d.provisional {
            assert_eq!(d.stable_id.as_str(), shown);
        }
        if matches!(d.name, DeclName::Member { .. }) {
            members += 1;
        }
    }
    assert!(members > 0, "std member stubs take part");
    assert_eq!(
        p.display_path(def_id(p, "std.prelude.List.take")),
        "std.prelude.List.take"
    );
    let quoted = p
        .defs()
        .find(|d| d.name == DeclName::Text("say \"q\" \\ done".to_string()))
        .expect("the test with quotes");
    assert_eq!(
        p.display_path(quoted.id),
        "app.paths.\"say \\\"q\\\" \\\\ done\""
    );
}
