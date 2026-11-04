use cpl_evolve::{concurrent::*, verify::VerifyConfig, Snapshot};
use cpl_project::{LoadOptions, VirtualTree};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

fn snapshot() -> Snapshot {
    Snapshot::new(
        VirtualTree {
            files: BTreeMap::from([
                (
                    "cpl.toml".into(),
                    b"[project]\nname=\"effects\"\nedition=\"0.1\"\n".to_vec(),
                ),
                (
                    "src/m.cpl".into(),
                    br#"module m
interface Store {
    fn read() -> Int
    fn write(value: Int) -> Int
}
mutate fn fetch(using db: Store) -> Int
    effects { db.read }
{ db.read() }
mutate fn store(n: Int, using db: Store) -> Int
    effects { db.write }
{ db.write(n) }
"#
                    .to_vec(),
                ),
            ]),
        },
        &LoadOptions::default(),
    )
    .unwrap()
}
fn task(id: &str, resource: &str) -> Task {
    Task {
        id: id.into(),
        function: "m.fetch".into(),
        arguments: vec![],
        using: BTreeMap::from([("db".into(), resource.into())]),
        after: BTreeSet::new(),
    }
}
fn scope() -> Scope {
    let resource = Resource {
        identity: "db:one".into(),
        capability: "m.Store".into(),
        operations: BTreeMap::from([("read".into(), Access::Read)]),
    };
    Scope {
        tasks: vec![task("one", "db"), task("two", "alias")],
        resources: BTreeMap::from([("db".into(), resource.clone()), ("alias".into(), resource)]),
        verification: VerifyConfig::default(),
    }
}
#[derive(Default)]
struct Host {
    calls: AtomicUsize,
    active: AtomicUsize,
    maximum: AtomicUsize,
}
impl EffectHost for Host {
    fn call(&self, _resource: &str, op: &str, args: &[Value]) -> Result<Value, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.maximum.fetch_max(active, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(40));
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(if op == "write" {
            args[0].clone()
        } else {
            json!(42)
        })
    }
}
#[test]
fn independent_effects_run_concurrently_and_replay_without_host_calls() {
    let s = snapshot();
    let scope = scope();
    let host = Arc::new(Host::default());
    let execution = execute(&s, &scope, host.clone(), 2).unwrap();
    assert!(execution.completed);
    assert_eq!(host.calls.load(Ordering::SeqCst), 2);
    assert_eq!(host.maximum.load(Ordering::SeqCst), 2);
    assert_eq!(
        execution
            .events
            .iter()
            .map(|e| e.sequence)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(execution.results[0]["value"], 42);
    assert_eq!(replay(&s, &scope, &execution, 1).unwrap(), execution);
    assert_eq!(host.calls.load(Ordering::SeqCst), 2);
    let mut forged = execution.clone();
    forged.events[0].arguments.push(json!(1));
    assert!(replay(&s, &scope, &forged, 2).is_err());
    let mut forged = execution.clone();
    forged.events[0].sequence = 99;
    assert!(replay(&s, &scope, &forged, 2).is_err());
}
#[test]
fn aliased_exclusive_effects_require_explicit_order_before_any_host_runs() {
    let s = snapshot();
    let mut scope = scope();
    scope.tasks[1].function = "m.store".into();
    scope.tasks[1].arguments = vec![json!(9)];
    let host = Arc::new(Host::default());
    assert_eq!(
        execute(&s, &scope, host.clone(), 2)
            .unwrap_err()
            .code
            .as_str(),
        "concurrent_order_required"
    );
    assert_eq!(host.calls.load(Ordering::SeqCst), 0);
    scope.tasks[1].after.insert("one".into());
    let execution = execute(&s, &scope, host.clone(), 2).unwrap();
    assert_eq!(
        execution.plan.waves,
        vec![vec!["one".to_string()], vec!["two".to_string()]]
    );
    assert_eq!(execution.results[1]["value"], 9);
    assert_eq!(host.maximum.load(Ordering::SeqCst), 1);
    assert_eq!(replay(&s, &scope, &execution, 4).unwrap(), execution);
}
#[test]
fn cycles_unknown_bindings_and_unknown_resource_modes_fail_closed() {
    let s = snapshot();
    let mut sc = scope();
    sc.tasks[0].after.insert("two".into());
    sc.tasks[1].after.insert("one".into());
    assert_eq!(plan(&s, &sc).unwrap_err().code.as_str(), "concurrent_cycle");
    let mut sc = scope();
    sc.tasks[1].using.insert("db".into(), "missing".into());
    assert!(plan(&s, &sc).is_err());
    let mut sc = scope();
    for resource in sc.resources.values_mut() {
        resource.operations.clear();
    }
    assert_eq!(
        plan(&s, &sc).unwrap_err().code.as_str(),
        "concurrent_order_required"
    );
    let mut sc = scope();
    sc.tasks[0].arguments = vec![json!(1)];
    assert!(plan(&s, &sc).is_err());
    let mut sc = scope();
    sc.tasks[1].id = "one".into();
    assert!(plan(&s, &sc).is_err());
}
#[test]
fn faults_join_the_started_wave_and_do_not_start_dependents() {
    struct Broken;
    impl EffectHost for Broken {
        fn call(&self, _: &str, _: &str, _: &[Value]) -> Result<Value, String> {
            Ok(json!("not an integer"))
        }
    }
    let s = snapshot();
    let mut sc = scope();
    let mut third = task("three", "db");
    third.after.insert("one".into());
    sc.tasks.push(third);
    let execution = execute(&s, &sc, Arc::new(Broken), 2).unwrap();
    assert!(!execution.completed);
    assert_eq!(execution.results.len(), 2);
    assert_eq!(execution.events.len(), 2);
    assert_eq!(replay(&s, &sc, &execution, 1).unwrap(), execution);
}

/// A host whose responses depend on the resource and arguments, recording the calls it serves.
struct Recording(std::sync::Mutex<Vec<String>>);
impl EffectHost for Recording {
    fn call(&self, resource: &str, operation: &str, arguments: &[Value]) -> Result<Value, String> {
        self.0
            .lock()
            .unwrap()
            .push(format!("{resource}.{operation}({arguments:?})"));
        match (resource, operation) {
            ("db:users", "get") => Ok(json!(10)),
            ("db:settings", "get") => Ok(json!(5)),
            (_, "put") => Ok(json!({"$type": "Unit"})),
            _ => Err("no such fixture".into()),
        }
    }
}

#[test]
fn concurrent_blocks_run_effectful_tasks_in_order_and_replay() {
    let source = br#"module m
@read_only("get")
interface Store {
    fn get(key: Text) -> Int
    fn put(key: Text, value: Int) -> Unit
}
mutate fn profile(using users: Store, using settings: Store) -> Int
    effects { users.get, settings.get, settings.put }
{
    concurrent {
        let user = users.get("ada")
        let theme = settings.get("ada")
    }
    concurrent {
        settings.put("seen", user)
        let again = users.get("ada")
    }
    user * 10 + theme + again
}
"#;
    let s = Snapshot::new(
        VirtualTree {
            files: BTreeMap::from([
                (
                    "cpl.toml".into(),
                    b"[project]\nname=\"effects\"\nedition=\"0.1\"\n".to_vec(),
                ),
                ("src/m.cpl".into(), source.to_vec()),
            ]),
        },
        &LoadOptions::default(),
    )
    .unwrap_or_else(|d| panic!("{}: {:?}", d.message, d.data));
    let store = |identity: &str| Resource {
        identity: identity.into(),
        capability: "m.Store".into(),
        operations: BTreeMap::from([("get".into(), Access::Read)]),
    };
    let scope = Scope {
        tasks: vec![Task {
            id: "profile".into(),
            function: "m.profile".into(),
            arguments: vec![],
            using: BTreeMap::from([
                ("users".into(), "users".into()),
                ("settings".into(), "settings".into()),
            ]),
            after: BTreeSet::new(),
        }],
        resources: BTreeMap::from([
            ("users".into(), store("db:users")),
            ("settings".into(), store("db:settings")),
        ]),
        verification: VerifyConfig::default(),
    };
    let host = Arc::new(Recording(Default::default()));
    let execution = execute(&s, &scope, host.clone(), 2).unwrap();
    assert!(execution.completed, "{:#?}", execution.results);
    assert_eq!(execution.results[0]["value"], json!(115));
    // Overlapped tasks may reach the host in either order; the record keeps block order.
    let mut calls = host.0.lock().unwrap().clone();
    calls.sort();
    assert_eq!(
        calls,
        vec![
            "db:settings.get([String(\"ada\")])",
            "db:settings.put([String(\"seen\"), Number(10)])",
            "db:users.get([String(\"ada\")])",
            "db:users.get([String(\"ada\")])",
        ]
    );
    let trace: Vec<_> = execution
        .events
        .iter()
        .map(|e| (e.sequence, e.resource.as_str(), e.operation.as_str()))
        .collect();
    assert_eq!(
        trace,
        vec![
            (1, "db:users", "get"),
            (2, "db:settings", "get"),
            (3, "db:settings", "put"),
            (4, "db:users", "get"),
        ]
    );
    let replayed = replay(&s, &scope, &execution, 1).unwrap();
    assert_eq!(replayed, execution);
    assert_eq!(host.0.lock().unwrap().len(), 4, "replay contacts no host");
}

/// A host whose every call takes `delay`, as a remote service would.
struct Slow(Duration);
impl EffectHost for Slow {
    fn call(&self, _: &str, _: &str, _: &[Value]) -> Result<Value, String> {
        std::thread::sleep(self.0);
        Ok(json!(1))
    }
}

#[test]
fn independent_tasks_of_one_block_overlap_their_host_calls() {
    let source = br#"module m
interface Store {
    fn read() -> Int
}
mutate fn gather(using a: Store, using b: Store, using c: Store, using d: Store) -> Int
    effects { a.read, b.read, c.read, d.read }
{
    concurrent {
        let w = a.read()
        let x = b.read()
        let y = c.read()
        let z = d.read()
    }
    w + x + y + z
}
"#;
    let s = Snapshot::new(
        VirtualTree {
            files: BTreeMap::from([
                (
                    "cpl.toml".into(),
                    b"[project]\nname=\"effects\"\nedition=\"0.1\"\n".to_vec(),
                ),
                ("src/m.cpl".into(), source.to_vec()),
            ]),
        },
        &LoadOptions::default(),
    )
    .unwrap();
    let names = ["a", "b", "c", "d"];
    let scope = Scope {
        tasks: vec![Task {
            id: "gather".into(),
            function: "m.gather".into(),
            arguments: vec![],
            using: names
                .iter()
                .map(|n| (n.to_string(), n.to_string()))
                .collect(),
            after: BTreeSet::new(),
        }],
        resources: names
            .iter()
            .map(|n| {
                (
                    n.to_string(),
                    Resource {
                        identity: format!("service:{n}"),
                        capability: "m.Store".into(),
                        operations: BTreeMap::new(),
                    },
                )
            })
            .collect(),
        verification: VerifyConfig::default(),
    };
    let delay = Duration::from_millis(150);
    let started = std::time::Instant::now();
    let execution = execute(&s, &scope, Arc::new(Slow(delay)), 1).unwrap();
    let elapsed = started.elapsed();
    assert_eq!(execution.results[0]["value"], json!(4));
    if std::thread::available_parallelism().map_or(1, |n| n.get()) > 1
        && std::env::var("CPL_PARALLELISM").map_or(true, |v| v != "1")
    {
        assert!(elapsed < delay * 3, "four 150 ms calls took {elapsed:?}");
    }
    let resources: Vec<_> = execution
        .events
        .iter()
        .map(|e| e.resource.clone())
        .collect();
    assert_eq!(
        resources,
        vec!["service:a", "service:b", "service:c", "service:d"]
    );
    assert_eq!(replay(&s, &scope, &execution, 1).unwrap(), execution);
}

#[test]
fn aliased_resources_keep_their_tasks_in_order() {
    let source = br#"module m
interface Store {
    fn read() -> Int
}
mutate fn pair(using a: Store, using b: Store) -> Int
    effects { a.read, b.read }
{
    concurrent {
        let x = a.read()
        let y = b.read()
    }
    x + y
}
"#;
    let s = Snapshot::new(
        VirtualTree {
            files: BTreeMap::from([
                (
                    "cpl.toml".into(),
                    b"[project]\nname=\"effects\"\nedition=\"0.1\"\n".to_vec(),
                ),
                ("src/m.cpl".into(), source.to_vec()),
            ]),
        },
        &LoadOptions::default(),
    )
    .unwrap();
    // Two parameters, one host resource: the handles differ but the identity is shared.
    let shared = Resource {
        identity: "service:shared".into(),
        capability: "m.Store".into(),
        operations: BTreeMap::new(),
    };
    let scope = Scope {
        tasks: vec![Task {
            id: "pair".into(),
            function: "m.pair".into(),
            arguments: vec![],
            using: BTreeMap::from([("a".into(), "first".into()), ("b".into(), "second".into())]),
            after: BTreeSet::new(),
        }],
        resources: BTreeMap::from([("first".into(), shared.clone()), ("second".into(), shared)]),
        verification: VerifyConfig::default(),
    };
    let delay = Duration::from_millis(100);
    let started = std::time::Instant::now();
    let execution = execute(&s, &scope, Arc::new(Slow(delay)), 1).unwrap();
    assert_eq!(execution.results[0]["value"], json!(2));
    assert!(started.elapsed() >= delay * 2, "aliased tasks overlapped");
}
