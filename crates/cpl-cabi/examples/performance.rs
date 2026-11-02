//! Reproducible in-process evaluator and warmed native-driver timings. Compile/startup is reported separately.
use cpl_eval::{on_eval_stack, EvalConfig, Interpreter, RunEngine, Value};
use cpl_project::{load_project_from_tree, LoadOptions, VirtualTree};
use serde_json::json;
use std::time::Instant;

fn main() {
    on_eval_stack(|| {
        let tree = VirtualTree {
            files: [
                (
                    "cpl.toml",
                    include_str!("../../../examples/parallel/cpl.toml"),
                ),
                (
                    "src/compute.cpl",
                    include_str!("../../../examples/parallel/src/compute.cpl"),
                ),
                ("src/kernels.cpl", include_str!("performance.cpl")),
            ]
            .into_iter()
            .map(|(path, source)| (path.to_owned(), source.as_bytes().to_vec()))
            .collect(),
        };
        let project = load_project_from_tree(
            &tree,
            cpl_diag::FileOrigin::Virtual,
            &LoadOptions::default(),
        )
        .unwrap();
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
        for (name, function, args) in [
            ("counter", "kernels.counter", vec![Value::int(100_000)]),
            ("calls", "kernels.calls", vec![Value::int(50_000)]),
            (
                "scoped_build",
                "kernels.scoped_build",
                vec![Value::int(5_000)],
            ),
            (
                "concat_build",
                "kernels.concat_build",
                vec![Value::int(5_000)],
            ),
            (
                "alternating_text",
                "kernels.alternating_text",
                vec![
                    Value::text(&"abcλ🙂".repeat(4000)),
                    Value::text(&"🙂λxyz".repeat(4000)),
                    Value::int(10_000),
                ],
            ),
            (
                "divide_loop",
                "kernels.divide_loop",
                vec![Value::int(123_456_789), Value::int(97), Value::int(20_000)],
            ),
            (
                "text_build",
                "kernels.text_build",
                vec![Value::text(&"abcλ🙂".repeat(8)), Value::int(20_000)],
            ),
            (
                "bytes_build",
                "kernels.bytes_build",
                vec![Value::bytes(&[0x81; 64]), Value::int(20_000)],
            ),
            (
                "text_build_lengths",
                "kernels.text_build_lengths",
                vec![Value::text(&"abcλ🙂".repeat(8)), Value::int(20_000)],
            ),
            (
                "text_contains_prefix",
                "kernels.text_contains",
                vec![
                    Value::text(&"a".repeat(100_000)),
                    Value::text(&format!("{}b", "a".repeat(1_000))),
                    Value::int(20),
                ],
            ),
            (
                "text_split_prefix",
                "kernels.text_split",
                vec![
                    Value::text(&"a".repeat(100_000)),
                    Value::text(&format!("{}b", "a".repeat(1_000))),
                    Value::int(20),
                ],
            ),
            (
                "text_join",
                "kernels.text_join",
                vec![
                    Value::list(vec![Value::text(&"abcλ🙂".repeat(8)); 5_000]),
                    Value::text(", "),
                    Value::int(30),
                ],
            ),
            (
                "text_join_single",
                "kernels.text_join",
                vec![
                    Value::list(vec![Value::text(&"abcλ🙂".repeat(8_000))]),
                    Value::text(", "),
                    Value::int(1_000),
                ],
            ),
            (
                "flatten_lists",
                "kernels.flatten_lists",
                vec![
                    Value::list(vec![Value::list(vec![Value::int(1); 8]); 5_000]),
                    Value::int(30),
                ],
            ),
            ("work", "work", vec![Value::int(100_000)]),
            ("build", "build", vec![Value::int(10_000)]),
            (
                "text_scan",
                "text_scan",
                vec![Value::text(&"abcλ🙂".repeat(4000)), Value::int(10_000)],
            ),
            (
                "batches",
                "batches",
                vec![Value::list((0..8).map(|_| Value::int(100_000)).collect())],
            ),
            (
                "contains_scan",
                "contains_scan",
                vec![
                    Value::list((0..10_000).map(Value::int).collect()),
                    Value::int(-1),
                    Value::int(100),
                ],
            ),
            (
                "equality_scan",
                "equality_scan",
                vec![
                    Value::list((0..10_000).map(Value::int).collect()),
                    Value::list((-1..9_999).map(Value::int).collect()),
                    Value::int(1_000),
                ],
            ),
            (
                "equality_equal",
                "equality_scan",
                vec![
                    Value::list((0..10_000).map(Value::int).collect()),
                    Value::list((0..10_000).map(Value::int).collect()),
                    Value::int(100),
                ],
            ),
        ] {
            let path = if function.contains('.') {
                function.to_owned()
            } else {
                format!("compute.{function}")
            };
            let id = p
                .lookup(&cpl_diag::SymbolPath::parse(&path).unwrap())
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
        let mut nodes =
            cpl_cabi::wire::to_nodes(&Value::list((0..10_000).map(Value::int).collect()));
        nodes
            .nodes
            .push(cpl_cabi::protocol::Node::Bytes(vec![0xab; 8192]));
        let encoded = serde_json::to_string(&nodes).unwrap();
        for encode in [true, false] {
            let mut samples = Vec::new();
            for _ in 0..5 {
                let start = Instant::now();
                for _ in 0..20 {
                    if encode {
                        std::hint::black_box(serde_json::to_string(&nodes).unwrap());
                    } else {
                        std::hint::black_box(
                            serde_json::from_str::<cpl_cabi::protocol::Nodes>(&encoded).unwrap(),
                        );
                    }
                }
                samples.push(start.elapsed().as_secs_f64() * 1000.0);
            }
            samples.sort_by(f64::total_cmp);
            rows.push(json!({"case":if encode {"wire_encode_nodes"} else {"wire_decode_nodes"},"engine":"wire","iterations":20,"median_ms":samples[2],"samples_ms":samples}));
        }
        println!("{}",serde_json::to_string_pretty(&json!({"parallelism":std::env::var("CPL_PARALLELISM").unwrap_or("auto".into()),"check_ms":check_ms,"native_startup_ms":startup_ms,"results":rows})).unwrap());
    });
}
