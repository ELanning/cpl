use cpl_cabi::NativeSession;
use cpl_eval::{on_eval_stack, EvalConfig, Interpreter, RunEngine, Value};
use cpl_project::{load_project, LoadOptions};
use serde_json::{json, Value as Json};
use std::path::Path;

#[test]
fn parallel_maps_match_the_interpreter_under_sanitizers() {
    on_eval_stack(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/parallel");
        let project = load_project(&root, &LoadOptions::default()).unwrap();
        let checked = cpl_sema::check(&project);
        assert!(!checked.has_errors(), "{:#?}", checked.diagnostics);
        let p = &checked.program;
        let cfg = EvalConfig {
            max_call_depth: 32,
            time_budget: Some(std::time::Duration::from_secs(5)),
            ..EvalConfig::default()
        };
        let dcfg = cpl_emit_c::package::DriverConfig {
            cache_dir: Path::new(env!("CARGO_TARGET_TMPDIR")).join("parallel-cache"),
            profile: cpl_emit_c::cc::Profile::Optimized,
            sanitize: vec![
                cpl_emit_c::cc::Sanitizer::Address,
                cpl_emit_c::cc::Sanitizer::Undefined,
            ],
            coverage: false,
            cc: None,
            runtime: cpl_rt::RuntimeSource::Embedded,
            werror: true,
        };
        let mut native =
            NativeSession::start(p, &project, checked.interop.as_ref().unwrap(), &cfg, &dcfg)
                .unwrap();
        let mut eval = Interpreter::new(p, cfg);
        let list = |xs: &[i64]| Value::list(xs.iter().map(|x| Value::int(*x)).collect());
        for (name, args) in [
            ("batches", vec![list(&[0, 1, 8, 0, 20, 2, 3, 4, 5, 6, 7])]),
            ("batches", vec![list(&[])]),
            ("captured", vec![list(&[3, 0, -1, 2]), Value::int(7)]),
            ("nested", vec![list(&[1, 2, 3, 4])]),
            ("failures", vec![list(&[1, 0, 0, 4])]),
            ("depths", vec![list(&[1, 3, 100, 1000])]),
            ("contracts", vec![list(&[1, 0, -1, 2])]),
            ("snapshots", vec![Value::int(1000)]),
            ("alias_argument", vec![]),
            (
                "numbers",
                vec![Value::int((1_i64 << 62) - 1), Value::int(2)],
            ),
            ("numbers", vec![Value::int(-(1_i64 << 62)), Value::int(-2)]),
            ("numbers", vec![Value::int(17), Value::int(-3)]),
            ("fail_preserved", vec![Value::int(0)]),
        ] {
            let id = p
                .lookup(&cpl_diag::SymbolPath::parse(&format!("compute.{name}")).unwrap())
                .unwrap();
            let render = |r: Result<Value, cpl_eval::Fault>| -> Json {
                match r {
                    Ok(v) => {
                        json!({"value":cpl_eval::value_to_json(p,&v,&p.def(id).kind.as_fn().unwrap().ret)})
                    }
                    Err(f) => {
                        let mut j =
                            serde_json::to_value(f.to_diagnostic(p).to_json(p.source_map()))
                                .unwrap();
                        cpl_cabi::compare::normalize(&mut j);
                        j
                    }
                }
            };
            for _ in 0..20 {
                assert_eq!(
                    render(eval.call_checked(id, args.clone(), vec![])),
                    render(native.call_checked(id, args.clone(), vec![])),
                    "{name}"
                );
            }
        }
        let number_id = p
            .lookup(&cpl_diag::SymbolPath::parse("compute.numbers").unwrap())
            .unwrap();
        let mut seed = 0x5041_5241_4c4c_454c_u64;
        for i in 0..2_000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let mut a = num_bigint::BigInt::from(seed as i64);
            let mut b = num_bigint::BigInt::from((seed.rotate_left(29) as i64) | 1);
            if i % 7 == 0 {
                a <<= 80;
            }
            if i % 11 == 0 {
                b <<= 64;
            }
            let args = vec![Value::Int(a), Value::Int(b)];
            assert_eq!(
                eval.call_checked(number_id, args.clone(), vec![]).unwrap(),
                native.call_checked(number_id, args, vec![]).unwrap(),
                "seeded arithmetic case {i}"
            );
        }
        let cancel_id = p
            .lookup(&cpl_diag::SymbolPath::parse("compute.cancel_later").unwrap())
            .unwrap();
        let start = std::time::Instant::now();
        for engine in [
            &mut eval as &mut dyn RunEngine,
            &mut native as &mut dyn RunEngine,
        ] {
            let fault = engine
                .call_checked(cancel_id, vec![list(&[0, 1, 1, 1, 1, 1, 1, 1])], vec![])
                .unwrap_err();
            assert_eq!(fault.to_diagnostic(p).code.0, "division_by_zero");
        }
        assert!(
            start.elapsed() < std::time::Duration::from_secs(3),
            "later tasks must cancel before their five-second budget"
        );
        let deadline_id = p
            .lookup(&cpl_diag::SymbolPath::parse("compute.deadlines").unwrap())
            .unwrap();
        let bounded = EvalConfig {
            time_budget: Some(std::time::Duration::from_millis(50)),
            ..EvalConfig::default()
        };
        native.reconfigure(&bounded).unwrap();
        let mut timed = Interpreter::new(p, bounded);
        let start = std::time::Instant::now();
        for engine in [
            &mut timed as &mut dyn RunEngine,
            &mut native as &mut dyn RunEngine,
        ] {
            let fault = engine
                .call_checked(deadline_id, vec![list(&[1; 16])], vec![])
                .unwrap_err();
            assert_eq!(fault.code(), "resource_exhaustion");
            assert_eq!(fault.data["resource"], "time");
            assert_eq!(fault.data["limit"], 50);
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(3));
        let r = native.shutdown().unwrap();
        assert_eq!(
            (r.live_handles, r.live_caps, r.live_objects, r.live_buffers),
            (0, 0, 0, 0)
        );
        assert!(r.exit_status.success());
    });
}
