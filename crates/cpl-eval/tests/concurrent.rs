mod common;
use common::{check_source, def};
use cpl_eval::concurrent::{with_machine, SubmitError};
use cpl_eval::{machine::MachineContext, on_eval_stack, EvalConfig, Interpreter};
use serde_json::json;

const SOURCE: &str = "module m\nprogram App { n: Int }\nenum Event {\n Add(Int)\n}\nmachine reduce(s: App, e: Event) -> App { match e { Add(n) => App { n: s.n + n } } }\n";

#[test]
fn concurrent_producers_commit_once_and_replay_in_sequence_order() {
    on_eval_stack(|| {
        let p = check_source(SOURCE);
        let id = def(&p, "m.reduce");
        let cfg = EvalConfig::default();
        let initial = json!({"$type":"m.App","n":0});
        let (mut records, escaped) =
            with_machine(&p, id, initial.clone(), cfg.clone(), 8, |mailbox| {
                let all = std::thread::scope(|scope| {
                    let jobs: Vec<_> = (0..8)
                        .map(|_| {
                            let mailbox = mailbox.clone();
                            scope.spawn(move || {
                                let mut records = Vec::new();
                                for _ in 0..100 {
                                    let mut event =
                                        json!({"$type":"m.Event","$variant":"Add","$payload":[1]});
                                    let reply = loop {
                                        match mailbox.try_dispatch(event) {
                                            Ok(reply) => break reply,
                                            Err(SubmitError::Full(e)) => {
                                                event = e;
                                                std::thread::yield_now();
                                            }
                                            Err(SubmitError::Closed(_)) => {
                                                panic!("closed before scope exit")
                                            }
                                        }
                                    };
                                    records.push(reply.wait().unwrap());
                                }
                                records
                            })
                        })
                        .collect();
                    jobs.into_iter()
                        .flat_map(|j| j.join().unwrap())
                        .collect::<Vec<_>>()
                });
                (all, mailbox)
            })
            .unwrap();
        assert!(matches!(
            escaped.try_dispatch(json!(null)),
            Err(SubmitError::Closed(_))
        ));
        records.sort_by_key(|r| r.sequence);
        let mut context = MachineContext::new(&p, id, &initial, &cfg).unwrap();
        let mut engine = Interpreter::new(&p, cfg);
        for (i, r) in records.iter().enumerate() {
            assert_eq!(r.sequence, i as u64 + 1);
            context.replay(&mut engine, r).unwrap();
        }
        assert_eq!(context.snapshot(), json!({"$type":"m.App","n":800}));
    });
}

#[test]
fn invalid_events_do_not_advance_the_mailbox_state_and_capacity_is_checked() {
    let p = check_source(SOURCE);
    let id = def(&p, "m.reduce");
    let initial = json!({"$type":"m.App","n":0});
    assert!(with_machine(&p, id, initial.clone(), EvalConfig::default(), 0, |_| ()).is_err());
    with_machine(&p, id, initial, EvalConfig::default(), 1, |mailbox| {
        let bad = mailbox
            .try_dispatch(json!(null))
            .unwrap()
            .wait()
            .unwrap_err();
        assert_eq!(bad.data["state_unchanged"], true);
        let good = mailbox
            .try_dispatch(json!({"$type":"m.Event","$variant":"Add","$payload":[2]}))
            .unwrap()
            .wait()
            .unwrap();
        assert_eq!(good.sequence, 1);
        assert_eq!(good.after["n"], 2);
    })
    .unwrap();
}

#[test]
fn scope_exit_drains_accepted_events_even_when_replies_drop_or_the_caller_panics() {
    let p = check_source(SOURCE);
    let id = def(&p, "m.reduce");
    for panic_on_exit in [false, true] {
        let mut last = None;
        let mut escaped = None;
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_machine(
                &p,
                id,
                json!({"$type":"m.App","n":0}),
                EvalConfig::default(),
                32,
                |mailbox| {
                    for _ in 0..16 {
                        // Dropping a reply does not retract the accepted event.
                        drop(
                            mailbox
                                .try_dispatch(json!({
                                    "$type":"m.Event","$variant":"Add","$payload":[1]
                                }))
                                .unwrap(),
                        );
                    }
                    last = Some(
                        mailbox
                            .try_dispatch(json!({
                                "$type":"m.Event","$variant":"Add","$payload":[1]
                            }))
                            .unwrap(),
                    );
                    escaped = Some(mailbox);
                    assert!(!panic_on_exit, "intentional host panic");
                },
            )
            .unwrap();
        }));
        assert_eq!(outcome.is_err(), panic_on_exit);
        let record = last
            .unwrap()
            .try_recv()
            .unwrap()
            .expect("joined worker replied")
            .unwrap();
        assert_eq!(record.sequence, 17);
        assert_eq!(record.after["n"], 17);
        assert!(matches!(
            escaped.unwrap().try_dispatch(json!(null)),
            Err(SubmitError::Closed(_))
        ));
    }
}
