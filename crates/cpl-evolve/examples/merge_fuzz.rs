//! Fuzzing of the three-way semantic merge: `cargo run --release -p cpl-evolve --example merge_fuzz
//! [cases] [first-seed]`.
//!
//! Each case is a random program of `cpl_eval::testing::programs` (the base) and two edits of it,
//! each rewriting one node of one function. Merging the two edited programs against the base must:
//!
//! * edits of different functions: compose without a conflict, into exactly the program that has
//!   both edits;
//! * edits of the same function: either report a conflict, or compose into exactly the program
//!   that has both edits (edits of different facets combine).
//!
//! Programs are compared by their formatted text. A violation is printed with the programs and
//! the seed; the process exits 1 when there was one.

use cpl_eval::testing::programs::{generate, Rng};
use cpl_evolve::{merge::merge, verify::VerifyConfig, Snapshot};
use cpl_project::{LoadOptions, VirtualTree};
use cpl_syntax::formatter::{format_source, FileKind};

fn tree(source: &str) -> VirtualTree {
    VirtualTree {
        files: [
            (
                "cpl.toml".to_string(),
                b"[project]\nname = \"m\"\nedition = \"0.1\"\n".to_vec(),
            ),
            ("src/m.cpl".to_string(), source.as_bytes().to_vec()),
        ]
        .into_iter()
        .collect(),
    }
}

fn snapshot(source: &str) -> Option<Snapshot> {
    Snapshot::new(tree(source), &LoadOptions::default()).ok()
}

fn formatted(source: &str) -> String {
    format_source(source, FileKind::Cpl, cpl_diag::FileId(0)).unwrap_or_else(|_| source.to_string())
}

fn main() {
    let mut args = std::env::args().skip(1);
    let cases: u64 = args.next().and_then(|n| n.parse().ok()).unwrap_or(300);
    let first: u64 = args.next().and_then(|n| n.parse().ok()).unwrap_or(1);
    let config = VerifyConfig {
        cases: 10,
        ..VerifyConfig::default()
    };
    let (mut merged, mut conflicts, mut skipped, mut violations) = (0usize, 0usize, 0usize, 0usize);
    let start = std::time::Instant::now();
    for seed in first..first + cases {
        let base = generate(seed, &[]);
        let functions: Vec<usize> = (0..base.nodes.len())
            .filter(|&f| base.nodes[f] > 0)
            .collect();
        if functions.is_empty() {
            skipped += 1;
            continue;
        }
        let mut rng = Rng::new(seed ^ 0x0003_E26E);
        let pick = |rng: &mut Rng, f: usize| {
            (
                f,
                1 + rng.below(base.nodes[f] as u64) as usize,
                rng.next_u64(),
            )
        };
        let a = functions[rng.below(functions.len() as u64) as usize];
        // mostly different functions, sometimes the same one
        let b = if functions.len() > 1 && rng.chance(75) {
            let others: Vec<usize> = functions.iter().copied().filter(|&f| f != a).collect();
            others[rng.below(others.len() as u64) as usize]
        } else {
            a
        };
        let (ma, mb) = (pick(&mut rng, a), pick(&mut rng, b));
        if ma.0 == mb.0 && ma.1 == mb.1 {
            skipped += 1;
            continue;
        }
        let left = generate(seed, &[ma]);
        let right = generate(seed, &[mb]);
        let both = generate(seed, &[ma, mb]);
        let (Some(sb), Some(sl), Some(sr)) = (
            snapshot(&base.source),
            snapshot(&left.source),
            snapshot(&right.source),
        ) else {
            skipped += 1;
            continue;
        };
        let report = |what: &str, extra: &str| {
            println!(
                "=== VIOLATION (seed {seed}): {what}\nleft {:?} {:?}\nright {:?} {:?}\n{extra}\n--- base\n{}\n--- both\n{}",
                ma, left.mutations, mb, right.mutations, base.source, both.source
            );
        };
        let result = match merge(&sb, &sl, &sr, &config, &LoadOptions::default()) {
            Ok(m) => m,
            Err(d) => {
                violations += 1;
                report("merge failed", &format!("{d:?}"));
                continue;
            }
        };
        let text = result
            .tree
            .as_ref()
            .and_then(|t| t.files.get("src/m.cpl"))
            .map(|b| String::from_utf8_lossy(b).to_string());
        let conflicted = !result.report.conflicts.is_empty();
        if conflicted {
            conflicts += 1;
            if a != b {
                violations += 1;
                report(
                    "edits of different functions conflict",
                    &format!("{:?}", result.report.conflicts),
                );
            }
            continue;
        }
        let Some(text) = text else {
            violations += 1;
            report("no conflict but no composed program", &result.report.status);
            continue;
        };
        merged += 1;
        // Identical edits merge into one; an edit that changes no text leaves the other side's.
        // (Two rewrites of nested nodes are not their textual composition, but they change one
        // facet on both sides, which is a conflict.)
        let expected = if left.source == right.source || right.source == base.source {
            &left.source
        } else if left.source == base.source {
            &right.source
        } else {
            &both.source
        };
        if formatted(&text) != formatted(expected) {
            violations += 1;
            report(
                "the composition is not the program with both edits",
                &format!("status {} \n--- composed\n{text}", result.report.status),
            );
            continue;
        }
        if result.report.status == "check_failed" {
            violations += 1;
            report(
                "the composition does not check",
                &format!("{:?}", result.report.diagnostics),
            );
        }
    }
    println!(
        "cases {cases} merged {merged} conflicts {conflicts} skipped {skipped} violations {violations} in {:.1}s",
        start.elapsed().as_secs_f64()
    );
    if violations > 0 {
        std::process::exit(1);
    }
}
