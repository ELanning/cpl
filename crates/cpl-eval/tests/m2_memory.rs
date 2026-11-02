//! The memory fault an adapter host reports (semantics S76; `spec/toolchain.md` 1.7, 1.7.3): the reference evaluator
//! never raises `resource_exhaustion` with `resource: "memory"` itself (safe Rust cannot observe allocation failure);
//! the one it raises is the fault a host reports for an allocation failure in a C adapter or in the binding layer of the
//! driver that runs it (`spec/cabi.md` Section 7). It is raised at the operation call, spanning the call as the faults of
//! semantics S80 do, with the evaluator's own backtrace, and blamed, as every exhaustion is, on the innermost active
//! declaration; the host cannot name that declaration (`HostContext` carries only the program).

mod common;

use cpl_eval::{
    on_eval_stack, BindOptions, CapabilityImpl, EvalConfig, Fault, HostContext, Interpreter,
    Resource, Value,
};
use cpl_hir::Program;
use serde_json::Value as Json;

const STORE: &str = r#"module app.mem

pub capability Store {
    fn get() -> Int
}

pub fn load(using s: Store) -> Int
    effects { s.get }
{
    s.get()
}

pub fn outer(using s: Store) -> Int
    effects { s.get }
{
    load(using s) + 1
}
"#;

/// A host whose every operation reports the fault it was given.
struct Failing(Fault);

impl CapabilityImpl for Failing {
    fn call(
        &mut self,
        _op: &str,
        _args: Vec<Value>,
        _ctx: &mut HostContext<'_>,
    ) -> Result<Value, Fault> {
        Err(self.0.clone())
    }
}

/// Call `function` with one `Store` handle reporting `fault`, bound with or without validation: the fault's
/// diagnostic object.
fn fault_of(p: &Program, function: &str, fault: Fault, validate: bool) -> Json {
    let f = common::def(p, &format!("app.mem.{function}"));
    let cap = common::def(p, "app.mem.Store");
    on_eval_stack(|| {
        let mut it = Interpreter::new(p, EvalConfig::default());
        let h = it.bind_capability_with(
            cap,
            Box::new(Failing(fault)),
            BindOptions {
                label: "store".to_string(),
                validate_results: validate,
            },
        );
        let fault = it
            .call(f, Vec::new(), vec![h])
            .expect_err("the host reports a failure");
        common::fault_json(p, &fault)
    })
}

fn frames(d: &Json) -> Vec<String> {
    d["backtrace"]
        .as_array()
        .map(|fs| {
            fs.iter()
                .map(|f| f["qualified_name"].as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn get_call() -> Json {
    common::location_after(STORE, "src/app/mem.cpl", "pub fn load(", "s.get()")
}

#[test]
fn a_reported_memory_fault_is_raised_at_the_operation_call_and_blamed_on_the_innermost_declaration()
{
    let p = common::check_source(STORE);
    for validate in [true, false] {
        let d = fault_of(
            &p,
            "outer",
            Fault::resource_exhaustion(Resource::Memory, 0),
            validate,
        );
        assert_eq!(d["code"], "resource_exhaustion", "{d}");
        assert_eq!(d["resource"], "memory");
        assert_eq!(d["limit"], 0);
        assert_eq!(d["symbol"], "app.mem.load", "{d}");
        assert_eq!(d["span"], get_call());
        // The evaluator's backtrace, innermost first (semantics 6.7).
        assert_eq!(frames(&d), ["app.mem.load", "app.mem.outer"]);
    }
}

#[test]
fn a_reported_memory_fault_keeps_its_limit_and_message() {
    let p = common::check_source(STORE);
    let reported = Fault::resource_exhaustion(Resource::Memory, 4096);
    let message = reported.message.clone();
    let d = fault_of(&p, "load", reported, true);
    assert_eq!(d["limit"], 4096);
    assert_eq!(d["message"], message);
    assert_eq!(d["symbol"], "app.mem.load");
    assert_eq!(d["span"], get_call());
}

#[test]
fn a_reported_fault_that_names_its_symbol_or_is_another_kind_is_left_as_reported() {
    let p = common::check_source(STORE);
    // A symbol the host gave is kept.
    let named = Fault::resource_exhaustion(Resource::Memory, 0)
        .blamed_on(&p, common::def(&p, "app.mem.outer"));
    let d = fault_of(&p, "outer", named, true);
    assert_eq!(d["symbol"], "app.mem.outer");
    assert_eq!(d["span"], get_call());
    // Any other host fault keeps the Milestone 1 handling: its span is set, nothing blamed.
    let d = fault_of(&p, "outer", Fault::internal("io", "the host failed"), true);
    assert_eq!(d["code"], "internal_error");
    assert!(d.get("symbol").is_none_or(Json::is_null), "{d}");
    assert_eq!(d["span"], get_call());
}
