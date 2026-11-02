//! `std.bytes` (stdlib 3.4, S14).

use super::{
    arity, bytes_arg, confusion, constraint_error, list_arg, requires_violation, text_arg,
};
use crate::fault::Fault;
use crate::interp::Interpreter;
use crate::value::Value;
use cpl_rtspec::messages::{self, render};
use num_traits::ToPrimitive;
use std::rc::Rc;

type Out = Result<Value, Fault>;

/// `from_list(values)`: `requires forall v in values: v >= 0 and v <= 255`
/// (a fault on the first value outside 0..255, checked left to right like
/// the `forall` it is); the bytes with those values.
pub(super) fn from_list(it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    const ID: &str = "bytes_from_list";
    arity(ID, args, 1)?;
    let values = list_arg(ID, args, 0)?;
    let mut out = Vec::with_capacity(values.len());
    for v in values.iter() {
        let Value::Int(n) = v else {
            return Err(confusion(ID, 0, "a List of Int", v));
        };
        match n.to_u8() {
            Some(b) => out.push(b),
            None => return Err(requires_violation(it, ID, 0, args)),
        }
    }
    Ok(Value::Bytes(Rc::from(out)))
}

/// `to_list(bytes)`: the byte values as `Int`s, in order.
pub(super) fn to_list(_it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("bytes_to_list", args, 1)?;
    let b = bytes_arg("bytes_to_list", args, 0)?;
    Ok(Value::list(b.iter().map(|&x| Value::int(x)).collect()))
}

/// `from_text(text)`: the UTF-8 encoding of the scalar sequence.
pub(super) fn from_text(_it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("bytes_from_text", args, 1)?;
    let t = text_arg("bytes_from_text", args, 0)?;
    Ok(Value::bytes(t.as_bytes()))
}

/// `to_text(bytes)`: strict UTF-8 decoding (no overlong forms, no encoded
/// surrogates, nothing above U+10FFFF, no truncated sequence): `Ok(text)`,
/// else `Err(ConstraintError { type_name: "Text", message: "invalid UTF-8
/// at byte <offset>" })` with the 0-based offset of the first byte that is
/// not part of a valid prefix.
pub(super) fn to_text(it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("bytes_to_text", args, 1)?;
    let b = bytes_arg("bytes_to_text", args, 0)?;
    let p = it.program();
    Ok(match std::str::from_utf8(b) {
        Ok(text) => Value::ok(p, Value::text(text)),
        Err(e) => Value::err(
            p,
            constraint_error(
                p,
                "Text",
                &render(
                    &messages::BYTES_TO_TEXT_FAILED,
                    &[("offset", &e.valid_up_to().to_string())],
                ),
            ),
        ),
    })
}
