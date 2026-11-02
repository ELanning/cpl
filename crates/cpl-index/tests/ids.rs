//! Stable identities (`spec/toolchain.md` 4, 12): the generated `@id`
//! format, provisional identities, and `cpl id assign` as a plan and a
//! byte-exact insertion whose result re-parses with the new ids.

mod common;

use common::*;
use cpl_diag::{FileId, StableId};
use cpl_index::{
    apply_id_assignments, declaration_identity, generate_id, is_generated_format,
    plan_id_assignments, IdAssignment, IdKind,
};
use cpl_project::VirtualTree;
use cpl_syntax::ast::DeclKind;
use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;
use std::collections::{BTreeMap, BTreeSet};
use std::convert::Infallible;

/// A generator that replays fixed bytes, cycling.
struct Replay {
    bytes: Vec<u8>,
    at: usize,
}

impl Replay {
    fn new(bytes: &[u8]) -> Replay {
        Replay {
            bytes: bytes.to_vec(),
            at: 0,
        }
    }
}

impl rand::TryRng for Replay {
    type Error = Infallible;
    fn try_next_u32(&mut self) -> Result<u32, Infallible> {
        let mut b = [0u8; 4];
        self.try_fill_bytes(&mut b)?;
        Ok(u32::from_le_bytes(b))
    }
    fn try_next_u64(&mut self) -> Result<u64, Infallible> {
        let mut b = [0u8; 8];
        self.try_fill_bytes(&mut b)?;
        Ok(u64::from_le_bytes(b))
    }
    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Infallible> {
        for d in dst {
            *d = self.bytes[self.at % self.bytes.len()];
            self.at += 1;
        }
        Ok(())
    }
}

fn id(s: &str) -> StableId {
    StableId::parse(s).unwrap_or_else(|e| panic!("{s}: {e}"))
}

// ---------------------------------------------------------------------------
// 4.1 The @id value
// ---------------------------------------------------------------------------

#[test]
fn id_kinds_follow_the_section_4_1_table() {
    let project = load_tree(&tree(&[(
        "src/m.cpl",
        "module m\n\npub type W(Int)\npub type R = Int where self > 0\n\nstruct S {\n    a: Int\n}\n\nenum E {\n    A\n}\n\ncapability C {\n    fn get(id: Int) -> Int\n}\n\nfn f() -> Int {\n    1\n}\n\ntest \"t\" {\n    assert true\n}\n\nproperty \"p\"\n    for x in gen.int\n{\n    assert true\n}\n",
    )]));
    let m = module(&project, "src/m.cpl");
    let kinds: Vec<&str> = m
        .declarations
        .iter()
        .map(|d| IdKind::for_decl(&d.kind).prefix())
        .collect();
    assert_eq!(kinds, ["ty", "ty", "ty", "ty", "cap", "fn", "test", "prop"]);
    for kind in IdKind::ALL {
        assert_eq!(IdKind::from_prefix(kind.prefix()), Some(kind));
        assert_eq!(kind.to_string(), kind.prefix());
    }
    assert_eq!(IdKind::from_prefix("pgn"), None);
}

#[test]
fn generate_id_encodes_ten_bytes_as_lowercase_base32() {
    // RFC 4648: BASE32("fooba") = "MZXW6YTB".
    assert_eq!(
        generate_id(IdKind::Fn, &mut Replay::new(b"foobafooba")).as_str(),
        "fn_mzxw6ytbmzxw6ytb"
    );
    assert_eq!(
        generate_id(IdKind::Ty, &mut Replay::new(&[0])).as_str(),
        "ty_aaaaaaaaaaaaaaaa"
    );
    assert_eq!(
        generate_id(IdKind::Cap, &mut Replay::new(&[0xff])).as_str(),
        "cap_7777777777777777"
    );
    // 0x00 0x44 0x32 0x14 0xc7 encodes "abcdefgh": the bits are read most
    // significant first.
    assert_eq!(
        generate_id(
            IdKind::Test,
            &mut Replay::new(&[0x00, 0x44, 0x32, 0x14, 0xc7, 0x42, 0x54, 0xb6, 0x35, 0xcf])
        )
        .as_str(),
        "test_abcdefghijklmnop"
    );
    // Exactly ten bytes are drawn per id.
    let mut rng = Replay::new(b"0123456789abcdefghij");
    let first = generate_id(IdKind::Prop, &mut rng);
    let second = generate_id(IdKind::Prop, &mut rng);
    assert_eq!(rng.at, 20);
    assert_ne!(first, second);
}

#[test]
fn generated_ids_have_the_generated_format_and_follow_the_seed() {
    let draw = |seed: u64| {
        let mut rng = ChaCha20Rng::seed_from_u64(seed);
        (0..1000)
            .map(|i| generate_id(IdKind::ALL[i % 5], &mut rng))
            .collect::<Vec<_>>()
    };
    let ids = draw(7);
    let distinct: BTreeSet<&StableId> = ids.iter().collect();
    assert_eq!(distinct.len(), ids.len());
    for (i, id) in ids.iter().enumerate() {
        assert!(is_generated_format(id), "{id}");
        assert!(!id.is_provisional());
        assert_eq!(id.kind_prefix(), IdKind::ALL[i % 5].prefix());
        // It is an accepted @id value.
        assert_eq!(
            &StableId::from_annotation(id.as_str()).expect("accepted"),
            id
        );
    }
    assert_eq!(ids, draw(7));
    assert_ne!(ids, draw(8));
}

#[test]
fn the_generated_format_is_stricter_than_the_accepted_syntax() {
    assert!(is_generated_format(&id("fn_abcdefghijklmnop")));
    assert!(is_generated_format(&id("prop_a2345674a2345674")));
    // Accepted but not generated: the design's illustrative id, the 4.1
    // example (`9` is not a base32 digit), wrong kinds and lengths.
    for accepted in [
        "pgn_7c2d",
        "fn_k7w3qz2mb9ptx4ve",
        "fx_abcdefghijklmnop",
        "fn_abcdefghijklmno",
        "fn_abcdefghijklmnopq",
        "fn_abcdefgh_ijklmnop",
        "fn_abcdefghijklmn01",
    ] {
        assert!(!is_generated_format(&id(accepted)), "{accepted}");
    }
    // A provisional identity never is.
    assert!(!is_generated_format(&StableId::provisional(
        "m",
        &cpl_diag::DeclName::Ident("fn_abcdefghijklmnop".into())
    )));
}

// ---------------------------------------------------------------------------
// 4.3 Provisional identities
// ---------------------------------------------------------------------------

#[test]
fn provisional_identities_are_qualified_names() {
    let project = load_dir(&workspace_path("examples/reader"));
    let pages = module(&project, "src/reader/pages.cpl");
    let tests = module(&project, "src/tests/pagination.cpl");
    let identity = |m, name: &str| declaration_identity(&module_path(m), decl(m, name));
    for (m, name, expected) in [
        (pages, "clean", "reader.pages.clean"),
        (pages, "Page", "reader.pages.Page"),
        (
            tests,
            "pagination handles empty input",
            "tests.pagination.\"pagination handles empty input\"",
        ),
    ] {
        let found = identity(m, name);
        assert_eq!(found.as_str(), expected);
        assert!(found.is_provisional());
        assert_eq!(found.kind_prefix(), "");
        // The display form round-trips through StableId::parse.
        assert_eq!(id(expected), found);
    }
    let paginate = identity(pages, "paginate");
    assert_eq!(paginate.as_str(), "pgn_7c2d");
    assert!(!paginate.is_provisional());
    // Escaping follows the formatter: quotes and backslashes.
    let odd = load_tree(&tree(&[(
        "src/t.cpl",
        "module t\n\ntest \"say \\\"hi\\\" \\\\ now\" {\n    assert true\n}\n",
    )]));
    let t = module(&odd, "src/t.cpl");
    assert_eq!(
        declaration_identity(&module_path(t), &t.declarations[0]).as_str(),
        "t.\"say \\\"hi\\\" \\\\ now\""
    );
}

// ---------------------------------------------------------------------------
// 4.2 cpl id assign
// ---------------------------------------------------------------------------

/// Apply a plan to the tree a project was loaded from: every planned
/// file's assignments inserted in its text, every other file unchanged.
fn apply_plan(
    original: &VirtualTree,
    project: &cpl_project::Project,
    plan: &[IdAssignment],
) -> VirtualTree {
    let mut by_file: BTreeMap<FileId, Vec<IdAssignment>> = BTreeMap::new();
    for a in plan {
        by_file.entry(a.file).or_default().push(a.clone());
    }
    let mut out = original.clone();
    for (file, assignments) in by_file {
        let file = project.source_map.file(file);
        out.files.insert(
            file.path.clone(),
            apply_id_assignments(file.text(), &assignments).into_bytes(),
        );
    }
    out
}

#[test]
fn id_assign_on_the_reader_example() {
    let original = tree_of_dir(&workspace_path("examples/reader"));
    let project = load_tree(&original);
    let plan = plan_id_assignments(&project, &mut ChaCha20Rng::seed_from_u64(1));
    // Every declaration but `paginate` (which has `@id("pgn_7c2d")`), in
    // (file path, source order) order.
    let paths: Vec<&str> = plan.iter().map(|a| a.path.as_str()).collect();
    assert_eq!(plan.len(), 20);
    assert_eq!(paths[0], "reader.images.Dimensions");
    assert_eq!(paths[3], "reader.pages.PageId");
    assert_eq!(paths[19], "tests.pagination.\"pagination preserves input\"");
    assert!(!paths.contains(&"reader.pages.paginate"));
    let files: Vec<&str> = plan
        .iter()
        .map(|a| project.source_map.file(a.file).path.as_str())
        .collect();
    let mut sorted = files.clone();
    sorted.sort();
    assert_eq!(files, sorted);
    for a in &plan {
        assert!(is_generated_format(&a.id), "{}", a.id);
        assert_eq!(a.id.kind_prefix(), a.kind.prefix());
        assert_eq!(a.indent, "");
    }
    // `clean` is reported at line 21 (the 7.2 example) and its id goes
    // right before `fn`.
    let clean = plan
        .iter()
        .find(|a| a.path == "reader.pages.clean")
        .expect("clean");
    let pages = project.source_map.file(clean.file);
    assert_eq!(
        project
            .source_map
            .line_col(clean.file, clean.insert_at)
            .line,
        21
    );
    assert_eq!(
        serde_json::to_value(clean.to_json(&project.source_map)).expect("serializes"),
        serde_json::json!({
            "path": "src/reader/pages.cpl",
            "line": 21,
            "symbol": "reader.pages.clean",
            "kind": "fn",
            "id": clean.id.as_str(),
        })
    );
    assert!(pages.text()[clean.insert_at as usize..].starts_with("fn clean("));
    let tail = plan
        .iter()
        .find(|a| a.path.ends_with("\"pagination preserves a short tail\""))
        .expect("test");
    assert_eq!(tail.kind, IdKind::Test);

    // Applying it inserts one line per assignment and changes nothing else.
    let assigned = apply_plan(&original, &project, &plan);
    for (path, bytes) in &assigned.files {
        let text = std::str::from_utf8(bytes).expect("utf-8");
        let old = std::str::from_utf8(&original.files[path]).expect("utf-8");
        let ids: BTreeSet<String> = plan
            .iter()
            .filter(|a| project.source_map.file(a.file).path == *path)
            .map(|a| format!("@id(\"{}\")", a.id))
            .collect();
        let kept: Vec<&str> = text
            .split_inclusive('\n')
            .filter(|line| !ids.contains(line.trim_end()))
            .collect();
        assert_eq!(kept.concat(), old, "{path}");
        assert_eq!(
            text.len(),
            old.len() + ids.iter().map(|l| l.len() + 1).sum::<usize>()
        );
    }
    // The result re-parses, every declaration carries its id, and a second
    // run assigns nothing.
    let reloaded = load_tree(&assigned);
    assert!(
        reloaded.diagnostics.iter().all(|d| !d.is_error()),
        "{:?}",
        reloaded.diagnostics
    );
    let wanted: BTreeMap<&str, &StableId> = plan.iter().map(|a| (a.path.as_str(), &a.id)).collect();
    let mut seen = 0;
    for path in [
        "src/reader/images.cpl",
        "src/reader/pages.cpl",
        "src/tests/pagination.cpl",
    ] {
        let m = module(&reloaded, path);
        let mp = module_path(m);
        for d in &m.declarations {
            let identity = declaration_identity(&mp, d);
            let display = cpl_index::symbol_path(&mp, d).display();
            match wanted.get(display.as_str()) {
                Some(&assigned_id) => {
                    assert_eq!(&identity, assigned_id);
                    assert_eq!(d.id(), Some(assigned_id.as_str()));
                    seen += 1;
                }
                None => assert_eq!(identity.as_str(), "pgn_7c2d"),
            }
        }
    }
    assert_eq!(seen, 20);
    assert!(plan_id_assignments(&reloaded, &mut ChaCha20Rng::seed_from_u64(2)).is_empty());
    // The same seed plans the same ids.
    assert_eq!(
        plan,
        plan_id_assignments(&project, &mut ChaCha20Rng::seed_from_u64(1))
    );
}

#[test]
fn ids_go_before_the_first_annotation_or_the_keyword_after_the_docs() {
    // Section 4.2 step 3: the line goes immediately before the first
    // annotation or keyword, after the docs, and "the inserted line is at
    // column 0. No other byte changes": an indented declaration (indentation
    // is never significant, grammar 1.1) keeps its own line, indentation
    // included, below an `@id` line at column 0; `indent` records that
    // indentation (Section 1.8).
    let src = "module m

/// Docs first.
@prove
fn a() -> Int {
    1
}

@prove
/// Docs after the annotation.
fn b() -> Int {
    1
}

/// Only docs.
pub fn c() -> Int {
    1
}

    pub fn indented() -> Int {
        1
    }

\t/// Tabbed docs.
\ttest \"t\" {
\t    assert true
\t}
";
    let project = load_tree(&tree(&[("src/m.cpl", src)]));
    let plan = plan_id_assignments(&project, &mut ChaCha20Rng::seed_from_u64(3));
    assert_eq!(plan.len(), 5);
    assert_eq!(plan[3].indent, "    ");
    assert_eq!(plan[4].indent, "\t");
    assert_eq!(plan[4].kind, IdKind::Test);
    let file = project.source_map.file(plan[0].file);
    let at = |i: usize| &file.text()[plan[i].insert_at as usize..];
    assert!(at(0).starts_with("@prove\nfn a("));
    assert!(at(1).starts_with("@prove\n/// Docs after"));
    assert!(at(2).starts_with("pub fn c("));
    assert!(at(3).starts_with("pub fn indented("));
    assert!(at(4).starts_with("test \"t\""));
    let ids: Vec<&str> = plan.iter().map(|a| a.id.as_str()).collect();
    let expected = format!(
        "module m

/// Docs first.
@id(\"{}\")
@prove
fn a() -> Int {{
    1
}}

@id(\"{}\")
@prove
/// Docs after the annotation.
fn b() -> Int {{
    1
}}

/// Only docs.
@id(\"{}\")
pub fn c() -> Int {{
    1
}}

@id(\"{}\")
    pub fn indented() -> Int {{
        1
    }}

\t/// Tabbed docs.
@id(\"{}\")
\ttest \"t\" {{
\t    assert true
\t}}
",
        ids[0], ids[1], ids[2], ids[3], ids[4]
    );
    let applied = apply_id_assignments(file.text(), &plan);
    assert_eq!(applied, expected);
    // The order of the assignments given does not matter.
    let reversed: Vec<IdAssignment> = plan.iter().rev().cloned().collect();
    assert_eq!(apply_id_assignments(file.text(), &reversed), expected);
    // Re-parsed, the docs and annotations are where they were.
    let reloaded = load_tree(&tree(&[("src/m.cpl", &applied)]));
    let m = module(&reloaded, "src/m.cpl");
    for (d, id) in m.declarations.iter().zip(&ids) {
        assert_eq!(d.id(), Some(*id));
    }
    assert!(decl(m, "a").has_annotation("prove"));
    assert_eq!(
        decl(m, "b").docs.as_ref().map(|d| d.text()).as_deref(),
        Some("Docs after the annotation.")
    );
    assert_eq!(
        decl(m, "t").docs.as_ref().map(|d| d.text()).as_deref(),
        Some("Tabbed docs.")
    );
}

#[test]
fn crlf_files_gain_an_lf_terminated_line() {
    // Section 4.2: the inserted line ends with LF; the lexer accepts LF and
    // CRLF alike (grammar 1.1), so the file still parses.
    let src = "module m\r\n\r\nfn f() -> Int {\r\n    1\r\n}\r\n";
    let project = load_tree(&tree(&[("src/m.cpl", src)]));
    let plan = plan_id_assignments(&project, &mut Replay::new(b"foobafooba"));
    assert_eq!(plan.len(), 1);
    let applied = apply_id_assignments(src, &plan);
    assert_eq!(
        applied,
        "module m\r\n\r\n@id(\"fn_mzxw6ytbmzxw6ytb\")\nfn f() -> Int {\r\n    1\r\n}\r\n"
    );
    let reloaded = load_tree(&tree(&[("src/m.cpl", &applied)]));
    assert_eq!(
        module(&reloaded, "src/m.cpl").declarations[0].id(),
        Some("fn_mzxw6ytbmzxw6ytb")
    );
}

#[test]
fn planning_never_reuses_an_id() {
    // The first draw (ten zero bytes) is `fn_aaaaaaaaaaaaaaaa`, which `f`
    // already carries; `g` gets the next draw. The third declaration's first
    // draw repeats `g`'s and is drawn again too.
    let src = "module m

@id(\"fn_aaaaaaaaaaaaaaaa\")
fn f() -> Int {
    1
}

fn g() -> Int {
    1
}

fn h() -> Int {
    1
}
";
    let project = load_tree(&tree(&[("src/m.cpl", src)]));
    let mut bytes = vec![0u8; 10];
    bytes.extend_from_slice(b"foobafooba");
    bytes.extend_from_slice(b"foobafooba");
    bytes.extend_from_slice(&[0xff; 10]);
    let plan = plan_id_assignments(&project, &mut Replay::new(&bytes));
    let ids: Vec<&str> = plan.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, ["fn_mzxw6ytbmzxw6ytb", "fn_7777777777777777"]);
}

#[test]
fn planning_skips_std_annotated_and_unparsed_files() {
    let project = load_tree(&tree(&[
        (
            "src/a.cpl",
            "module a\n\n@id(\"Bad\")\nfn f() -> Int {\n    1\n}\n\nfn g() -> Int {\n    1\n}\n",
        ),
        ("src/b.cpl", "module b\n\nfn ( {\n"),
    ]));
    let plan = plan_id_assignments(&project, &mut ChaCha20Rng::seed_from_u64(4));
    // `f` has an (invalid) @id, which `cpl id assign` never changes; b.cpl
    // does not parse; std is never touched.
    let paths: Vec<&str> = plan.iter().map(|a| a.path.as_str()).collect();
    assert_eq!(paths, ["a.g"]);
    let a = project.source_map.find("src/a.cpl").expect("a");
    assert!(plan.iter().all(|p| p.file == a));
}

#[test]
fn a_top_level_declaration_starts_its_own_line() {
    // What `IdAssignment::indent` relies on: only horizontal whitespace
    // precedes a declaration's first annotation or keyword on its line.
    for src in [
        "module m\n\nfn f() -> Int {\n    1\n} fn g() -> Int {\n    2\n}\n",
        "module m\n\n@prove fn f() -> Int {\n    1\n}\n",
        "module m\n\npub type A(Int) pub type B(Int)\n",
        "module m pub type A(Int)\n",
    ] {
        let parsed = cpl_syntax::parse_module(src, FileId(0));
        assert!(parsed.value.is_none(), "{src:?} parsed");
    }
    // Every declaration of every corpus and std file.
    let project = load_dir(&workspace_path("examples/reader"));
    for (entry, parsed) in project.files.iter().zip(&project.parsed) {
        let cpl_project::ParsedFile::Module(m) = parsed else {
            continue;
        };
        let text = project.source_map.file(entry.file).text();
        for d in &m.declarations {
            let start = d
                .annotations
                .first()
                .map(|a| a.span.start)
                .unwrap_or(match &d.kind {
                    DeclKind::Fn(f) => f.span.start,
                    DeclKind::Type(t) => t.span.start,
                    DeclKind::Struct(s) => s.span.start,
                    DeclKind::Enum(e) => e.span.start,
                    DeclKind::Capability(c) => c.span.start,
                    DeclKind::Test(t) => t.span.start,
                    DeclKind::Property(p) => p.span.start,
                }) as usize;
            let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
            assert!(text[line_start..start].trim().is_empty());
        }
    }
}
