//! The typecheck corpus harness (`spec/toolchain.md` 12-12.2, T38;
//! `crates/cpl-sema/tests/corpus/README.md`).
//!
//! Every case is checked and its diagnostics are compared, as an exact
//! multiset of (file, line, severity, code), with the markers of its
//! `.cpl` files:
//!
//! * `// expect-error: a, b` on the line where the primary span starts;
//!   `// expect-error@+N: c` for the line N lines below; `// expect-error(global): d`
//!   for a diagnostic without a span or whose span lies outside the case's
//!   `.cpl` files (the manifest, std). `expect-warning` and `expect-info`
//!   take the same forms.
//! * A single file (`ok/*.cpl`, `errors/<category>/*.cpl`) is placed at
//!   the conventional path of its `module` header in a `VirtualTree` with a
//!   synthetic `cpl.toml` (`name = "errors"`, or `"ok"` under `ok/`).
//! * A directory with its own `cpl.toml` is loaded as the project.
//! * Cases under `release/` are checked with `CheckConfig { release: true }`.
//! * A positive case (`ok/`) checks with zero errors (toolchain 12's
//!   positive row; its warnings and infos are marked): an error in it fails
//!   the case even when an `expect-error` marker names it, and so does
//!   such a marker.
//! * Every diagnostic's code is one of the tables (semantics 9.4 and
//!   toolchain Appendix B in `cpl_sema::codes`, the `project` codes of
//!   loading, the `parse` codes) and carries that table's category
//!   (toolchain 12's negative row: "codes are those of semantics 9.4").
//!
//! The harness prints `corpus: N files, P pass, F fail`, then the missing
//! and unexpected diagnostics of every failing case, and fails while any
//! case fails. `CPL_CORPUS_FILTER=<substring>` restricts the run to the
//! cases whose path contains the substring.

mod common;

use common::*;
use cpl_diag::{Category, FileOrigin, Severity};
use cpl_sema::{check_with, CheckConfig};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// (file, line, severity, code): `file` and `line` are `None` for a
/// `(global)` marker or a diagnostic outside the case's `.cpl` files.
type Key = (Option<String>, Option<u32>, Severity, String);

/// One corpus case.
struct Case {
    /// The path shown in reports, relative to the corpus root.
    name: String,
    kind: CaseKind,
    release: bool,
}

enum CaseKind {
    File(PathBuf),
    Dir(PathBuf),
}

fn collect_cases(root: &Path, dir: &Path, release: bool, out: &mut Vec<Case>) {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .expect("read a corpus directory")
        .map(|e| e.expect("an entry").path())
        .collect();
    entries.sort();
    for path in entries {
        let name = path
            .strip_prefix(root)
            .expect("under the corpus")
            .to_string_lossy()
            .replace('\\', "/");
        if path.is_dir() {
            let release = release || name == "release";
            if path.join("cpl.toml").is_file() {
                out.push(Case {
                    name,
                    kind: CaseKind::Dir(path),
                    release,
                });
            } else {
                collect_cases(root, &path, release, out);
            }
        } else if path.extension().is_some_and(|e| e == "cpl") {
            out.push(Case {
                name,
                kind: CaseKind::File(path),
                release,
            });
        }
    }
}

/// The markers of one file (`path` is its project-relative path).
fn markers(path: &str, text: &str) -> Vec<Key> {
    let mut out = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let number = index as u32 + 1;
        let mut rest = line;
        while let Some(at) = rest.find("// expect-") {
            rest = &rest[at + "// expect-".len()..];
            let (severity, tail) = if let Some(t) = rest.strip_prefix("error") {
                (Severity::Error, t)
            } else if let Some(t) = rest.strip_prefix("warning") {
                (Severity::Warning, t)
            } else if let Some(t) = rest.strip_prefix("info") {
                (Severity::Info, t)
            } else {
                continue;
            };
            let (place, tail) = if let Some(t) = tail.strip_prefix("(global)") {
                (None, t)
            } else if let Some(t) = tail.strip_prefix("@+") {
                let digits: String = t.chars().take_while(char::is_ascii_digit).collect();
                let offset: u32 = digits.parse().expect("a marker's @+N");
                (Some(number + offset), &t[digits.len()..])
            } else {
                (Some(number), tail)
            };
            let Some(list) = tail.strip_prefix(':') else {
                continue;
            };
            for item in list.split(',') {
                let code: String = item
                    .trim_start()
                    .chars()
                    .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_')
                    .collect();
                if !code.is_empty() {
                    out.push((place.map(|_| path.to_string()), place, severity, code));
                }
            }
        }
    }
    out
}

/// The outcome of one case.
struct Outcome {
    missing: Vec<Key>,
    unexpected: Vec<Key>,
    /// The errors and `expect-error` markers of a positive case (`ok/`),
    /// which admits none.
    forbidden: Vec<Key>,
    /// Diagnostics whose code is in no table, or whose category is not the
    /// table's.
    uncoded: Vec<String>,
}

impl Outcome {
    fn passed(&self) -> bool {
        self.missing.is_empty()
            && self.unexpected.is_empty()
            && self.forbidden.is_empty()
            && self.uncoded.is_empty()
    }
}

/// Why a diagnostic's code or category is not that of the code tables.
fn uncoded(d: &cpl_diag::Diagnostic) -> Option<String> {
    let known = cpl_sema::codes::ALL.iter().any(|(c, _, _)| *c == d.code)
        || cpl_sema::project::codes::PROJECT.contains(&d.code)
        || cpl_syntax::codes::ALL.contains(&d.code);
    let category = cpl_sema::codes::category(d.code);
    if !known {
        Some(format!("{} is in no code table", d.code.0))
    } else if d.category != category {
        Some(format!(
            "{} has category {}, the table's is {}",
            d.code.0,
            d.category.as_str(),
            category.as_str()
        ))
    } else {
        None
    }
}

fn run(case: &Case) -> Outcome {
    let config = CheckConfig {
        release: case.release,
        ..CheckConfig::default()
    };
    let (project, expected) = match &case.kind {
        CaseKind::File(path) => {
            let text = fs::read_to_string(path).expect("read a corpus file");
            let rel = conventional_path(&text);
            let name = if case.name.starts_with("ok/") {
                "ok"
            } else {
                "errors"
            };
            let manifest = manifest(name);
            let project = load_tree(&tree(&[
                ("cpl.toml", manifest.as_str()),
                (rel.as_str(), text.as_str()),
            ]));
            (project, markers(&rel, &text))
        }
        CaseKind::Dir(dir) => {
            let project = load_dir(dir);
            let tree = tree_of_dir(dir);
            let mut expected = Vec::new();
            for (rel, bytes) in &tree.files {
                if rel.ends_with(".cpl") {
                    expected.extend(markers(rel, &String::from_utf8_lossy(bytes)));
                }
            }
            (project, expected)
        }
    };
    let result = check_with(&project, &config);
    let uncoded: Vec<String> = result.diagnostics.iter().filter_map(uncoded).collect();
    let sm = result.program.source_map();
    let actual: Vec<Key> = result
        .diagnostics
        .iter()
        .map(|d| {
            let place = d.span.and_then(|s| {
                let file = sm.file(s.file);
                let in_case = file.origin != FileOrigin::Std && file.path.ends_with(".cpl");
                in_case.then(|| (file.path.clone(), sm.line_col(s.file, s.start).line))
            });
            match place {
                Some((file, line)) => (Some(file), Some(line), d.severity, d.code.0.to_string()),
                None => (None, None, d.severity, d.code.0.to_string()),
            }
        })
        .collect();
    let mut want: BTreeMap<Key, usize> = BTreeMap::new();
    for k in expected {
        *want.entry(k).or_default() += 1;
    }
    let mut got: BTreeMap<Key, usize> = BTreeMap::new();
    for k in actual {
        *got.entry(k).or_default() += 1;
    }
    let mut missing = Vec::new();
    let mut unexpected = Vec::new();
    for (k, &n) in &want {
        let have = got.get(k).copied().unwrap_or(0);
        for _ in have..n {
            missing.push(k.clone());
        }
    }
    for (k, &n) in &got {
        let wanted = want.get(k).copied().unwrap_or(0);
        for _ in wanted..n {
            unexpected.push(k.clone());
        }
    }
    // Toolchain 12, positive typecheck: zero errors in `ok/`, each error
    // listed once whether it is a diagnostic, a marker, or both.
    let mut forbidden = Vec::new();
    if case.name.starts_with("ok/") {
        let mut errors: BTreeMap<&Key, usize> = BTreeMap::new();
        for (k, &n) in want.iter().chain(&got) {
            if k.2 == Severity::Error {
                let count = errors.entry(k).or_default();
                *count = (*count).max(n);
            }
        }
        for (k, n) in errors {
            forbidden.extend(std::iter::repeat_n(k.clone(), n));
        }
    }
    Outcome {
        missing,
        unexpected,
        forbidden,
        uncoded,
    }
}

fn show(k: &Key) -> String {
    let (file, line, severity, code) = k;
    match (file, line) {
        (Some(f), Some(l)) => format!("{f}:{l}: {severity}[{code}]"),
        _ => format!("(global): {severity}[{code}]"),
    }
}

/// The stage a missing or unexpected code waits for (for the summary).
fn stage_of(code: &str) -> &'static str {
    let category = cpl_sema::codes::ALL
        .iter()
        .find(|(c, _, _)| c.0 == code)
        .map(|(_, category, _)| *category);
    match category {
        Some(Category::Type) => "type",
        Some(Category::Effect) => "effect",
        Some(Category::Contract) => "contract",
        Some(Category::Hole | Category::ReleasePolicy) => "policy",
        _ => "resolve",
    }
}

#[test]
fn typecheck_corpus() {
    let root = corpus();
    let mut cases = Vec::new();
    collect_cases(&root, &root, false, &mut cases);
    if let Ok(filter) = std::env::var("CPL_CORPUS_FILTER") {
        cases.retain(|c| c.name.contains(&filter));
    }
    let mut failures = Vec::new();
    for case in &cases {
        let outcome = run(case);
        if !outcome.passed() {
            failures.push((case, outcome));
        }
    }
    let total = cases.len();
    let failed = failures.len();
    println!(
        "corpus: {total} files, {} pass, {failed} fail",
        total - failed
    );
    // Which later stage each failing case waits for: the earliest stage
    // among its missing and unexpected codes (resolve < type < effect <
    // contract < policy).
    let order = ["resolve", "type", "effect", "contract", "policy"];
    let mut waits: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, outcome) in &failures {
        let stage = outcome
            .missing
            .iter()
            .chain(&outcome.unexpected)
            .chain(&outcome.forbidden)
            .map(|k| stage_of(&k.3))
            .min_by_key(|s| order.iter().position(|o| o == s))
            .unwrap_or("resolve");
        *waits.entry(stage).or_default() += 1;
    }
    if failed > 0 {
        let summary: Vec<String> = order
            .iter()
            .filter_map(|s| waits.get(s).map(|n| format!("{n} {s}")))
            .collect();
        println!(
            "failing cases by the earliest stage they need: {}",
            summary.join(", ")
        );
    }
    for (case, outcome) in &failures {
        println!("FAIL {}", case.name);
        for k in &outcome.missing {
            println!("    missing    {}", show(k));
        }
        for k in &outcome.unexpected {
            println!("    unexpected {}", show(k));
        }
        for why in &outcome.uncoded {
            println!("    bad code   {why}");
        }
        for k in &outcome.forbidden {
            println!(
                "    forbidden  {} (a positive case admits no error)",
                show(k)
            );
        }
    }
    assert!(
        failed == 0,
        "{failed} of {total} corpus cases fail (see the report above)"
    );
}
