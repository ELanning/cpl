//! Timing of the verifier on every function of a project:
//! `cargo run --release -p cpl-prove --example prove_bench [project-dir] [iterations]`.
use cpl_project::{load_project, LoadOptions};
use cpl_prove::verify;
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let root: PathBuf = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/reader"));
    let iterations: u32 = args.next().and_then(|n| n.parse().ok()).unwrap_or(20);
    let project = load_project(&root, &LoadOptions::default()).expect("the project loads");
    let checked = cpl_sema::check(&project);
    let p = &checked.program;
    let functions: Vec<_> = p
        .defs()
        .filter(|d| p.module(d.module).origin != cpl_diag::FileOrigin::Std)
        .filter(|d| {
            d.kind
                .as_fn()
                .is_some_and(|f| matches!(f.body, cpl_hir::FnBody::Block(_)))
        })
        .collect();
    let mut total = 0.0;
    for d in &functions {
        let start = Instant::now();
        let mut report = None;
        for _ in 0..iterations {
            report = verify(p, d.id);
        }
        let ms = start.elapsed().as_secs_f64() * 1000.0 / f64::from(iterations);
        total += ms;
        let r = report.expect("a function");
        println!(
            "{:40} {:>9.3} ms  {:8} {} obligations",
            p.display_path(d.id),
            ms,
            r.status.as_str(),
            r.verdicts.len()
        );
    }
    println!("{:40} {:>9.3} ms", "total", total);
}
