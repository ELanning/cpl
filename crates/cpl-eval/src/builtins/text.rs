//! `std.text` (stdlib 6). Every operation works on the scalar sequence;
//! because UTF-8 is self-synchronizing, Rust's byte-level `str` searches
//! (`starts_with`, `ends_with`, `contains`, `split`) find exactly the
//! scalar-aligned occurrences.

use super::{arity, confusion, constraint_error, int_arg, list_arg, requires_violation, text_arg};
use crate::fault::Fault;
use crate::interp::Interpreter;
use crate::value::Value;
use cpl_diag::quote_text;
use cpl_rtspec::messages::{self, render};
// The 25 `White_Space` scalars of stdlib S22, from the one table both engines
// read (stdlib S47): never Rust's `char::is_whitespace`, whose set may change
// with a Rust release.
use cpl_rtspec::whitespace::is_white_space;
use num_bigint::BigInt;
use std::rc::Rc;

type Out = Result<Value, Fault>;

/// The result text, sharing the argument's buffer when nothing changed.
fn same_or_new(original: &Rc<str>, result: &str) -> Value {
    if result.len() == original.len() {
        Value::Text(Rc::clone(original))
    } else {
        Value::text(result)
    }
}

/// `trim(t)`: without leading and trailing White_Space scalars (S22).
pub(super) fn trim(_it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("text_trim", args, 1)?;
    let t = text_arg("text_trim", args, 0)?;
    Ok(same_or_new(t, t.trim_matches(is_white_space)))
}

/// `trim_start(t)`: leading White_Space removed.
pub(super) fn trim_start(_it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("text_trim_start", args, 1)?;
    let t = text_arg("text_trim_start", args, 0)?;
    Ok(same_or_new(t, t.trim_start_matches(is_white_space)))
}

/// `trim_end(t)`: trailing White_Space removed.
pub(super) fn trim_end(_it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("text_trim_end", args, 1)?;
    let t = text_arg("text_trim_end", args, 0)?;
    Ok(same_or_new(t, t.trim_end_matches(is_white_space)))
}

/// `starts_with(t, prefix)`; `true` for `prefix == ""`.
pub(super) fn starts_with(_it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("text_starts_with", args, 2)?;
    let t = text_arg("text_starts_with", args, 0)?;
    let prefix = text_arg("text_starts_with", args, 1)?;
    Ok(Value::Bool(t.starts_with(&**prefix)))
}

/// `ends_with(t, suffix)`; `true` for `suffix == ""`.
pub(super) fn ends_with(_it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("text_ends_with", args, 2)?;
    let t = text_arg("text_ends_with", args, 0)?;
    let suffix = text_arg("text_ends_with", args, 1)?;
    Ok(Value::Bool(t.ends_with(&**suffix)))
}

/// `contains(t, needle)`: a contiguous scalar subsequence; `true` for `""`.
pub(super) fn contains(_it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("text_contains", args, 2)?;
    let t = text_arg("text_contains", args, 0)?;
    let needle = text_arg("text_contains", args, 1)?;
    Ok(Value::Bool(t.contains(&**needle)))
}

/// `split(t, sep)`: `requires not sep.is_empty()` (a fault); the pieces
/// between leftmost-first, non-overlapping occurrences of `sep` (S24):
/// `split("", sep) == [""]`, `split("a,", ",") == ["a", ""]`,
/// `split("aaa", "aa") == ["", "a"]`.
pub(super) fn split(it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    const ID: &str = "text_split";
    arity(ID, args, 2)?;
    let t = text_arg(ID, args, 0)?;
    let sep = text_arg(ID, args, 1)?;
    if sep.is_empty() {
        return Err(requires_violation(it, ID, 0, args));
    }
    Ok(Value::list(t.split(&**sep).map(Value::text).collect()))
}

/// `join(parts, sep)`: the parts with `sep` between consecutive ones;
/// `join([], sep) == ""`.
pub(super) fn join(_it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    const ID: &str = "text_join";
    arity(ID, args, 2)?;
    let parts = list_arg(ID, args, 0)?;
    let sep = text_arg(ID, args, 1)?;
    let mut out = String::new();
    for (i, part) in parts.iter().enumerate() {
        let Value::Text(s) = part else {
            return Err(confusion(ID, 0, "a List of Text", part));
        };
        if i > 0 {
            out.push_str(sep);
        }
        out.push_str(s);
    }
    Ok(Value::text(&out))
}

/// `to_upper(t)`: ASCII `a`-`z` to upper case, every other scalar
/// unchanged (S23).
pub(super) fn to_upper(_it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("text_to_upper", args, 1)?;
    let t = text_arg("text_to_upper", args, 0)?;
    Ok(if t.bytes().any(|b| b.is_ascii_lowercase()) {
        Value::text(&t.to_ascii_uppercase())
    } else {
        Value::Text(Rc::clone(t))
    })
}

/// `to_lower(t)`: ASCII `A`-`Z` to lower case, every other scalar
/// unchanged (S23).
pub(super) fn to_lower(_it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("text_to_lower", args, 1)?;
    let t = text_arg("text_to_lower", args, 0)?;
    Ok(if t.bytes().any(|b| b.is_ascii_uppercase()) {
        Value::text(&t.to_ascii_lowercase())
    } else {
        Value::Text(Rc::clone(t))
    })
}

/// The grammar of S25: `("+" | "-")? ASCII_DIGIT+`, no length limit,
/// leading zeros accepted, `-0` is `0`; nothing else (no whitespace, no
/// `_`, no other digit scripts).
pub(crate) fn parse_decimal(t: &str) -> Option<BigInt> {
    let (negative, digits) = match t.as_bytes().first() {
        Some(b'+') => (false, &t[1..]),
        Some(b'-') => (true, &t[1..]),
        _ => (false, t),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // Only ASCII digits reach the parser, so its own leniencies (a sign,
    // `_` separators) cannot widen the grammar.
    let magnitude = BigInt::parse_bytes(digits.as_bytes(), 10)?;
    Some(if negative { -magnitude } else { magnitude })
}

/// `parse_int(t)`: `Ok(n)` for the grammar of S25, else
/// `Err(ConstraintError { type_name: "Int", message })` (S44; the message
/// is informative only, stdlib 1.5).
pub(super) fn parse_int(it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("text_parse_int", args, 1)?;
    let t = text_arg("text_parse_int", args, 0)?;
    let p = it.program();
    Ok(match parse_decimal(t) {
        Some(n) => Value::ok(p, Value::Int(n)),
        None => Value::err(
            p,
            constraint_error(
                p,
                "Int",
                &render(&messages::PARSE_INT_FAILED, &[("quoted", &quote_text(t))]),
            ),
        ),
    })
}

/// `from_int(n)`: equal to `n.to_text()` (S10).
pub(super) fn from_int(_it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("text_from_int", args, 1)?;
    Ok(Value::text(&int_arg("text_from_int", args, 0)?.to_string()))
}

/// `chars(t)`: one single-scalar `Text` per scalar value, in order.
pub(super) fn chars(_it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("text_chars", args, 1)?;
    let t = text_arg("text_chars", args, 0)?;
    let mut buf = [0u8; 4];
    Ok(Value::list(
        t.chars()
            .map(|c| Value::text(c.encode_utf8(&mut buf)))
            .collect(),
    ))
}
