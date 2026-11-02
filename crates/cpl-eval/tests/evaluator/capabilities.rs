//! Capability operations (toolchain 1.7.1; semantics 6.9, S31, S57) on a
//! checked program: the operation's `requires` before the host is invoked,
//! its `ensures` on the host's result (`attributed_to: "binding"`), `using`
//! forwarding, effect tracing, and unbound handles. No handle can be created
//! from CPL in Milestone 1 (6.9), so the tests bind host implementations
//! through `Interpreter::bind_capability`, as the interop adapters of
//! Milestones 2-4 will.

use crate::common;
use crate::support::diag;
use cpl_eval::{
    CapabilityImpl, EvalConfig, Fault, Handle, HandleId, HostContext, Interpreter, Value,
};
use cpl_hir::{DefId, Program};
use serde_json::json;
use std::cell::Cell;
use std::rc::Rc;

const FILE: &str = "src/m.cpl";

const SOURCE: &str = r#"module m

pub capability Store {
    fn get(id: Int) -> Int
        requires id > 0
        ensures result >= id
}

pub capability Other {
    fn get(id: Int) -> Int
}

pub fn load(id: Int, using db: Store) -> Int
    effects { db.get }
{
    db.get(id)
}

pub fn twice(id: Int, using db: Store) -> Int
    effects { db.get }
{
    load(id, using db) + load(id + 1, using db)
}
"#;

/// `get(id)` answers `id * factor`, or a host fault when `factor` is 0.
struct Times {
    factor: i64,
    calls: Rc<Cell<u32>>,
}

impl CapabilityImpl for Times {
    fn call(
        &mut self,
        op: &str,
        args: Vec<Value>,
        _: &mut HostContext<'_>,
    ) -> Result<Value, Fault> {
        self.calls.set(self.calls.get() + 1);
        assert_eq!(op, "get");
        if self.factor == 0 {
            return Err(Fault::internal("host_failure", "the store is down"));
        }
        let id = args[0].as_int().expect("an Int").clone();
        Ok(Value::Int(id * self.factor))
    }
}

struct Fixture {
    p: Program,
    store: DefId,
    other: DefId,
    load: DefId,
    twice: DefId,
}

fn fixture() -> Fixture {
    let p = common::check_files(&[(FILE, SOURCE)]);
    let d = |name: &str| common::def(&p, &format!("m.{name}"));
    Fixture {
        store: d("Store"),
        other: d("Other"),
        load: d("load"),
        twice: d("twice"),
        p,
    }
}

fn backtrace(d: &serde_json::Value) -> Vec<String> {
    d["backtrace"]
        .as_array()
        .map(|frames| {
            frames
                .iter()
                .map(|f| f["qualified_name"].as_str().unwrap_or("?").to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn bind(it: &mut Interpreter<'_>, store: DefId, factor: i64) -> (Handle, Rc<Cell<u32>>) {
    let calls = Rc::new(Cell::new(0));
    let handle = it.bind_capability(
        store,
        Box::new(Times {
            factor,
            calls: Rc::clone(&calls),
        }),
        "fixture:store",
    );
    (handle, calls)
}

#[test]
fn an_operation_call_reaches_the_host_and_is_traced() {
    let fx = fixture();
    let config = EvalConfig {
        trace_effects: true,
        ..EvalConfig::default()
    };
    let mut it = Interpreter::new(&fx.p, config);
    let (h, calls) = bind(&mut it, fx.store, 2);
    assert_eq!(
        it.call(fx.load, vec![Value::int(5)], vec![h.clone()])
            .unwrap(),
        Value::int(10)
    );
    assert_eq!(calls.get(), 1);
    let events = it.effects();
    assert_eq!(events.len(), 1);
    assert_eq!(
        (events[0].handle, events[0].op.as_str(), events[0].def),
        (h.id, "get", fx.load)
    );
    // `using` arguments forward the handle.
    assert_eq!(
        it.call(fx.twice, vec![Value::int(5)], vec![h]).unwrap(),
        Value::int(22)
    );
    assert_eq!(calls.get(), 3);
    assert_eq!(it.effects().len(), 3);
    assert!(it.effects().iter().all(|e| e.def == fx.load));
    // Tracing is off by default.
    let mut quiet = Interpreter::new(&fx.p, EvalConfig::default());
    let (h, _) = bind(&mut quiet, fx.store, 2);
    quiet.call(fx.load, vec![Value::int(5)], vec![h]).unwrap();
    assert!(quiet.effects().is_empty());
}

#[test]
fn the_operation_requires_runs_before_the_host_s31() {
    let fx = fixture();
    let mut it = Interpreter::new(&fx.p, EvalConfig::default());
    let (h, calls) = bind(&mut it, fx.store, 2);
    let fault = it.call(fx.load, vec![Value::int(0)], vec![h]).unwrap_err();
    assert_eq!(calls.get(), 0, "the host was not invoked");
    let d = diag(&fx.p, &fault);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause_kind"], "requires");
    assert_eq!(d["symbol"], "m.Store.get");
    assert_eq!(d["qualified_name"], "m.Store.get");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["attributed_to"], "caller");
    assert_eq!(d["caller"], "m.load");
    assert_eq!(d["clause"], "id > 0");
    assert_eq!(d["input"], json!({"id": 0}));
    assert_eq!(
        d["span"],
        common::location_of(SOURCE, FILE, "requires id > 0")
    );
    assert_eq!(backtrace(&d), ["m.load"]);
}

#[test]
fn the_operation_ensures_blames_the_binding_s57() {
    let fx = fixture();
    let mut it = Interpreter::new(&fx.p, EvalConfig::default());
    // A binding that answers id * -1 < id breaks `result >= id`.
    let (h, calls) = bind(&mut it, fx.store, -1);
    let fault = it.call(fx.load, vec![Value::int(5)], vec![h]).unwrap_err();
    assert_eq!(calls.get(), 1);
    let d = diag(&fx.p, &fault);
    assert_eq!(d["clause_kind"], "ensures");
    assert_eq!(d["attributed_to"], "binding");
    assert_eq!(d["symbol"], "m.Store.get");
    assert_eq!(d["clause"], "result >= id");
    assert_eq!(d["input"], json!({"id": 5}));
    assert_eq!(d["actual"], json!(-5));
    // Unchecked mode checks neither side.
    let mut it = Interpreter::new(
        &fx.p,
        EvalConfig {
            check_contracts: false,
            ..EvalConfig::default()
        },
    );
    let (h, _) = bind(&mut it, fx.store, -1);
    assert_eq!(
        it.call(fx.load, vec![Value::int(5)], vec![h.clone()])
            .unwrap(),
        Value::int(-5)
    );
    assert_eq!(
        it.call(fx.load, vec![Value::int(0)], vec![h]).unwrap(),
        Value::int(0)
    );
}

#[test]
fn host_faults_and_unbound_handles() {
    let fx = fixture();
    let mut it = Interpreter::new(&fx.p, EvalConfig::default());
    let (h, _) = bind(&mut it, fx.store, 0);
    let d = diag(
        &fx.p,
        &it.call(fx.load, vec![Value::int(5)], vec![h]).unwrap_err(),
    );
    assert_eq!(d["code"], "internal_error");
    assert_eq!(d["kind"], "host_failure");
    assert_eq!(d["span"], common::location_of(SOURCE, FILE, "db.get(id)"));
    // A handle the host never bound (semantics 6.9).
    let ghost = Handle {
        id: HandleId(9),
        capability: fx.store,
        label: "ghost".to_string(),
    };
    let d = diag(
        &fx.p,
        &it.call(fx.load, vec![Value::int(5)], vec![ghost])
            .unwrap_err(),
    );
    assert_eq!(d["code"], "internal_error");
    assert_eq!(d["kind"], "unbound_capability");
    // A handle of another capability, or a missing one, is refused.
    let (wrong, _) = bind(&mut it, fx.other, 2);
    let f = it
        .call(fx.load, vec![Value::int(5)], vec![wrong])
        .unwrap_err();
    assert_eq!(f.data["kind"], "call_mismatch");
    let f = it.call(fx.load, vec![Value::int(5)], vec![]).unwrap_err();
    assert_eq!(f.data["kind"], "call_mismatch");
}
