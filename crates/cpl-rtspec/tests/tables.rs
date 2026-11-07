//! The tables of cpl-rtspec against the specifications and the files they must agree with (`spec/toolchain.md` 12,
//! the "shared tables" row; T70; `spec/stdlib.md` S22, S47, 10.4-10.6; `spec/cabi.md` 2.7, 4.9).
//!
//! cpl-rtspec depends on no crate of the workspace, not even as a dev-dependency (toolchain 1.2), so these tests read
//! the sources they compare with as text: the specifications, the standard-library stubs of `std/`, the frozen C
//! headers of `crates/cpl-rt/c/`, and, while the evaluator and the checker still keep copies of a table, those copies
//! (`crates/cpl-eval/src/builtins/mod.rs`, `crates/cpl-sema/src/codes.rs`). Once a copy is replaced by this crate's
//! table, which is the point of T70, its comparison is moot and the test says so instead of failing.

use cpl_rtspec::builtins::{self, BuiltinSpec, BUILTINS, IDS};
use cpl_rtspec::messages::{self, Template, ALL};
use cpl_rtspec::{arith, foreign, interop, limits, whitespace};
use num_bigint::BigInt;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

// ---- reading the workspace ---------------------------------------------------------------------------------------

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn read(relative: &str) -> String {
    let path = workspace().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// The lines of the section that starts with the heading line `heading`, up to the next heading of any level.
fn section<'a>(doc: &'a str, heading: &str) -> Vec<&'a str> {
    let mut lines = doc.lines().skip_while(|l| l.trim_end() != heading);
    assert!(lines.next().is_some(), "heading `{heading}` not found");
    lines.take_while(|l| !l.starts_with('#')).collect()
}

/// The data rows of the first Markdown table in `lines` whose header row starts with `header`, as trimmed cells.
fn table_rows(lines: &[&str], header: &str) -> Vec<Vec<String>> {
    let start = lines
        .iter()
        .position(|l| l.starts_with(header))
        .unwrap_or_else(|| panic!("table `{header}` not found"));
    lines[start + 2..]
        .iter()
        .take_while(|l| l.starts_with('|'))
        .map(|l| {
            l.trim()
                .trim_start_matches('|')
                .trim_end_matches('|')
                .split(" | ")
                .map(|c| c.trim().to_string())
                .collect()
        })
        .collect()
}

/// Every `` `token` `` of `cell`, in order.
fn backticked(cell: &str) -> Vec<String> {
    cell.split('`')
        .enumerate()
        .filter(|(i, _)| i % 2 == 1)
        .map(|(_, s)| s.to_string())
        .collect()
}

/// The `#define NAME VALUE` lines of a C header, the value's first token with an unsigned suffix removed.
fn c_defines(header: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in header.lines() {
        let mut words = line.split_whitespace();
        if words.next() != Some("#define") {
            continue;
        }
        if let (Some(name), Some(value)) = (words.next(), words.next()) {
            out.insert(name.to_string(), value.trim_end_matches('u').to_string());
        }
    }
    out
}

// ---- the standard-library stubs ----------------------------------------------------------------------------------

/// One `@builtin` declaration of `std/*.cpl`, read from its text.
#[derive(Debug)]
struct Stub {
    id: String,
    qualified_name: String,
    params: Vec<String>,
    member: bool,
    requires: Vec<String>,
}

/// The parameter names of a parameter list `a: T, b: Map<K, V>` (commas inside `<...>` belong to the types).
fn param_names(list: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut depth = 0usize;
    let mut current = String::new();
    for c in list.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                names.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    if !current.trim().is_empty() {
        names.push(current);
    }
    names
        .iter()
        .map(|p| p.split(':').next().unwrap().trim().to_string())
        .collect()
}

/// The receiver type's name of a member stub's first parameter, `List` for `list: List<T>`.
fn receiver(list: &str) -> String {
    let first = list
        .split(':')
        .nth(1)
        .expect("a member stub has a receiver");
    first
        .trim()
        .split(['<', ',', ')'])
        .next()
        .unwrap()
        .trim()
        .to_string()
}

fn stubs() -> Vec<Stub> {
    let mut out = Vec::new();
    let mut files: Vec<_> = std::fs::read_dir(workspace().join("std"))
        .expect("std/ is readable")
        .map(|e| e.expect("a std entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "cpl"))
        .collect();
    files.sort();
    for file in files {
        let text = std::fs::read_to_string(&file).expect("a std stub file is readable");
        let module = text
            .lines()
            .find_map(|l| l.strip_prefix("module "))
            .expect("a stub file has a module header")
            .trim()
            .to_string();
        let lines: Vec<&str> = text.lines().collect();
        let mut member = false;
        let mut builtin: Option<String> = None;
        let mut i = 0;
        while i < lines.len() {
            let line = lines[i].trim_end();
            if line == "@method" || line == "@property" {
                member = true;
            } else if let Some(rest) = line.strip_prefix("@builtin(\"") {
                builtin = Some(rest.trim_end_matches("\")").to_string());
            } else if let Some(id) = builtin.take().filter(|_| line.starts_with("pub ")) {
                let qualified_name;
                let mut params = Vec::new();
                if let Some(rest) = line.strip_prefix("pub type ") {
                    let name = rest.split(['<', ' ']).next().unwrap();
                    qualified_name = format!("{module}.{name}");
                } else {
                    let rest = line
                        .strip_prefix("pub fn ")
                        .expect("a builtin stub is a fn or a type");
                    let name = rest.split(['<', '(']).next().unwrap();
                    let open = rest.find('(').expect("a parameter list");
                    let close = rest.find(')').expect("a parameter list");
                    let list = &rest[open + 1..close];
                    params = param_names(list);
                    // A `@property` without parameters is a module property (`gen.int`), no member: the
                    // annotation rules of cpl-sema (stdlib 10.2).
                    member = member && !params.is_empty();
                    qualified_name = if member {
                        format!("{module}.{}.{name}", receiver(list))
                    } else {
                        format!("{module}.{name}")
                    };
                }
                let mut requires = Vec::new();
                while let Some(next) = lines.get(i + 1) {
                    let next = next.trim();
                    if let Some(clause) = next.strip_prefix("requires ") {
                        requires.push(clause.to_string());
                    } else if !next.starts_with("ensures ") {
                        break;
                    }
                    i += 1;
                }
                out.push(Stub {
                    id,
                    qualified_name,
                    params,
                    member,
                    requires,
                });
                member = false;
            } else if !line.starts_with("///") && !line.starts_with('@') {
                member = false;
            }
            i += 1;
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

fn stub<'a>(stubs: &'a [Stub], id: &str) -> &'a Stub {
    stubs
        .iter()
        .find(|s| s.id == id)
        .unwrap_or_else(|| panic!("no stub declares @builtin(\"{id}\")"))
}

// ---- builtins ----------------------------------------------------------------------------------------------------

#[test]
fn builtins_hold_exactly_the_ids_of_stdlib_10_6() {
    let doc = read("spec/stdlib.md");
    let lines = section(&doc, "### 10.6 Builtin registry");
    let rows = table_rows(&lines, "| Group | Ids |");
    let mut ids = BTreeSet::new();
    let mut not_callable = BTreeSet::new();
    for row in &rows {
        for id in backticked(&row[1]) {
            if row[0] == "types" || row[0] == "`std.gen`" {
                not_callable.insert(id.clone());
            }
            assert!(ids.insert(id.clone()), "stdlib 10.6 lists `{id}` twice");
        }
    }
    assert_eq!(ids.len(), 60, "stdlib 10.6 lists 60 ids");
    assert_eq!(not_callable.len(), 14, "7 type ids and 7 generator ids");
    let ours: Vec<&str> = BUILTINS.iter().map(|b| b.id).collect();
    assert_eq!(ours, ids.iter().map(String::as_str).collect::<Vec<_>>());
    for b in &BUILTINS {
        assert_eq!(
            b.callable,
            !not_callable.contains(b.id),
            "callability of `{}`",
            b.id
        );
    }
    assert_eq!(BUILTINS.iter().filter(|b| b.callable).count(), 46);
}

#[test]
fn ids_are_the_builtin_ids_in_sorted_order() {
    let ours: Vec<&str> = BUILTINS.iter().map(|b| b.id).collect();
    assert_eq!(IDS.to_vec(), ours);
    assert!(
        IDS.windows(2).all(|w| w[0] < w[1]),
        "sorted by id, byte order, no duplicates"
    );
    for (i, b) in BUILTINS.iter().enumerate() {
        let found: &BuiltinSpec = builtins::spec(b.id).expect("every id is found");
        assert_eq!(found, &BUILTINS[i]);
        assert!(std::ptr::eq(found, builtins::spec(b.id).unwrap()));
    }
    assert!(builtins::spec("").is_none());
    assert!(builtins::spec("list_takes").is_none());
    assert!(builtins::spec("zzz").is_none());
}

#[test]
fn every_id_has_one_stub_and_its_qualified_name_is_the_fallback_symbol() {
    let stubs = stubs();
    let ids: Vec<&str> = stubs.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, IDS.to_vec(), "the @builtin ids of std/*.cpl");
    for b in &BUILTINS {
        let s = stub(&stubs, b.id);
        assert_eq!(
            b.fallback_symbol, s.qualified_name,
            "fallback symbol of `{}`",
            b.id
        );
    }
    assert_eq!(
        builtins::spec("list_take").unwrap().fallback_symbol,
        "std.prelude.List.take"
    );
    assert_eq!(
        builtins::spec("bool").unwrap().fallback_symbol,
        "std.prelude.Bool"
    );
}

#[test]
fn requires_texts_and_inputs_are_the_stubs() {
    let stubs = stubs();
    for b in &BUILTINS {
        let s = stub(&stubs, b.id);
        let clauses: Vec<&str> = b.requires.iter().map(|r| r.clause).collect();
        assert_eq!(clauses, s.requires, "requires clauses of `{}`", b.id);
        let mut names = s.params.clone();
        if s.member {
            names[0] = "self".to_string();
        }
        for r in b.requires {
            assert_eq!(r.inputs, names, "input names of `{}`", b.id);
        }
    }
    let with_requires: Vec<&str> = BUILTINS
        .iter()
        .filter(|b| !b.requires.is_empty())
        .map(|b| b.id)
        .collect();
    assert_eq!(
        with_requires,
        [
            "bytes_from_list",
            "gen_range",
            "list_drop",
            "list_take",
            "text_split"
        ]
    );
}

#[test]
fn requires_texts_and_fallback_symbols_are_the_evaluators() {
    let source = read("crates/cpl-eval/src/builtins/mod.rs");
    let Some(start) = source.find("const REQUIRES:") else {
        println!(
            "note: crates/cpl-eval/src/builtins/mod.rs keeps no REQUIRES table of its own; the evaluator reads \
             cpl-rtspec's (T70), so there is no second copy to compare"
        );
        return;
    };
    let block = &source[start..start + source[start..].find("];").expect("the table ends")];
    let mut entries = 0;
    for entry in block.split("Requires {").skip(1) {
        let field = |name: &str| -> String {
            let at = entry
                .find(&format!("{name}: "))
                .unwrap_or_else(|| panic!("field {name}"));
            let rest = &entry[at + name.len() + 2..];
            rest[..rest.find('\n').unwrap()]
                .trim_end_matches(',')
                .to_string()
        };
        let id = field("id").trim_matches('"').to_string();
        let symbol = field("symbol").trim_matches('"').to_string();
        let strings = |list: String| -> Vec<String> {
            list.trim_start_matches("&[")
                .trim_end_matches(']')
                .split(", ")
                .map(|s| s.trim_matches('"').to_string())
                .collect()
        };
        let inputs = strings(field("inputs"));
        let clauses = strings(field("clauses"));
        let b = builtins::spec(&id)
            .unwrap_or_else(|| panic!("the evaluator's `{id}` is a registry id"));
        assert_eq!(b.fallback_symbol, symbol, "fallback symbol of `{id}`");
        let ours: Vec<&str> = b.requires.iter().map(|r| r.clause).collect();
        assert_eq!(ours, clauses, "requires clauses of `{id}`");
        for r in b.requires {
            assert_eq!(r.inputs, inputs, "input names of `{id}`");
        }
        entries += 1;
    }
    assert_eq!(
        entries, 5,
        "the evaluator's five builtins with a requires clause"
    );
}

#[test]
fn ids_are_the_evaluators_registry_and_the_checkers_builtin_ids() {
    let source = read("crates/cpl-eval/src/builtins/mod.rs");
    let start = source
        .find("static REGISTRY")
        .expect("the evaluator's registry: crates/cpl-eval/src/builtins/mod.rs");
    let block = &source[start..start + source[start..].find("];").expect("the registry ends")];
    let registry: Vec<&str> = block
        .lines()
        .filter_map(|l| l.trim().strip_prefix("(\""))
        .map(|l| &l[..l.find('"').unwrap()])
        .collect();
    assert_eq!(registry, IDS.to_vec(), "the evaluator's REGISTRY");

    let codes = read("crates/cpl-sema/src/codes.rs");
    let start = codes
        .find("pub const BUILTIN_IDS")
        .expect("cpl_sema::codes::BUILTIN_IDS");
    let line_end = start + codes[start..].find('\n').unwrap();
    if codes[start..line_end].contains("cpl_rtspec") {
        println!("note: cpl_sema::codes::BUILTIN_IDS is cpl_rtspec::builtins::IDS (T70)");
        return;
    }
    let block = &codes[line_end..line_end + codes[line_end..].find("];").unwrap()];
    let ids: Vec<&str> = block
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with('"'))
        .map(|l| l.trim_end_matches(',').trim_matches('"'))
        .collect();
    assert_eq!(ids, IDS.to_vec(), "cpl_sema::codes::BUILTIN_IDS");
}

#[test]
fn callable_builtins_are_the_c_entry_points_of_cpl_rt_h() {
    let header = read("crates/cpl-rt/c/cpl_rt.h");
    let mut declared = BTreeMap::new();
    for (macro_name, arity) in [("CPLRT_BI1(", 1usize), ("CPLRT_BI2(", 2)] {
        for piece in header.split(macro_name).skip(1) {
            let id = &piece[..piece.find(')').unwrap()];
            if id != "id" {
                assert!(
                    declared.insert(id.to_string(), arity).is_none(),
                    "`{id}` declared twice"
                );
            }
        }
    }
    let stubs = stubs();
    for b in &BUILTINS {
        if b.callable {
            assert_eq!(
                b.c_entry,
                format!("cplrt_bi({})", b.id),
                "C entry of `{}`",
                b.id
            );
            let arity = declared
                .remove(b.id)
                .unwrap_or_else(|| panic!("cpl_rt.h declares no cplrt_bi_{}", b.id));
            assert_eq!(
                arity,
                stub(&stubs, b.id).params.len(),
                "arity of cplrt_bi_{}",
                b.id
            );
        } else {
            assert_eq!(
                b.c_entry, "",
                "`{}` is not callable and has no C entry point",
                b.id
            );
        }
    }
    assert!(
        declared.is_empty(),
        "cpl_rt.h declares builtins the registry lacks: {declared:?}"
    );
}

// ---- White_Space ----------------------------------------------------------------------------------------------------

/// The scalars that stdlib S22 enumerates, read from its decision text.
fn s22_scalars() -> BTreeSet<u32> {
    let doc = read("spec/stdlib.md");
    let start = doc.find("S22. *Whitespace*").expect("the S22 decision");
    let text = &doc[start..];
    let list = &text
        [text.find("property:").unwrap() + "property:".len()..text.find("Nothing else").unwrap()];
    let mut out = BTreeSet::new();
    for item in list.split(',') {
        let item = item.trim().trim_end_matches('.');
        let bounds: Vec<u32> = item
            .split('-')
            .map(|u| {
                u32::from_str_radix(u.trim().trim_start_matches("U+"), 16).expect("a U+ scalar")
            })
            .collect();
        let (lo, hi) = (bounds[0], *bounds.last().unwrap());
        out.extend(lo..=hi);
    }
    out
}

#[test]
fn ranges_are_the_25_white_space_scalars_of_s22() {
    let expected = s22_scalars();
    assert_eq!(expected.len(), 25, "stdlib S22 enumerates 25 scalars");
    let mut ours = BTreeSet::new();
    let mut previous_hi: Option<u32> = None;
    for &(lo, hi) in &whitespace::RANGES {
        assert!(lo <= hi, "inclusive range {lo:#x}..={hi:#x}");
        if let Some(p) = previous_hi {
            assert!(lo > p + 1, "ascending, disjoint, and not adjacent: {lo:#x}");
        }
        previous_hi = Some(hi);
        ours.extend(lo..=hi);
    }
    assert_eq!(ours, expected);
    for c in (0..=0x10FFFFu32).filter_map(char::from_u32) {
        assert_eq!(
            whitespace::is_white_space(c),
            expected.contains(&u32::from(c)),
            "is_white_space(U+{:04X})",
            u32::from(c)
        );
    }
    assert!(!whitespace::is_white_space('\u{200B}'));
    assert!(!whitespace::is_white_space('\u{FEFF}'));
    assert!(whitespace::is_white_space('\u{A0}'));
}

// ---- messages -----------------------------------------------------------------------------------------------------

/// The placeholder names of a template, in order, by the rule of `cpl_rtspec::messages`.
fn placeholders(t: &Template) -> Vec<String> {
    let bytes = t.text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            let rest = &bytes[i + 1..];
            let len = rest
                .iter()
                .take_while(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || **b == b'_')
                .count();
            if len > 0 && rest[0].is_ascii_lowercase() && rest.get(len) == Some(&b'}') {
                out.push(t.text[i + 1..i + 1 + len].to_string());
                i += len + 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

#[test]
fn every_template_renders_with_exactly_its_placeholders() {
    let mut names = BTreeSet::new();
    let mut texts = BTreeSet::new();
    for t in ALL {
        assert!(names.insert(t.name), "two templates named {}", t.name);
        assert!(
            texts.insert(t.text),
            "two templates with the text {:?}",
            t.text
        );
        assert!(
            t.name
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
                && t.name.as_bytes()[0].is_ascii_uppercase(),
            "the C macro CPLRT_MSG_{} must be an identifier",
            t.name
        );
        // The generated header writes the text as a C string literal unchanged (spec/cabi.md B.2).
        assert!(
            t.text
                .bytes()
                .all(|b| (0x20..0x7f).contains(&b) && b != b'"' && b != b'\\'),
            "{}: a C string literal holds the text unchanged",
            t.name
        );
        let names = placeholders(t);
        // Every brace of a template belongs to a placeholder.
        let braces = t.text.bytes().filter(|&b| b == b'{' || b == b'}').count();
        assert_eq!(braces, 2 * names.len(), "{}: a stray brace", t.name);
        let args: Vec<(String, String)> = names
            .iter()
            .map(|n| (n.clone(), format!("<{n}>")))
            .collect();
        let borrowed: Vec<(&str, &str)> =
            args.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
        let rendered = messages::render(t, &borrowed);
        let mut expected = t.text.to_string();
        for n in &names {
            expected = expected.replacen(&format!("{{{n}}}"), &format!("<{n}>"), 1);
        }
        assert_eq!(rendered, expected, "{}", t.name);
        assert!(
            !rendered.contains('{') && !rendered.contains('}'),
            "{}",
            t.name
        );
        for missing in &names {
            let fewer: Vec<(&str, &str)> = borrowed
                .iter()
                .copied()
                .filter(|(n, _)| n != missing)
                .collect();
            let outcome = std::panic::catch_unwind(|| messages::render(t, &fewer));
            assert!(
                outcome.is_err(),
                "{}: rendering without {{{missing}}} must panic",
                t.name
            );
        }
    }
}

#[test]
fn templates_render_the_evaluators_messages() {
    use messages::*;
    assert_eq!(
        render(
            &CONTRACT_VIOLATION,
            &[("clause_kind", "requires"), ("clause", "b != 0")]
        ),
        "requires clause failed: b != 0"
    );
    assert_eq!(
        render(
            &CONTRACT_VIOLATION,
            &[
                ("clause_kind", "ensures"),
                ("clause", "flatten(result) == items")
            ]
        ),
        "ensures clause failed: flatten(result) == items"
    );
    assert_eq!(
        render(&CHECK_FAILED, &[("value", "0"), ("predicate", "self > 0")]),
        "0 does not satisfy self > 0"
    );
    assert_eq!(
        render(
            &INTERNAL_ERROR,
            &[
                ("kind", "no_matching_arm"),
                ("detail", "no arm of the match matches 3")
            ]
        ),
        "internal error (no_matching_arm): no arm of the match matches 3"
    );
    assert_eq!(
        render(&BYTES_TO_TEXT_FAILED, &[("offset", "18")]),
        "invalid UTF-8 at byte 18"
    );
    assert_eq!(
        render(&PARSE_INT_FAILED, &[("quoted", "\"x1\"")]),
        "\"x1\" is not a decimal integer"
    );
    assert_eq!(
        render(&RESOURCE_STACK, &[("limit", "10000")]),
        "the call depth limit of 10000 activations was exceeded"
    );
    assert_eq!(
        render(
            &RESOURCE_STACK_GUARD,
            &[("depth", "7"), ("limit", "100000")]
        ),
        "the evaluator's stack is exhausted at call depth 7 (limit 100000)"
    );
    assert_eq!(render(&DIVISION_BY_ZERO, &[]), "division by zero");
    assert_eq!(render(&RESOURCE_MEMORY, &[]), "memory was exhausted");
    assert_eq!(
        render(
            &DETAIL_GENERATOR_NOT_A_VALUE,
            &[("id", "gen_range")]
        ),
        "the generator builtin gen_range has no value outside a property's `for` clause; the property runner \
         interprets it"
    );
    // A value is inserted as given and never rescanned; extra arguments are ignored.
    assert_eq!(
        render(
            &ASSERTION_FAILED,
            &[("clause", "{clause} and {x}"), ("unused", "u")]
        ),
        "assertion failed: {clause} and {x}"
    );
    // The first argument of a name wins.
    assert_eq!(
        render(&UNRESOLVED_HOLE, &[("hole", "a"), ("hole", "b")]),
        "evaluation reached the unresolved hole ?a"
    );
    // Braces that do not form a placeholder are copied.
    let literal = Template {
        name: "X",
        text: "{a} {B} {} {1x} {c d} {e",
    };
    assert_eq!(render(&literal, &[("a", "A")]), "A {B} {} {1x} {c d} {e");
    let adjacent = Template {
        name: "Y",
        text: "{a}{b_2}",
    };
    assert_eq!(render(&adjacent, &[("a", "1"), ("b_2", "2")]), "12");
}

#[test]
fn the_message_set_covers_the_kinds_of_the_specifications() {
    let names: BTreeSet<&str> = ALL.iter().map(|t| t.name).collect();
    for required in [
        "CONTRACT_VIOLATION",
        "ASSERTION_FAILED",
        "UNRESOLVED_HOLE",
        "DIVISION_BY_ZERO",
        "REFINEMENT_VIOLATION",
        "RESOURCE_STACK",
        "RESOURCE_STACK_GUARD",
        "RESOURCE_TIME",
        "RESOURCE_MEMORY",
        "RESOURCE_STEPS",
        "INTERNAL_ERROR",
        "CHECK_FAILED",
        "PARSE_INT_FAILED",
        "BYTES_TO_TEXT_FAILED",
        // the internal_error details the native lowering raises (spec/cabi.md 1.4)
        "DETAIL_NO_MATCHING_ARM",
        "DETAIL_REFINEMENT_PROOF_UNSOUND",
        "DETAIL_BUILTIN_TYPE_CALLED",
        "DETAIL_GENERATOR_NOT_A_VALUE",
        "DETAIL_GENERATOR_NOT_A_VALUE_EXPR",
        "DETAIL_NOT_A_FUNCTION",
        "DETAIL_CALL_MISMATCH",
        // semantics S80's four binding kinds and stdlib S45's messages
        "RESULT_ILL_TYPED",
        "RESULT_ILL_TYPED_WHOLE",
        "RESULT_REFINEMENT",
        "RESULT_REFINEMENT_WHOLE",
        "DETAIL_BINDING_ERROR_UNREPRESENTABLE",
        "DETAIL_BINDING_PROTOCOL_STATUS",
        "DETAIL_BINDING_PROTOCOL_NO_RESULT",
        "DETAIL_BINDING_PROTOCOL_NO_FAILURE",
        "DETAIL_BINDING_PROTOCOL_NO_RESPONSE",
        "ADAPTER_HOST_EXITED",
        "FOREIGN_RETURNED",
        "FOREIGN_ARGUMENT_DOES_NOT_FIT",
        "FOREIGN_ARGUMENT_CONTAINS_NUL",
    ] {
        assert!(names.contains(required), "no template {required}");
    }
    // spec/cabi.md B.2 shows these three first, in this order.
    assert_eq!(
        ALL.iter().take(3).map(|t| t.name).collect::<Vec<_>>(),
        ["CONTRACT_VIOLATION", "DIVISION_BY_ZERO", "INTERNAL_ERROR"]
    );
    // The S80 and S45 texts, rendered as those decisions write them.
    use messages::*;
    let op = ("operation", "reader.images.ImageCodec.dimensions");
    assert_eq!(
        render(&RESULT_ILL_TYPED, &[op, ("expected_type", "Positive"), ("pointer", "/$payload/0/width")]),
        "the result of reader.images.ImageCodec.dimensions is not a value of Positive at /$payload/0/width"
    );
    assert_eq!(
        render(
            &RESULT_REFINEMENT,
            &[op, ("expected_type", "Positive"), ("pointer", "/$payload/0/width"), ("clause", "self > 0")]
        ),
        "the result of reader.images.ImageCodec.dimensions does not satisfy Positive at /$payload/0/width: self > 0"
    );
    assert_eq!(
        render(
            &RESULT_REFINEMENT_WHOLE,
            &[op, ("expected_type", "Positive"), ("clause", "self > 0")]
        ),
        "the result of reader.images.ImageCodec.dimensions does not satisfy Positive: self > 0"
    );
    assert_eq!(
        render(
            &DETAIL_BINDING_PROTOCOL_NO_RESPONSE,
            &[
                ("binding", "pages"),
                ("operation", "reader.pages.PageRead.get"),
                ("arguments", "PageId(\"p\")")
            ]
        ),
        "pages has no scripted response for reader.pages.PageRead.get(PageId(\"p\"))"
    );
    let status = render(&ADAPTER_HOST_SIGNAL, &[("signal", "11")]);
    assert_eq!(
        render(
            &ADAPTER_HOST_EXITED,
            &[("binding", "image_codec"), ("status", &status)]
        ),
        "the adapter host for image_codec exited with signal 11"
    );
    assert_eq!(render(&ADAPTER_HOST_STATUS, &[("code", "3")]), "status 3");
    assert_eq!(
        render(
            &FOREIGN_RETURNED,
            &[("function", "imgtest_thumbnail"), ("code", "-2")]
        ),
        "imgtest_thumbnail returned -2"
    );
}

/// The templates of what the Milestone 1 evaluator never produced: the binding faults of semantics S80 and the
/// `ForeignError` messages of stdlib S45. Every other template is a message the Milestone 1 evaluator writes.
const MILESTONE_2_TEMPLATES: [&str; 15] = [
    "RESULT_ILL_TYPED",
    "RESULT_ILL_TYPED_WHOLE",
    "RESULT_REFINEMENT",
    "RESULT_REFINEMENT_WHOLE",
    "DETAIL_BINDING_ERROR_UNREPRESENTABLE",
    "DETAIL_BINDING_PROTOCOL_STATUS",
    "DETAIL_BINDING_PROTOCOL_NO_RESULT",
    "DETAIL_BINDING_PROTOCOL_NO_FAILURE",
    "DETAIL_BINDING_PROTOCOL_NO_RESPONSE",
    "ADAPTER_HOST_EXITED",
    "ADAPTER_HOST_STATUS",
    "ADAPTER_HOST_SIGNAL",
    "FOREIGN_RETURNED",
    "FOREIGN_ARGUMENT_DOES_NOT_FIT",
    "FOREIGN_ARGUMENT_CONTAINS_NUL",
];

/// The literal text of a template between its placeholders: `n + 1` pieces for `n` placeholders.
fn pieces(t: &Template) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut rest = t.text;
    for name in placeholders(t) {
        let marker = format!("{{{name}}}");
        let at = rest.find(&marker).expect("a placeholder of the template");
        out.last_mut().unwrap().push_str(&rest[..at]);
        out.push(String::new());
        rest = &rest[at + marker.len()..];
    }
    out.last_mut().unwrap().push_str(rest);
    out
}

/// Whether `source[i..]` begins with the rest of a Rust string literal that is `pieces` joined by format
/// placeholders (`{}`, `{name}`, `{n:?}`), up to its closing `"`.
fn format_literal_at(source: &str, mut i: usize, pieces: &[String]) -> bool {
    for (k, piece) in pieces.iter().enumerate() {
        if !source[i..].starts_with(piece.as_str()) {
            return false;
        }
        i += piece.len();
        if k + 1 < pieces.len() {
            if !source[i..].starts_with('{') {
                return false;
            }
            let Some(close) = source[i..].find('}') else {
                return false;
            };
            if source[i + 1..i + close].contains(['{', '"', '\n']) {
                return false;
            }
            i += close + 1;
        }
    }
    source[i..].starts_with('"')
}

/// Whether `source` holds a string literal that is the template's text as a format string or as plain text.
fn has_format_literal(source: &str, t: &Template) -> bool {
    let pieces = pieces(t);
    source
        .match_indices('"')
        .any(|(at, _)| format_literal_at(source, at + 1, &pieces))
}

/// Every `.rs` file below `dir`, concatenated, with each string continuation (`\` at the end of a line and the
/// leading whitespace of the next) removed, as the compiler reads it.
fn rust_sources(dir: &Path) -> String {
    fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("a source directory is readable") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                collect(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    collect(dir, &mut files);
    files.sort();
    let mut out = String::new();
    for file in files {
        let text = std::fs::read_to_string(&file).expect("a source file is readable");
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\\' && chars.peek() == Some(&'\n') {
                while chars.peek().is_some_and(|c| c.is_whitespace()) {
                    chars.next();
                }
                continue;
            }
            out.push(c);
        }
        out.push('\n');
    }
    out
}

/// The string literal that begins at `source[at]` (a `"`), without its quotes.
fn literal_at(source: &str, at: usize) -> Option<&str> {
    let body = source.get(at + 1..)?;
    let mut escaped = false;
    for (i, c) in body.char_indices() {
        match c {
            '\\' if !escaped => escaped = true,
            '"' if !escaped => return Some(&body[..i]),
            _ => escaped = false,
        }
    }
    None
}

/// The `(kind, detail)` of every `internal_error` call site of the evaluator whose kind and detail are literals:
/// `Fault::internal("<kind>", "<detail>")` and `self.internal("<kind>", "<detail>", span)`, the detail possibly a
/// `format!("<detail>", ...)`. A site whose detail is rendered from a cpl-rtspec template has no literal detail.
fn internal_details(source: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (at, call) in source.match_indices("internal(") {
        let mut i = at + call.len();
        let skip_ws = |i: usize| i + (source[i..].len() - source[i..].trim_start().len());
        i = skip_ws(i);
        let Some(kind) = source[i..]
            .starts_with('"')
            .then(|| literal_at(source, i))
            .flatten()
        else {
            continue;
        };
        i = skip_ws(i + kind.len() + 2);
        if !source[i..].starts_with(',') {
            continue;
        }
        i = skip_ws(i + 1);
        if source[i..].starts_with("format!(") {
            i = skip_ws(i + "format!(".len());
        }
        if !source[i..].starts_with('"') {
            continue;
        }
        if let Some(detail) = literal_at(source, i) {
            out.push((kind.to_string(), detail.to_string()));
        }
    }
    out
}

#[test]
fn milestone_1_templates_are_the_evaluators_format_strings_byte_for_byte() {
    let source = rust_sources(&workspace().join("crates/cpl-eval/src"));
    let switched = source.contains("cpl_rtspec::messages");
    let milestone_1: Vec<&Template> = ALL
        .iter()
        .filter(|t| !MILESTONE_2_TEMPLATES.contains(&t.name))
        .collect();
    assert_eq!(milestone_1.len() + MILESTONE_2_TEMPLATES.len(), ALL.len());
    for t in &milestone_1 {
        if has_format_literal(&source, t) {
            continue;
        }
        assert!(
            switched,
            "{}: crates/cpl-eval/src writes no message {:?} (its placeholders as format arguments)",
            t.name, t.text
        );
        println!(
            "note: {} is no literal of crates/cpl-eval/src any more; the evaluator renders it from cpl-rtspec (T70)",
            t.name
        );
    }
    // The other direction: every internal_error detail the evaluator writes as a literal is one of the templates.
    let details = internal_details(&source);
    assert!(
        switched || details.len() >= 30,
        "the evaluator's internal_error call sites were found ({} of them)",
        details.len()
    );
    for (kind, detail) in &details {
        let quoted = format!("{detail}\"");
        assert!(
            ALL.iter()
                .any(|t| format_literal_at(&quoted, 0, &pieces(t))),
            "the evaluator's internal_error kind {kind} writes the detail {detail:?}, which no template holds"
        );
    }
}

/// A template text of a specification with `<name>` placeholders, as (its literal text with every placeholder
/// written `{}`, the placeholder names in order).
fn spec_template(text: &str) -> (String, Vec<String>) {
    let mut skeleton = String::new();
    let mut names = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        let after = &rest[open + 1..];
        let len = after
            .bytes()
            .take_while(|b| b.is_ascii_alphabetic() || *b == b'_')
            .count();
        if len > 0 && after.as_bytes().get(len) == Some(&b'>') {
            skeleton.push_str(&rest[..open]);
            skeleton.push_str("{}");
            names.push(after[..len].to_string());
            rest = &after[len + 1..];
        } else {
            skeleton.push_str(&rest[..=open]);
            rest = after;
        }
    }
    skeleton.push_str(rest);
    (skeleton, names)
}

/// Whether `token`, a backticked token of a specification's prose, is a message template: text with a `<name>`
/// placeholder, not only a placeholder, and not a fragment that begins with a space.
fn is_spec_template(token: &str) -> bool {
    let (skeleton, names) = spec_template(token);
    !names.is_empty() && skeleton != "{}" && token.contains(' ') && !token.starts_with(' ')
}

/// `ours` is the specification's `text`: the same literal text, and the same placeholder names (lowercased, so
/// `<Kind>` is `{kind}`) except where the specification writes the generic `<n>`.
fn assert_template_is(ours: &Template, text: &str) {
    let (skeleton, names) = spec_template(text);
    let our_names = placeholders(ours);
    let our_skeleton = pieces(ours).join("{}");
    assert_eq!(
        our_skeleton, skeleton,
        "{} against the specification's `{text}`",
        ours.name
    );
    assert_eq!(our_names.len(), names.len(), "{}", ours.name);
    for (our, spec) in our_names.iter().zip(&names) {
        if spec != "n" {
            assert_eq!(
                *our,
                spec.to_lowercase(),
                "{}: a placeholder of `{text}`",
                ours.name
            );
        }
    }
}

#[test]
fn binding_templates_are_the_texts_of_s80_and_s45() {
    use messages::*;
    // semantics S80: the `detail` column of the four binding kinds, in the table's order.
    let doc = read("spec/semantics.md");
    let start = doc
        .find("Decision: [D-282] S80.")
        .expect("the S80 decision");
    let lines: Vec<&str> = doc[start..].lines().collect();
    let rows = table_rows(&lines, "| `kind` | Raised when | `detail` |");
    let kinds: Vec<String> = rows.iter().map(|r| backticked(&r[0]).remove(0)).collect();
    assert_eq!(
        kinds,
        [
            "binding_result_ill_typed",
            "binding_error_unrepresentable",
            "binding_protocol",
            "binding_host_failed"
        ]
    );
    let s80: Vec<String> = rows.iter().flat_map(|r| backticked(&r[2])).collect();
    let ours_s80 = [
        &RESULT_ILL_TYPED,
        &DETAIL_BINDING_ERROR_UNREPRESENTABLE,
        &DETAIL_BINDING_PROTOCOL_STATUS,
        &DETAIL_BINDING_PROTOCOL_NO_RESULT,
        &DETAIL_BINDING_PROTOCOL_NO_FAILURE,
        &DETAIL_BINDING_PROTOCOL_NO_RESPONSE,
        &ADAPTER_HOST_EXITED,
    ];
    assert_eq!(s80.len(), ours_s80.len(), "the S80 detail texts: {s80:?}");
    for (ours, text) in ours_s80.iter().zip(&s80) {
        assert_template_is(ours, text);
    }

    // stdlib S45: the message templates of its `message` paragraph, in the paragraph's order.
    let doc = read("spec/stdlib.md");
    let start = doc
        .find("The `message` is the one the adapter or host callback reports")
        .expect("the S45 message paragraph");
    let end = start
        + doc[start..]
            .find("The `details` are")
            .expect("the paragraph ends");
    let paragraph = doc[start..end].lines().collect::<Vec<_>>().join(" ");
    let s45: Vec<String> = backticked(&paragraph)
        .into_iter()
        .filter(|token| is_spec_template(token))
        .collect();
    let ours_s45 = [
        &FOREIGN_RETURNED,
        &FOREIGN_ARGUMENT_DOES_NOT_FIT,
        &FOREIGN_ARGUMENT_CONTAINS_NUL,
        &RESULT_REFINEMENT,
        &RESULT_ILL_TYPED,
        &ADAPTER_HOST_EXITED,
        &ADAPTER_HOST_STATUS,
        &ADAPTER_HOST_SIGNAL,
    ];
    assert_eq!(
        s45.len(),
        ours_s45.len(),
        "the S45 message templates: {s45:?}"
    );
    for (ours, text) in ours_s45.iter().zip(&s45) {
        assert_template_is(ours, text);
    }

    // "with ` at <pointer>` left out when the failing position is the whole result" (S45, S80).
    assert!(paragraph.contains("with ` at <pointer>` left out"));
    for (whole, at) in [
        (&RESULT_ILL_TYPED_WHOLE, &RESULT_ILL_TYPED),
        (&RESULT_REFINEMENT_WHOLE, &RESULT_REFINEMENT),
    ] {
        assert_eq!(
            whole.text,
            at.text.replacen(" at {pointer}", "", 1),
            "{}",
            whole.name
        );
    }

    // These are exactly the Milestone 2 templates.
    let covered: BTreeSet<&str> = ours_s80
        .iter()
        .chain(&ours_s45)
        .map(|t| t.name)
        .chain([RESULT_ILL_TYPED_WHOLE.name, RESULT_REFINEMENT_WHOLE.name])
        .collect();
    assert_eq!(covered, MILESTONE_2_TEMPLATES.into_iter().collect());
}

// ---- limits, division, foreign, interop ---------------------------------------------------------------------------

#[test]
fn limits_and_versions() {
    assert_eq!(limits::DEFAULT_MAX_CALL_DEPTH, 10_000);
    assert_eq!(limits::EVAL_STACK_SIZE, 1_073_741_824);
    assert_eq!(limits::GUARD_MARGIN, 67_108_864);
    assert_eq!(limits::DRIVER_PROTOCOL, 1);
    let defines = c_defines(&read("crates/cpl-rt/c/include/cpl.h"));
    assert_eq!(
        limits::C_API_VERSION,
        (
            defines["CPL_API_VERSION_MAJOR"].parse().unwrap(),
            defines["CPL_API_VERSION_MINOR"].parse().unwrap()
        ),
        "the C API version of cpl.h"
    );
}

#[test]
fn div_rem_is_s37() {
    let big = |n: i128| BigInt::from(n);
    for (a, b, q, r) in [
        (7, -2, -3, 1),
        (-7, 2, -3, -1),
        (7, 2, 3, 1),
        (-7, -2, 3, -1),
        (0, 5, 0, 0),
        (6, 3, 2, 0),
    ] {
        assert_eq!(
            arith::div_rem(&big(a), &big(b)),
            Some((big(q), big(r))),
            "{a} / {b}"
        );
    }
    assert_eq!(arith::div_rem(&big(5), &big(0)), None);
    assert_eq!(arith::div_rem(&big(0), &big(0)), None);
    // -2^62 / -1 leaves the small range of the native runtime (spec/cabi.md 2.5); here it is just exact.
    let two_62: BigInt = big(1) << 62u32;
    let min_small = -two_62.clone();
    assert_eq!(arith::div_rem(&min_small, &big(-1)), Some((two_62, big(0))));
    // A grid: a == q * b + r, |r| < |b|, r has a's sign or is 0, and q truncates toward zero (as i128's operators).
    for a in -40i128..=40 {
        for b in (-9i128..=9).filter(|&b| b != 0) {
            let (q, r) = arith::div_rem(&big(a), &big(b)).unwrap();
            assert_eq!(
                (q.clone(), r.clone()),
                (big(a / b), big(a % b)),
                "{a} / {b}"
            );
            assert_eq!(q * big(b) + r, big(a));
        }
    }
    let huge: BigInt = "-123456789012345678901234567890123456789".parse().unwrap();
    let d: BigInt = "98765432109876543210".parse().unwrap();
    let (q, r) = arith::div_rem(&huge, &d).unwrap();
    assert_eq!(&q * &d + &r, huge);
    assert!(r <= BigInt::from(0) && -&r < d);
    // Both sides of the unsigned single-limb fast path, including values outside i64 and its signed overflow pair.
    let magnitudes: Vec<BigInt> = [
        0,
        1,
        3,
        (1u128 << 63) - 1,
        1u128 << 63,
        u64::MAX as u128,
        1u128 << 64,
        (1u128 << 64) + 1,
        1u128 << 127,
    ]
    .into_iter()
    .map(BigInt::from)
    .collect();
    for a in magnitudes.iter().flat_map(|n| [n.clone(), -n]) {
        for b in magnitudes
            .iter()
            .filter(|n| n.sign() != num_bigint::Sign::NoSign)
            .flat_map(|n| [n.clone(), -n])
        {
            let q = &a / &b;
            let r = &a - &q * &b;
            assert_eq!(arith::div_rem(&a, &b), Some((q, r)), "{a} / {b}");
        }
    }
}

#[test]
fn foreign_kinds_are_std_foreign_in_declaration_order() {
    let text = read("std/foreign.cpl");
    let start = text
        .find("pub enum ForeignErrorKind {")
        .expect("std.foreign.ForeignErrorKind");
    let body = &text[start..start + text[start..].find('}').unwrap()];
    let variants: Vec<&str> = body
        .lines()
        .skip(1)
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("///"))
        .collect();
    assert_eq!(variants, foreign::KINDS.to_vec());
    let defines = c_defines(&read("crates/cpl-rt/c/include/cpl.h"));
    for (i, name) in [
        "CPL_FOREIGN_EXCEPTION",
        "CPL_FOREIGN_INVALID_ARGUMENT",
        "CPL_FOREIGN_INVALID_RESULT",
        "CPL_FOREIGN_TRANSPORT",
        "CPL_FOREIGN_TIMEOUT",
        "CPL_FOREIGN_CANCELLED",
        "CPL_FOREIGN_UNAVAILABLE",
    ]
    .iter()
    .enumerate()
    {
        assert_eq!(
            defines[*name],
            i.to_string(),
            "{name} is the index of {}",
            foreign::KINDS[i]
        );
    }
    assert_eq!(
        foreign::DETAIL_ORDER,
        [
            "adapter",
            "capability",
            "operation",
            "foreign_type",
            "foreign_code",
            "pointer",
            "expected_type"
        ]
    );
}

#[test]
fn interop_codes_are_cabi_4_9_in_its_order_with_the_statuses_of_cpl_h() {
    let doc = read("spec/cabi.md");
    let lines = section(&doc, "### 4.9 C API error codes");
    let rows = table_rows(&lines, "| Code | Status | Payload |");
    let defines = c_defines(&read("crates/cpl-rt/c/include/cpl.h"));
    let expected: Vec<(String, i32)> = rows
        .iter()
        .map(|row| {
            let code = backticked(&row[0]).remove(0);
            let status = backticked(&row[1]).remove(0);
            let value = defines
                .get(&status)
                .unwrap_or_else(|| panic!("cpl.h defines {status}"))
                .parse()
                .unwrap();
            (code, value)
        })
        .collect();
    assert_eq!(expected.len(), 24);
    let ours: Vec<(String, i32)> = interop::CODES
        .iter()
        .map(|c| (c.code.to_string(), c.status))
        .collect();
    assert_eq!(ours, expected);
    assert_eq!(interop::BIND_FAILED.code, "interop_bind_failed");
    assert_eq!(interop::BIND_FAILED.status, 7);
    assert_eq!(interop::CODES[0], interop::VERSION_MISMATCH);
    assert_eq!(interop::CODES[23], interop::BIND_FAILED);
}
