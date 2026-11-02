//! Adversarial tests of stable identities and facets (`spec/toolchain.md`
//! Sections 4 and 5, with the API of 1.8): the generated `@id` format and
//! alphabet, provisional identity text, `IdKind` per declaration kind, the
//! order and determinism of `plan_id_assignments`, the byte-exactness of
//! `apply_id_assignments` (indentation, CRLF, doc comments and several
//! annotations, the end of a file), the applicable facets and the canonical
//! text of every facet of every declaration kind, independence from comments
//! and layout, hash truncation, the `.definition` hash, and facet maps with
//! duplicate ids.
//!
//! Every expectation is the specification's answer, written by hand from the
//! cited section and never read back from the implementation; where a
//! section is silent or contradicts another, the comment says which reading
//! the test takes and why.

use cpl_diag::json::{Revision, RevisionState, SymbolVersion};
use cpl_diag::{quote_text, Facet, FileId, FileOrigin, StableId};
use cpl_index::{
    applicable_facets, apply_id_assignments, declaration_identity, definition_hash, facet_hash,
    facet_map, facet_text, generate_id, is_generated_format, plan_id_assignments, source_digest,
    symbol_path, symbol_version, FacetMap, IdAssignment, IdKind,
};
use cpl_project::{load_project_from_tree, LoadOptions, Project, VirtualTree};
use cpl_syntax::ast;
use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;
use std::collections::{BTreeMap, BTreeSet};
use std::convert::Infallible;

// ===========================================================================
// Helpers
// ===========================================================================

/// Section 5.3's test vector: the hash of the empty text.
const EMPTY_HASH: &str = "af1349b9f5f9a1a6a0404dea36dcc949";
const MANIFEST: &str = "[project]\nname = \"t\"\nedition = \"0.1\"\n";

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

/// A generator whose byte `k` is `k / 10 + 1`: the `i`-th draw of ten bytes
/// (Section 4.1) is ten copies of `i + 1`, so no two draws are equal and the
/// id each declaration receives is known in advance ([`draw`]).
#[derive(Default)]
struct Draws {
    at: usize,
}

impl rand::TryRng for Draws {
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
            *d = u8::try_from(self.at / 10 + 1).expect("fewer than 255 draws");
            self.at += 1;
        }
        Ok(())
    }
}

/// The id that the `i`-th draw of [`Draws`] encodes for a kind prefix.
fn draw(kind: &str, i: usize) -> String {
    let byte = u8::try_from(i + 1).expect("fewer than 255 draws");
    format!("{kind}_{}", rfc4648_base32(&[byte; 10]))
}

/// RFC 4648 Section 6 base32 with the lowercase alphabet and no padding,
/// written independently of the crate: the bits are consumed most
/// significant first, five at a time.
fn rfc4648_base32(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut out = String::new();
    let mut acc: u64 = 0;
    let mut bits: u32 = 0;
    for &byte in bytes {
        acc = (acc << 8) | u64::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(ALPHABET[((acc >> bits) & 31) as usize]));
        }
        acc &= (1 << bits) - 1;
    }
    if bits > 0 {
        out.push(char::from(ALPHABET[((acc << (5 - bits)) & 31) as usize]));
    }
    out
}

/// SplitMix64: test bytes that do not come from any generator of the crate.
fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn tree_with(manifest: &str, files: &[(&str, &str)]) -> VirtualTree {
    let mut tree = VirtualTree::default();
    tree.files
        .insert("cpl.toml".to_string(), manifest.as_bytes().to_vec());
    for (path, text) in files {
        tree.files
            .insert((*path).to_string(), text.as_bytes().to_vec());
    }
    tree
}

#[track_caller]
fn load_with(manifest: &str, files: &[(&str, &str)]) -> Project {
    load_project_from_tree(
        &tree_with(manifest, files),
        FileOrigin::Virtual,
        &LoadOptions::default(),
    )
    .unwrap_or_else(|e| panic!("the tree does not load: {e}"))
}

#[track_caller]
fn load(files: &[(&str, &str)]) -> Project {
    load_with(MANIFEST, files)
}

/// The revision of a tree outside any repository.
fn revision(project: &Project) -> Revision {
    Revision {
        commit: None,
        state: RevisionState::Worktree,
        digest: source_digest(project),
    }
}

#[track_caller]
fn sid(s: &str) -> StableId {
    StableId::parse(s).unwrap_or_else(|e| panic!("{s}: {e}"))
}

#[track_caller]
fn parse(src: &str) -> ast::Module {
    let parsed = cpl_syntax::parse_module(src, FileId(0));
    match parsed.value {
        Some(m) => m,
        None => panic!("does not parse: {:?}\n{src}", parsed.diagnostics),
    }
}

fn module_path(m: &ast::Module) -> Vec<String> {
    m.header
        .path
        .segments
        .iter()
        .map(|s| s.text.clone())
        .collect()
}

#[track_caller]
fn decl<'m>(m: &'m ast::Module, name: &str) -> &'m ast::Declaration {
    m.declarations
        .iter()
        .find(|d| d.display_name() == name)
        .unwrap_or_else(|| panic!("no declaration {name:?}"))
}

/// The canonical text of one facet of the declaration `name` of `src`.
#[track_caller]
fn text(src: &str, name: &str, facet: Facet) -> Option<String> {
    let m = parse(src);
    facet_text(&module_path(&m), decl(&m, name), facet)
}

#[track_caller]
fn version(src: &str, name: &str) -> SymbolVersion {
    let m = parse(src);
    symbol_version(&module_path(&m), decl(&m, name))
}

/// The canonical text of every applicable facet of every declaration of
/// `src`, by display name.
#[track_caller]
fn texts_of(src: &str) -> BTreeMap<String, BTreeMap<Facet, String>> {
    let m = parse(src);
    let mp = module_path(&m);
    m.declarations
        .iter()
        .map(|d| {
            let facets = applicable_facets(&d.kind)
                .iter()
                .map(|&f| {
                    (
                        f,
                        facet_text(&mp, d, f).expect("an applicable facet has a text"),
                    )
                })
                .collect();
            (d.display_name().to_string(), facets)
        })
        .collect()
}

/// The facet names of Section 5 (the `<facet>` of the 5.4 `.definition`
/// lines), spelled here rather than taken from `Facet::as_str`.
fn facet_name(f: Facet) -> &'static str {
    match f {
        Facet::Identity => "identity",
        Facet::Name => "name",
        Facet::Signature => "signature",
        Facet::Contract => "contract",
        Facet::Effects => "effects",
        Facet::Body => "body",
        Facet::Docs => "docs",
        Facet::VerificationPolicy => "verification_policy",
    }
}

fn blake3_hex(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex().to_string()
}

fn is_lower_hex(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Section 5.4 step 2 by hand: one line `<facet>=<hash>` per applicable
/// facet in canonical order, joined with LF, hashed as in 5.3.
fn definition_oracle(texts: &[(Facet, &str)]) -> String {
    let lines: Vec<String> = texts
        .iter()
        .map(|(f, t)| format!("{}={}", facet_name(*f), &blake3_hex(t)[..32]))
        .collect();
    blake3_hex(&lines.join("\n"))[..32].to_string()
}

/// The duplicates of a facet map as `id -> set of files` (Section 4.5 and
/// 1.8 say "with their files" and fix no order).
fn duplicates(map: &FacetMap) -> BTreeMap<String, BTreeSet<String>> {
    map.duplicates
        .iter()
        .map(|(id, files)| (id.as_str().to_string(), files.iter().cloned().collect()))
        .collect()
}

/// The byte offset of `marker`, which must occur exactly once in `src`.
#[track_caller]
fn at(src: &str, marker: &str) -> usize {
    assert_eq!(
        src.matches(marker).count(),
        1,
        "marker {marker:?} is not unique"
    );
    src.find(marker).expect("present")
}

/// `src` with the line `@id("<id>")` LF inserted at each offset: the
/// result Section 4.2 step 3 prescribes.
fn splice(src: &str, insertions: &[(usize, String)]) -> String {
    let mut sorted = insertions.to_vec();
    sorted.sort();
    let mut out = String::new();
    let mut copied = 0;
    for (offset, id) in sorted {
        out.push_str(&src[copied..offset]);
        out.push_str("@id(\"");
        out.push_str(&id);
        out.push_str("\")\n");
        copied = offset;
    }
    out.push_str(&src[copied..]);
    out
}

/// Plan `src` as the single file `src/m.cpl` of a project with [`Draws`],
/// and apply the plan to the file's text.
#[track_caller]
fn assign(src: &str) -> (Vec<IdAssignment>, String, Project) {
    let project = load(&[("src/m.cpl", src)]);
    assert!(
        project.diagnostics.iter().all(|d| !d.is_error()),
        "{:?}",
        project.diagnostics
    );
    let file = project.source_map.find("src/m.cpl").expect("the file");
    assert_eq!(
        project.source_map.file(file).text(),
        src,
        "the source map holds the file's bytes"
    );
    let plan = plan_id_assignments(&project, &mut Draws::default());
    assert!(plan.iter().all(|a| a.file == file));
    let out = apply_id_assignments(src, &plan);
    (plan, out, project)
}

/// Re-plan an assigned text: idempotency (4.2 rules) means nothing is left.
#[track_caller]
fn assert_nothing_left_to_assign(out: &str) {
    let project = load(&[("src/m.cpl", out)]);
    let again = plan_id_assignments(&project, &mut ChaCha20Rng::seed_from_u64(99));
    assert!(again.is_empty(), "a second run assigns {again:?}");
}

/// One edit of a base source and the facets it must change.
struct Edit {
    label: &'static str,
    from: &'static str,
    to: &'static str,
    changes: &'static [Facet],
}

/// Apply each edit to `base` and compare the facet hashes of the
/// declaration `name` before and after; every mismatch is collected so that
/// one run reports all of them. The `.definition` hash must change exactly
/// when a facet does.
#[track_caller]
fn check_edits(base: &str, name: &str, edits: &[Edit]) {
    let before = version(base, name);
    let mut failures = Vec::new();
    for e in edits {
        assert_eq!(
            base.matches(e.from).count(),
            1,
            "{}: {:?} must occur once in the base",
            e.label,
            e.from
        );
        let after = version(&base.replacen(e.from, e.to, 1), name);
        let differ: BTreeSet<Facet> = Facet::ALL
            .into_iter()
            .filter(|f| before.facets.get(f) != after.facets.get(f))
            .collect();
        let want: BTreeSet<Facet> = e.changes.iter().copied().collect();
        if differ != want {
            failures.push(format!(
                "{}: expected {want:?} to change, found {differ:?}",
                e.label
            ));
        }
        if (before.definition != after.definition) == want.is_empty() {
            failures.push(format!(
                "{}: the .definition hash must change exactly when a facet does",
                e.label
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The facets whose hashes differ between declaration `a_name` of `a` and
/// declaration `b_name` of `b`.
#[track_caller]
fn differing(a: &str, a_name: &str, b: &str, b_name: &str) -> BTreeSet<Facet> {
    let (va, vb) = (version(a, a_name), version(b, b_name));
    Facet::ALL
        .into_iter()
        .filter(|f| va.facets.get(f) != vb.facets.get(f))
        .collect()
}

// ===========================================================================
// 4.1 The generated @id value
// ===========================================================================

#[test]
fn generated_ids_agree_with_an_independent_rfc4648_encoder() {
    // 4.1: ten bytes drawn from the generator, encoded as sixteen base32
    // characters (RFC 4648 alphabet, lowercase, no padding: 80 bits
    // exactly), after the kind prefix and `_`.
    let mut state = 0x5eed_u64;
    for i in 0..1000 {
        let mut bytes = [0u8; 10];
        bytes[..8].copy_from_slice(&splitmix(&mut state).to_be_bytes());
        bytes[8..].copy_from_slice(&splitmix(&mut state).to_be_bytes()[..2]);
        let kind = IdKind::ALL[i % 5];
        let mut rng = Replay::new(&bytes);
        let id = generate_id(kind, &mut rng);
        assert_eq!(
            id.as_str(),
            format!("{}_{}", kind.prefix(), rfc4648_base32(&bytes)),
            "{bytes:02x?}"
        );
        assert_eq!(rng.at, 10, "one id draws exactly ten bytes");
    }
}

#[test]
fn generated_id_characters_follow_the_rfc4648_bit_order() {
    let id = |bytes: [u8; 10]| {
        generate_id(IdKind::Ty, &mut Replay::new(&bytes))
            .as_str()
            .to_string()
    };
    // The first character is the five high bits of the first byte.
    assert_eq!(id([0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0]), "ty_qaaaaaaaaaaaaaaa");
    assert_eq!(id([0xf8, 0, 0, 0, 0, 0, 0, 0, 0, 0]), "ty_7aaaaaaaaaaaaaaa");
    // The second straddles the first two bytes.
    assert_eq!(id([0x04, 0, 0, 0, 0, 0, 0, 0, 0, 0]), "ty_aqaaaaaaaaaaaaaa");
    // The last character is the five low bits of the last byte: 80 bits
    // leave no padding bits.
    let mut last = [0u8; 10];
    last[9] = 0x01;
    assert_eq!(id(last), "ty_aaaaaaaaaaaaaaab");
    last[9] = 0x1f;
    assert_eq!(id(last), "ty_aaaaaaaaaaaaaaa7");
    last[9] = 0x20;
    assert_eq!(id(last), "ty_aaaaaaaaaaaaaaba");
}

#[test]
fn generated_ids_use_all_32_characters_of_the_alphabet_and_no_other() {
    // 4.1: `base32 = "a".."z" | "2".."7"`; `0`, `1`, `8`, `9`, upper case,
    // and `=` padding never appear.
    let mut rng = ChaCha20Rng::seed_from_u64(0x00c0_ffee);
    let mut seen = BTreeSet::new();
    for i in 0..400 {
        let kind = IdKind::ALL[i % 5];
        let id = generate_id(kind, &mut rng);
        let (prefix, body) = id.as_str().split_once('_').expect("kind `_` base32");
        assert_eq!(prefix, kind.prefix());
        assert_eq!(body.chars().count(), 16, "{id}");
        seen.extend(body.chars());
    }
    let alphabet: BTreeSet<char> = "abcdefghijklmnopqrstuvwxyz234567".chars().collect();
    assert_eq!(seen, alphabet);
}

#[test]
fn the_generated_format_accepts_every_kind_and_rejects_near_misses() {
    // 4.1: `^(fn|ty|cap|test|prop)_[a-z2-7]{16}$`.
    for good in [
        "fn_abcdefghijklmnop",
        "ty_2345672345672345",
        "cap_qrstuvwxyz234567",
        "test_aaaaaaaaaaaaaaaa",
        "prop_7777777777777777",
    ] {
        assert!(is_generated_format(&sid(good)), "{good}");
    }
    // Each of these has the accepted syntax but not the generated format.
    for near in [
        "fn_abcdefghijklmn0p",
        "fn_abcdefghijklmn1p",
        "fn_abcdefghijklmn8p",
        "fn_abcdefghijklmn9p",
        "fn_aaaaaaa_aaaaaaaa",
        "fn_aaaaaaaaaaaaaaaa_b",
        "fun_aaaaaaaaaaaaaaaa",
        "f_aaaaaaaaaaaaaaaa",
        "type_aaaaaaaaaaaaaaaa",
        "property_aaaaaaaaaaaaaaaa",
        "tests_aaaaaaaaaaaaaaaa",
        "fnx_aaaaaaaaaaaaaaaa",
    ] {
        assert!(!is_generated_format(&sid(near)), "{near}");
    }
    // A provisional identity never has it, not even a test whose name looks
    // like a generated id.
    assert!(!is_generated_format(&sid("m.\"fn_aaaaaaaaaaaaaaaa\"")));
    assert!(!is_generated_format(&sid("fn.aaaaaaaaaaaaaaaa")));
}

#[test]
fn only_accepted_id_values_become_the_identity() {
    // 4.1: an `@id` argument must match `^[a-z][a-z0-9]*(_[a-z0-9]+)+$` and
    // have 4 to 64 characters; `@id("Foo")`, `@id("")`, `@id(7)`, and two
    // literals are `invalid_annotation_argument` (semantics 1.8, S2). A
    // StableId is validated on construction (1.4), so such an annotation
    // identifies nothing and the declaration keeps its provisional identity.
    let longest = format!("a{}", "_bc".repeat(21));
    let too_long = format!("ab{}", "_bc".repeat(21));
    assert_eq!((longest.len(), too_long.len()), (64, 65));
    let identity = |arg: &str| {
        let src = format!("module m\n\n@id({arg})\nfn f() -> Int {{\n    1\n}}\n");
        let m = parse(&src);
        let mp = module_path(&m);
        let d = &m.declarations[0];
        let id = declaration_identity(&mp, d);
        assert_eq!(
            facet_text(&mp, d, Facet::Identity).as_deref(),
            Some(id.as_str()),
            "the identity facet is the identity"
        );
        id
    };
    for value in [
        "ab_c",
        "a1_2",
        "x_y_z_0",
        "pgn_7c2d",
        "fn_k7w3qz2mb9ptx4ve",
        longest.as_str(),
    ] {
        let id = identity(&quote_text(value));
        assert_eq!(id.as_str(), value);
        assert!(!id.is_provisional(), "{value}");
    }
    for value in [
        "",
        "Foo",
        "abc",
        "a_b",
        "_ab_c",
        "1ab_c",
        "ab__c",
        "ab_c_",
        "ab-c_d",
        "ab_C",
        "AB_c",
        "ab_c ",
        " ab_c",
        "ab_c\t",
        "ab_c\u{e9}",
        "ab.c_d",
        too_long.as_str(),
    ] {
        assert_eq!(
            identity(&quote_text(value)).as_str(),
            "m.f",
            "@id({value:?})"
        );
    }
    for arg in ["true", "42", "", "\"ab_c\", \"de_f\""] {
        assert_eq!(identity(arg).as_str(), "m.f", "@id({arg})");
    }
}

// ===========================================================================
// 4.1 IdKind per declaration kind
// ===========================================================================

#[test]
fn definition_less_types_and_bodiless_fns_have_the_kinds_of_their_keywords() {
    // 4.1's table: every `type` is `ty` and every `fn` is `fn`. The
    // definition-less and bodiless forms (grammar 3.5, 3.9; stdlib 10.2
    // G1, G2) parse in project files, where `cpl check` reports
    // `missing_body`; `cpl id assign` only parses (4.2 step 1), so they get
    // ids of their kinds.
    let src = "module m\n\npub type Opaque<T>\ntype Plain\n\nfn stub(a: Int) -> Int\n    requires a > 0\n\npub fn bare() -> Int\n";
    let m = parse(src);
    let kinds: Vec<IdKind> = m
        .declarations
        .iter()
        .map(|d| IdKind::for_decl(&d.kind))
        .collect();
    assert_eq!(kinds, [IdKind::Ty, IdKind::Ty, IdKind::Fn, IdKind::Fn]);
    let (plan, out, _) = assign(src);
    let ids: Vec<&str> = plan.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(
        ids,
        [draw("ty", 0), draw("ty", 1), draw("fn", 2), draw("fn", 3)]
    );
    assert_eq!(
        out,
        splice(
            src,
            &[
                (at(src, "pub type Opaque"), draw("ty", 0)),
                (at(src, "type Plain"), draw("ty", 1)),
                (at(src, "fn stub"), draw("fn", 2)),
                (at(src, "pub fn bare"), draw("fn", 3)),
            ]
        )
    );
    assert_nothing_left_to_assign(&out);
}

#[test]
fn the_kind_follows_the_declaration_not_the_id_prefix() {
    // T21 [D-157]: the prefix is informative; a generated `fn_...` id moved
    // onto a type by a human remains that type's identity, and the
    // declaration's kind (4.1's table) is still `ty`.
    let project = load(&[(
        "src/m.cpl",
        "module m\n\n@id(\"fn_aaaaaaaaaaaaaaaa\")\npub struct S {\n    a: Int\n}\n\n@id(\"ty_aaaaaaaaaaaaaaaa\")\nfn f() -> Int {\n    1\n}\n",
    )]);
    let map = facet_map(&project, revision(&project));
    let s = map
        .get(&sid("fn_aaaaaaaaaaaaaaaa"))
        .expect("S keeps its id");
    assert_eq!((s.kind, s.name.as_str()), (IdKind::Ty, "S"));
    assert!(!s.version.provisional);
    let f = map
        .get(&sid("ty_aaaaaaaaaaaaaaaa"))
        .expect("f keeps its id");
    assert_eq!((f.kind, f.name.as_str()), (IdKind::Fn, "f"));
    assert_eq!(map.entries.len(), 2);
}

// ===========================================================================
// 4.3 Provisional identities
// ===========================================================================

#[test]
fn provisional_identities_quote_test_and_property_names_canonically() {
    // 4.3: `module_path "." text_literal`, the name "canonically escaped, in
    // double quotes"; the escaping is formatter rule 10 (2.4): `\\`, `\"`,
    // `\n`, `\r`, `\t`, the other U+0000-U+001F and U+007F as `\u{H}` with
    // minimal lowercase hex digits, every other scalar value literally (the
    // C1 range included, D62). The literal is decoded first (grammar 1.5),
    // so the source spelling of an escape does not matter. The `name` facet
    // is the same quoted literal (5.2).
    let cases: [(&str, &str); 14] = [
        (r"back\\slash", r"back\\slash"),
        (r#"say \"hi\""#, r#"say \"hi\""#),
        (r"line\nbreak", r"line\nbreak"),
        (r"carriage\rreturn", r"carriage\rreturn"),
        ("raw\ttab", r"raw\ttab"),
        (r"escaped\ttab", r"escaped\ttab"),
        (r"start\u{1}", r"start\u{1}"),
        (r"unit\u{001F}sep", r"unit\u{1f}sep"),
        (r"del\u{7F}", r"del\u{7f}"),
        ("next\u{85}line", "next\u{85}line"),
        (r"nb\u{a0}sp", "nb\u{a0}sp"),
        (
            "gr\u{f6}\u{df}e \u{2713} \u{1f980}",
            "gr\u{f6}\u{df}e \u{2713} \u{1f980}",
        ),
        (r"\u{41}BC", "ABC"),
        (r"\u{000041}x", "Ax"),
    ];
    for (spelled, shown) in cases {
        let src = format!(
            "module m.n\n\ntest \"{spelled}\" {{\n    assert true\n}}\n\nproperty \"p {spelled}\"\n    for x in gen.int\n{{\n    assert x == x\n}}\n"
        );
        let m = parse(&src);
        let mp = module_path(&m);
        let shown_names = [shown.to_string(), format!("p {shown}")];
        for (d, shown) in m.declarations.iter().zip(shown_names) {
            let expected = format!("m.n.\"{shown}\"");
            let id = declaration_identity(&mp, d);
            assert_eq!(id.as_str(), expected, "{spelled:?}");
            assert!(id.is_provisional());
            assert_eq!(id.kind_prefix(), "");
            assert_eq!(
                facet_text(&mp, d, Facet::Identity).as_deref(),
                Some(expected.as_str())
            );
            assert_eq!(
                facet_text(&mp, d, Facet::Name),
                Some(format!("\"{shown}\""))
            );
            assert_eq!(symbol_path(&mp, d).display(), expected);
            // 4.4: the display form parses back to the same identity.
            assert_eq!(
                StableId::parse(&expected).map_err(|e| e.to_string()),
                Ok(id)
            );
        }
    }
}

#[test]
fn an_empty_test_name_still_has_a_provisional_identity() {
    // `empty_test_name` is a check error (semantics 1.8), but the file parses
    // and the facet map is syntactic (5.4), so the identity is the module
    // path, `.`, and the empty literal (4.3).
    let src = "module m\n\ntest \"\" {\n    assert true\n}\n";
    let m = parse(src);
    let id = declaration_identity(&module_path(&m), &m.declarations[0]);
    assert_eq!(id.as_str(), "m.\"\"");
    assert_eq!(sid("m.\"\""), id);
    assert_eq!(text(src, "", Facet::Name).as_deref(), Some("\"\""));
}

#[test]
fn identities_and_entries_use_the_module_header_not_the_file_location() {
    // 4.3 and 5.4 step 1 ("using the file's `module` header"); 3.7: the
    // header is authoritative and the location only conventional.
    let project = load(&[
        (
            "src/x/y.cpl",
            "module q.r.s\n\nfn f() -> Int {\n    1\n}\n\npub type T(Int)\n",
        ),
        (
            "src/q/r/s/more.cpl",
            "module q.r.s\n\ntest \"t\" {\n    assert true\n}\n",
        ),
    ]);
    let map = facet_map(&project, revision(&project));
    let ids: BTreeSet<&str> = map.entries.keys().map(StableId::as_str).collect();
    assert_eq!(ids, BTreeSet::from(["q.r.s.f", "q.r.s.T", "q.r.s.\"t\""]));
    for entry in map.entries.values() {
        assert_eq!(entry.module, "q.r.s");
        assert_eq!(entry.version.path, entry.version.id.as_str());
        assert!(entry.version.provisional);
    }
    assert_eq!(map.get(&sid("q.r.s.f")).expect("f").file, "src/x/y.cpl");
    let t = map.get(&sid("q.r.s.\"t\"")).expect("t");
    assert_eq!(
        (t.file.as_str(), t.name.as_str()),
        ("src/q/r/s/more.cpl", "t")
    );
}

#[test]
fn facet_text_reads_the_module_path_only_for_a_provisional_identity() {
    // 1.8: `module_path` is the file's `module` header, "which the
    // provisional `identity` text needs"; no other facet reads it (5.2).
    let src = "module a\n\nfn f() -> Int {\n    1\n}\n\n@id(\"fn_aaaaaaaaaaaaaaaa\")\nfn g() -> Int {\n    1\n}\n";
    let m = parse(src);
    let own = module_path(&m);
    let other = vec!["x".to_string(), "y".to_string()];
    assert_eq!(
        facet_text(&other, decl(&m, "f"), Facet::Identity).as_deref(),
        Some("x.y.f")
    );
    assert_eq!(
        facet_text(&other, decl(&m, "g"), Facet::Identity).as_deref(),
        Some("fn_aaaaaaaaaaaaaaaa")
    );
    for d in &m.declarations {
        for facet in Facet::ALL.into_iter().filter(|f| *f != Facet::Identity) {
            assert_eq!(facet_text(&other, d, facet), facet_text(&own, d, facet));
        }
    }
    let g = symbol_version(&other, decl(&m, "g"));
    assert_eq!(
        (g.id.as_str(), g.path.as_str()),
        ("fn_aaaaaaaaaaaaaaaa", "x.y.g")
    );
}

#[test]
fn annotated_declarations_of_every_kind_are_not_provisional_and_keep_their_paths() {
    // 4.3 and `SymbolVersion` (1.4.2): `id` is the `@id` value,
    // `provisional` is false, and `path` is still the display symbol path.
    let src = r#"module m

@id("ty_aaaaaaaaaaaaaaaa")
pub type W(Int)

@id("ty_baaaaaaaaaaaaaaa")
pub type R = Int where self > 0

@id("ty_caaaaaaaaaaaaaaa")
pub struct S {
    a: Int
}

@id("ty_daaaaaaaaaaaaaaa")
pub enum E {
    A
}

@id("cap_aaaaaaaaaaaaaaaa")
pub capability C {
    fn get() -> Int
}

@id("fn_aaaaaaaaaaaaaaaa")
fn f() -> Int {
    1
}

@id("test_aaaaaaaaaaaaaaaa")
test "t" {
    assert true
}

@id("prop_aaaaaaaaaaaaaaaa")
property "p"
    for x in gen.int
{
    assert x == x
}
"#;
    let m = parse(src);
    let mp = module_path(&m);
    let found: Vec<(String, String, bool)> = m
        .declarations
        .iter()
        .map(|d| {
            let v = symbol_version(&mp, d);
            (v.id.as_str().to_string(), v.path, v.provisional)
        })
        .collect();
    let expected = [
        ("ty_aaaaaaaaaaaaaaaa", "m.W"),
        ("ty_baaaaaaaaaaaaaaa", "m.R"),
        ("ty_caaaaaaaaaaaaaaa", "m.S"),
        ("ty_daaaaaaaaaaaaaaa", "m.E"),
        ("cap_aaaaaaaaaaaaaaaa", "m.C"),
        ("fn_aaaaaaaaaaaaaaaa", "m.f"),
        ("test_aaaaaaaaaaaaaaaa", "m.\"t\""),
        ("prop_aaaaaaaaaaaaaaaa", "m.\"p\""),
    ]
    .map(|(id, path)| (id.to_string(), path.to_string(), false));
    assert_eq!(found, expected);
}

#[test]
fn only_an_annotation_named_id_supplies_the_identity() {
    // semantics S2: the identity is the argument of the `@id` annotation. An
    // `@identity` or `@ids` annotation, a doc comment, or an ordinary comment
    // that mentions `@id` is not it, and `cpl id assign` (4.2 step 2:
    // "whose annotations contain no `id`") still assigns, before the first
    // annotation.
    let src = "module m\n\n/// @id(\"fn_aaaaaaaaaaaaaaaa\")\n// @id(\"fn_bbbbbbbbbbbbbbbb\")\n@identity(\"fn_cccccccccccccccc\")\n@ids(\"fn_dddddddddddddddd\")\nfn f() -> Int {\n    1\n}\n";
    let m = parse(src);
    assert_eq!(
        declaration_identity(&module_path(&m), &m.declarations[0]).as_str(),
        "m.f"
    );
    let (plan, out, _) = assign(src);
    assert_eq!(plan.len(), 1);
    assert_eq!(out, splice(src, &[(at(src, "@identity"), draw("fn", 0))]));
}

// ===========================================================================
// 4.2 plan_id_assignments: order and determinism
// ===========================================================================

#[test]
fn the_plan_follows_lexicographic_path_order_across_source_roots() {
    // 4.2 step 2: files in path order, declarations in source order; path
    // order is the lexicographic order of project-relative paths (3.2, T15),
    // whatever the order of `[build] sources`: `-` < `.` < `/` < `0` and
    // upper case before lower case. One id is generated per declaration, in
    // that order, so with [`Draws`] the i-th assignment carries the i-th draw.
    let manifest =
        "[project]\nname = \"t\"\nedition = \"0.1\"\n\n[build]\nsources = [\"src\", \"lib\"]\n";
    let project = load_with(
        manifest,
        &[
            (
                "src/b.cpl",
                "module b\n\nfn b1() -> Int {\n    1\n}\n\nfn b2() -> Int {\n    2\n}\n",
            ),
            (
                "src/a.cpl",
                "module a\n\npub type A1(Int)\n\nfn a2() -> Int {\n    2\n}\n",
            ),
            (
                "src/a/z.cpl",
                "module a.z\n\ntest \"z1\" {\n    assert true\n}\n",
            ),
            ("lib/q.cpl", "module q\n\nfn q1() -> Int {\n    1\n}\n"),
            ("src/a-b.cpl", "module ab\n\nfn ab1() -> Int {\n    1\n}\n"),
            ("src/a0.cpl", "module azero\n\nfn z0() -> Int {\n    1\n}\n"),
            ("src/A.cpl", "module upper\n\nfn u1() -> Int {\n    1\n}\n"),
        ],
    );
    let mut rng = Draws::default();
    let plan = plan_id_assignments(&project, &mut rng);
    let found: Vec<(String, String, String)> = plan
        .iter()
        .map(|a| {
            (
                project.source_map.file(a.file).path.clone(),
                a.path.clone(),
                a.id.as_str().to_string(),
            )
        })
        .collect();
    let expected: Vec<(String, String, String)> = [
        ("lib/q.cpl", "q.q1", "fn"),
        ("src/A.cpl", "upper.u1", "fn"),
        ("src/a-b.cpl", "ab.ab1", "fn"),
        ("src/a.cpl", "a.A1", "ty"),
        ("src/a.cpl", "a.a2", "fn"),
        ("src/a/z.cpl", "a.z.\"z1\"", "test"),
        ("src/a0.cpl", "azero.z0", "fn"),
        ("src/b.cpl", "b.b1", "fn"),
        ("src/b.cpl", "b.b2", "fn"),
    ]
    .iter()
    .enumerate()
    .map(|(i, (file, path, kind))| (file.to_string(), path.to_string(), draw(kind, i)))
    .collect();
    assert_eq!(found, expected);
    // Ten bytes per declaration and nothing more (4.1, 4.2 step 2).
    assert_eq!(rng.at, 10 * plan.len());
    // The same generator state plans the same ids.
    assert_eq!(plan, plan_id_assignments(&project, &mut Draws::default()));
}

#[test]
fn every_kind_gets_an_id_of_its_kind_right_before_its_first_token() {
    // 4.2 steps 2 and 3 over one declaration of every kind: `IdKind::for_decl`
    // picks the prefix, `insert_at` is `pub` or the keyword after the doc
    // comments, `indent` is empty at column 0, `path` the display symbol
    // path, and the declaration that already has an `@id` is skipped.
    let project = load(&[("src/shop/items.cpl", KINDS)]);
    let plan = plan_id_assignments(&project, &mut Draws::default());
    let expected = [
        ("pub type ItemId", "ty", "shop.items.ItemId", IdKind::Ty),
        ("type Small", "ty", "shop.items.Small", IdKind::Ty),
        ("pub type Opaque", "ty", "shop.items.Opaque", IdKind::Ty),
        ("pub struct Item", "ty", "shop.items.Item", IdKind::Ty),
        ("pub enum Outcome", "ty", "shop.items.Outcome", IdKind::Ty),
        (
            "pub capability Store",
            "cap",
            "shop.items.Store",
            IdKind::Cap,
        ),
        ("fn stub", "fn", "shop.items.stub", IdKind::Fn),
        (
            "test \"a",
            "test",
            "shop.items.\"a \\\"quoted\\\" name\"",
            IdKind::Test,
        ),
        (
            "property \"round",
            "prop",
            "shop.items.\"round trip\"",
            IdKind::Prop,
        ),
    ];
    assert_eq!(plan.len(), expected.len());
    for (i, (a, (marker, prefix, path, kind))) in plan.iter().zip(expected).enumerate() {
        assert_eq!(a.insert_at as usize, at(KINDS, marker), "{path}");
        assert_eq!(a.id.as_str(), draw(prefix, i), "{path}");
        assert_eq!((a.path.as_str(), a.kind), (path, kind));
        assert_eq!(a.indent, "", "{path}");
    }
    let file = project.source_map.file(plan[0].file);
    let insertions: Vec<(usize, String)> = plan
        .iter()
        .map(|a| (a.insert_at as usize, a.id.as_str().to_string()))
        .collect();
    let out = apply_id_assignments(file.text(), &plan);
    assert_eq!(out, splice(KINDS, &insertions));
}

#[test]
fn the_plan_covers_only_cpl_files_of_the_source_set() {
    // 4.2: "With no paths it covers every source-set `.cpl` file": `.work`
    // and `.patch` files have no declarations, a `.cpl` file outside the
    // source directories is not in the source set (Section 0), and std is
    // never touched (4.2 rules).
    let work = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/reader/work/pagination.work"
    ))
    .expect("the corpus work file");
    let tree = tree_with(
        MANIFEST,
        &[
            ("src/m.cpl", "module m\n\nfn f() -> Int {\n    1\n}\n"),
            ("notes/n.cpl", "module notes\n\nfn n() -> Int {\n    1\n}\n"),
            ("work/pagination.work", &work),
        ],
    );
    let project = load_project_from_tree(
        &tree,
        FileOrigin::Virtual,
        &LoadOptions {
            include_work_files: true,
            ..LoadOptions::default()
        },
    )
    .expect("loads");
    let plan = plan_id_assignments(&project, &mut Draws::default());
    let paths: Vec<&str> = plan.iter().map(|a| a.path.as_str()).collect();
    assert_eq!(paths, ["m.f"]);
    let files: BTreeSet<&str> = plan
        .iter()
        .map(|a| project.source_map.file(a.file).path.as_str())
        .collect();
    assert_eq!(files, BTreeSet::from(["src/m.cpl"]));
}

#[test]
fn planning_never_reuses_an_id_declared_later_in_path_order() {
    // 4.2 rules: `cpl id assign` never reuses an id, including one written
    // in a file that comes after the one being planned. With [`Draws`] the
    // first two draws are exactly the ids `src/z.cpl` already carries.
    let taken = [draw("fn", 0), draw("fn", 1)];
    let z = format!(
        "module z\n\n@id(\"{}\")\nfn g() -> Int {{\n    1\n}}\n\n@id(\"{}\")\nfn h() -> Int {{\n    1\n}}\n",
        taken[0], taken[1]
    );
    let project = load(&[
        (
            "src/a.cpl",
            "module a\n\nfn f() -> Int {\n    1\n}\n\nfn k() -> Int {\n    1\n}\n",
        ),
        ("src/z.cpl", &z),
    ]);
    let plan = plan_id_assignments(&project, &mut Draws::default());
    let paths: Vec<&str> = plan.iter().map(|a| a.path.as_str()).collect();
    assert_eq!(paths, ["a.f", "a.k"]);
    for a in &plan {
        assert!(is_generated_format(&a.id), "{}", a.id);
        assert_eq!(a.id.kind_prefix(), "fn");
        assert!(
            !taken.iter().any(|t| t == a.id.as_str()),
            "{} is already declared in src/z.cpl",
            a.id
        );
    }
    assert_ne!(plan[0].id, plan[1].id);
}

// ===========================================================================
// 4.2 apply_id_assignments: byte-exactness
// ===========================================================================

#[test]
fn the_id_goes_before_the_first_of_several_annotations_with_docs_interleaved() {
    // 4.2 step 3: "immediately before the declaration's first annotation if
    // it has any"; the docs on either side and an ordinary comment among the
    // prefix lines stay where they are, and the docs remain one value
    // (grammar 1.2).
    let src = "module m\n\n/// First doc.\n@prove\n/// Second doc.\n// an ordinary comment among the prefix lines\n@unknown(\"x\", 1)\n@prove\npub fn f() -> Int {\n    1\n}\n";
    let (plan, out, _) = assign(src);
    assert_eq!(plan.len(), 1);
    let first = at(src, "@prove\n/// Second");
    assert_eq!(plan[0].insert_at as usize, first);
    assert_eq!(plan[0].indent, "");
    assert_eq!(out, splice(src, &[(first, draw("fn", 0))]));
    let m = parse(&out);
    let f = &m.declarations[0];
    assert_eq!(f.id(), Some(draw("fn", 0).as_str()));
    assert_eq!(
        f.docs.as_ref().map(ast::Docs::text).as_deref(),
        Some("First doc.\nSecond doc.")
    );
    let names: Vec<&str> = f.annotations.iter().map(|a| a.name.text.as_str()).collect();
    assert_eq!(names, ["id", "prove", "unknown", "prove"]);
    assert_nothing_left_to_assign(&out);
}

#[test]
fn without_annotations_the_id_goes_right_before_the_keyword_after_docs_and_comments() {
    // 4.2 step 3: "otherwise immediately before the first token of its
    // declaration (`pub` or the keyword), after its doc comments": an
    // ordinary comment or a blank line between the docs and the keyword
    // stays above the inserted line.
    let src = "module m\n\n/// Doc.\n// a comment after the docs\n\npub struct S {\n    a: Int\n}\n\n// only a comment\nenum E {\n    A\n}\n\ncapability C {\n    fn get() -> Int\n}\n";
    let (plan, out, _) = assign(src);
    let kinds: Vec<IdKind> = plan.iter().map(|a| a.kind).collect();
    assert_eq!(kinds, [IdKind::Ty, IdKind::Ty, IdKind::Cap]);
    assert_eq!(
        out,
        splice(
            src,
            &[
                (at(src, "pub struct S"), draw("ty", 0)),
                (at(src, "enum E"), draw("ty", 1)),
                (at(src, "capability C"), draw("cap", 2)),
            ]
        )
    );
    assert_nothing_left_to_assign(&out);
}

#[test]
fn crlf_files_keep_every_crlf_and_gain_lf_terminated_lines() {
    // 4.2 step 3: the inserted line is `@id("...")` followed by LF and no
    // other byte changes, so every CRLF of the file stays; the lexer accepts
    // the mixed result (grammar 1.1). The last declaration has no final line
    // terminator.
    let src = "module m\r\n\r\n/// Doc one.\r\n@prove\r\n/// Doc two.\r\nfn f() -> Int {\r\n    1\r\n}\r\n\r\npub type T(Int)\r\npub type U(Int)\r\n\r\ntest \"t\" {\r\n    assert true\r\n}";
    let (plan, out, _) = assign(src);
    assert_eq!(
        out,
        splice(
            src,
            &[
                (at(src, "@prove"), draw("fn", 0)),
                (at(src, "pub type T"), draw("ty", 1)),
                (at(src, "pub type U"), draw("ty", 2)),
                (at(src, "test \"t\""), draw("test", 3)),
            ]
        )
    );
    assert_eq!(plan.len(), 4);
    assert_eq!(out.matches("\r\n").count(), src.matches("\r\n").count());
    assert_eq!(
        out.matches('\n').count(),
        src.matches('\n').count() + 4,
        "exactly one LF per inserted line"
    );
    assert!(out.ends_with("    assert true\r\n}"));
    // The result parses, every id is in place, and only identities changed.
    let before = texts_of(src);
    let after = texts_of(&out);
    for (name, facets) in &before {
        for (facet, t) in facets {
            if *facet != Facet::Identity {
                assert_eq!(&after[name][facet], t, "{name} {facet}");
            }
        }
    }
    assert_eq!(after["f"][&Facet::Docs], "Doc one.\nDoc two.");
    assert_eq!(after["f"][&Facet::Identity], draw("fn", 0));
    assert_nothing_left_to_assign(&out);
}

#[test]
fn declarations_at_the_end_of_a_file_without_a_final_line_terminator() {
    // 4.2 step 3 at the end of a file: nothing is appended, and the
    // definition-less type and the bodiless fn that end the file (grammar
    // 3.5, 3.9) get their lines like any other declaration.
    let src =
        "module m\n\npub type Opaque<T>\n\n@prove\nfn stub(a: Int) -> Int\n    requires a > 0";
    let (_, out, _) = assign(src);
    assert_eq!(
        out,
        splice(
            src,
            &[
                (at(src, "pub type"), draw("ty", 0)),
                (at(src, "@prove"), draw("fn", 1)),
            ]
        )
    );
    assert!(out.ends_with("    requires a > 0"));
    assert_nothing_left_to_assign(&out);
}

#[test]
fn a_declaration_on_the_line_after_the_header_or_a_use() {
    // 4.2 step 3 with no blank line before the declaration (grammar 3.1
    // needs only one NEWLINE).
    let src = "module m\nfn f() -> Int {\n    1\n}\n";
    let (_, out, _) = assign(src);
    assert_eq!(out, splice(src, &[(at(src, "fn f"), draw("fn", 0))]));
    let src = "module m\nuse std.text.{trim}\nfn g() -> Text {\n    trim(\" x \")\n}";
    let (_, out, _) = assign(src);
    assert_eq!(out, splice(src, &[(at(src, "fn g"), draw("fn", 0))]));
    assert_nothing_left_to_assign(&out);
}

#[test]
fn a_byte_order_mark_is_kept_and_offsets_count_its_bytes() {
    // grammar 1.1: the lexer skips a leading byte-order mark, but `cpl id
    // assign` changes no byte other than the inserted line (4.2 step 3), so
    // the mark stays; `insert_at` is a byte offset into the file's text.
    let src = "\u{feff}module m\n\nfn f() -> Int {\n    1\n}\n";
    let (plan, out, _) = assign(src);
    assert_eq!(plan[0].insert_at as usize, at(src, "fn f"));
    assert_eq!(out, splice(src, &[(at(src, "fn f"), draw("fn", 0))]));
    assert!(out.starts_with('\u{feff}'));
}

#[test]
fn multibyte_text_before_an_insertion_point_is_left_intact() {
    // 4.2 step 3: byte offsets across multibyte comments, doc comments, and
    // text literals (non-ASCII is permitted in all three, grammar 1.1).
    let src = "module m\n\n// commentaire : \u{e9}, \u{2713}, \u{1f980}\n/// Gr\u{f6}\u{df}e in \u{1f4a1} units.\nfn f() -> Text {\n    \"na\u{ef}ve \u{1f980}\"\n}\n\n/// \u{dc}n\u{ef}c\u{f6}d\u{e9}.\n@prove\nfn g() -> Int {\n    1\n}\n";
    let (_, out, _) = assign(src);
    assert_eq!(
        out,
        splice(
            src,
            &[
                (at(src, "fn f"), draw("fn", 0)),
                (at(src, "@prove"), draw("fn", 1)),
            ]
        )
    );
    assert_nothing_left_to_assign(&out);
}

#[test]
fn an_empty_plan_changes_nothing() {
    // 4.2: "the result differs from `text` only by the inserted lines".
    let src = "module m\r\n\r\n@id(\"fn_aaaaaaaaaaaaaaaa\")\r\nfn f() -> Int {\r\n    1\r\n}";
    let (plan, out, _) = assign(src);
    assert!(plan.is_empty());
    assert_eq!(out, src);
}

#[test]
fn the_inserted_line_is_at_column_zero_even_before_an_indented_declaration() {
    // 4.2 step 3: "The inserted line is at column 0. No other byte changes."
    // Indentation is never significant (grammar 1.1), so an unformatted file
    // may indent a top-level declaration; the reading that satisfies both
    // sentences inserts `@id("...")` LF at the start of the declaration's
    // line, before its indentation, which stays untouched. (The `indent`
    // field's comment in 1.8, "always \"\" for top-level items", does not hold
    // for such a file; reported as a spec issue.)
    let src =
        "module m\n\n    /// Indented docs.\n    pub fn indented() -> Int {\n        1\n    }\n";
    let (plan, out, _) = assign(src);
    assert_eq!(plan[0].insert_at as usize, at(src, "pub fn"));
    assert_eq!(
        out,
        splice(src, &[(at(src, "    pub fn"), draw("fn", 0))]),
        "the inserted line must start at column 0"
    );
}

#[test]
fn assignment_json_reports_one_based_lines_with_crlf_and_a_bom() {
    // 7.2 `IdAssignOutput`: `line` is the 1-based line of the declaration;
    // CRLF is one line terminator (grammar 1.1) and a byte-order mark is not
    // a line.
    let src = "\u{feff}module m\r\n\r\nfn f() -> Int {\r\n    1\r\n}\r\n\r\n\r\ntest \"t\" {\r\n    assert true\r\n}\r\n";
    let (plan, _, project) = assign(src);
    let json: Vec<serde_json::Value> = plan
        .iter()
        .map(|a| serde_json::to_value(a.to_json(&project.source_map)).expect("serializes"))
        .collect();
    assert_eq!(
        json,
        [
            serde_json::json!({ "path": "src/m.cpl", "line": 3, "symbol": "m.f", "kind": "fn", "id": draw("fn", 0) }),
            serde_json::json!({ "path": "src/m.cpl", "line": 8, "symbol": "m.\"t\"", "kind": "test", "id": draw("test", 1) }),
        ]
    );
}

#[test]
fn assigning_ids_changes_only_the_identity_facet_of_each_declaration() {
    // 4.2 + 5.2: an inserted `@id` line is an annotation, so of the eight
    // facets only `identity` may change, and it becomes the planned id;
    // `move_item`, which had an `@id`, is untouched.
    let project = load(&[("src/shop/items.cpl", KINDS)]);
    let plan = plan_id_assignments(&project, &mut Draws::default());
    let out = apply_id_assignments(project.source_map.file(plan[0].file).text(), &plan);
    let before = texts_of(KINDS);
    let after = texts_of(&out);
    let planned: BTreeMap<&str, &str> = plan
        .iter()
        .map(|a| (a.path.as_str(), a.id.as_str()))
        .collect();
    let m = parse(&out);
    let mp = module_path(&m);
    for d in &m.declarations {
        let name = d.display_name();
        let path = symbol_path(&mp, d).display();
        for (facet, t) in &before[name] {
            match (facet, planned.get(path.as_str())) {
                (Facet::Identity, Some(id)) => assert_eq!(after[name][facet], *id, "{name}"),
                _ => assert_eq!(&after[name][facet], t, "{name} {facet}"),
            }
        }
    }
    assert_eq!(after["move_item"][&Facet::Identity], "fn_aaaaaaaaaaaaaaab");
}

// ===========================================================================
// 5.1 Coverage and 5.2 Canonical text, every facet of every kind
// ===========================================================================

/// One declaration of every kind of 5.1's coverage table, plus the
/// definition-less `type` and bodiless `fn` forms (grammar 3.5, 3.9), which
/// parse in project files as well.
const KINDS: &str = r#"module shop.items

/// An item id.
pub type ItemId(Text)

/// A small count.
type Small = Int where self >= 0 and self < 10

/// A definition-less type.
pub type Opaque<T>

/// An item.
pub struct Item<T> {
    /// Its id.
    id: ItemId
    payload: List<T>
}

pub enum Outcome<E> {
    Done
    /// Failed with a reason.
    Failed(E, Text)
}

/// The store.
pub capability Store {
    /// Reads.
    fn get(id: ItemId) -> Result<Item<Int>, Text>
        ensures true
        requires id != ItemId("")
    fn put(item: Item<Int>, force: Bool) -> Result<Unit, Text>
    fn remove(id: ItemId) -> Bool
        requires true
}

/// Moves an item.
@id("fn_aaaaaaaaaaaaaaab")
@prove
pub fn move_item<T, U>(item: Item<T>, n: Int, using store: Store, using log: Store) -> Int
    ensures result >= 0
    effects { store.put, log.get, store.get }
    requires n > 0
    decreases n
    requires n < 100
{
    var x = n + 1
    while x > 0
        decreases x
        invariant x >= 0
    {
        x = x - 1
    }
    x
}

/// A stub.
fn stub(a: Int) -> Int
    requires a > 0

test "a \"quoted\" name" {
    assert true
}

/// Holds.
property "round trip"
    for a in gen.int
    for b in gen.list(gen.int)
{
    assert a == a
}
"#;

/// [`KINDS`] with comments everywhere, blank lines, broken groups with
/// trailing commas, irregular spacing and tabs, trailing whitespace on a doc
/// line, clauses in another order, a repeated effect, docs written after the
/// annotations, and test and property names spelled with escapes. `<TAB>`
/// and `<WS>` stand for a tab and for trailing spaces.
const KINDS_MESSY: &str = r#"// a comment above the header
module shop.items // the header's comment

// a comment above ItemId
/// An item id.<WS>
pub type ItemId(Text) // trailing

/// A small count.
type Small = Int where self  >=  0   and   self < 10

/// A definition-less type.
pub type Opaque<T> // opaque



// blank lines and a comment above
/// An item.
pub struct Item<T>
{
    // a comment above the first field

    /// Its id.
    id: ItemId // a field comment


<TAB>payload:   List<T>
    // dangling
}

pub enum Outcome<E> {
    Done // done
    /// Failed with a reason.
    Failed(
        E, // the error
        Text,
    )
}

/// The store.
pub capability Store {
    /// Reads.
    fn get(
        id: ItemId,
    ) -> Result<Item<Int>, Text>
        // a comment among the clauses
        requires id  !=  ItemId("")
        ensures true

    fn put(item: Item<Int>,   force: Bool) -> Result<Unit, Text> // put
    fn remove(id: ItemId) -> Bool
        requires   true // why
}

@prove
// a comment between the annotations
@id("fn_aaaaaaaaaaaaaaab") // the id
/// Moves an item.
pub fn move_item<T, U>(
    item: Item<T>, // first
    n: Int,
    using store: Store,
    using log: Store, // last
) -> Int
    requires n > 0

    effects {
        store.put,
        log.get, // reads
        store.get,
        store.put,
    }
    ensures result >= 0 // nonneg
    decreases n
    requires n < 100
{
    // start

    var x = n  +  1

    while x > 0
        decreases x
        // the invariant
        invariant x >= 0
    {
<TAB><TAB>x = x - 1 // step
    }

    x // the result
}

/// A stub.
fn stub(a: Int) -> Int
    requires a > 0 // the stub's contract

test "a \u{22}quoted\u{0022} name" {
    // nothing to set up
    assert true
}

/// Holds.
property "round\u{20}trip"
    for a in gen.int // ints
    for b in gen.list(
        gen.int,
    )
{
    assert a == a
}
// a comment after the last declaration
"#;

fn kinds_messy() -> String {
    KINDS_MESSY.replace("<TAB>", "\t").replace("<WS>", "   ")
}

/// The canonical text of every facet, in `Facet::ALL` order (identity, name,
/// signature, contract, effects, body, docs, verification_policy), of every
/// declaration of [`KINDS`], written by hand from 5.1's coverage table and
/// 5.2's canonical-text table; `None` is a facet that does not apply.
fn kinds_expected() -> Vec<(&'static str, [Option<&'static str>; 8])> {
    vec![
        (
            "ItemId",
            [
                Some("shop.items.ItemId"),
                Some("ItemId"),
                Some("pub type _(Text)"),
                None,
                None,
                None,
                Some("An item id."),
                None,
            ],
        ),
        (
            "Small",
            [
                Some("shop.items.Small"),
                Some("Small"),
                Some("type _ = Int"),
                Some("where self >= 0 and self < 10"),
                None,
                None,
                Some("A small count."),
                None,
            ],
        ),
        // 5.1 has no column for a definition-less type; 5.2 gives its
        // signature (`pub type _<T>`), and it has a wrapper's facets: it has
        // no predicate, so no contract.
        (
            "Opaque",
            [
                Some("shop.items.Opaque"),
                Some("Opaque"),
                Some("pub type _<T>"),
                None,
                None,
                None,
                Some("A definition-less type."),
                None,
            ],
        ),
        (
            "Item",
            [
                Some("shop.items.Item"),
                Some("Item"),
                Some("pub struct _<T> {\n    id: ItemId\n    payload: List<T>\n}"),
                None,
                None,
                None,
                Some("An item.\nmember id\nIts id."),
                None,
            ],
        ),
        // 5.2 docs, read literally: the declaration's (empty) doc text, then
        // LF `member <name>` LF and its lines.
        (
            "Outcome",
            [
                Some("shop.items.Outcome"),
                Some("Outcome"),
                Some("pub enum _<E> {\n    Done\n    Failed(E, Text)\n}"),
                None,
                None,
                None,
                Some("\nmember Failed\nFailed with a reason."),
                None,
            ],
        ),
        // The capability contract lists only the operations with clauses;
        // within one, `requires` precede `ensures` (design line 634:
        // "Contracts are ordered requires clauses followed by ensures clauses
        // in canonical output"; 5.2 is not explicit for operations).
        (
            "Store",
            [
                Some("shop.items.Store"),
                Some("Store"),
                Some("pub capability _ {\n    fn get(id: ItemId) -> Result<Item<Int>, Text>\n    fn put(item: Item<Int>, force: Bool) -> Result<Unit, Text>\n    fn remove(id: ItemId) -> Bool\n}"),
                Some("fn get\n    requires id != ItemId(\"\")\n    ensures true\nfn remove\n    requires true"),
                None,
                None,
                Some("The store.\nmember get\nReads."),
                None,
            ],
        ),
        (
            "move_item",
            [
                Some("fn_aaaaaaaaaaaaaaab"),
                Some("move_item"),
                Some("pub fn _<T, U>(item: Item<T>, n: Int, using store: Store, using log: Store) -> Int"),
                Some("requires n > 0\nrequires n < 100\nensures result >= 0\ndecreases n"),
                Some("effects { log.get, store.get, store.put }"),
                Some("{\n    var x = n + 1\n    while x > 0\n        invariant x >= 0\n        decreases x\n    {\n        x = x - 1\n    }\n    x\n}"),
                Some("Moves an item."),
                Some("@prove"),
            ],
        ),
        // A bodiless fn: 5.1 gives every fn all eight facets and 5.2 is
        // silent on a missing block; the empty text keeps the facet present
        // (5.3: "keeps every applicable facet present in every map").
        (
            "stub",
            [
                Some("shop.items.stub"),
                Some("stub"),
                Some("fn _(a: Int) -> Int"),
                Some("requires a > 0"),
                Some(""),
                Some(""),
                Some("A stub."),
                Some(""),
            ],
        ),
        (
            "a \"quoted\" name",
            [
                Some("shop.items.\"a \\\"quoted\\\" name\""),
                Some("\"a \\\"quoted\\\" name\""),
                None,
                None,
                None,
                Some("{\n    assert true\n}"),
                Some(""),
                None,
            ],
        ),
        (
            "round trip",
            [
                Some("shop.items.\"round trip\""),
                Some("\"round trip\""),
                Some("for a in gen.int\nfor b in gen.list(gen.int)"),
                None,
                None,
                Some("{\n    assert a == a\n}"),
                Some("Holds."),
                None,
            ],
        ),
    ]
}

#[test]
fn every_facet_of_every_declaration_kind_has_its_section_5_2_text() {
    let m = parse(KINDS);
    let mp = module_path(&m);
    let expected = kinds_expected();
    assert_eq!(m.declarations.len(), expected.len());
    let mut mismatches = Vec::new();
    for (name, texts) in &expected {
        let d = decl(&m, name);
        for (facet, want) in Facet::ALL.iter().zip(texts) {
            let got = facet_text(&mp, d, *facet);
            if got.as_deref() != *want {
                mismatches.push(format!("{name} {facet}: expected {want:?}, found {got:?}"));
            }
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

#[test]
fn versions_hold_exactly_the_applicable_facets_with_their_hashes() {
    // 5.1: an inapplicable facet is absent from the map; 5.3: every present
    // one, empty texts included, is the truncated blake3 of its text; 5.4:
    // the `.definition` line format, recomputed by hand.
    let m = parse(KINDS);
    let mp = module_path(&m);
    for (name, texts) in kinds_expected() {
        let d = decl(&m, name);
        let applicable: Vec<Facet> = Facet::ALL
            .into_iter()
            .zip(texts)
            .filter(|(_, t)| t.is_some())
            .map(|(f, _)| f)
            .collect();
        assert_eq!(applicable_facets(&d.kind), applicable.as_slice(), "{name}");
        let v = symbol_version(&mp, d);
        assert_eq!(
            v.facets.keys().copied().collect::<Vec<_>>(),
            applicable,
            "{name}"
        );
        let actual: Vec<(Facet, String)> = applicable
            .iter()
            .map(|&f| (f, facet_text(&mp, d, f).expect("applies")))
            .collect();
        for (f, t) in &actual {
            assert_eq!(v.facets[f].as_str(), &blake3_hex(t)[..32], "{name} {f}");
        }
        let lines: Vec<(Facet, &str)> = actual.iter().map(|(f, t)| (*f, t.as_str())).collect();
        assert_eq!(v.definition.as_str(), definition_oracle(&lines), "{name}");
        assert_eq!(v.provisional, name != "move_item", "{name}");
    }
}

#[test]
fn comments_blank_lines_layout_and_crlf_change_no_facet_of_any_kind() {
    // 5.2: ordinary comments are excluded from every facet, blank lines and
    // source line breaks are not reproduced, groups are flat, the effects set
    // is sorted and deduplicated (T26), clauses are in canonical order, doc
    // lines lose trailing whitespace like the formatter's (rule 1), and text
    // literals are decoded before they are re-escaped (rule 10). CRLF is one
    // line terminator (grammar 1.1).
    let clean = texts_of(KINDS);
    for (label, src) in [
        ("messy", kinds_messy()),
        ("messy, CRLF", kinds_messy().replace('\n', "\r\n")),
        ("clean, CRLF", KINDS.replace('\n', "\r\n")),
        // grammar 1.1: a leading byte-order mark is skipped.
        ("clean, BOM", format!("\u{feff}{KINDS}")),
    ] {
        let other = texts_of(&src);
        assert_eq!(
            other.keys().collect::<Vec<_>>(),
            clean.keys().collect::<Vec<_>>(),
            "{label}"
        );
        let mut mismatches = Vec::new();
        for (name, facets) in &clean {
            for (facet, t) in facets {
                let o = &other[name][facet];
                if o != t {
                    mismatches.push(format!("{label}: {name} {facet}: {t:?} vs {o:?}"));
                }
            }
        }
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    }
}

#[test]
fn formatting_a_file_changes_no_facet() {
    // T14 [D-149]: the formatter changes only whitespace, line structure,
    // clause order, and text-literal escapes, none of which reaches a
    // canonical text (5.2), so `cpl fmt` never changes a content version.
    for src in [
        KINDS.to_string(),
        kinds_messy(),
        kinds_messy().replace('\n', "\r\n"),
    ] {
        let formatted = cpl_syntax::format_source(&src, cpl_syntax::FileKind::Cpl, FileId(0))
            .unwrap_or_else(|e| panic!("formats: {e:?}"));
        assert_eq!(texts_of(&formatted), texts_of(&src));
    }
}

#[test]
fn canonical_text_never_breaks_a_long_line_even_after_the_formatter_broke_it() {
    // 5.2: "every clause, signature line, and `effects` set is one line";
    // the width rule (2.4 rule 6 (iii)) applies to files, and once `cpl fmt`
    // has broken the long groups the facets must still be the same.
    let src = "module m\n\nfn long(first_parameter_with_a_long_name: List<List<Int>>, second_parameter_with_a_long_name: Result<Text, Text>, using store_with_a_long_name: Store) -> Result<List<List<Int>>, Text>\n    requires check_something_long(first_parameter_with_a_long_name, second_parameter_with_a_long_name, 12345)\n    effects { store_with_a_long_name.write_everything, store_with_a_long_name.read_everything, store_with_a_long_name.delete }\n{\n    let value = some_function_with_a_long_name(first_parameter_with_a_long_name, second_parameter_with_a_long_name)\n    Ok(value)\n}\n";
    let expected = [
        (
            Facet::Signature,
            "fn _(first_parameter_with_a_long_name: List<List<Int>>, second_parameter_with_a_long_name: Result<Text, Text>, using store_with_a_long_name: Store) -> Result<List<List<Int>>, Text>",
        ),
        (
            Facet::Contract,
            "requires check_something_long(first_parameter_with_a_long_name, second_parameter_with_a_long_name, 12345)",
        ),
        (
            Facet::Effects,
            "effects { store_with_a_long_name.delete, store_with_a_long_name.read_everything, store_with_a_long_name.write_everything }",
        ),
        (
            Facet::Body,
            "{\n    let value = some_function_with_a_long_name(first_parameter_with_a_long_name, second_parameter_with_a_long_name)\n    Ok(value)\n}",
        ),
    ];
    let formatted = cpl_syntax::format_source(src, cpl_syntax::FileKind::Cpl, FileId(0))
        .unwrap_or_else(|e| panic!("formats: {e:?}"));
    assert!(
        formatted.lines().all(|l| l.chars().count() <= 100),
        "the formatter breaks the long lines:\n{formatted}"
    );
    for source in [src, formatted.as_str()] {
        for (facet, want) in expected {
            assert!(want.lines().any(|l| l.chars().count() > 100));
            assert_eq!(
                text(source, "long", facet).as_deref(),
                Some(want),
                "{facet}"
            );
        }
    }
}

#[test]
fn line_breaks_inside_parentheses_are_not_reproduced() {
    // 5.2: canonical text ignores `BinaryExpr::break_before_op` (2.4 rule 9)
    // and keeps parentheses exactly as written, in clauses, predicates, and
    // bodies alike.
    let src = "module m\n\ntype Small = Int where (self >= 0\n    and self < 10)\n\nfn f(a: Int, b: Int) -> Int\n    requires (a > 0\n        and b > 0)\n    ensures (result\n        == a + b)\n{\n    let c = (a\n        + b)\n    c\n}\n";
    assert_eq!(
        text(src, "Small", Facet::Contract).as_deref(),
        Some("where (self >= 0 and self < 10)")
    );
    assert_eq!(
        text(src, "f", Facet::Contract).as_deref(),
        Some("requires (a > 0 and b > 0)\nensures (result == a + b)")
    );
    assert_eq!(
        text(src, "f", Facet::Body).as_deref(),
        Some("{\n    let c = (a + b)\n    c\n}")
    );
}

#[test]
fn a_body_is_its_block_at_level_zero_with_every_group_flat() {
    // 5.2 `body`: "the block formatted at level 0", canonical (no comments,
    // no blank lines, groups flat, line breaks inside parentheses removed),
    // with the expression layout of 2.4 rule 8 and the literal spellings of
    // rule 10 (`1_000` is `1000`, `\u{41}` is `A`, a tab is `\t`).
    let src = r#"module m

fn shapes(p: Page, xs: List<Int>) -> Result<Int, Text> {
    let a = check<Positive>(  1_000  )?
    var total = (a
        + 2)
    let page = Page {
        ..p,
        text: "tab\there \u{41}",
    }
    let ys = [
        1,
        -2,
    ]
    assert forall x in ys: x != 0
    if not xs.is_empty() {
        total = total + 1
    } else if total > 3 {
        total = 0
    } else {
        return Err("none")
    }
    let label = match page.text {
        "a" => 1
        _ => {
            2
        }
    }
    Ok(total)
}
"#;
    let expected = r#"{
    let a = check<Positive>(1000)?
    var total = (a + 2)
    let page = Page { ..p, text: "tab\there A" }
    let ys = [1, -2]
    assert forall x in ys: x != 0
    if not xs.is_empty() {
        total = total + 1
    } else if total > 3 {
        total = 0
    } else {
        return Err("none")
    }
    let label = match page.text {
        "a" => 1
        _ => {
            2
        }
    }
    Ok(total)
}"#;
    assert_eq!(text(src, "shapes", Facet::Body).as_deref(), Some(expected));
}

#[test]
fn an_empty_effects_set_is_the_same_facet_as_no_effects_clause() {
    // 5.2 `effects`: "empty string when there is no clause or the set is
    // empty"; 5.3: it then has the empty-text hash rather than being absent.
    let none = "module m\n\nfn f() -> Int {\n    1\n}\n";
    let empty = "module m\n\nfn f() -> Int\n    effects {}\n{\n    1\n}\n";
    let commented =
        "module m\n\nfn f() -> Int\n    effects {\n        // nothing yet\n    }\n{\n    1\n}\n";
    for src in [none, empty, commented] {
        assert_eq!(text(src, "f", Facet::Effects).as_deref(), Some(""));
        assert_eq!(
            version(src, "f").facets[&Facet::Effects].as_str(),
            EMPTY_HASH
        );
    }
    assert_eq!(
        version(none, "f").definition,
        version(empty, "f").definition
    );
    assert_eq!(
        version(none, "f").definition,
        version(commented, "f").definition
    );
}

#[test]
fn empty_bodies_have_empty_braces() {
    // 2.4 rule 8: "Blocks are always multi-line except the empty block `{}`",
    // so an empty test body is `{}`. A struct, enum, or capability body with
    // no member is not covered by any rule; the reading taken is the
    // formatter's `{}` (format_fragment is the producer, 5.2), reported as a
    // spec gap since 5.2's `{` LF ... LF `}` template is ambiguous for it.
    let src = "module m\n\npub struct S {}\n\nenum E {}\n\npub capability C {}\n\ntest \"t\" {}\n\nfn f() -> Unit {}\n";
    assert_eq!(
        text(src, "S", Facet::Signature).as_deref(),
        Some("pub struct _ {}")
    );
    assert_eq!(
        text(src, "E", Facet::Signature).as_deref(),
        Some("enum _ {}")
    );
    assert_eq!(
        text(src, "C", Facet::Signature).as_deref(),
        Some("pub capability _ {}")
    );
    assert_eq!(text(src, "C", Facet::Contract).as_deref(), Some(""));
    assert_eq!(text(src, "C", Facet::Docs).as_deref(), Some(""));
    assert_eq!(text(src, "t", Facet::Body).as_deref(), Some("{}"));
    assert_eq!(text(src, "f", Facet::Body).as_deref(), Some("{}"));
}

#[test]
fn doc_lines_keep_inner_empty_lines_and_leading_spaces() {
    // 2.1 `DocLine`: the text after `///` with one leading space removed;
    // 5.2 `docs`: the lines joined with LF (an empty `///` line is an empty
    // line of the value).
    let src = "module m\n\n/// First.\n///\n///   indented by two\n///no space\nfn f() -> Int {\n    1\n}\n";
    assert_eq!(
        text(src, "f", Facet::Docs).as_deref(),
        Some("First.\n\n  indented by two\nno space")
    );
}

#[test]
fn module_docs_are_not_the_docs_of_the_first_declaration() {
    // grammar 1.2: doc comments before the `module` header attach to the
    // module; the first declaration's `docs` facet is empty.
    let src = "/// Module docs.\nmodule m\n\nfn f() -> Int {\n    1\n}\n";
    assert_eq!(text(src, "f", Facet::Docs).as_deref(), Some(""));
    assert_eq!(version(src, "f").facets[&Facet::Docs].as_str(), EMPTY_HASH);
}

#[test]
fn prove_on_a_declaration_that_is_not_a_fn_adds_no_verification_policy() {
    // 5.1: `verification_policy` is `-` for every kind but `fn`; an `@prove`
    // elsewhere is a check error (semantics 1.8), not a facet.
    let src = "module m\n\n@prove\npub type W(Int)\n\n@prove\npub type R = Int where self > 0\n\n@prove\nstruct S {\n    a: Int\n}\n\n@prove\nenum E {\n    A\n}\n\n@prove\ncapability C {\n    fn get() -> Int\n}\n\n@prove\ntest \"t\" {\n    assert true\n}\n\n@prove\nproperty \"p\"\n    for x in gen.int\n{\n    assert x == x\n}\n";
    for name in ["W", "R", "S", "E", "C", "t", "p"] {
        assert_eq!(text(src, name, Facet::VerificationPolicy), None, "{name}");
        assert!(
            !version(src, name)
                .facets
                .contains_key(&Facet::VerificationPolicy),
            "{name}"
        );
    }
}

#[test]
fn symbol_versions_serialize_facets_by_name_with_32_hex_hashes() {
    // 1.4.2 `SymbolVersion` and the baseline of 5.5: `facets` holds only the
    // applicable facets, keyed by their snake_case names, and every hash is
    // 32 lowercase hex characters (5.3, T27).
    let src = "module m\n\nfn f() -> Int {\n    1\n}\n\ntest \"t\" {\n    assert true\n}\n";
    let f = serde_json::to_value(version(src, "f")).expect("serializes");
    let keys: BTreeSet<&str> = f["facets"]
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        BTreeSet::from([
            "identity",
            "name",
            "signature",
            "contract",
            "effects",
            "body",
            "docs",
            "verification_policy"
        ])
    );
    for hash in f["facets"].as_object().expect("an object").values() {
        assert!(is_lower_hex(hash.as_str().expect("a string"), 32), "{hash}");
    }
    assert!(is_lower_hex(
        f["definition"].as_str().expect("a string"),
        32
    ));
    assert_eq!(f["id"], "m.f");
    assert_eq!(f["path"], "m.f");
    assert_eq!(f["provisional"], true);
    let t = serde_json::to_value(version(src, "t")).expect("serializes");
    let keys: BTreeSet<&str> = t["facets"]
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, BTreeSet::from(["identity", "name", "body", "docs"]));
    assert_eq!(t["id"], "m.\"t\"");
}

// ===========================================================================
// Which facets an edit changes (independence and sensitivity)
// ===========================================================================

const FN_BASE: &str = r#"module m

/// Doc.
@prove
pub fn f(a: Int, using db: Db) -> Int
    effects { db.get }
    requires a > 0
    requires a < 10
    ensures result > 0
    decreases a
{
    var x = a
    while x > 0
        invariant x >= 0
        decreases x
    {
        x = x - 1
    }
    x
}
"#;

#[test]
fn edits_of_a_fn_change_exactly_their_facets() {
    use Facet::*;
    check_edits(
        FN_BASE,
        "f",
        &[
            Edit { label: "drop pub", from: "pub fn f(", to: "fn f(", changes: &[Signature] },
            Edit { label: "a parameter type", from: "a: Int,", to: "a: Nat,", changes: &[Signature] },
            Edit { label: "the return type", from: ") -> Int\n", to: ") -> Nat\n", changes: &[Signature] },
            Edit { label: "a using parameter type", from: "using db: Db", to: "using db: Store", changes: &[Signature] },
            Edit { label: "a repeated effect (T26)", from: "effects { db.get }", to: "effects { db.get, db.get }", changes: &[] },
            Edit { label: "a further effect", from: "effects { db.get }", to: "effects { db.put, db.get }", changes: &[Effects] },
            Edit { label: "parentheses in a requires (5.2)", from: "requires a > 0\n", to: "requires (a > 0)\n", changes: &[Contract] },
            Edit { label: "the order of two requires (5.2: source order)", from: "    requires a > 0\n    requires a < 10\n", to: "    requires a < 10\n    requires a > 0\n", changes: &[Contract] },
            Edit { label: "ensures written before the requires", from: "    requires a > 0\n    requires a < 10\n    ensures result > 0\n", to: "    ensures result > 0\n    requires a > 0\n    requires a < 10\n", changes: &[] },
            Edit {
                label: "every clause in reverse order",
                from: "    effects { db.get }\n    requires a > 0\n    requires a < 10\n    ensures result > 0\n    decreases a\n",
                to: "    decreases a\n    ensures result > 0\n    requires a > 0\n    requires a < 10\n    effects { db.get }\n",
                changes: &[],
            },
            Edit { label: "the fn's decreases (T23: contract)", from: "    decreases a\n", to: "    decreases a + 1\n", changes: &[Contract] },
            Edit { label: "a loop invariant (T23: body)", from: "invariant x >= 0", to: "invariant x >= 1", changes: &[Body] },
            Edit { label: "the loop's decreases (T23: body)", from: "        decreases x\n", to: "        decreases x + 1\n", changes: &[Body] },
            Edit { label: "a body statement", from: "x = x - 1", to: "x = x - 2", changes: &[Body] },
            Edit { label: "the doc text", from: "/// Doc.", to: "/// Docs.", changes: &[Docs] },
            Edit { label: "a //// banner is not documentation (D60)", from: "/// Doc.", to: "//// Doc.", changes: &[Docs] },
            Edit { label: "drop @prove", from: "@prove\n", to: "", changes: &[VerificationPolicy] },
            Edit { label: "@prove() is @prove", from: "@prove\n", to: "@prove()\n", changes: &[] },
            Edit { label: "an @id", from: "@prove\n", to: "@prove\n@id(\"fn_aaaaaaaaaaaaaaaa\")\n", changes: &[Identity] },
            Edit { label: "comments and blank lines in the body", from: "    var x = a\n", to: "    // a comment\n\n    var x = a // trailing\n\n", changes: &[] },
            Edit { label: "a comment among the prefix lines", from: "/// Doc.\n@prove\n", to: "/// Doc.\n// note\n@prove\n", changes: &[] },
            Edit { label: "docs written after the annotation (grammar 1.2)", from: "/// Doc.\n@prove\n", to: "@prove\n/// Doc.\n", changes: &[] },
        ],
    );
}

#[test]
fn edits_of_a_struct_change_exactly_their_facets() {
    use Facet::*;
    let base = "module m\n\n/// A page.\npub struct Page {\n    /// The id.\n    id: Int\n    text: Text\n}\n";
    check_edits(
        base,
        "Page",
        &[
            Edit {
                label: "a field's docs (T25)",
                from: "/// The id.",
                to: "/// The key.",
                changes: &[Docs],
            },
            Edit {
                label: "docs on another field (T25)",
                from: "    text: Text\n",
                to: "    /// The text.\n    text: Text\n",
                changes: &[Docs],
            },
            Edit {
                label: "the field order (T13)",
                from: "    /// The id.\n    id: Int\n    text: Text\n",
                to: "    text: Text\n    /// The id.\n    id: Int\n",
                changes: &[Signature],
            },
            Edit {
                label: "a field name",
                from: "text: Text",
                to: "body: Text",
                changes: &[Signature],
            },
            Edit {
                label: "a field type",
                from: "id: Int",
                to: "id: Nat",
                changes: &[Signature],
            },
            Edit {
                label: "comments and blank lines among the fields",
                from: "    text: Text\n",
                to: "\n    // a note\n    text: Text // trailing\n    // dangling\n",
                changes: &[],
            },
            Edit {
                label: "drop pub",
                from: "pub struct",
                to: "struct",
                changes: &[Signature],
            },
            Edit {
                label: "a type parameter",
                from: "Page {",
                to: "Page<T> {",
                changes: &[Signature],
            },
            Edit {
                label: "the declaration's docs",
                from: "/// A page.",
                to: "/// A page!",
                changes: &[Docs],
            },
        ],
    );
}

#[test]
fn edits_of_a_capability_change_exactly_their_facets() {
    use Facet::*;
    let base = "module m\n\n/// Store.\npub capability C {\n    /// Reads.\n    fn get(id: Int) -> Int\n        requires id > 0\n        ensures result > 0\n    fn put(id: Int) -> Int\n}\n";
    check_edits(
        base,
        "C",
        &[
            Edit { label: "an operation's clause", from: "requires id > 0", to: "requires id >= 0", changes: &[Contract] },
            Edit {
                label: "an operation's clauses in another order (design line 634)",
                from: "        requires id > 0\n        ensures result > 0\n",
                to: "        ensures result > 0\n        requires id > 0\n",
                changes: &[],
            },
            Edit { label: "a clause on another operation", from: "    fn put(id: Int) -> Int\n", to: "    fn put(id: Int) -> Int\n        requires id > 1\n", changes: &[Contract] },
            Edit { label: "an operation's docs (T25)", from: "/// Reads.", to: "/// Gets.", changes: &[Docs] },
            Edit { label: "an operation's parameter name", from: "fn get(id: Int)", to: "fn get(key: Int)", changes: &[Signature] },
            Edit { label: "an operation's return type", from: "fn put(id: Int) -> Int", to: "fn put(id: Int) -> Bool", changes: &[Signature] },
            Edit {
                label: "the operation order, only one with clauses",
                from: "    /// Reads.\n    fn get(id: Int) -> Int\n        requires id > 0\n        ensures result > 0\n    fn put(id: Int) -> Int\n",
                to: "    fn put(id: Int) -> Int\n    /// Reads.\n    fn get(id: Int) -> Int\n        requires id > 0\n        ensures result > 0\n",
                changes: &[Signature],
            },
            Edit { label: "comments among the operations", from: "    fn put(id: Int) -> Int\n", to: "    // a note\n\n    fn put(id: Int) -> Int // trailing\n", changes: &[] },
            Edit { label: "the declaration's docs", from: "/// Store.", to: "/// Stores.", changes: &[Docs] },
        ],
    );
}

#[test]
fn edits_of_types_enums_properties_and_tests_change_exactly_their_facets() {
    use Facet::*;
    let refinement = "module m\n\n/// Small.\npub type Small = Int where self < 10\n";
    check_edits(
        refinement,
        "Small",
        &[
            Edit {
                label: "the predicate",
                from: "self < 10",
                to: "self < 11",
                changes: &[Contract],
            },
            Edit {
                label: "parentheses around the predicate",
                from: "where self < 10",
                to: "where (self < 10)",
                changes: &[Contract],
            },
            Edit {
                label: "the base",
                from: "= Int where",
                to: "= Nat where",
                changes: &[Signature],
            },
            Edit {
                label: "spacing",
                from: "where self < 10",
                to: "where   self   <   10",
                changes: &[],
            },
            Edit {
                label: "docs",
                from: "/// Small.",
                to: "/// Tiny.",
                changes: &[Docs],
            },
        ],
    );
    let wrapper = "module m\n\npub type Id(Text)\n";
    check_edits(
        wrapper,
        "Id",
        &[
            Edit {
                label: "the payload",
                from: "(Text)",
                to: "(Int)",
                changes: &[Signature],
            },
            Edit {
                label: "drop pub",
                from: "pub type",
                to: "type",
                changes: &[Signature],
            },
            Edit {
                label: "spacing and a comment",
                from: "(Text)\n",
                to: "( Text ) // why\n",
                changes: &[],
            },
        ],
    );
    let enumeration = "module m\n\npub enum E<T> {\n    /// First.\n    A(T, Int)\n    B\n}\n";
    check_edits(
        enumeration,
        "E",
        &[
            Edit {
                label: "a variant's docs (T25)",
                from: "/// First.",
                to: "/// Second.",
                changes: &[Docs],
            },
            Edit {
                label: "a payload type",
                from: "A(T, Int)",
                to: "A(T, Text)",
                changes: &[Signature],
            },
            Edit {
                label: "the variant order",
                from: "    /// First.\n    A(T, Int)\n    B\n",
                to: "    B\n    /// First.\n    A(T, Int)\n",
                changes: &[Signature],
            },
            Edit {
                label: "a broken payload with a trailing comma",
                from: "A(T, Int)",
                to: "A(\n        T,\n        Int,\n    )",
                changes: &[],
            },
        ],
    );
    let property = "module m\n\n/// Holds.\nproperty \"p\"\n    for a in gen.int\n    for b in gen.bool\n{\n    assert a == a\n}\n";
    check_edits(
        property,
        "p",
        &[
            Edit {
                label: "a generator",
                from: "gen.bool",
                to: "gen.text",
                changes: &[Signature],
            },
            Edit {
                label: "the order of the for clauses",
                from: "    for a in gen.int\n    for b in gen.bool\n",
                to: "    for b in gen.bool\n    for a in gen.int\n",
                changes: &[Signature],
            },
            Edit {
                label: "the body",
                from: "assert a == a",
                to: "assert a == a + 0",
                changes: &[Body],
            },
            Edit {
                label: "the docs",
                from: "/// Holds.",
                to: "/// Always holds.",
                changes: &[Docs],
            },
            Edit {
                label: "the name spelled with an escape",
                from: "property \"p\"",
                to: "property \"\\u{70}\"",
                changes: &[],
            },
        ],
    );
    let test = "module m\n\ntest \"t\" {\n    assert true\n}\n";
    check_edits(
        test,
        "t",
        &[
            Edit {
                label: "the body",
                from: "assert true",
                to: "assert false",
                changes: &[Body],
            },
            Edit {
                label: "the name spelled with an escape",
                from: "test \"t\"",
                to: "test \"\\u{74}\"",
                changes: &[],
            },
            Edit {
                label: "docs",
                from: "test \"t\"",
                to: "/// Documented.\ntest \"t\"",
                changes: &[Docs],
            },
            Edit {
                label: "a comment in the body",
                from: "    assert true\n",
                to: "    // why\n    assert true // because\n",
                changes: &[],
            },
        ],
    );
}

#[test]
fn renaming_an_unannotated_declaration_changes_its_identity_and_name() {
    // 4.3: provisional identities are unstable under rename; T24 keeps the
    // name out of the signature, so nothing else changes.
    use Facet::*;
    let cases = [
        (
            "module m\n\nfn f(a: Int) -> Int {\n    a\n}\n",
            "module m\n\nfn g(a: Int) -> Int {\n    a\n}\n",
            "f",
            "g",
        ),
        (
            "module m\n\npub struct S {\n    a: Int\n}\n",
            "module m\n\npub struct T {\n    a: Int\n}\n",
            "S",
            "T",
        ),
        (
            "module m\n\ntest \"t\" {\n    assert true\n}\n",
            "module m\n\ntest \"u\" {\n    assert true\n}\n",
            "t",
            "u",
        ),
    ];
    for (src, renamed, old, new) in cases {
        assert_eq!(
            differing(src, old, renamed, new),
            BTreeSet::from([Identity, Name]),
            "{old} -> {new}"
        );
    }
}

#[test]
fn renaming_an_annotated_test_or_struct_changes_only_its_name() {
    // 4.6 and T24: a rename keeping `@id` preserves the identity.
    let test =
        "module m\n\n@id(\"test_aaaaaaaaaaaaaaaa\")\ntest \"before\" {\n    assert true\n}\n";
    let renamed = test.replace("\"before\"", "\"after\"");
    assert_eq!(
        differing(test, "before", &renamed, "after"),
        BTreeSet::from([Facet::Name])
    );
    let s = "module m\n\n@id(\"ty_aaaaaaaaaaaaaaaa\")\npub struct Before {\n    a: Int\n}\n";
    let renamed = s.replace("Before", "After");
    assert_eq!(
        differing(s, "Before", &renamed, "After"),
        BTreeSet::from([Facet::Name])
    );
}

#[test]
fn moving_a_declaration_changes_only_a_provisional_identity() {
    // 4.6: a move keeping `@id` preserves identity, and no facet text but a
    // provisional identity reads the module path (5.2), so an annotated
    // declaration keeps every hash and only its path changes.
    let annotated =
        "module a.b\n\n@id(\"fn_aaaaaaaaaaaaaaaa\")\npub fn f(x: Int) -> Int {\n    x\n}\n";
    let moved = annotated.replace("module a.b", "module c");
    let (before, after) = (version(annotated, "f"), version(&moved, "f"));
    assert_eq!(before.facets, after.facets);
    assert_eq!(before.definition, after.definition);
    assert_eq!(before.id, after.id);
    assert_eq!(
        (before.path.as_str(), after.path.as_str()),
        ("a.b.f", "c.f")
    );
    let plain = annotated.replace("@id(\"fn_aaaaaaaaaaaaaaaa\")\n", "");
    let moved = plain.replace("module a.b", "module c");
    assert_eq!(
        differing(&plain, "f", &moved, "f"),
        BTreeSet::from([Facet::Identity])
    );
}

// ===========================================================================
// 5.3 Content hash, 5.4 the .definition hash
// ===========================================================================

#[test]
fn facet_hashes_are_the_first_128_bits_of_blake3_of_the_utf8_text() {
    // 5.3: `lowercase_hex(blake3(utf8(text)))[0..32]`; T27: truncated to 128
    // bits, the full digest being 256.
    let long = "x".repeat(10_000);
    for t in [
        "a",
        "fn _() -> Int",
        "{\n    1\n}",
        "gr\u{f6}\u{df}e \u{2713} \u{1f980}",
        "line\r\nwith a CRLF",
        "\u{feff}a byte-order mark",
        "trailing LF\n",
        long.as_str(),
    ] {
        let full = blake3_hex(t);
        let hash = facet_hash(t);
        assert_eq!(full.len(), 64);
        assert_eq!(hash.as_str(), &full[..32], "{t:?}");
        assert!(is_lower_hex(hash.as_str(), 32));
    }
    // The whole text is hashed, and a final LF is part of it (canonical
    // texts have none, 5.2).
    assert_ne!(facet_hash(&long), facet_hash(&format!("{}y", &long[1..])));
    assert_ne!(facet_hash("a"), facet_hash("a\n"));
}

#[test]
fn the_definition_hash_is_recomputed_by_hand_for_a_fn_a_test_and_a_struct() {
    // 5.4 step 2: one line `<facet>=<hash>` per applicable facet in
    // `Facet::ALL` order, joined with LF, then `facet_hash` of that text.
    use Facet::*;
    let src = "module m\n\nfn f() -> Int {\n    1\n}\n\ntest \"t\" {\n    assert true\n}\n\npub struct S {\n    a: Int\n}\n";
    let expected = [
        (
            "f",
            definition_oracle(&[
                (Identity, "m.f"),
                (Name, "f"),
                (Signature, "fn _() -> Int"),
                (Contract, ""),
                (Effects, ""),
                (Body, "{\n    1\n}"),
                (Docs, ""),
                (VerificationPolicy, ""),
            ]),
        ),
        (
            "t",
            definition_oracle(&[
                (Identity, "m.\"t\""),
                (Name, "\"t\""),
                (Body, "{\n    assert true\n}"),
                (Docs, ""),
            ]),
        ),
        (
            "S",
            definition_oracle(&[
                (Identity, "m.S"),
                (Name, "S"),
                (Signature, "pub struct _ {\n    a: Int\n}"),
                (Docs, ""),
            ]),
        ),
    ];
    for (name, definition) in expected {
        assert_eq!(version(src, name).definition.as_str(), definition, "{name}");
    }
    // The empty facets are present with the 5.3 vector.
    let f = version(src, "f");
    for facet in [Contract, Effects, Docs, VerificationPolicy] {
        assert_eq!(f.facets[&facet].as_str(), EMPTY_HASH, "{facet}");
    }
}

#[test]
fn the_definition_hash_of_no_facets_is_the_hash_of_the_empty_text() {
    // 5.4 step 2 with zero lines: the joined text is empty.
    assert_eq!(definition_hash(&BTreeMap::new()).as_str(), EMPTY_HASH);
    let one = BTreeMap::from([(Facet::Docs, facet_hash(""))]);
    assert_eq!(
        definition_hash(&one).as_str(),
        &blake3_hex(&format!("docs={EMPTY_HASH}"))[..32]
    );
}

// ===========================================================================
// 4.5, 5.4 Facet maps and duplicate ids
// ===========================================================================

#[test]
fn a_test_and_a_property_of_one_name_are_one_duplicated_identity() {
    // semantics S3: tests and properties share the test namespace, so a test
    // and a property named alike have one qualified name, `m."same"` (4.4: a
    // path cannot tell them apart); 5.4 step 3 keeps the first and records
    // the duplicate (4.5).
    let src = "module m\n\ntest \"same\" {\n    assert true\n}\n\nproperty \"same\"\n    for x in gen.int\n{\n    assert x == x\n}\n\nfn other() -> Int {\n    1\n}\n";
    let project = load(&[("src/m.cpl", src)]);
    let map = facet_map(&project, revision(&project));
    let same = sid("m.\"same\"");
    let entry = map.get(&same).expect("the first occurrence");
    assert_eq!(entry.kind, IdKind::Test);
    assert!(
        !entry.version.facets.contains_key(&Facet::Signature),
        "a test's facets"
    );
    assert_eq!(map.entries.len(), 2);
    assert_eq!(
        duplicates(&map),
        BTreeMap::from([(
            same.as_str().to_string(),
            BTreeSet::from(["src/m.cpl".to_string()])
        )])
    );
}

#[test]
fn the_first_occurrence_in_path_order_is_kept_whatever_the_order_of_source_roots() {
    // 4.5: the facet map "keeps the first occurrence in path order"; path
    // order is lexicographic in the project-relative path (3.2, T15), so
    // `aa/` comes before `zz/` even though `[build] sources` lists `zz`
    // first. The second occurrence is reachable only as a duplicate.
    let manifest =
        "[project]\nname = \"t\"\nedition = \"0.1\"\n\n[build]\nsources = [\"zz\", \"aa\"]\n";
    let project = load_with(
        manifest,
        &[
            (
                "zz/b.cpl",
                "module b\n\n@id(\"dup_one\")\npub struct S {\n    a: Int\n}\n",
            ),
            (
                "aa/a.cpl",
                "module a\n\nfn x() -> Int {\n    1\n}\n\n@id(\"dup_one\")\nfn f() -> Int {\n    2\n}\n",
            ),
        ],
    );
    let map = facet_map(&project, revision(&project));
    let entry = map.get(&sid("dup_one")).expect("kept");
    assert_eq!(
        (
            entry.kind,
            entry.name.as_str(),
            entry.module.as_str(),
            entry.file.as_str(),
            entry.version.path.as_str()
        ),
        (IdKind::Fn, "f", "a", "aa/a.cpl", "a.f")
    );
    let ids: BTreeSet<&str> = map.entries.keys().map(StableId::as_str).collect();
    assert_eq!(ids, BTreeSet::from(["a.x", "dup_one"]));
    assert_eq!(
        duplicates(&map),
        BTreeMap::from([(
            "dup_one".to_string(),
            BTreeSet::from(["aa/a.cpl".to_string(), "zz/b.cpl".to_string()])
        )])
    );
}

#[test]
fn within_one_file_the_first_declaration_of_a_duplicated_id_is_kept() {
    // 5.4 step 3, "keep the first": the entry is the first declaration's,
    // facets included.
    let src = "module m\n\n@id(\"ty_aaaaaaaaaaaaaaaa\")\npub type First(Int)\n\n@id(\"ty_aaaaaaaaaaaaaaaa\")\npub type Second(Text)\n";
    let project = load(&[("src/m.cpl", src)]);
    let map = facet_map(&project, revision(&project));
    let entry = map.get(&sid("ty_aaaaaaaaaaaaaaaa")).expect("kept");
    assert_eq!(entry.name, "First");
    assert_eq!(
        entry.facet(Facet::Signature),
        Some(&facet_hash("pub type _(Int)"))
    );
    assert_eq!(map.entries.len(), 1);
    assert_eq!(
        duplicates(&map).keys().collect::<Vec<_>>(),
        ["ty_aaaaaaaaaaaaaaaa"]
    );
}

#[test]
fn members_are_not_entries_of_the_facet_map() {
    // 5.4 step 1: one entry per `Declaration` of `Module::declarations`.
    // Capability operations and enum variants have qualified names
    // (`m.C.get`, semantics 1.4) but are members, covered by their
    // declaration's facets.
    let src = "module m\n\npub capability C {\n    fn get() -> Int\n    fn put(v: Int) -> Int\n}\n\npub enum E {\n    A\n    B(Int)\n}\n\npub struct S {\n    a: Int\n}\n";
    let project = load(&[("src/m.cpl", src)]);
    let map = facet_map(&project, revision(&project));
    let ids: BTreeSet<&str> = map.entries.keys().map(StableId::as_str).collect();
    assert_eq!(ids, BTreeSet::from(["m.C", "m.E", "m.S"]));
    assert!(map.duplicates.is_empty());
    // 5.1: an inapplicable facet is absent, an applicable empty one present.
    let s = map.get(&sid("m.S")).expect("S");
    assert_eq!(s.facet(Facet::Body), None);
    assert_eq!(s.facet(Facet::Contract), None);
    assert_eq!(s.facet(Facet::Docs).map(|h| h.as_str()), Some(EMPTY_HASH));
    let c = map.get(&sid("m.C")).expect("C");
    assert_eq!(
        c.facet(Facet::Contract).map(|h| h.as_str()),
        Some(EMPTY_HASH)
    );
    assert!(c.effects.is_empty());
}

#[test]
fn a_module_split_over_two_files_has_the_same_facets_as_one_file() {
    // 5.4: the map is keyed by stable id and every text is per declaration;
    // splitting a module over files (3.7) changes only `FacetEntry::file`.
    let one = load(&[(
        "src/m.cpl",
        "module m\n\nfn f() -> Int {\n    1\n}\n\npub type T(Int)\n",
    )]);
    let two = load(&[
        ("src/m.cpl", "module m\n\nfn f() -> Int {\n    1\n}\n"),
        ("src/m/more.cpl", "module m\n\npub type T(Int)\n"),
    ]);
    let (a, b) = (
        facet_map(&one, revision(&one)),
        facet_map(&two, revision(&two)),
    );
    assert_eq!(
        a.entries.keys().collect::<Vec<_>>(),
        b.entries.keys().collect::<Vec<_>>()
    );
    for (id, entry) in &a.entries {
        let other = &b.entries[id];
        assert_eq!(entry.version, other.version, "{id}");
        assert_eq!(
            (entry.kind, &entry.module, &entry.name),
            (other.kind, &other.module, &other.name)
        );
    }
    assert_eq!(b.get(&sid("m.T")).expect("T").file, "src/m/more.cpl");
}

// ===========================================================================
// Further corners
// ===========================================================================

#[test]
fn clauses_containing_if_or_match_keep_the_block_layout_of_rule_8() {
    // 5.2: canonical text "is produced by `cpl_syntax::formatter::
    // format_fragment` (Section 1.5) on the relevant AST parts", and a fn
    // contract is every clause "one per line, no indentation". Its "every
    // clause ... is one line" (and S49's `clause`, "the formatter's rendering
    // on one line") cannot hold for a clause that contains an `if` or `match`
    // (semantics 5.1, S32): the formatter lays out every non-empty block over
    // several lines (2.4 rule 8), and a contract block may hold `let`
    // statements before its tail (S32), which no single line of CPL can hold,
    // since two statements never share a line (grammar 2.3). The reading
    // taken (reported as a spec deviation): each clause begins an unindented
    // line and its blocks keep rule 8's layout at level 0, exactly the
    // canonical clause text of the expression (`Fragment::Expr`, the text of
    // semantics 9.2's `clause` and of the HIR's `Clause::text`), so the
    // source layout still never matters. (An earlier version of this test
    // expected one line per clause.)
    let src = "module m\n\nfn f(a: Int) -> Int\n    requires if a > 0 { true } else { false }\n    ensures match a {\n        0 => true\n        _ => result == a\n    }\n{\n    a\n}\n\ntype T = Int where if self > 0 { true } else { self == 0 }\n";
    let contract = text(src, "f", Facet::Contract).expect("applies");
    assert_eq!(
        contract,
        "requires if a > 0 {\n    true\n} else {\n    false\n}\nensures match a {\n    0 => true\n    _ => result == a\n}"
    );
    let clause_starts: Vec<&str> = contract
        .lines()
        .filter(|line| !line.starts_with(' ') && !line.starts_with('}'))
        .collect();
    assert_eq!(clause_starts, ["requires if a > 0 {", "ensures match a {"]);
    let m = parse(src);
    let ast::DeclKind::Fn(f) = &decl(&m, "f").kind else {
        panic!("f is a fn");
    };
    let clauses: Vec<String> = f
        .clauses
        .iter()
        .filter_map(|clause| match clause {
            ast::FnClause::Contract(c) => Some(format!(
                "{} {}",
                c.kind.keyword(),
                cpl_syntax::format_fragment(
                    cpl_syntax::Fragment::Expr(&c.expr),
                    cpl_syntax::FragmentMode::Canonical
                )
            )),
            _ => None,
        })
        .collect();
    assert_eq!(contract, clauses.join("\n"));
    let predicate = text(src, "T", Facet::Contract).expect("applies");
    assert_eq!(
        predicate,
        "where if self > 0 {\n    true\n} else {\n    self == 0\n}"
    );
    // The source layout of the blocks and the spacing of the arms do not
    // matter.
    let relaid = "module m\n\nfn f(a: Int) -> Int\n    requires if a > 0 {\n        true\n    } else {\n        false\n    }\n    ensures match a {\n        0  =>  true\n        _ => result  ==  a\n    }\n{\n    a\n}\n\ntype T = Int where if self > 0 {\n    true\n} else {\n    self == 0\n}\n";
    assert_eq!(
        text(relaid, "f", Facet::Contract).as_deref(),
        Some(contract.as_str())
    );
    assert_eq!(
        text(relaid, "T", Facet::Contract).as_deref(),
        Some(predicate.as_str())
    );
    // A block of a `let` and its tail, which no one-line form could hold.
    let with_let = "module m\n\nfn g(a: Int) -> Int\n    ensures if a > 0 {\n        let b = a * 2\n        b > a\n    } else {\n        true\n    }\n{\n    a\n}\n";
    assert_eq!(
        text(with_let, "g", Facet::Contract).as_deref(),
        Some("ensures if a > 0 {\n    let b = a * 2\n    b > a\n} else {\n    true\n}")
    );
}

/// A body exercising the comma groups and nested blocks of 2.4 rules 6 and
/// 8; [`BODY_COMMENTED`] is the same body with a comment at every attachment
/// point of 2.3.
const BODY_CLEAN: &str = r#"module m

fn shapes(p: Page, xs: List<Int>) -> Result<Int, Text> {
    let a = f(1, 2)
    let page = Page { ..p, text: "x" }
    let ys = [1, 2]
    if a > 0 {
        return Ok(a)
    } else {
        a = 0
    }
    let label = match a {
        0 => 1
        _ => {
            2
        }
    }
    Ok(label)
}
"#;

const BODY_COMMENTED: &str = r#"module m

fn shapes(p: Page, xs: List<Int>) -> Result<Int, Text> { // on the header line
    // leading
    let a = f(
        1, // one
        // before two
        2,
        // dangling in the arguments
    )
    let page = Page {
        // before the spread
        ..p, // the spread
        text: "x", // the text
        // dangling in the literal
    }
    let ys = [
        1, // first
        2,
        // dangling in the list
    ]

    if a > 0 { // after the if header
        // in the then branch
        return Ok(a) // returning
    } else {
        a = 0
        // dangling in the else branch
    }
    let label = match a { // after the match header
        // before an arm
        0 => 1 // an arm
        _ => {
            2 // a tail
        }
        // dangling in the match
    }
    Ok(label) // the tail
    // dangling in the body
} // after the body
"#;

#[test]
fn comments_at_every_attachment_point_of_a_body_are_excluded() {
    // 5.2: "Comments (ordinary `//`) are excluded from every facet text";
    // 2.3 attaches them as leading, trailing, and dangling trivia of
    // statements, arguments, list elements, struct-literal entries (the
    // spread too), match arms, and blocks, and none may reach the body.
    let clean = text(BODY_CLEAN, "shapes", Facet::Body).expect("applies");
    assert!(!clean.contains("//"));
    for src in [
        BODY_COMMENTED.to_string(),
        BODY_COMMENTED.replace('\n', "\r\n"),
    ] {
        assert_eq!(text(&src, "shapes", Facet::Body), Some(clean.clone()));
        assert_eq!(
            version(&src, "shapes").definition,
            version(BODY_CLEAN, "shapes").definition
        );
    }
}

#[test]
fn an_invalid_utf8_file_is_skipped_by_the_plan_and_the_facet_map() {
    // grammar 1.1 and D70: invalid UTF-8 is the parse diagnostic
    // `invalid_utf8` and the file counts as failed; 4.2 step 1 skips a file
    // that failed, and 5.4 has no tree for it.
    let mut tree = tree_with(
        MANIFEST,
        &[("src/a.cpl", "module a\n\nfn f() -> Int {\n    1\n}\n")],
    );
    tree.files.insert(
        "src/b.cpl".to_string(),
        b"module b\n\nfn g() -> Text {\n    \"\xff\"\n}\n".to_vec(),
    );
    let project =
        load_project_from_tree(&tree, FileOrigin::Virtual, &LoadOptions::default()).expect("loads");
    assert!(project.diagnostics.iter().any(|d| d.code == "invalid_utf8"));
    let plan = plan_id_assignments(&project, &mut Draws::default());
    let paths: Vec<&str> = plan.iter().map(|a| a.path.as_str()).collect();
    assert_eq!(paths, ["a.f"]);
    let map = facet_map(&project, revision(&project));
    let ids: Vec<&str> = map.entries.keys().map(StableId::as_str).collect();
    assert_eq!(ids, ["a.f"]);
}

#[test]
fn indented_docs_above_a_column_zero_declaration() {
    // 4.2 step 3 when only the prefix lines are indented: the declaration's
    // first token is at column 0, so the inserted line is too, after the docs.
    let src = "module m\n\n    /// Indented docs.\nfn f() -> Int {\n    1\n}\n\n\t/// Tabbed docs.\n@prove\nfn g() -> Int {\n    1\n}\n";
    let (plan, out, _) = assign(src);
    assert!(plan.iter().all(|a| a.indent.is_empty()));
    assert_eq!(
        out,
        splice(
            src,
            &[
                (at(src, "fn f"), draw("fn", 0)),
                (at(src, "@prove"), draw("fn", 1)),
            ]
        )
    );
    assert_nothing_left_to_assign(&out);
}

#[test]
fn std_member_annotations_in_a_project_file_name_nothing() {
    // 3.6: `@builtin`, `@method`, and `@property` are `unknown_annotation` in
    // project files; 4.3's `UPPER_IDENT "." identifier` form is for std member
    // stubs only, so these keep plain qualified names. An annotation named by
    // a keyword (`@property`, D71) is still the first annotation, and the id
    // goes before it.
    let src = "module m\n\n@property\nfn len(xs: List<Int>) -> Int {\n    1\n}\n\n@method\n@builtin(\"list_take\")\nfn take(xs: List<Int>, n: Int) -> List<Int> {\n    xs\n}\n";
    let m = parse(src);
    let mp = module_path(&m);
    let ids: Vec<String> = m
        .declarations
        .iter()
        .map(|d| declaration_identity(&mp, d).as_str().to_string())
        .collect();
    assert_eq!(ids, ["m.len", "m.take"]);
    let (_, out, _) = assign(src);
    assert_eq!(
        out,
        splice(
            src,
            &[
                (at(src, "@property"), draw("fn", 0)),
                (at(src, "@method"), draw("fn", 1)),
            ]
        )
    );
}

#[test]
fn an_id_argument_is_read_as_its_decoded_text() {
    // grammar 1.5: a TEXT literal carries its decoded value, so an `@id`
    // spelled with an escape is the same identity; an argument group broken
    // over lines with a comment (2.4 rule 6) still holds one TEXT argument.
    // Either way the declaration has an id and `cpl id assign` leaves it.
    let escaped = "module m\n\n@id(\"fn_\\u{61}aaaaaaaaaaaaaaa\")\nfn f() -> Int {\n    1\n}\n";
    let broken =
        "module m\n\n@id(\n    \"fn_aaaaaaaaaaaaaaaa\", // the id\n)\nfn f() -> Int {\n    1\n}\n";
    for src in [escaped, broken] {
        assert_eq!(
            text(src, "f", Facet::Identity).as_deref(),
            Some("fn_aaaaaaaaaaaaaaaa"),
            "{src:?}"
        );
        let (plan, out, _) = assign(src);
        assert!(plan.is_empty());
        assert_eq!(out, src);
    }
}

#[test]
fn fn_contracts_with_a_single_kind_of_clause() {
    // 5.2: requires lines, then ensures lines, then `decreases`, each only
    // when written; the `effects` clause is never part of the contract.
    let src = "module m\n\nfn r(n: Int) -> Int\n    requires n > 0\n{\n    n\n}\n\nfn e(n: Int) -> Int\n    ensures result == n\n{\n    n\n}\n\nfn d(n: Int) -> Int\n    decreases n\n{\n    n\n}\n\nfn x(using db: Db) -> Int\n    effects { db.get }\n{\n    1\n}\n";
    for (name, contract) in [
        ("r", "requires n > 0"),
        ("e", "ensures result == n"),
        ("d", "decreases n"),
        ("x", ""),
    ] {
        assert_eq!(
            text(src, name, Facet::Contract).as_deref(),
            Some(contract),
            "{name}"
        );
    }
}
