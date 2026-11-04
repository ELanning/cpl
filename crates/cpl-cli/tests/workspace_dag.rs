//! The crate dependency DAG (toolchain 1.2, T69): every `crates/*/Cargo.toml`, its normal and its dev dependencies,
//! against the forbidden edges of the table: no Milestone 1 crate depends on `cpl-rt`, `cpl-emit-c`, or `cpl-cabi`, so
//! the checker and the evaluator never come to depend on code emission; the only new edges into `cpl-sema` and
//! `cpl-eval` go to the data-only `cpl-rtspec`; `cpl-rtspec` depends on no internal crate and `cpl-rt` on none but
//! `cpl-rtspec` and `cpl-diag`; `cpl-emit-c` does not depend on `cpl-cabi`; and every Milestone 1 edge stays as it
//! is (`cpl-eval` reaches `cpl-sema` only as a dev-dependency).

mod common;

use std::collections::{BTreeMap, BTreeSet};

/// The internal dependencies of every crate: (normal, dev).
fn edges() -> BTreeMap<String, (BTreeSet<String>, BTreeSet<String>)> {
    let crates = common::workspace().join("crates");
    let mut out = BTreeMap::new();
    for e in std::fs::read_dir(&crates).unwrap() {
        let dir = e.unwrap().path();
        let manifest = dir.join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        let doc: toml::Table =
            toml::from_str(&std::fs::read_to_string(&manifest).unwrap()).unwrap();
        let name = doc["package"]["name"].as_str().unwrap().to_string();
        let internal = |key: &str| -> BTreeSet<String> {
            doc.get(key)
                .and_then(|t| t.as_table())
                .map(|t| {
                    t.keys()
                        .filter(|k| k.starts_with("cpl-"))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()
        };
        out.insert(
            name,
            (internal("dependencies"), internal("dev-dependencies")),
        );
    }
    out
}

#[test]
fn the_forbidden_edges_of_the_table_are_absent() {
    let edges = edges();
    let m2 = ["cpl-rt", "cpl-emit-c", "cpl-cabi"];
    let all: BTreeSet<&str> = edges.keys().map(String::as_str).collect();
    // (crate, forbidden dependencies, also forbidden as a dev-dependency)
    let forbidden: Vec<(&str, Vec<&str>, bool)> = vec![
        (
            "cpl-diag",
            all.iter().copied().filter(|c| *c != "cpl-diag").collect(),
            true,
        ),
        (
            "cpl-syntax",
            vec![
                "cpl-project",
                "cpl-hir",
                "cpl-sema",
                "cpl-eval",
                "cpl-index",
                "cpl-work",
                "cpl-rt",
                "cpl-emit-c",
                "cpl-cabi",
            ],
            true,
        ),
        (
            "cpl-project",
            vec![
                "cpl-hir",
                "cpl-sema",
                "cpl-eval",
                "cpl-index",
                "cpl-work",
                "cpl-rt",
                "cpl-emit-c",
                "cpl-cabi",
            ],
            false,
        ),
        (
            "cpl-hir",
            vec![
                "cpl-project",
                "cpl-sema",
                "cpl-eval",
                "cpl-index",
                "cpl-work",
                "cpl-rt",
                "cpl-emit-c",
                "cpl-cabi",
            ],
            false,
        ),
        (
            "cpl-sema",
            vec![
                "cpl-eval",
                "cpl-index",
                "cpl-work",
                "cpl-rt",
                "cpl-emit-c",
                "cpl-cabi",
            ],
            false,
        ),
        // cpl-eval: cpl-sema as a normal dependency (it is a dev-dependency, for its tests)
        (
            "cpl-eval",
            vec![
                "cpl-sema",
                "cpl-index",
                "cpl-work",
                "cpl-rt",
                "cpl-emit-c",
                "cpl-cabi",
            ],
            false,
        ),
        (
            "cpl-index",
            vec!["cpl-eval", "cpl-work", "cpl-rt", "cpl-emit-c", "cpl-cabi"],
            false,
        ),
        (
            "cpl-work",
            vec!["cpl-eval", "cpl-rt", "cpl-emit-c", "cpl-cabi"],
            false,
        ),
        (
            "cpl-rtspec",
            all.iter().copied().filter(|c| *c != "cpl-rtspec").collect(),
            false,
        ),
        (
            "cpl-rt",
            all.iter()
                .copied()
                .filter(|c| !["cpl-rt", "cpl-rtspec", "cpl-diag"].contains(c))
                .collect(),
            false,
        ),
        ("cpl-emit-c", vec!["cpl-cabi"], true),
        // The verifier reads the typed HIR only: the checker is a dev-dependency (its tests
        // check the programs they verify), and nothing that runs programs is a dependency.
        (
            "cpl-prove",
            vec![
                "cpl-project",
                "cpl-sema",
                "cpl-eval",
                "cpl-index",
                "cpl-work",
                "cpl-evolve",
                "cpl-rt",
                "cpl-emit-c",
                "cpl-cabi",
            ],
            false,
        ),
    ];
    let mut found = Vec::new();
    for (krate, banned, dev_too) in &forbidden {
        let Some((normal, dev)) = edges.get(*krate) else {
            continue;
        };
        for b in banned {
            if normal.contains(*b) {
                found.push(format!("{krate} -> {b}"));
            }
            if *dev_too && dev.contains(*b) {
                found.push(format!("{krate} -(dev)-> {b}"));
            }
        }
    }
    // no Milestone 1 crate depends on the Milestone 2 code crates, not even for its tests (cpl-cli is the exception)
    for krate in [
        "cpl-diag",
        "cpl-syntax",
        "cpl-project",
        "cpl-hir",
        "cpl-sema",
        "cpl-eval",
        "cpl-index",
        "cpl-work",
    ] {
        if let Some((normal, dev)) = edges.get(krate) {
            for b in m2 {
                if normal.contains(b) || dev.contains(b) {
                    found.push(format!("the Milestone 1 crate {krate} depends on {b}"));
                }
            }
        }
    }
    // the only new edges into cpl-sema and cpl-eval are to the data-only cpl-rtspec, and
    // cpl-sema's to the verifier, which reads the HIR alone (spec/verification.md)
    let allowed_sema = [
        "cpl-hir",
        "cpl-project",
        "cpl-syntax",
        "cpl-diag",
        "cpl-rtspec",
        "cpl-prove",
    ];
    let allowed_eval = ["cpl-hir", "cpl-syntax", "cpl-diag", "cpl-rtspec"];
    for (krate, allowed) in [
        ("cpl-sema", &allowed_sema[..]),
        ("cpl-eval", &allowed_eval[..]),
    ] {
        if let Some((normal, _)) = edges.get(krate) {
            for d in normal {
                if !allowed.contains(&d.as_str()) {
                    found.push(format!("{krate} -> {d}, outside its table row"));
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "forbidden edges of spec/toolchain.md 1.2:\n{}",
        found.join("\n")
    );
    // the four crates of Milestone 2 are there, and cpl-cli depends on every other crate
    for c in ["cpl-rtspec", "cpl-rt", "cpl-emit-c", "cpl-cabi"] {
        assert!(
            edges.contains_key(c),
            "the Milestone 2 crate {c} is missing"
        );
    }
    let (cli, _) = &edges["cpl-cli"];
    for c in edges.keys().filter(|c| *c != "cpl-cli") {
        assert!(cli.contains(c), "cpl-cli does not depend on {c}");
    }
}
