//! The one `Int` division of CPL (`spec/toolchain.md` T9 as amended by T70; semantics S37; stdlib S8).
//!
//! Division and remainder are defined once, here: `cpl_eval::arith::div_rem` delegates to [`div_rem`] (and raises
//! `division_by_zero` for its `None`), `cpl-sema`'s constant evaluator calls it, and the native runtime's one
//! division, `cplrt_int_divrem`, mirrors it (`spec/cabi.md` 2.5), so the three cannot disagree on a sign.

use num_bigint::{BigInt, Sign};

/// `(a / b, a % b)` exactly as semantics S37 specifies: the quotient truncated toward zero and the remainder derived
/// from it as `a - (a / b) * b`, so the remainder has the sign of `a` (or is zero), `|a % b| < |b|`, and
/// `a == (a / b) * b + a % b` always holds (`7 / -2 == -3`, `7 % -2 == 1`, `-7 / 2 == -3`, `-7 % 2 == -1`). `None` for
/// a zero divisor, which every caller reports as the fault `division_by_zero` with `input: {"a", "b"}` (S8).
pub fn div_rem(a: &BigInt, b: &BigInt) -> Option<(BigInt, BigInt)> {
    if b.sign() == Sign::NoSign {
        return None;
    }
    // num-bigint's `/` truncates toward zero; the remainder is derived from the quotient by the definition of S37
    // rather than taken from the host's `%`, so the two can never disagree.
    let quotient = a / b;
    let remainder = a - &quotient * b;
    Some((quotient, remainder))
}
