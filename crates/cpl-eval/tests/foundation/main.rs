//! The evaluator's foundation: values, `Int` arithmetic, the builtins, the
//! value encodings, faults, the interpreter's handle and frame state, and
//! the hand-built test programs of `cpl_eval::testing` (whose std must be
//! the checker's, `builder`).

#[path = "../common/mod.rs"]
mod common;

mod arith;
mod builder;
mod builtins;
mod fault;
mod interp;
mod json;
mod laws;
mod support;
mod types;
mod value;
