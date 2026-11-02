//! The `interop_*` codes of the C API with their statuses (`spec/cabi.md` 4.6, 4.9; toolchain 1.11, T70, T76).
//!
//! Category `interop`, severity error, span-less: the `code` of every usage-class error object of the C API, of a
//! driver's `{"error": E}` answers, and, for [`BIND_FAILED`] alone, of a command (`cpl run`, semantics S78). The
//! runtime's generated header takes the table from [`CODES`] as `CPLRT_INTEROP` with one index macro per code
//! (`CPLRT_INTEROP_VERSION_MISMATCH` 0, ...: the code without its `interop_` prefix, in upper case, `spec/cabi.md`
//! B.2), so the C side cannot drift from this one.

/// One `interop_*` code and the `cpl_status` an error object of that code carries (`spec/cabi.md` 4.9).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct InteropCode {
    pub code: &'static str,
    pub status: i32,
}

// The statuses of `cpl.h` (`spec/cabi.md` Appendix A.1) that the codes carry.
const CPL_E_ARGUMENT: i32 = 3;
const CPL_E_RANGE: i32 = 4;
const CPL_E_NOMEM: i32 = 5;
const CPL_E_VERSION: i32 = 6;
const CPL_E_STATE: i32 = 7;
const CPL_E_CANCELLED: i32 = 8;
const CPL_E_UNSUPPORTED: i32 = 9;

const fn code(code: &'static str, status: i32) -> InteropCode {
    InteropCode { code, status }
}

/// `CPL_E_VERSION`; payload `library`, `requested` (`{"major", "minor"}`).
pub const VERSION_MISMATCH: InteropCode = code("interop_version_mismatch", CPL_E_VERSION);
/// `CPL_E_VERSION`; payload `struct`, `size`, `minimum`, `maximum` (the library's size).
pub const STRUCT_SIZE_UNSUPPORTED: InteropCode =
    code("interop_struct_size_unsupported", CPL_E_VERSION);
/// `CPL_E_ARGUMENT`; payload `parameter`.
pub const HANDLE_INVALID: InteropCode = code("interop_handle_invalid", CPL_E_ARGUMENT);
/// `CPL_E_ARGUMENT`; payload `parameter`.
pub const HANDLE_RELEASED: InteropCode = code("interop_handle_released", CPL_E_ARGUMENT);
/// `CPL_E_ARGUMENT`; payload `parameter` (a handle of another runtime or package).
pub const HANDLE_FOREIGN: InteropCode = code("interop_handle_foreign", CPL_E_ARGUMENT);
/// `CPL_E_ARGUMENT`; payload `parameter`.
pub const NULL_POINTER: InteropCode = code("interop_null_pointer", CPL_E_ARGUMENT);
/// `CPL_E_ARGUMENT`; payload `parameter`, `pointer` (RFC 6901), `expected`, `found`.
pub const ARGUMENT_TYPE_MISMATCH: InteropCode =
    code("interop_argument_type_mismatch", CPL_E_ARGUMENT);
/// `CPL_E_ARGUMENT`; payload `parameter`, `offset` (`valid_up_to`).
pub const UTF8_INVALID: InteropCode = code("interop_utf8_invalid", CPL_E_ARGUMENT);
/// `CPL_E_ARGUMENT`; payload `parameter`, `found`.
pub const BOOL_OUT_OF_RANGE: InteropCode = code("interop_bool_out_of_range", CPL_E_ARGUMENT);
/// `CPL_E_RANGE`; payload `value` (decimal), `type` (`int64`, `int32`, `uint64`, `uint32`), `position`.
pub const INT_OUT_OF_RANGE: InteropCode = code("interop_int_out_of_range", CPL_E_RANGE);
/// `CPL_E_ARGUMENT`; payload `index`, `len`.
pub const INDEX_OUT_OF_RANGE: InteropCode = code("interop_index_out_of_range", CPL_E_ARGUMENT);
/// `CPL_E_ARGUMENT`; payload `type`, `variant`.
pub const VARIANT_OUT_OF_RANGE: InteropCode = code("interop_variant_out_of_range", CPL_E_ARGUMENT);
/// `CPL_E_ARGUMENT`; payload `type`, `expected`, `found`.
pub const ARITY_MISMATCH: InteropCode = code("interop_arity_mismatch", CPL_E_ARGUMENT);
/// `CPL_E_ARGUMENT`; payload `expected`, `found` (kind words), and `parameter` for an argument.
pub const KIND_MISMATCH: InteropCode = code("interop_kind_mismatch", CPL_E_ARGUMENT);
/// `CPL_E_ARGUMENT`; payload `name` or `type`.
pub const TYPE_UNKNOWN: InteropCode = code("interop_type_unknown", CPL_E_ARGUMENT);
/// `CPL_E_ARGUMENT`; payload `parameter`, `expected`, `found` (capability qualified names).
pub const CAPABILITY_MISMATCH: InteropCode = code("interop_capability_mismatch", CPL_E_ARGUMENT);
/// `CPL_E_ARGUMENT`; payload `pointer`, `detail` (the strict reading of toolchain 7.3, duplicate keys included).
pub const JSON_INVALID: InteropCode = code("interop_json_invalid", CPL_E_ARGUMENT);
/// `CPL_E_STATE`; payload `active_calls`.
pub const RUNTIME_BUSY: InteropCode = code("interop_runtime_busy", CPL_E_STATE);
/// `CPL_E_STATE`; payload `live_handles`, `live_caps`, `live_buffers`.
pub const RUNTIME_IN_USE: InteropCode = code("interop_runtime_in_use", CPL_E_STATE);
/// `CPL_E_STATE`; no payload.
pub const RUNTIME_POISONED: InteropCode = code("interop_runtime_poisoned", CPL_E_STATE);
/// `CPL_E_UNSUPPORTED`; payload `option`.
pub const OPTION_UNSUPPORTED: InteropCode = code("interop_option_unsupported", CPL_E_UNSUPPORTED);
/// `CPL_E_NOMEM`; no payload.
pub const OUT_OF_MEMORY: InteropCode = code("interop_out_of_memory", CPL_E_NOMEM);
/// `CPL_E_CANCELLED`; no payload.
pub const CANCELLED: InteropCode = code("interop_cancelled", CPL_E_CANCELLED);
/// `CPL_E_STATE` from `<prefix>_bind_<binding>` (`cpl run`: exit 1); payload `binding`, `kind` `"Unavailable"` when a
/// declarative binding's `[state]` could not be created (stdlib S45), and the adapter's own diagnostic as a note.
pub const BIND_FAILED: InteropCode = code("interop_bind_failed", CPL_E_STATE);

/// Every `interop_*` code with its C status, in the order of `spec/cabi.md` 4.9.
pub const CODES: &[InteropCode] = &[
    VERSION_MISMATCH,
    STRUCT_SIZE_UNSUPPORTED,
    HANDLE_INVALID,
    HANDLE_RELEASED,
    HANDLE_FOREIGN,
    NULL_POINTER,
    ARGUMENT_TYPE_MISMATCH,
    UTF8_INVALID,
    BOOL_OUT_OF_RANGE,
    INT_OUT_OF_RANGE,
    INDEX_OUT_OF_RANGE,
    VARIANT_OUT_OF_RANGE,
    ARITY_MISMATCH,
    KIND_MISMATCH,
    TYPE_UNKNOWN,
    CAPABILITY_MISMATCH,
    JSON_INVALID,
    RUNTIME_BUSY,
    RUNTIME_IN_USE,
    RUNTIME_POISONED,
    OPTION_UNSUPPORTED,
    OUT_OF_MEMORY,
    CANCELLED,
    BIND_FAILED,
];
