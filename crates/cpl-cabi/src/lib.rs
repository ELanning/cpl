//! cpl-cabi: the toolchain's side of the C boundary (`spec/toolchain.md` 1.14, T69; `spec/cabi.md` Sections 3 and 7).
//!
//! The client of cpl-driver protocol 1 ([`protocol`]), the native engine built on it ([`NativeSession`]), the
//! reconstruction of a native fault as a `cpl_eval::Fault` ([`fault_from_record`]), the node-list codec of values
//! ([`wire`]), the `CapabilityImpl` through which the evaluator reaches a backend-`c` adapter ([`adapter::CAdapter`],
//! cabi.md Section 7), the evaluator's side of the driver's scripted host ([`scripted`]), and the comparison of two
//! engines' outputs under semantics S75 ([`compare`]). The toolchain never loads C into its own process: every native
//! evaluation runs in a driver child process, so a crash in emitted code, the runtime, or an adapter cannot take `cpl`
//! down, and a sanitized driver runs where an instrumented library could not be loaded. Its tests hold the
//! differential harness and the C host suites (toolchain 12.6-12.7).
//!
//! | Module | Contents | Owner |
//! | - | - | - |
//! | [`protocol`] | the message types of cpl-driver protocol 1, complete | contracts (track 0), frozen |
//! | crate root, `session`, `fault` (internal), [`wire`], [`scripted`], [`compare`], [`codes`] | [`NativeSession`] and its engine traits, [`fault_from_record`], the node-list codec, `ScriptHost`, the S75 comparison, the codes | native engine (track N) |
//! | [`adapter`] | `CAdapter` and the hosts of `cpl run` | imports and C hosts (track F1) |
//! | [`pending`] | stand-ins for items other tracks have not yet added to the crates this one depends on | contracts (track 0), deleted when unused |
//!
//! The differential harness itself (`tests/differential.rs`, `tests/differential/`) is track H's.

pub mod adapter;
pub mod codes;
pub mod compare;
#[doc(hidden)]
pub mod pending;
pub mod protocol;
pub mod scripted;
pub mod wire;
pub mod worker;

mod fault;
mod session;

pub use fault::fault_from_record;
pub use session::{Coverage, NativeError, NativeSession, Renderings, ShutdownReport};
