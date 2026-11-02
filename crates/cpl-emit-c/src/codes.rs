//! The diagnostic codes this crate produces (`spec/toolchain.md` 1.13, Appendix B, T76): one `DiagCode` per `build_*`
//! code (category `build`, span-less), `build_target_not_enabled` included, which `cpl build` raises, and the
//! `internal_error` kinds of the native build. Owner: the build track (D).

use cpl_diag::DiagCode;

/// `cpl build` on a project without `"c"` in `[interop] targets`; `targets`; exit 1.
pub const BUILD_TARGET_NOT_ENABLED: DiagCode = DiagCode("build_target_not_enabled");
/// No C compiler by T72's precedence; `searched`; exit 2 (also `cpl run` and `cpl test` when they need a driver).
pub const BUILD_CC_UNAVAILABLE: DiagCode = DiagCode("build_cc_unavailable");
/// A compiler that is neither gcc 9 or later nor clang 12 or later; `found`, `minimum`; exit 2.
pub const BUILD_CC_UNSUPPORTED: DiagCode = DiagCode("build_cc_unsupported");
/// The compiler's target is outside the platform matrix of `spec/cabi.md` Section 1; `target`; exit 2.
pub const BUILD_TARGET_UNSUPPORTED: DiagCode = DiagCode("build_target_unsupported");
/// The compiler's target is of the matrix's second tier (a warning); `target`.
pub const BUILD_TARGET_UNTESTED: DiagCode = DiagCode("build_target_untested");
/// A requested sanitizer that the compiler cannot link; `sanitizer`; exit 2.
pub const BUILD_SANITIZER_UNAVAILABLE: DiagCode = DiagCode("build_sanitizer_unavailable");
/// An adapter or vendored source of a binding does not compile; `binding`, `file`, `stderr`; exit 1.
pub const BUILD_ADAPTER_COMPILE_FAILED: DiagCode = DiagCode("build_adapter_compile_failed");
/// A `libraries` entry the linker cannot find; `binding`, `library`; exit 1.
pub const BUILD_NATIVE_DEPENDENCY_MISSING: DiagCode = DiagCode("build_native_dependency_missing");
/// The link failed, an unresolved symbol included; `stderr`, `libraries`; exit 1.
pub const BUILD_LINK_FAILED: DiagCode = DiagCode("build_link_failed");
/// `cpl build --release` without `cpl.lock`, or with one without `targets.c` (T75); exit 1.
pub const BUILD_LOCK_MISSING: DiagCode = DiagCode("build_lock_missing");
/// Under `--release`, the lock's `targets.c`, `foreign.c.<b>.headers`, or `foreign.c.<b>.libraries` differ from the
/// build (T75); `differences`; exit 1.
pub const BUILD_LOCK_MISMATCH: DiagCode = DiagCode("build_lock_mismatch");
/// An ABI break by `spec/cabi.md` 9.3 under an unchanged `prefix` and `abi` (T75, T81); `symbol`, `id`, `expected`,
/// `found`; one per break; exit 1.
pub const BUILD_ABI_CHANGED: DiagCode = DiagCode("build_abi_changed");

/// Milestone 3: `--target python` without a CPython 3.10 or later that has `Python.h` (`spec/python.md` 1);
/// `interpreter`, `reason`; exit 2.
pub const BUILD_PYTHON_UNAVAILABLE: DiagCode = DiagCode("build_python_unavailable");

/// The `internal_error` kind of a compiler that rejected emitted or runtime C, with its `stderr`; exit 2.
pub const EMITTED_C_REJECTED: &str = "emitted_c_rejected";
/// The `internal_error` kind of [`crate::emit`] given a program with errors; exit 2.
pub const EMIT_FAILED: &str = "emit_failed";
