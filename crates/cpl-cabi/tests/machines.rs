use cpl_cabi::NativeSession;
use cpl_eval::{machine::MachineContext, on_eval_stack, EvalConfig, Interpreter, RunEngine};
use cpl_project::{load_project, LoadOptions};
use serde_json::{json, Value as Json};
use std::path::Path;

fn normalized(d: cpl_diag::Diagnostic, p: &cpl_hir::Program) -> Json {
    let mut json = serde_json::to_value(d.to_json(p.source_map())).unwrap();
    cpl_cabi::compare::normalize(&mut json);
    json
}

#[test]
fn machines_invariants_boundaries_and_seeded_events_agree_under_sanitizers() {
    if cpl_rt::testing::find_cc().is_none() {
        assert_ne!(std::env::var("CPL_REQUIRE_CC").as_deref(), Ok("1"));
        return;
    }
    on_eval_stack(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/projects/machines");
        let project = load_project(&root, &LoadOptions::default()).unwrap();
        let checked = cpl_sema::check(&project);
        assert!(!checked.has_errors(), "{:#?}", checked.diagnostics);
        let p = &checked.program;
        let cfg = EvalConfig {
            check_contracts: false,
            ..EvalConfig::default()
        };
        let dcfg = cpl_emit_c::package::DriverConfig {
            cache_dir: Path::new(env!("CARGO_TARGET_TMPDIR")).join("machine-cache"),
            profile: cpl_emit_c::cc::Profile::Debug,
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
        native.set_request_timeout(Some(std::time::Duration::from_secs(30)));
        let mut eval = Interpreter::new(p, cfg.clone());
        let lookup = |name: &str| {
            p.lookup(&cpl_diag::SymbolPath::parse(&format!("m.{name}")).unwrap())
                .unwrap()
        };
        let id = lookup("reduce");
        let state = |n: i64| json!({"$type":"m.App","count":{"$type":"m.Count","n":n}});
        let mut a = MachineContext::new(p, id, &state(50), &cfg).unwrap();
        let mut b = MachineContext::new(p, id, &state(50), &cfg).unwrap();
        let mut replay = MachineContext::new(p, id, &state(50), &cfg).unwrap();
        let mut seed = 0x4d41_4348_494e_4553_u64;
        let mut count = 50_i64;
        for i in 0..2_000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let delta = (seed % 81) as i64 - 40;
            let event = if i % 47 == 0 {
                json!({"$type":"m.Event","$variant":"Divide","$payload":[0]})
            } else {
                json!({"$type":"m.Event","$variant":"Add","$payload":[delta]})
            };
            match (
                a.dispatch(&mut eval, &event),
                b.dispatch(&mut native, &event),
            ) {
                (Ok(ar), Ok(br)) => {
                    assert_ne!(i % 47, 0);
                    count += delta;
                    assert!((0..=100).contains(&count));
                    assert_eq!(ar, br);
                    replay.replay(&mut native, &ar).unwrap();
                }
                (Err(ad), Err(bd)) => {
                    assert!(i % 47 == 0 || !(0..=100).contains(&(count + delta)));
                    assert_eq!(normalized(ad, p), normalized(bd, p), "event {i}: {event}");
                }
                other => panic!("engine disagreement at event {i}: {other:?}"),
            }
            assert_eq!(a.snapshot(), state(count));
            assert_eq!(b.snapshot(), state(count));
            assert_eq!(replay.snapshot(), state(count));
        }
        // Typed host values can be malicious even when they have the right shape. Neither engine may trust them.
        for (name, args, must_fail) in [
            ("make", vec![json!(-1)], true),
            ("make", vec![json!(25)], false),
            (
                "spread",
                vec![json!({"$type":"m.Count","n":3}), json!(101)],
                true,
            ),
            ("nested", vec![json!([{"$type":"m.Count","n":-1}])], true),
            ("bag", vec![json!([])], true),
            ("bag", vec![json!([1])], false),
            ("bag_len", vec![json!({"$type":"m.Bag","xs":[]})], true),
        ] {
            let id = lookup(name);
            let f = p.def(id).kind.as_fn().unwrap();
            let values = args
                .iter()
                .zip(&f.params)
                .map(|(j, par)| cpl_eval::value_from_json(p, j, &par.ty).unwrap())
                .collect::<Vec<_>>();
            let render = |r: Result<cpl_eval::Value, cpl_eval::Fault>| match r {
                Ok(v) => {
                    assert!(!must_fail, "{name} accepted invalid input");
                    json!({"value":cpl_eval::value_to_json(p,&v,&f.ret)})
                }
                Err(f) => {
                    assert!(must_fail, "{name}: {f:?}");
                    normalized(f.to_diagnostic(p), p)
                }
            };
            assert_eq!(
                render(eval.call_checked(id, values.clone(), vec![])),
                render(native.call_checked(id, values, vec![])),
                "{name}"
            );
        }
        let report = native.shutdown().unwrap();
        assert_eq!(
            (
                report.live_handles,
                report.live_caps,
                report.live_objects,
                report.live_buffers
            ),
            (0, 0, 0, 0)
        );
        assert!(report.exit_status.success());
    });
}
