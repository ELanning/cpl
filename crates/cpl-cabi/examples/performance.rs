//! Reproducible in-process evaluator and warmed native-driver timings. Compile/startup is reported separately.
use cpl_eval::{on_eval_stack, EvalConfig, Interpreter, RunEngine, Value};
use cpl_project::{load_project, LoadOptions};
use serde_json::json;
use std::{path::Path, time::Instant};

fn main() {
    on_eval_stack(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/parallel");
        let project = load_project(&root, &LoadOptions::default()).unwrap();
        let start = Instant::now();
        let checked = cpl_sema::check(&project);
        let check_ms = start.elapsed().as_secs_f64() * 1000.0;
        assert!(!checked.has_errors(), "{:?}", checked.diagnostics);
        let p = &checked.program;
        let cfg = EvalConfig::default();
        let dc = cpl_emit_c::package::DriverConfig {
            cache_dir: std::env::temp_dir().join("cpl-performance-native"),
            profile: cpl_emit_c::cc::Profile::Optimized,
            sanitize: vec![],
            coverage: false,
            cc: None,
            runtime: cpl_rt::RuntimeSource::Embedded,
            werror: true,
        };
        let start = Instant::now();
        let mut native = cpl_cabi::NativeSession::start(
            p,
            &project,
            checked.interop.as_ref().unwrap(),
            &cfg,
            &dc,
        )
        .unwrap();
        let startup_ms = start.elapsed().as_secs_f64() * 1000.0;
        let mut eval = Interpreter::new(p, cfg);
        let mut rows = Vec::new();
        for (name, args) in [
            ("work", vec![Value::int(100_000)]),
            ("build", vec![Value::int(10_000)]),
            (
                "text_scan",
                vec![Value::text(&"abcλ🙂".repeat(4000)), Value::int(10_000)],
            ),
            (
                "batches",
                vec![Value::list((0..8).map(|_| Value::int(100_000)).collect())],
            ),
        ] {
            let id = p
                .lookup(&cpl_diag::SymbolPath::parse(&format!("compute.{name}")).unwrap())
                .unwrap();
            let expected = eval.call_checked(id, args.clone(), vec![]).unwrap();
            assert_eq!(
                native.call_checked(id, args.clone(), vec![]).unwrap(),
                expected
            );
            for (engine, runner) in [
                ("eval", &mut eval as &mut dyn RunEngine),
                ("native", &mut native as &mut dyn RunEngine),
            ] {
                let mut samples = Vec::new();
                for _ in 0..5 {
                    let start = Instant::now();
                    let value = runner.call_checked(id, args.clone(), vec![]).unwrap();
                    samples.push(start.elapsed().as_secs_f64() * 1000.0);
                    assert_eq!(value, expected);
                }
                samples.sort_by(f64::total_cmp);
                rows.push(json!({"case":name,"engine":engine,"median_ms":samples[2],"samples_ms":samples}));
            }
        }
        let report = native.shutdown().unwrap();
        assert_eq!(
            (
                report.live_objects,
                report.live_handles,
                report.live_caps,
                report.live_buffers
            ),
            (0, 0, 0, 0)
        );
        assert!(report.exit_status.success());
        println!("{}",serde_json::to_string_pretty(&json!({"parallelism":std::env::var("CPL_PARALLELISM").unwrap_or("auto".into()),"check_ms":check_ms,"native_startup_ms":startup_ms,"results":rows})).unwrap());
    });
}
