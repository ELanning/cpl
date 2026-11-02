//! Shared helpers for the tests that build a program by hand (the impossible
//! states of `faults`): a module with a source text, evaluation on the large
//! evaluation stack, and faults as diagnostic JSON.

use cpl_eval::json::{value_from_json, value_to_json};
use cpl_eval::stack::on_eval_stack;
use cpl_eval::testing::{Hir, ProgramBuilder};
use cpl_eval::{EvalConfig, Fault, Interpreter};
use cpl_hir::{DefId, Expr, LocalId, ModuleId, Program, Ty};
use serde_json::Value as Json;

/// A module whose file holds 40 lines of 40 characters, so spans set with
/// [`Hir::at`] resolve to distinct lines and columns.
pub fn module(b: &mut ProgramBuilder, path: &str) -> (ModuleId, Hir) {
    let text = format!("{}\n", "x".repeat(40)).repeat(40);
    let m = b.module_with_text(path, &text);
    let h = b.hir(m);
    (m, h)
}

pub fn int(h: &Hir, l: LocalId) -> Expr {
    h.local(l, Ty::Int)
}

/// Call `f` with value-JSON arguments (decoded against its parameters) on
/// the evaluation stack; the result's value JSON or the fault.
pub fn eval(p: &Program, f: DefId, args: &[Json]) -> Result<Json, Fault> {
    eval_with(p, EvalConfig::default(), f, args)
}

pub fn eval_with(p: &Program, config: EvalConfig, f: DefId, args: &[Json]) -> Result<Json, Fault> {
    on_eval_stack(move || {
        let fd = p.def(f).kind.as_fn().expect("a function");
        let values = args
            .iter()
            .zip(&fd.params)
            .map(|(j, param)| value_from_json(p, j, &param.ty).expect("a decodable argument"))
            .collect();
        let mut it = Interpreter::new(p, config);
        it.call(f, values, Vec::new())
            .map(|v| value_to_json(p, &v, &fd.ret))
    })
}

/// The value of a call that must succeed.
pub fn value(p: &Program, f: DefId, args: &[Json]) -> Json {
    eval(p, f, args).unwrap_or_else(|fault| panic!("faulted: {fault}: {}", diag(p, &fault)))
}

/// A fault as its S49 diagnostic object.
pub fn diag(p: &Program, fault: &Fault) -> Json {
    serde_json::to_value(fault.to_diagnostic(p).to_json(p.source_map())).expect("serializable")
}

/// The diagnostic object of a call that must fault.
pub fn fault_diag(p: &Program, f: DefId, args: &[Json]) -> Json {
    match eval(p, f, args) {
        Ok(v) => panic!("returned {v} instead of faulting"),
        Err(fault) => diag(p, &fault),
    }
}

/// The qualified names of a fault's backtrace, innermost first.
pub fn backtrace(d: &Json) -> Vec<String> {
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
