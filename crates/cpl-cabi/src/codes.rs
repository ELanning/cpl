//! The diagnostic code and the `internal_error` kinds this crate produces (`spec/toolchain.md` 1.14, Appendix B, T76).
//! Owner: the native-engine track (N).

use cpl_diag::DiagCode;

/// Under `--engine both`, the engines' outputs differ after semantics S75's normalization (category `internal`,
/// span-less, payload `eval`, `native`, and `function` or `test`); exit 2 (T73).
pub const ENGINE_DIVERGENCE: DiagCode = DiagCode("engine_divergence");
/// The `internal_error` kind of a driver that died, with `signal` or `exit_status`; exit 2.
pub const NATIVE_DRIVER_FAILED: &str = "native_driver_failed";
/// The `internal_error` kind of a malformed driver message; exit 2.
pub const NATIVE_PROTOCOL: &str = "native_protocol";
