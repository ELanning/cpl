//! The interpreter, `cpl run`, and the test runner on checked programs:
//! each test writes CPL source, checks it with the real checker (`cpl-sema`,
//! a dev-dependency of this crate only, toolchain 1.2 and T50; `common`),
//! and runs it through the public API. A test that needs a state no checked
//! program reaches (an impossible HIR form, a host-bound capability handle)
//! builds that program by hand through `cpl_eval::testing` and says why.
//!
//! The property runner's tests are `tests/properties.rs`, the interpreter
//! corpus `tests/corpus.rs`, and the example table `tests/examples.rs`.

#[path = "../common/mod.rs"]
mod common;

mod capabilities;
mod contracts;
mod depth;
mod faults;
mod run;
mod runner;
mod semantics;
mod support;
