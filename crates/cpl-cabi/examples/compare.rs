//! The kernels of `bench/cpl` on the native engine (the optimized C driver), with the sizes of
//! `bench/rust` and `bench/go`, so the three can be compared: prints `name n result ms` per
//! kernel, the best of three warmed calls. Arguments name kernels to run (default: all).
//! Timings include the driver protocol's round trip (tens of microseconds).
use cpl_eval::{on_eval_stack, EvalConfig, RunEngine, Value};
use std::time::Instant;

/// The kernels and their sizes (as in `bench/rust/src/main.rs`).
pub const KERNELS: [(&str, i64); 9] = [
    ("arith", 50_000_000),
    ("fib", 32),
    ("collatz", 300_000),
    ("list_index", 5_000_000),
    ("list_walk", 5_000_000),
    ("sorting", 500_000),
    ("structs", 20_000_000),
    ("text", 1_000_000),
    ("bigint", 5_000),
];

fn main() {
    let only: Vec<String> = std::env::args().skip(1).collect();
    on_eval_stack(|| {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bench/cpl");
        let project = cpl_project::load_project(&root, &cpl_project::LoadOptions::default())
            .expect("bench/cpl loads");
        let checked = cpl_sema::check(&project);
        assert!(!checked.has_errors(), "{:?}", checked.diagnostics);
        let p = &checked.program;
        let cfg = EvalConfig::default();
        let dc = cpl_emit_c::package::DriverConfig {
            cache_dir: std::env::temp_dir().join("cpl-compare-native"),
            profile: cpl_emit_c::cc::Profile::Optimized,
            sanitize: vec![],
            coverage: false,
            cc: None,
            runtime: cpl_rt::RuntimeSource::Embedded,
            werror: true,
        };
        let mut native = cpl_cabi::NativeSession::start(
            p,
            &project,
            checked.interop.as_ref().expect("interop"),
            &cfg,
            &dc,
        )
        .expect("the native driver starts");
        for (name, n) in KERNELS {
            if !only.is_empty() && !only.iter().any(|o| o == name) {
                continue;
            }
            let id = p
                .lookup(&cpl_diag::SymbolPath::parse(&format!("bench.{name}")).unwrap())
                .unwrap();
            let mut best = f64::MAX;
            let mut result = String::new();
            for _ in 0..3 {
                let start = Instant::now();
                let value = native
                    .call_checked(id, vec![Value::int(n)], vec![])
                    .unwrap_or_else(|e| panic!("{name}: {e:?}"));
                best = best.min(start.elapsed().as_secs_f64() * 1000.0);
                result = match value.as_int() {
                    Some(i) => i.to_string(),
                    None => format!("{value:?}"),
                };
            }
            println!("{name} {n} {result} {best:.2}");
        }
        native.shutdown().expect("the driver shuts down");
    });
}
