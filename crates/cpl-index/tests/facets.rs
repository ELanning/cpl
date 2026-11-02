//! Facets and content versions (`spec/toolchain.md` 5, 12): the coverage
//! table, canonical texts and hashes of every declaration of
//! `examples/reader` (golden `tests/golden/facets.json`, regenerated with
//! `UPDATE_GOLDEN=1`), hand-checked texts of 5.2, the facet map with its
//! duplicates (4.5, 5.4), and module content hashes (semantics S48).

mod common;

use common::*;
use cpl_diag::json::FacetHash;
use cpl_diag::{Facet, FileOrigin, StableId};
use cpl_index::{
    applicable_facets, declaration_identity, definition_hash, facet_hash, facet_map, facet_text,
    module_content_hash, symbol_path, symbol_version, FacetEntry, IdKind,
};
use cpl_project::ParsedFile;
use cpl_syntax::ast;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

const EMPTY_HASH: &str = "af1349b9f5f9a1a6a0404dea36dcc949";

fn reader() -> cpl_project::Project {
    load_dir(&workspace_path("examples/reader"))
}

/// The canonical text of a facet of a named declaration of a parsed file.
#[track_caller]
fn text(module: &ast::Module, name: &str, facet: Facet) -> String {
    facet_text(&module_path(module), decl(module, name), facet)
        .unwrap_or_else(|| panic!("{name} has no {facet} facet"))
}

// ---------------------------------------------------------------------------
// 5.3 and 5.4: hashes
// ---------------------------------------------------------------------------

#[test]
fn facet_hash_is_the_first_32_hex_digits_of_blake3() {
    // The test vector of 5.3: the empty text.
    assert_eq!(facet_hash("").as_str(), EMPTY_HASH);
    // BLAKE3("abc") = 6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85.
    assert_eq!(
        facet_hash("abc").as_str(),
        "6437b3ac38465133ffb63b75273a8db5"
    );
    let hash = facet_hash("pub type _(Text)");
    assert_eq!(hash.as_str().len(), 32);
    assert!(hash
        .as_str()
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
}

#[test]
fn definition_hash_hashes_one_line_per_facet_in_canonical_order() {
    let mut facets = BTreeMap::new();
    // Inserted out of order: the canonical order is Facet::ALL's.
    facets.insert(Facet::Docs, facet_hash(""));
    facets.insert(Facet::Identity, facet_hash("m.T"));
    facets.insert(Facet::Signature, facet_hash("pub type _(Int)"));
    facets.insert(Facet::Name, facet_hash("T"));
    let expected = format!(
        "identity={}\nname={}\nsignature={}\ndocs={}",
        facet_hash("m.T"),
        facet_hash("T"),
        facet_hash("pub type _(Int)"),
        facet_hash("")
    );
    assert_eq!(definition_hash(&facets), facet_hash(&expected));
    // It is what symbol_version records.
    let project = load_tree(&tree(&[("src/m.cpl", "module m\n\npub type T(Int)\n")]));
    let m = module(&project, "src/m.cpl");
    let version = symbol_version(&module_path(m), &m.declarations[0]);
    assert_eq!(version.definition, facet_hash(&expected));
}

// ---------------------------------------------------------------------------
// 5.1 Coverage
// ---------------------------------------------------------------------------

const KINDS: &str = "module m

pub type Id(Text)
pub type Small = Int where self < 10

pub struct S<A> {
    a: A
}

pub enum E {
    V
}

pub capability C {
    fn get(id: Int) -> Int
}

fn f() -> Int {
    1
}

test \"t\" {
    assert true
}

property \"p\"
    for x in gen.int
{
    assert x == x
}
";

#[test]
fn applicable_facets_follow_the_coverage_table() {
    use Facet::*;
    let project = load_tree(&tree(&[("src/m.cpl", KINDS)]));
    let m = module(&project, "src/m.cpl");
    let facets = |name: &str| applicable_facets(&decl(m, name).kind).to_vec();
    assert_eq!(facets("f"), Facet::ALL.to_vec());
    assert_eq!(facets("Id"), [Identity, Name, Signature, Docs]);
    assert_eq!(facets("Small"), [Identity, Name, Signature, Contract, Docs]);
    assert_eq!(facets("S"), [Identity, Name, Signature, Docs]);
    assert_eq!(facets("E"), [Identity, Name, Signature, Docs]);
    assert_eq!(facets("C"), [Identity, Name, Signature, Contract, Docs]);
    assert_eq!(facets("t"), [Identity, Name, Body, Docs]);
    assert_eq!(facets("p"), [Identity, Name, Signature, Body, Docs]);
    // A facet outside the table has no text, and every one inside has one.
    for d in &m.declarations {
        for facet in Facet::ALL {
            let applies = applicable_facets(&d.kind).contains(&facet);
            assert_eq!(
                facet_text(&module_path(m), d, facet).is_some(),
                applies,
                "{} {facet}",
                d.display_name()
            );
        }
        // The version carries exactly the applicable facets.
        let version = symbol_version(&module_path(m), d);
        assert_eq!(
            version.facets.keys().copied().collect::<Vec<_>>(),
            applicable_facets(&d.kind)
        );
    }
    // A std opaque type has a wrapper's facets; a bodiless std fn all
    // eight, with an empty body text.
    let prelude = module(&project, "std/prelude.cpl");
    assert_eq!(
        applicable_facets(&decl(prelude, "List").kind),
        [Identity, Name, Signature, Docs]
    );
    let flatten = decl(prelude, "flatten");
    assert_eq!(applicable_facets(&flatten.kind), Facet::ALL);
    assert_eq!(
        facet_text(&module_path(prelude), flatten, Body).as_deref(),
        Some("")
    );
}

// ---------------------------------------------------------------------------
// The golden file: every declaration of examples/reader
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct GoldenFacet {
    text: String,
    hash: FacetHash,
}

#[derive(Serialize)]
struct GoldenDeclaration {
    id: StableId,
    path: String,
    provisional: bool,
    kind: &'static str,
    module: String,
    name: String,
    file: String,
    effects: BTreeSet<String>,
    facets: BTreeMap<Facet, GoldenFacet>,
    definition: FacetHash,
}

#[derive(Serialize)]
struct Golden {
    project: &'static str,
    declarations: Vec<GoldenDeclaration>,
}

#[test]
fn facets_golden_for_every_declaration_of_examples_reader() {
    let project = reader();
    let map = facet_map(&project, unversioned(&project));
    let mut declarations = Vec::new();
    let mut paths: Vec<&str> = project
        .files
        .iter()
        .zip(&project.parsed)
        .filter(|(e, p)| e.origin != FileOrigin::Std && matches!(p, ParsedFile::Module(_)))
        .map(|(e, _)| project.source_map.file(e.file).path.as_str())
        .collect();
    paths.sort();
    assert_eq!(
        paths,
        [
            "src/reader/images.cpl",
            "src/reader/pages.cpl",
            "src/tests/pagination.cpl"
        ]
    );
    for path in paths {
        let m = module(&project, path);
        let mp = module_path(m);
        for d in &m.declarations {
            let entry = FacetEntry::new(&mp, d, path);
            // The facet map holds exactly this entry.
            assert_eq!(map.get(&entry.version.id), Some(&entry));
            let facets = applicable_facets(&d.kind)
                .iter()
                .map(|&f| {
                    let text = facet_text(&mp, d, f).expect("applies");
                    assert_eq!(entry.version.facets[&f], facet_hash(&text));
                    (
                        f,
                        GoldenFacet {
                            hash: facet_hash(&text),
                            text,
                        },
                    )
                })
                .collect();
            declarations.push(GoldenDeclaration {
                id: entry.version.id.clone(),
                path: entry.version.path.clone(),
                provisional: entry.version.provisional,
                kind: entry.kind.prefix(),
                module: entry.module.clone(),
                name: entry.name.clone(),
                file: entry.file.clone(),
                effects: entry.effects.clone(),
                facets,
                definition: entry.version.definition.clone(),
            });
        }
    }
    // 15 declarations in reader.pages, 3 in reader.images, 3 tests.
    assert_eq!(declarations.len(), 21);
    assert_eq!(map.entries.len(), 21);
    assert!(map.duplicates.is_empty());
    let golden = Golden {
        project: "examples/reader",
        declarations,
    };
    let mut json = serde_json::to_string_pretty(&golden).expect("serialize");
    json.push('\n');
    check_golden(&crate_dir().join("tests/golden/facets.json"), &json);
}

// ---------------------------------------------------------------------------
// 5.2 by hand
// ---------------------------------------------------------------------------

#[test]
fn canonical_texts_of_paginate_are_the_section_5_2_example() {
    let project = reader();
    let pages = module(&project, "src/reader/pages.cpl");
    let t = |facet| text(pages, "paginate", facet);
    assert_eq!(
        t(Facet::Signature),
        "pub fn _<T>(items: List<T>, size: Positive) -> List<List<T>>"
    );
    assert_eq!(
        t(Facet::Contract),
        "ensures flatten(result) == items\n\
         ensures forall page in result: page.len > 0 and page.len <= size\n\
         ensures forall page in result.drop_last(): page.len == size"
    );
    assert_eq!(t(Facet::Effects), "");
    assert_eq!(t(Facet::VerificationPolicy), "@prove");
    assert_eq!(t(Facet::Identity), "pgn_7c2d");
    assert_eq!(t(Facet::Name), "paginate");
    assert_eq!(t(Facet::Docs), "");
    // The body: level 0, loop clauses included (T23), no blank lines.
    assert_eq!(
        t(Facet::Body),
        "{\n    var pages: List<List<T>> = []\n    var rest = items\n    while rest.len > size\n        invariant flatten(pages) ++ rest == items\n        invariant forall page in pages: page.len == size\n        decreases rest.len\n    {\n        pages = pages.push(rest.take(size))\n        rest = rest.drop(size)\n    }\n    if not rest.is_empty() {\n        pages = pages.push(rest)\n    }\n    pages\n}"
    );
    // Empty texts hash to the 5.3 vector, so every facet is present.
    let version = symbol_version(&module_path(pages), decl(pages, "paginate"));
    assert_eq!(version.facets[&Facet::Effects].as_str(), EMPTY_HASH);
    assert_eq!(version.facets[&Facet::Docs].as_str(), EMPTY_HASH);
    assert_eq!(version.facets.len(), 8);
    assert!(!version.provisional);
    assert_eq!(version.path, "reader.pages.paginate");
}

#[test]
fn canonical_texts_of_the_other_reader_declarations() {
    let project = reader();
    let pages = module(&project, "src/reader/pages.cpl");
    // Contracts: requires in source order, then ensures (5.2).
    assert_eq!(
        text(pages, "divide_exactly", Facet::Contract),
        "requires b != 0\nrequires a % b == 0\nensures result * b == a"
    );
    assert_eq!(text(pages, "divide_exactly", Facet::VerificationPolicy), "");
    assert_eq!(
        text(pages, "divide_exactly", Facet::Identity),
        "reader.pages.divide_exactly"
    );
    // Types.
    assert_eq!(text(pages, "PageId", Facet::Signature), "pub type _(Text)");
    assert_eq!(
        text(pages, "Positive", Facet::Signature),
        "pub type _ = Int"
    );
    assert_eq!(text(pages, "Positive", Facet::Contract), "where self > 0");
    assert_eq!(
        text(pages, "Page", Facet::Signature),
        "pub struct _ {\n    id: PageId\n    text: Text\n}"
    );
    assert_eq!(
        text(pages, "StoreError", Facet::Signature),
        "pub enum _ {\n    Missing(PageId)\n    Unavailable\n}"
    );
    // Capabilities: operations without contracts; no clauses, empty contract.
    assert_eq!(
        text(pages, "PageRead", Facet::Signature),
        "pub capability _ {\n    fn get(id: PageId) -> Result<Page, StoreError>\n}"
    );
    assert_eq!(text(pages, "PageRead", Facet::Contract), "");
    // Functions: parameters flat, `using` included, `pub ` only when public.
    assert_eq!(
        text(pages, "load_page", Facet::Signature),
        "pub fn _(id: PageId, using db: PageRead) -> Result<Page, StoreError>"
    );
    assert_eq!(
        text(pages, "load_page", Facet::Effects),
        "effects { db.get }"
    );
    assert_eq!(
        text(pages, "load_page", Facet::Body),
        "{\n    db.get(id)\n}"
    );
    assert_eq!(
        text(pages, "clean", Facet::Signature),
        "fn _(page: Page) -> Page"
    );
    assert_eq!(
        text(pages, "clean", Facet::Body),
        "{\n    Page { ..page, text: trim(page.text) }\n}"
    );
    assert_eq!(text(pages, "clean", Facet::Identity), "reader.pages.clean");
    assert_eq!(
        text(pages, "label", Facet::Body),
        "{\n    match result {\n        Ok(page) => page.text\n        Err(Missing(_)) => \"Missing page\"\n        Err(Unavailable) => \"Storage unavailable\"\n    }\n}"
    );
    assert_eq!(
        text(pages, "paginate_ints", Facet::Signature),
        "pub fn _(items: List<Int>, size: Int) -> Result<List<List<Int>>, ConstraintError>"
    );
    let images = module(&project, "src/reader/images.cpl");
    assert_eq!(
        text(images, "image_width", Facet::Effects),
        "effects { codec.dimensions }"
    );
    assert_eq!(
        text(images, "Dimensions", Facet::Signature),
        "pub struct _ {\n    width: Positive\n    height: Positive\n}"
    );
    // Tests and properties: the name is the escaped literal; the identity
    // the quoted qualified name (4.3); a property's signature its clauses.
    let tests = module(&project, "src/tests/pagination.cpl");
    let empty = "pagination handles empty input";
    assert_eq!(
        text(tests, empty, Facet::Name),
        "\"pagination handles empty input\""
    );
    assert_eq!(
        text(tests, empty, Facet::Identity),
        "tests.pagination.\"pagination handles empty input\""
    );
    assert_eq!(
        text(tests, empty, Facet::Body),
        "{\n    let items: List<Int> = []\n    assert paginate(items, 2) == []\n}"
    );
    assert_eq!(
        text(tests, "pagination preserves input", Facet::Signature),
        "for items in gen.list(gen.int)\nfor size in gen.positive_int"
    );
}

#[test]
fn entries_record_kind_module_name_file_and_effects() {
    let project = reader();
    let map = facet_map(&project, unversioned(&project));
    let entry = |id: &str| map.get(&StableId::parse(id).expect("id")).expect(id);
    let load_page = entry("reader.pages.load_page");
    assert_eq!(load_page.kind, IdKind::Fn);
    assert_eq!(load_page.module, "reader.pages");
    assert_eq!(load_page.name, "load_page");
    assert_eq!(load_page.file, "src/reader/pages.cpl");
    assert_eq!(load_page.effects.iter().collect::<Vec<_>>(), ["db.get"]);
    assert!(entry("reader.pages.paginate_ints").effects.is_empty());
    assert_eq!(entry("reader.pages.Page").kind, IdKind::Ty);
    assert_eq!(entry("reader.pages.PageRead").kind, IdKind::Cap);
    assert_eq!(entry("pgn_7c2d").name, "paginate");
    let test = entry("tests.pagination.\"pagination preserves a short tail\"");
    assert_eq!(test.kind, IdKind::Test);
    assert_eq!(test.name, "pagination preserves a short tail");
    assert_eq!(test.module, "tests.pagination");
    let property = entry("tests.pagination.\"pagination preserves input\"");
    assert_eq!(property.kind, IdKind::Prop);
    assert!(property.version.provisional);
    // The revision is recorded as given.
    assert_eq!(map.revision, unversioned(&project));
}

// ---------------------------------------------------------------------------
// Docs, comments, layout
// ---------------------------------------------------------------------------

#[test]
fn the_docs_facet_covers_member_docs_and_signatures_exclude_them() {
    let src = "module m

/// A page.
/// Two lines.
pub struct Page {
    /// The id.
    id: Int
    text: Text
    /// The tail.
    /// Of two lines.
    tail: Text
}

pub enum E {
    /// The first.
    A
    B
}

/// Reads.
pub capability C {
    fn get(id: Int) -> Int
    /// Writes.
    fn put(id: Int) -> Int
}

pub struct Bare {
    /// Only a member is documented.
    x: Int
}

/// A function.
fn f() -> Int {
    1
}
";
    let project = load_tree(&tree(&[("src/m.cpl", src)]));
    let m = module(&project, "src/m.cpl");
    assert_eq!(
        text(m, "Page", Facet::Docs),
        "A page.\nTwo lines.\nmember id\nThe id.\nmember tail\nThe tail.\nOf two lines."
    );
    assert_eq!(
        text(m, "Page", Facet::Signature),
        "pub struct _ {\n    id: Int\n    text: Text\n    tail: Text\n}"
    );
    assert_eq!(text(m, "E", Facet::Docs), "\nmember A\nThe first.");
    assert_eq!(text(m, "C", Facet::Docs), "Reads.\nmember put\nWrites.");
    // Read literally, a declaration without docs of its own starts with LF.
    assert_eq!(
        text(m, "Bare", Facet::Docs),
        "\nmember x\nOnly a member is documented."
    );
    assert_eq!(text(m, "f", Facet::Docs), "A function.");
}

#[test]
fn comments_layout_and_effect_order_leave_every_version_unchanged() {
    let plain = "module m

/// Docs.
@id(\"fn_aaaaaaaaaaaaaaaa\")
@prove
pub fn f(a: Int, using db: Db) -> Int
    effects { db.get, db.put }
    requires a > 0
    ensures result > 0
{
    let b = a + 1
    b
}
";
    let noisy = "module m

// A comment above.
/// Docs.
@id(\"fn_aaaaaaaaaaaaaaaa\") // the identity
@prove
pub fn f(
    a: Int, // the input
    using db: Db,
) -> Int
    // the clauses
    requires a  >  0
    effects { db.put, db.get, db.put }
    ensures result > 0 // positive
{
    // the body

    let b = a  +  1

    b // the result
}
";
    let version = |src: &str| {
        let project = load_tree(&tree(&[("src/m.cpl", src)]));
        let m = module(&project, "src/m.cpl");
        symbol_version(&module_path(m), &m.declarations[0])
    };
    assert_eq!(version(plain), version(noisy));
}

#[test]
fn a_rename_changes_only_the_name_facet() {
    // T24: the signature replaces the name with `_`.
    let version = |name: &str| {
        let src = format!(
            "module m\n\n@id(\"fn_aaaaaaaaaaaaaaaa\")\npub fn {name}<T>(x: T) -> T {{\n    x\n}}\n"
        );
        let project = load_tree(&tree(&[("src/m.cpl", &src)]));
        let m = module(&project, "src/m.cpl");
        symbol_version(&module_path(m), &m.declarations[0])
    };
    let (a, b) = (version("before"), version("after"));
    for facet in Facet::ALL {
        assert_eq!(
            a.facets[&facet] == b.facets[&facet],
            facet != Facet::Name,
            "{facet}"
        );
    }
    assert_eq!(a.id, b.id);
    assert_ne!(a.definition, b.definition);
}

// ---------------------------------------------------------------------------
// Identities
// ---------------------------------------------------------------------------

#[test]
fn malformed_id_annotations_leave_the_identity_provisional() {
    let src = "module m

@id(\"Foo\")
fn a() -> Int {
    1
}

@id(7)
fn b() -> Int {
    1
}

@id(\"x_one\", \"x_two\")
fn c() -> Int {
    1
}

@id
fn d() -> Int {
    1
}

@id(\"a.b\")
fn e() -> Int {
    1
}

@id(\"ok_1\")
@id(\"ok_2\")
fn f() -> Int {
    1
}

@id(\"pgn_7c2d\")
test \"named\" {
    assert true
}
";
    let project = load_tree(&tree(&[("src/m.cpl", src)]));
    let m = module(&project, "src/m.cpl");
    let mp = module_path(m);
    let ids: Vec<String> = m
        .declarations
        .iter()
        .map(|d| declaration_identity(&mp, d).as_str().to_string())
        .collect();
    assert_eq!(ids, ["m.a", "m.b", "m.c", "m.d", "m.e", "ok_1", "pgn_7c2d"]);
    assert!(declaration_identity(&mp, &m.declarations[0]).is_provisional());
    assert!(!declaration_identity(&mp, &m.declarations[6]).is_provisional());
    // A test's path is its quoted name whatever its identity.
    assert_eq!(
        symbol_path(&mp, &m.declarations[6]).display(),
        "m.\"named\""
    );
}

#[test]
fn std_member_stubs_are_named_through_their_receiver() {
    // stdlib S37: `std.prelude.List.take`; S40: a zero-parameter @property
    // is a module item (`std.gen.int`).
    let project = reader();
    let prelude = module(&project, "std/prelude.cpl");
    let mp = module_path(prelude);
    let paths: Vec<String> = prelude
        .declarations
        .iter()
        .map(|d| symbol_path(&mp, d).display())
        .collect();
    for expected in [
        "std.prelude.List.take",
        "std.prelude.List.len",
        "std.prelude.Text.len",
        "std.prelude.flatten",
        "std.prelude.Option",
    ] {
        assert!(paths.iter().any(|p| p == expected), "{expected}: {paths:?}");
    }
    let take = prelude
        .declarations
        .iter()
        .find(|d| symbol_path(&mp, d).display() == "std.prelude.List.take")
        .expect("take");
    assert_eq!(
        declaration_identity(&mp, take).as_str(),
        "std.prelude.List.take"
    );
    assert_eq!(facet_text(&mp, take, Facet::Name).as_deref(), Some("take"));
    let gen = module(&project, "std/gen.cpl");
    let gp = module_path(gen);
    assert!(gen
        .declarations
        .iter()
        .any(|d| symbol_path(&gp, d).display() == "std.gen.int"));
}

// ---------------------------------------------------------------------------
// 5.4 The facet map, 4.5 duplicates
// ---------------------------------------------------------------------------

#[test]
fn duplicate_ids_keep_the_first_occurrence_in_path_order() {
    let a = "module m

@id(\"fn_aaaaaaaaaaaaaaaa\")
fn first() -> Int {
    1
}

fn twice() -> Int {
    1
}
";
    let b = "module m

@id(\"fn_aaaaaaaaaaaaaaaa\")
fn copy() -> Int {
    2
}

fn twice() -> Int {
    2
}

fn twice() -> Int {
    3
}
";
    let project = load_tree(&tree(&[("src/m/b.cpl", b), ("src/m/a.cpl", a)]));
    let map = facet_map(&project, unversioned(&project));
    let id = StableId::parse("fn_aaaaaaaaaaaaaaaa").expect("id");
    // The first in path order: src/m/a.cpl before src/m/b.cpl.
    assert_eq!(map.get(&id).expect("entry").name, "first");
    assert_eq!(map.get(&id).expect("entry").file, "src/m/a.cpl");
    let twice = StableId::parse("m.twice").expect("id");
    assert_eq!(map.get(&twice).expect("entry").file, "src/m/a.cpl");
    assert_eq!(
        map.duplicates,
        [
            (
                id,
                vec!["src/m/a.cpl".to_string(), "src/m/b.cpl".to_string()]
            ),
            (
                twice,
                vec![
                    "src/m/a.cpl".to_string(),
                    "src/m/b.cpl".to_string(),
                    "src/m/b.cpl".to_string()
                ]
            ),
        ]
    );
    assert_eq!(map.entries.len(), 2);
}

#[test]
fn the_facet_map_leaves_out_std_and_files_that_do_not_parse() {
    let project = load_tree(&tree(&[
        ("src/a.cpl", "module a\n\nfn f() -> Int {\n    1\n}\n"),
        ("src/b.cpl", "module b\n\nfn g( -> Int {\n    1\n}\n"),
    ]));
    let map = facet_map(&project, unversioned(&project));
    let ids: Vec<&str> = map.entries.keys().map(StableId::as_str).collect();
    assert_eq!(ids, ["a.f"]);
    assert!(project.diagnostics.iter().any(|d| d.code == "syntax_error"));
}

#[test]
fn the_facet_map_is_the_same_from_a_directory_and_a_tree() {
    for example in ["examples/reader", "examples/holes"] {
        let root = workspace_path(example);
        let from_dir = load_dir(&root);
        let from_tree = load_tree(&tree_of_dir(&root));
        let a = facet_map(&from_dir, unversioned(&from_dir));
        let b = facet_map(&from_tree, unversioned(&from_tree));
        assert_eq!(a, b, "{example}");
    }
}

// ---------------------------------------------------------------------------
// Semantics S48: module content hashes
// ---------------------------------------------------------------------------

#[test]
fn module_content_hash_joins_the_formatted_files_in_path_order() {
    let one = "module m\n\n// unformatted\nfn  f() -> Int { 1 }\n";
    let two = "module m\n\nfn g() -> Int {\n    2\n}\n";
    let project = load_tree(&tree(&[
        ("src/m/two.cpl", two),
        ("src/m.cpl", one),
        ("src/broken.cpl", "module m\n\nfn ( {\n"),
    ]));
    let id = |path: &str| project.source_map.find(path).expect(path);
    let format = |src: &str| {
        cpl_syntax::format_source(src, cpl_syntax::FileKind::Cpl, cpl_diag::FileId(0))
            .expect("formats")
    };
    let expected = format!("{}\n{}", format(one), format(two));
    let expected = blake3::hash(expected.as_bytes()).to_hex().to_string();
    // Any order of the ids, repeats, and files without a tree are ignored.
    let hash = module_content_hash(
        &project,
        &[
            id("src/m/two.cpl"),
            id("src/broken.cpl"),
            id("src/m.cpl"),
            id("src/m/two.cpl"),
        ],
    );
    assert_eq!(hash.as_str(), expected);
    assert_eq!(hash.as_str().len(), 64);
    // A formatting-only edit does not change it.
    let reformatted = load_tree(&tree(&[
        ("src/m/two.cpl", two),
        ("src/m.cpl", &format(one)),
    ]));
    let id2 = |path: &str| reformatted.source_map.find(path).expect(path);
    assert_eq!(
        module_content_hash(&reformatted, &[id2("src/m.cpl"), id2("src/m/two.cpl")]),
        hash
    );
    // The std modules hash like any other.
    let std = module_content_hash(&project, &[id("std/prelude.cpl")]);
    assert_eq!(std.as_str().len(), 64);
}
