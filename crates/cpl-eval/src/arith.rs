//! `Int` arithmetic (semantics 6.3 and S37; stdlib 2.2 and S8;
//! `spec/toolchain.md` T9).
//!
//! `Int` is arbitrary precision, so `+`, `-`, `*`, and unary `-` are exact
//! and never fault. Division and remainder are implemented once, in
//! [`cpl_rtspec::arith::div_rem`], which [`div_rem`] calls (T9 as amended
//! by T70): the quotient truncates toward zero, the remainder
//! takes the dividend's sign (or is zero), and a zero divisor is the fault
//! `division_by_zero` (never a `contract_violation`: the operators are
//! intrinsics without a stub, stdlib S8). [`div`] and [`rem`] are views of
//! [`div_rem`], and [`binary`] is the interpreter's entry point for the
//! `Int` operators of the typed IR.

use crate::fault::Fault;
use crate::value::Value;
use cpl_rtspec::messages::{self, render};
use cpl_syntax::ast::BinaryOp;
use num_bigint::BigInt;

/// `(a / b, a % b)` exactly as semantics S37 specifies: `a / b` truncated
/// toward zero, `a % b == a - (a / b) * b`, so the remainder has the sign of
/// `a` (or is zero) and `|a % b| < |b|`, and `a == (a / b) * b + a % b`
/// always holds (`7 / -2 == -3`, `7 % -2 == 1`, `-7 / 2 == -3`,
/// `-7 % 2 == -1`).
///
/// A zero divisor is [`Fault::division_by_zero`] with `input: {"a", "b"}`;
/// the interpreter adds the enclosing declaration (`symbol`), the span of
/// the operator expression, and the backtrace.
///
/// T9 as amended by T70 (toolchain 1.7.1): the one division of CPL is
/// [`cpl_rtspec::arith::div_rem`], which this function calls, mapping its
/// `None` for a zero divisor to `division_by_zero`; `cpl-sema`'s constant
/// evaluator calls the same function and the C runtime mirrors it, so the
/// three cannot disagree.
pub fn div_rem(a: &BigInt, b: &BigInt) -> Result<(BigInt, BigInt), Fault> {
    cpl_rtspec::arith::div_rem(a, b).ok_or_else(|| Fault::division_by_zero(a, b))
}

/// `a / b`: the quotient of [`div_rem`].
pub fn div(a: &BigInt, b: &BigInt) -> Result<BigInt, Fault> {
    div_rem(a, b).map(|(q, _)| q)
}

/// `a % b`: the remainder of [`div_rem`].
pub fn rem(a: &BigInt, b: &BigInt) -> Result<BigInt, Fault> {
    div_rem(a, b).map(|(_, r)| r)
}

/// Unary `-a`.
pub fn negate(a: &BigInt) -> BigInt {
    -a
}

/// An `Int` operator of the typed IR applied to two evaluated operands
/// (stdlib 2.2): `+ - * / %` give an `Int`, `== != < <= > >=` a `Bool`.
/// `and`, `or`, and `++` are not `Int` operators; passing one is an
/// `internal_error` (`kind: "type_confusion"`), as is any operator the
/// checker would never pair with `Int` operands.
pub fn binary(op: BinaryOp, a: &BigInt, b: &BigInt) -> Result<Value, Fault> {
    Ok(match op {
        BinaryOp::Add => Value::Int(a + b),
        BinaryOp::Sub => Value::Int(a - b),
        BinaryOp::Mul => Value::Int(a * b),
        BinaryOp::Div => Value::Int(div(a, b)?),
        BinaryOp::Rem => Value::Int(rem(a, b)?),
        BinaryOp::Eq => Value::Bool(a == b),
        BinaryOp::Ne => Value::Bool(a != b),
        BinaryOp::Lt => Value::Bool(a < b),
        BinaryOp::Le => Value::Bool(a <= b),
        BinaryOp::Gt => Value::Bool(a > b),
        BinaryOp::Ge => Value::Bool(a >= b),
        BinaryOp::And | BinaryOp::Or | BinaryOp::Concat => {
            return Err(Fault::internal(
                "type_confusion",
                render(
                    &messages::DETAIL_TYPE_CONFUSION_NOT_INT,
                    &[("operator", op.symbol())],
                ),
            ))
        }
    })
}
