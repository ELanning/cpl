//! cpl-eval: the reference evaluator of CPL 0.1 (`spec/toolchain.md` 1.7):
//! runtime values, `Int` arithmetic, the standard-library builtins, the two
//! value encodings, faults, the interpreter, `cpl run`, and the test runner.
//!
//! | Module | Contents | Specification |
//! | - | - | - |
//! | [`value`] | [`Value`], its constructors, structural equality, `++` | toolchain 1.7, T8; semantics 6.1, 6.4, 6.5 |
//! | [`arith`] | the `Int` operators; [`arith::div_rem`], the one home of truncating division | semantics 6.3, S37; stdlib S8; toolchain T9 |
//! | [`types`] | small type helpers: `strip`, instantiation, the prelude naming rule | semantics 3.3; toolchain T30; stdlib S44 |
//! | [`fault`] | [`Fault`], [`FaultKind`], [`Fault::to_diagnostic`] | toolchain 1.7.3; semantics 6.7, S34, S57 |
//! | [`json`] | value JSON and value text: [`value_to_json`], [`value_from_json`], [`value_text`] | toolchain 7.3; semantics S40 |
//! | [`builtins`] | the builtin registry ([`builtins::lookup`]), every builtin, `check<R>` support | stdlib 1.4-1.7, 2-7, 10.4-10.6; semantics 6.10, S58 |
//! | [`interp`] | [`Interpreter`]: the tree-walking evaluator of the typed HIR with checked contracts, [`EvalConfig`], capability handles ([`BindOptions`]), the binding sites of a body ([`scope_at`]) | toolchain 1.7, 1.7.1, 1.7.4; semantics 5, 6, 7.1, 8.1, S41 |
//! | [`run`] | [`run_function`], the evaluation behind `cpl run`, with constant-expression and value-JSON arguments; from Milestone 2 [`prepare_run`], the hosts of `using` parameters ([`run_function_with_hosts`]), and [`RunEngine`] | toolchain 1.7, 1.7.4, 7.2; semantics S39, S78 |
//! | [`stack`] | [`on_eval_stack`]: the large-stack evaluation thread and its guard | semantics S38 |
//! | [`test`](mod@test) | the test and property runner, generic over its engine ([`TestEngine`]): selection, seeds, generators, shrinking, records, counterexamples | toolchain 1.7.2, 1.7.4; semantics 7; stdlib 8 |
//! | [`boundary`] | the capability boundary of Milestone 2: the validation of a host's result and the `ForeignError` of a reported failure | toolchain 1.7.4; semantics S77, S80; stdlib S45 |
//!
//! [`run_function`], [`run_tests`], and [`run_test`] evaluate on a dedicated
//! thread with a large stack; a host that drives an [`Interpreter`] itself
//! should do the same ([`on_eval_stack`]) for programs that recurse deeply.
//! Values need no such care: a [`Value`] of any nesting depth is dropped,
//! compared, and rendered (value JSON, value text) without recursion, on
//! whatever thread holds it ([`stack`]).
//!
//! Every text and table that a specification fixes and that the native
//! engine needs too comes from `cpl-rtspec` (toolchain 1.11, T70): the
//! message templates, the `White_Space` set, the builtin precondition
//! facts, the limits, and the one `Int` division.
//!
//! `cpl-eval` depends on `cpl-hir`, never on `cpl-sema` (T50): every
//! `Program`, `DefId`, and `Ty` here is a `cpl_hir` item. Its integration
//! tests check the programs they run with `cpl-sema`, a dev-dependency only
//! (toolchain 1.2). The doc-hidden module `testing` builds programs by hand
//! for what no checked program has and reads and runs the interpreter corpus
//! (`testing::corpus`, toolchain 12.5).

pub mod arith;
pub mod boundary;
pub mod builtins;
pub mod concurrent;
pub mod fault;
pub mod interp;
pub mod json;
pub mod machine;
pub mod run;
pub mod stack;
pub mod test;
#[doc(hidden)]
pub mod testing;
pub mod types;
pub mod value;

pub use fault::{AttributedTo, ClauseKind, Fault, FaultKind, Frame, Resource};
pub use interp::{
    scope_at, BindOptions, Budget, CapabilityImpl, EffectEvent, EvalConfig, Handle, HandleId,
    HostContext, Interpreter,
};
pub use json::{value_from_json, value_text, value_to_json, ArgError};
pub use run::{
    prepare_run, run_function, run_function_with_hosts, HostSource, PreparedRun, RunArg, RunEngine,
    RunError, RunHosts, RunOutcome,
};
pub use stack::on_eval_stack;
pub use test::{
    property_seed, run_test, run_test_with, run_tests, run_tests_with, seed_hex, select_tests,
    test_result_diagnostic, RangeSide, TestConfig, TestEngine, TestSelector,
};
pub use value::Value;
