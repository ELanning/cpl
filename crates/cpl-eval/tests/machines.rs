mod common;

use common::{check_source, def};
use cpl_eval::{machine::MachineContext, on_eval_stack, EvalConfig, Interpreter};
use serde_json::{json, Value};

const SOURCE: &str = r#"module m
program App { count: Int } where self.count >= 0 and self.count <= 100
enum Event {
    Add(Int)
    Divide(Int)
    Reset
}
initial fn start() -> App { App { count: 0 } }
machine reduce(state: App, event: Event) -> App {
    match event {
        Add(n) => App { count: state.count + n }
        Divide(n) => App { count: state.count / n }
        Reset => App { count: 0 }
    }
}
"#;

fn state(n: i64) -> Value {
    json!({"$type":"m.App","count":n})
}
fn add(n: i64) -> Value {
    json!({"$type":"m.Event","$variant":"Add","$payload":[n]})
}

#[test]
fn failed_validation_and_crashes_never_commit_and_receipts_detect_tampering() {
    on_eval_stack(|| {
        let p = check_source(SOURCE);
        let cfg = EvalConfig {
            check_contracts: false,
            ..EvalConfig::default()
        };
        let mut engine = Interpreter::new(&p, cfg.clone());
        let id = def(&p, "m.reduce");
        assert!(MachineContext::new(&p, id, &state(-1), &cfg).is_err());
        let mut ctx = MachineContext::new(&p, id, &state(0), &cfg).unwrap();
        let record = ctx.dispatch(&mut engine, &add(3)).unwrap();
        assert_eq!(ctx.snapshot(), state(3));
        for event in [
            add(-4),
            json!({"$type":"m.Event","$variant":"Divide","$payload":[0]}),
            json!({"$type":"m.Event","$variant":"Missing"}),
            json!(null),
        ] {
            let fault = ctx.dispatch(&mut engine, &event).unwrap_err();
            assert_eq!(fault.data["transition_sequence"], 2);
            assert_eq!(fault.data["state_unchanged"], true);
            assert_eq!(ctx.snapshot(), state(3));
            assert_eq!(ctx.sequence(), 1);
        }
        let mut replay = MachineContext::new(&p, id, &state(0), &cfg).unwrap();
        for field in [
            "after",
            "commands",
            "event",
            "machine",
            "program_digest",
            "sequence",
            "before",
        ] {
            let mut bad = serde_json::to_value(&record).unwrap();
            bad[field] = match field {
                "sequence" => json!(2),
                "before" | "after" => state(9),
                "commands" => json!([1]),
                "event" => add(4),
                _ => json!("forged"),
            };
            let bad = serde_json::from_value(bad).unwrap();
            assert!(
                replay.replay(&mut engine, &bad).is_err(),
                "accepted forged {field}"
            );
            assert_eq!(replay.snapshot(), state(0));
            assert_eq!(replay.sequence(), 0);
        }
        replay.replay(&mut engine, &record).unwrap();
        assert!(
            replay.replay(&mut engine, &record).is_err(),
            "duplicate receipt"
        );
        assert_eq!(replay.snapshot(), state(3));
    });
}

#[test]
fn seeded_event_streams_match_an_independent_model_and_replay() {
    on_eval_stack(|| {
        let p = check_source(SOURCE);
        let cfg = EvalConfig::default();
        let id = def(&p, "m.reduce");
        let mut engine = Interpreter::new(&p, cfg.clone());
        let mut context = MachineContext::new(&p, id, &state(50), &cfg).unwrap();
        let mut replay = MachineContext::new(&p, id, &state(50), &cfg).unwrap();
        let mut seed = 0x6ae7_df1d_89e4_83e8_u64;
        let mut count = 50_i64;
        let mut sequence = 0;
        for _ in 0..10_000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let change = (seed % 61) as i64 - 30;
            let next = count + change;
            match context.dispatch(&mut engine, &add(change)) {
                Ok(record) => {
                    assert!((0..=100).contains(&next));
                    count = next;
                    sequence += 1;
                    replay.replay(&mut engine, &record).unwrap();
                }
                Err(_) => assert!(!(0..=100).contains(&next)),
            }
            assert_eq!(context.snapshot(), state(count));
            assert_eq!(replay.snapshot(), state(count));
            assert_eq!(context.sequence(), sequence);
        }
    });
}

#[test]
fn child_commands_and_stale_replies_have_explicit_semantics() {
    on_eval_stack(|| {
        let p = common::example("state_machine");
        let cfg = EvalConfig::default();
        let start = common::run_json(&p, "app.main.start", &[]);
        let mut ctx = MachineContext::new(
            &p,
            def(&p, "app.main.reduce"),
            &common::value(&p, &start),
            &cfg,
        )
        .unwrap();
        let mut engine = Interpreter::new(&p, cfg);
        let event = |variant: &str, payload: Value| json!({"$type":"app.main.Event","$variant":variant,"$payload":payload});
        let record = ctx
            .dispatch(&mut engine, &event("Start", json!([7, "/example"])))
            .unwrap();
        assert_eq!(record.commands.len(), 1);
        assert_eq!(record.commands[0]["$variant"], "HttpGet");
        let pending = ctx.snapshot();
        assert!(ctx
            .dispatch(&mut engine, &event("Received", json!([6, "stale"])))
            .unwrap()
            .commands
            .is_empty());
        assert_eq!(ctx.snapshot(), pending);
        ctx.dispatch(&mut engine, &event("Received", json!([7, "done"])))
            .unwrap();
        assert_eq!(ctx.snapshot()["download"]["$variant"], "Done");
    });
}

#[test]
fn replay_identity_includes_import_resolution() {
    on_eval_stack(|| {
        let source = SOURCE
            .replace("module m", "module m\nuse a.{number}")
            .replace("count: 0", "count: number()");
        let other = source.replace("use a.{number}", "use b.{number}");
        let a = "module a\npub pure fn number() -> Int { 0 }\n";
        let b = "module b\npub pure fn number() -> Int { 0 }\n";
        let p = common::check_files(&[("src/m.cpl", &source), ("src/a.cpl", a), ("src/b.cpl", b)]);
        let q = common::check_files(&[("src/m.cpl", &other), ("src/a.cpl", a), ("src/b.cpl", b)]);
        let cfg = EvalConfig::default();
        let one = MachineContext::new(&p, def(&p, "m.reduce"), &state(0), &cfg).unwrap();
        let two = MachineContext::new(&q, def(&q, "m.reduce"), &state(0), &cfg).unwrap();
        assert_ne!(one.program_digest(), two.program_digest());
    });
}

#[test]
fn transition_resource_exhaustion_has_a_stack_and_cannot_commit() {
    on_eval_stack(|| {
        let source = SOURCE.replace("state.count + n", "recurse(n)")
            + "\npure fn recurse(n: Int) -> Int { recurse(n + 1) }\n";
        let p = check_source(&source);
        let cfg = EvalConfig {
            max_call_depth: 8,
            ..EvalConfig::default()
        };
        let mut engine = Interpreter::new(&p, cfg.clone());
        let mut context = MachineContext::new(&p, def(&p, "m.reduce"), &state(1), &cfg).unwrap();
        let d = context.dispatch(&mut engine, &add(1)).unwrap_err();
        assert_eq!(d.code.0, "resource_exhaustion");
        assert!(d.span.is_some());
        assert!(!d.backtrace.is_empty(), "{d:?}");
        assert_eq!(context.snapshot(), state(1));
        assert_eq!(context.sequence(), 0);
    });
}
