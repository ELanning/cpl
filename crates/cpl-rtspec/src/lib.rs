//! cpl-rtspec: the one source of every text and table that a specification fixes and that both engines of CPL need
//! (`spec/toolchain.md` 1.11, T70; `spec/stdlib.md` S47; `spec/cabi.md` 2.9).
//!
//! The reference evaluator (`cpl-eval`) renders and decides through this crate, `cpl-sema` reads its builtin ids and
//! its division instead of keeping copies, and `cpl-rt` formats it as the generated C header `cpl_rt_gen.h`, so the
//! native runtime cannot drift from the evaluator: two hand-kept copies of one table drift silently, and a generated
//! copy cannot (T70). It is a leaf crate of data and pure functions, with `num-bigint` its one external dependency.
//!
//! | Module | Contents | Specification |
//! | - | - | - |
//! | [`messages`] | every fault and value message whose text a specification fixes, as a `{name}` template, and [`messages::render`] | semantics 6.7, S77, S80; stdlib 1.5, S45; toolchain 1.7.3 |
//! | [`whitespace`] | the 25 `White_Space` scalars of `trim` as inclusive ranges | stdlib S22, S47 |
//! | [`builtins`] | the 59 builtin specifications of the registry: ids, callability, C entry points, fallback symbols, precondition texts and input names | stdlib 10.4, 10.6; cabi 2.7; toolchain 1.7.4 |
//! | [`limits`] | the default call-depth limit, the evaluation stack and its guard margin, the C API and driver protocol versions | semantics S38; toolchain 1.7; cabi 3.4, 3.8, 4.2 |
//! | [`arith`] | the one `Int` division, [`arith::div_rem`] | semantics S37; toolchain T9 as amended by T70 |
//! | [`foreign`] | the `ForeignErrorKind` names in declaration order and the fixed order of the detail keys | stdlib 7, S45 |
//! | [`interop`] | the `interop_*` codes of the C API with their statuses, in the order of `spec/cabi.md` 4.9 | cabi 4.6, 4.9 |
//!
//! Every text here is byte for byte what the Milestone 1 evaluator wrote, or, for what Milestone 1 never produced (the
//! binding messages of semantics S77 and S80 and of stdlib S45), the text those decisions fix. The tests of
//! `crates/cpl-rtspec/tests/tables.rs` check the tables against the specifications, the standard-library stubs, and
//! the C headers they must agree with.

pub mod arith;
pub mod builtins;
pub mod foreign;
pub mod interop;
pub mod limits;
pub mod messages;
pub mod whitespace;
