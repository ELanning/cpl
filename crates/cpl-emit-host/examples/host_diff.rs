//! Differential testing of a host backend against the evaluator:
//! `cargo run --release -p cpl-emit-host --example host_diff -- <rust|go|java> [cases] [first-seed]`.
//!
//! Each case is a random program of `cpl_eval::testing::programs`, compiled to the target with
//! its JSON driver, built with the target's toolchain, and run on random arguments beside the
//! evaluator (the harness is `tests/common`). `HOST_DIFF_FILE=path.cpl` compares one source, and
//! `HOST_DIFF_DIR=dir` every `.cpl` file under a directory, on swept inputs of any type;
//! `HOST_DIFF_REQUESTS=file` replays the requests of a file instead (one JSON request per line,
//! the driver's form), for hand-picked edge cases.
//! `JAVAC` and `JAVA` name the JDK for `java`; `HOST_DIFF_STEPS` sets the evaluator's budget.

#[path = "../tests/common/mod.rs"]
mod common;

use cpl_emit_host::Target;
use cpl_eval::testing::programs::generate;
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let target =
        Target::parse(&args.next().unwrap_or_else(|| "rust".into())).expect("rust, go, or java");
    let cases: u64 = args.next().and_then(|n| n.parse().ok()).unwrap_or(20);
    let first: u64 = args.next().and_then(|n| n.parse().ok()).unwrap_or(1);
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/host-diff")
        .join(target.name());
    let files: Vec<PathBuf> = match (
        std::env::var("HOST_DIFF_FILE"),
        std::env::var("HOST_DIFF_DIR"),
    ) {
        (Ok(f), _) => vec![PathBuf::from(f)],
        (_, Ok(d)) => common::cpl_sources(&PathBuf::from(d)),
        _ => Vec::new(),
    };
    let swept = !files.is_empty();
    let seeds: Vec<u64> = if swept {
        (0..files.len() as u64).collect()
    } else {
        (first..first + cases).collect()
    };
    let (mut programs, mut compared, mut divergences, mut failures) = (0, 0, 0, 0);
    let started = std::time::Instant::now();
    for seed in seeds {
        let source = if swept {
            match std::fs::read_to_string(&files[seed as usize]) {
                Ok(s) => s,
                Err(_) => continue,
            }
        } else {
            generate(seed, &[]).source
        };
        if common::check(&source).is_err() {
            continue;
        }
        programs += 1;
        match common::run_case(target, &root, seed, &source, swept) {
            Err(e) => {
                failures += 1;
                println!(
                    "=== BUILD FAILED (seed {seed})\n{}\n--- program\n{source}",
                    e.lines().take(60).collect::<Vec<_>>().join("\n")
                );
            }
            Ok(case) => {
                compared += case.compared;
                if let Some(d) = case.divergence {
                    divergences += 1;
                    println!("=== DIVERGENCE (seed {seed}) {d}");
                }
            }
        }
    }
    println!(
        "{}: programs {programs} compared {compared} divergences {divergences} build failures {failures} in {:.1}s",
        target.name(),
        started.elapsed().as_secs_f64()
    );
    if divergences > 0 || failures > 0 {
        std::process::exit(1);
    }
}
