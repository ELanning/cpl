//! Limits and versions that both engines share (`spec/toolchain.md` 1.11, T70).
//!
//! `cpl-eval` defines `EvalConfig::DEFAULT_MAX_CALL_DEPTH`, `stack::EVAL_STACK_SIZE`, and `stack::GUARD_MARGIN` from
//! these (toolchain 1.7.4), and the generated `cpl_rt_gen.h` carries them as `CPLRT_LIMIT_*` (`spec/cabi.md` B.2).

/// The default call-depth limit, `[run] max_call_depth = 10000` (semantics S38): active CPL function calls (S72).
pub const DEFAULT_MAX_CALL_DEPTH: u32 = 10_000;

/// The stack reserved for the evaluation thread of both engines: 1 GiB (toolchain 1.7; `spec/cabi.md` 3.1, 3.4).
pub const EVAL_STACK_SIZE: usize = 1 << 30;

/// The stack-guard margin: an activation that would start within this many bytes of the end of its thread's stack,
/// or within an eighth of a smaller stack, is refused with `resource_exhaustion` (`resource: "stack"`): 64 MiB
/// (toolchain 1.7; `spec/cabi.md` 3.4).
pub const GUARD_MARGIN: usize = 64 << 20;

/// `(major, minor)` of the C API (`spec/cabi.md` Section 4): `CPL_API_VERSION_MAJOR` and `CPL_API_VERSION_MINOR` of
/// `cpl.h`, and `BuildOutput::runtime.api` as `"<major>.<minor>"` (toolchain 7.2).
pub const C_API_VERSION: (u16, u16) = (0, 1);

/// The version of the cpl-driver protocol (`spec/cabi.md` 3.8): the `"protocol"` of a driver's banner line.
pub const DRIVER_PROTOCOL: u32 = 1;
