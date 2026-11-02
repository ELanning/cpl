//! The interpreter's state shared with the builtins: configuration, the
//! frame stack, and capability handles (toolchain 1.7; semantics 6.9).

use crate::support::{int, reader, text};
use cpl_eval::{
    CapabilityImpl, EvalConfig, Fault, FaultKind, HandleId, HostContext, Interpreter, Value,
};
use std::time::Duration;

#[test]
fn eval_config_defaults() {
    let c = EvalConfig::default();
    assert_eq!(c.max_call_depth, 10_000);
    assert_eq!(EvalConfig::DEFAULT_MAX_CALL_DEPTH, 10_000);
    assert_eq!(c.time_budget, None);
    assert!(c.check_contracts);
    assert!(!c.trace_effects);
    let custom = EvalConfig {
        time_budget: Some(Duration::from_millis(5)),
        ..EvalConfig::default()
    };
    let r = reader();
    let it = Interpreter::new(&r.p, custom.clone());
    assert_eq!(it.config(), &custom);
    assert!(std::ptr::eq(it.program(), &r.p));
}

#[test]
fn frames_name_the_current_declaration() {
    let r = reader();
    let mut it = Interpreter::new(&r.p, EvalConfig::default());
    assert_eq!(it.current_def(), None);
    it.push_frame(r.paginate, None);
    it.push_frame(r.take_n, None);
    assert_eq!(it.current_def(), Some(r.take_n));
    assert_eq!(it.frames().len(), 2);
    assert_eq!(it.pop_frame().map(|f| f.def), Some(r.take_n));
    assert_eq!(it.current_def(), Some(r.paginate));
    it.pop_frame();
    assert_eq!(it.current_def(), None);
    assert!(it.pop_frame().is_none());
}

/// A fixture capability: `get(id)` answers `"page <id>"`.
struct Pages {
    calls: u32,
}

impl CapabilityImpl for Pages {
    fn call(
        &mut self,
        op: &str,
        args: Vec<Value>,
        ctx: &mut HostContext<'_>,
    ) -> Result<Value, Fault> {
        self.calls += 1;
        assert!(!ctx.program.defs.is_empty());
        match (op, args.as_slice()) {
            ("get", [Value::Wrapper { inner, .. }]) => Ok(Value::text(&format!(
                "page {}",
                inner.as_text().unwrap_or("?")
            ))),
            _ => Err(Fault::internal("fixture", format!("unexpected call {op}"))),
        }
    }
}

#[test]
fn handles_bind_and_dispatch() {
    let r = reader();
    let mut it = Interpreter::new(&r.p, EvalConfig::default());
    let h0 = it.bind_capability(r.page_read, Box::new(Pages { calls: 0 }), "fixture:pages");
    let h1 = it.bind_capability(r.page_read, Box::new(Pages { calls: 0 }), "fixture:other");
    assert_eq!((h0.id, h1.id), (HandleId(0), HandleId(1)));
    assert_eq!(h0.capability, r.page_read);
    assert_eq!(it.handle(HandleId(1)).unwrap().label, "fixture:other");
    assert_eq!(it.handle(HandleId(9)), None);
    let page = Value::wrap(r.page_id, text("p"));
    assert_eq!(
        it.call_handle(h0.id, "get", vec![page]).unwrap(),
        text("page p")
    );
    let f = it.call_handle(h0.id, "put", vec![int(1)]).unwrap_err();
    assert_eq!(f.data["kind"], "fixture");
    assert!(it.effects().is_empty());
}

#[test]
fn an_unbound_handle_is_an_internal_error() {
    // semantics 6.9: kind "unbound_capability".
    let r = reader();
    let mut it = Interpreter::new(&r.p, EvalConfig::default());
    let f = it.call_handle(HandleId(3), "get", vec![]).unwrap_err();
    assert_eq!(f.kind, FaultKind::Internal);
    assert_eq!(f.data["kind"], "unbound_capability");
    let d = f.to_diagnostic(&r.p);
    assert_eq!(d.code, "internal_error");
    assert_eq!(d.data["kind"], "unbound_capability");
}
