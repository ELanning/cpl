//! `std.foreign` orders that both engines reproduce (`spec/stdlib.md` 7, S45; `spec/cabi.md` 2.9, 6.5).

/// The variants of `std.foreign.ForeignErrorKind` in declaration order (`std/foreign.cpl`): index `i` is the variant
/// index of the enum and the value of the corresponding `CPL_FOREIGN_*` of `cpl.h` (`CPL_FOREIGN_EXCEPTION` 0, ...,
/// `CPL_FOREIGN_UNAVAILABLE` 6).
pub const KINDS: [&str; 7] = [
    "Exception",
    "InvalidArgument",
    "InvalidResult",
    "Transport",
    "Timeout",
    "Cancelled",
    "Unavailable",
];

/// The fixed detail keys of a C binding's `ForeignError`, in their order (stdlib S45): the runtime's `adapter` (the
/// binding name of semantics 6.9), `capability` (its qualified name), and `operation` (the operation's qualified
/// name); then, for a failure that a declarative binding produces, `foreign_type` (the C function's name) and, for an
/// error status, `foreign_code` (the status in decimal); then, for an `InvalidResult` that the runtime or a
/// declarative binding produced, `pointer` and `expected_type`. The adapter's own entries come between `foreign_code`
/// and `pointer`, in the order it added them, and are not listed here.
pub const DETAIL_ORDER: &[&str] = &[
    "adapter",
    "capability",
    "operation",
    "foreign_type",
    "foreign_code",
    "pointer",
    "expected_type",
];
