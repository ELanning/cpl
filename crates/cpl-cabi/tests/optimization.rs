use cpl_cabi::NativeSession;
use cpl_diag::{FileOrigin, SymbolPath};
use cpl_eval::{on_eval_stack, EvalConfig, Interpreter, RunEngine, Value};
use cpl_project::{load_project_from_tree, LoadOptions, VirtualTree};
use serde_json::{json, Value as Json};
use std::path::Path;

#[test]
fn optimized_scopes_and_owned_buffers_preserve_values_and_faults() {
    on_eval_stack(|| {
        let tree = VirtualTree {
            files: [
                (
                    "cpl.toml",
                    "[project]\nname = \"optimization\"\nedition = \"0.1\"\n[interop]\ntargets = [\"c\"]\n",
                ),
                ("src/optimization.cpl", include_str!("optimization.cpl")),
            ]
            .into_iter()
            .map(|(name, source)| (name.to_owned(), source.as_bytes().to_vec()))
            .collect(),
        };
        let project =
            load_project_from_tree(&tree, FileOrigin::Virtual, &LoadOptions::default()).unwrap();
        let checked = cpl_sema::check(&project);
        assert!(!checked.has_errors(), "{:?}", checked.diagnostics);
        let p = &checked.program;
        let cfg = EvalConfig::default();
        for profile in [
            cpl_emit_c::cc::Profile::Debug,
            cpl_emit_c::cc::Profile::Optimized,
        ] {
            let dc = cpl_emit_c::package::DriverConfig {
                cache_dir: Path::new(env!("CARGO_TARGET_TMPDIR")).join("optimization-cache"),
                profile,
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
                NativeSession::start(p, &project, checked.interop.as_ref().unwrap(), &cfg, &dc)
                    .unwrap();
            let mut eval = Interpreter::new(p, cfg.clone());
            let mut cases = vec![
                (
                    "text_build_lengths",
                    vec![Value::text(&"abcλ🙂".repeat(8)), Value::int(20_000)],
                    Some(json!(40i64 * 20_000 * 20_001 / 2)),
                ),
                (
                    "aliases",
                    vec![],
                    Some(json!([[1, 2], [1, 2, 1, 2], [1, 2, 1, 2, 4, 7]])),
                ),
                ("escaped_values", vec![], Some(json!([1, 2, 3, 4]))),
                ("early_return", vec![Value::int(4)], Some(json!([4, 5]))),
                ("early_return", vec![Value::int(-4)], Some(json!([-3, -4]))),
                ("propagate", vec![Value::int(4)], None),
                ("propagate", vec![Value::int(0)], None),
                ("fault_bindings", vec![], None),
                ("builtin_order", vec![], Some(json!([1, 2, 3, 9]))),
                ("builtin_fault", vec![Value::int(-1)], None),
                ("text_fault", vec![], None),
                (
                    "text_aliases",
                    vec![],
                    Some(json!(["λ\0🙂", "λ\0🙂λ\0🙂", "λ\0🙂λ\0🙂x"])),
                ),
                (
                    "bytes_aliases",
                    vec![],
                    Some(json!([
                        {"$type":"Bytes", "hex":"0080ff"},
                        {"$type":"Bytes", "hex":"0080ff0080ff"},
                        {"$type":"Bytes", "hex":"0080ff0080ff03"}
                    ])),
                ),
            ];
            for n in [0, 1, 7, 8, 9, 31, 32, 33, 4096] {
                cases.push((
                    "build",
                    vec![Value::int(n)],
                    Some(json!((0..n).collect::<Vec<_>>())),
                ));
                cases.push((
                    "text_build",
                    vec![Value::text("λ\0🙂"), Value::int(n)],
                    Some(json!("λ\0🙂".repeat(n as usize))),
                ));
                cases.push((
                    "bytes_build",
                    vec![Value::bytes(&[0, 128, 255]), Value::int(n)],
                    Some(json!({"$type":"Bytes", "hex":"0080ff".repeat(n as usize)})),
                ));
            }
            let boundary: num_bigint::BigInt = num_bigint::BigInt::from(1) << 62;
            let huge: num_bigint::BigInt = num_bigint::BigInt::from(1) << 256;
            for a in [
                0.into(),
                7.into(),
                (-7).into(),
                -&boundary,
                boundary.clone() - 1,
                huge,
            ] {
                for b in [
                    0.into(),
                    1.into(),
                    (-1).into(),
                    7.into(),
                    (-7).into(),
                    boundary.clone() + 1,
                ] {
                    let expected = cpl_rtspec::arith::div_rem(&a, &b).map(|(q, r)| {
                        cpl_eval::json::value_to_json_untyped(
                            p,
                            &Value::list(vec![Value::int(q), Value::int(r)]),
                        )
                    });
                    cases.push((
                        "division",
                        vec![Value::int(a.clone()), Value::int(b)],
                        expected,
                    ));
                }
            }
            for (haystack, needle) in [
                ("".to_owned(), "x".to_owned()),
                ("a".repeat(10_000), format!("{}b", "a".repeat(1_000))),
                (
                    format!("{}b", "a".repeat(10_000)),
                    format!("{}b", "a".repeat(1_000)),
                ),
                ("a\0λ🙂a\0λ🙂".to_owned(), "\0λ".to_owned()),
                ("aaaaa".to_owned(), "aa".to_owned()),
                ("λ🙂λ🙂".to_owned(), "🙂λ".to_owned()),
            ] {
                let expected = json!([
                    u32::from(haystack.contains(&needle)),
                    haystack.split(&needle).count()
                ]);
                cases.push((
                    "search",
                    vec![Value::text(&haystack), Value::text(&needle)],
                    Some(expected),
                ));
            }
            let huge: num_bigint::BigInt = (num_bigint::BigInt::from(1) << 256) + 17;
            for seed in [num_bigint::BigInt::from(0), 42.into(), -&huge, huge] {
                for step in [num_bigint::BigInt::from(-23), 0.into(), &seed + 1] {
                    let a: num_bigint::BigInt = &seed + &step - 1;
                    let b = &step + &a;
                    let product = &a * &b;
                    let result = Value::list(
                        [
                            seed.clone(),
                            step.clone(),
                            seed.clone(),
                            product.clone(),
                            b,
                            (product + 2) * 2,
                        ]
                        .into_iter()
                        .map(Value::int)
                        .collect(),
                    );
                    cases.push((
                        "arithmetic",
                        vec![Value::int(seed.clone()), Value::int(step)],
                        Some(cpl_eval::json::value_to_json_untyped(p, &result)),
                    ));
                }
            }
            // Reuse one interpreter across calls, evicting cached texts and reusing environments.
            for i in 0..32 {
                let a = "λ🙂".repeat(i);
                let b = "abc\0é".repeat(32 - i);
                cases.push((
                    "text_lengths",
                    vec![Value::text(&a), Value::text(&b)],
                    Some(json!(20 * (2 * i + 5 * (32 - i)))),
                ));
            }
            for (name, args, expected) in cases {
                let id = p
                    .lookup(&SymbolPath::parse(&format!("optimization.{name}")).unwrap())
                    .unwrap();
                let render = |result: Result<Value, cpl_eval::Fault>| -> Json {
                    match result {
                        Ok(value) => {
                            json!({"value": cpl_eval::value_to_json(p, &value, &p.def(id).kind.as_fn().unwrap().ret)})
                        }
                        Err(fault) => {
                            let mut value = serde_json::to_value(
                                fault.to_diagnostic(p).to_json(p.source_map()),
                            )
                            .unwrap();
                            cpl_cabi::compare::normalize(&mut value);
                            value
                        }
                    }
                };
                let reference = render(eval.call_checked(id, args.clone(), vec![]));
                if let Some(expected) = expected {
                    assert_eq!(reference["value"], expected, "{name}");
                }
                assert_eq!(
                    render(native.call_checked(id, args, vec![])),
                    reference,
                    "{name} ({profile:?})"
                );
            }
            // Host-side copy-on-write mutation detaches a weak cache identity, so the next call must recount it.
            let lengths = p
                .lookup(&SymbolPath::parse("optimization.text_lengths").unwrap())
                .unwrap();
            let mut text = Value::text("λ");
            assert_eq!(
                eval.call_checked(lengths, vec![text.clone(), Value::text("")], vec![])
                    .unwrap(),
                Value::int(20)
            );
            if let Value::Text(buffer) = &mut text {
                std::rc::Rc::make_mut(buffer).push_str("🙂\0");
            }
            assert_eq!(
                eval.call_checked(lengths, vec![text, Value::text("")], vec![])
                    .unwrap(),
                Value::int(60)
            );
            let report = native.shutdown().unwrap();
            assert!(report.exit_status.success());
            assert_eq!(
                (
                    report.live_objects,
                    report.live_handles,
                    report.live_caps,
                    report.live_buffers
                ),
                (0, 0, 0, 0)
            );
        }
    });
}
