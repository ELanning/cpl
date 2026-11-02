//! The prelude's builtins: `flatten` (stdlib 1.6) and the members of `Int`
//! (2.3), `Text` and `Bytes` (3.3, 3.4), `List` (4.2, 4.5), `Option` (5.1),
//! and `Result` (5.2).

use super::{arity, bytes_arg, confusion, int_arg, list_arg, requires_violation, text_arg};
use crate::fault::Fault;
use crate::interp::Interpreter;
use crate::value::Value;
use cpl_hir::Program;
use num_bigint::BigInt;
use num_traits::{Signed, ToPrimitive};
use std::rc::Rc;

type Out = Result<Value, Fault>;

// ---- flatten -----------------------------------------------------------------

/// `flatten(lists)`: the inner lists concatenated in order; `[]` for `[]`.
pub(super) fn flatten(_it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    const ID: &str = "flatten";
    arity(ID, args, 1)?;
    let outer = list_arg(ID, args, 0)?;
    let mut inner = Vec::with_capacity(outer.len());
    for x in outer.iter() {
        match x {
            Value::List(xs) => inner.push(xs),
            other => return Err(confusion(ID, 0, "a List of Lists", other)),
        }
    }
    let non_empty: Vec<&Rc<Vec<Value>>> = inner.into_iter().filter(|xs| !xs.is_empty()).collect();
    Ok(match non_empty.as_slice() {
        [] => Value::empty_list(),
        [only] => Value::List(Rc::clone(only)),
        many => Value::list(many.iter().flat_map(|xs| xs.iter().cloned()).collect()),
    })
}

// ---- Int -----------------------------------------------------------------------

/// `n.to_text()`: optional `-`, then decimal digits without leading zeros.
pub(super) fn int_to_text(_it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("int_to_text", args, 1)?;
    Ok(Value::text(&int_arg("int_to_text", args, 0)?.to_string()))
}

/// `n.abs()`.
pub(super) fn int_abs(_it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("int_abs", args, 1)?;
    Ok(Value::Int(int_arg("int_abs", args, 0)?.abs()))
}

/// `a.min(b)`: the smaller operand, `a` when equal.
pub(super) fn int_min(_it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("int_min", args, 2)?;
    let a = int_arg("int_min", args, 0)?;
    let b = int_arg("int_min", args, 1)?;
    Ok(Value::Int(if b < a { b.clone() } else { a.clone() }))
}

/// `a.max(b)`: the larger operand, `a` when equal.
pub(super) fn int_max(_it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("int_max", args, 2)?;
    let a = int_arg("int_max", args, 0)?;
    let b = int_arg("int_max", args, 1)?;
    Ok(Value::Int(if b > a { b.clone() } else { a.clone() }))
}

// ---- Text and Bytes members --------------------------------------------------

/// `t.len`: the number of Unicode scalar values.
pub(super) fn text_len(it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("text_len", args, 1)?;
    Ok(Value::int(it.text_length(text_arg("text_len", args, 0)?)))
}

/// `t.is_empty()`.
pub(super) fn text_is_empty(_it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("text_is_empty", args, 1)?;
    Ok(Value::Bool(text_arg("text_is_empty", args, 0)?.is_empty()))
}

/// `b.len`: the number of bytes.
pub(super) fn bytes_len(_it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("bytes_len", args, 1)?;
    Ok(Value::int(bytes_arg("bytes_len", args, 0)?.len()))
}

/// `b.is_empty()`.
pub(super) fn bytes_is_empty(_it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("bytes_is_empty", args, 1)?;
    Ok(Value::Bool(
        bytes_arg("bytes_is_empty", args, 0)?.is_empty(),
    ))
}

// ---- List ------------------------------------------------------------------------

/// `xs.len`.
pub(super) fn list_len(_it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("list_len", args, 1)?;
    Ok(Value::int(list_arg("list_len", args, 0)?.len()))
}

/// `xs.is_empty()`.
pub(super) fn list_is_empty(_it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("list_is_empty", args, 1)?;
    Ok(Value::Bool(list_arg("list_is_empty", args, 0)?.is_empty()))
}

/// The count `n` of `take`/`drop` as an index into a list of length `len`:
/// `min(n, len)` for `n >= 0`. The comparison with the length happens
/// before any conversion to a machine integer, so a count beyond `usize`
/// clamps instead of overflowing (stdlib 4.1 implementation note).
fn clamp_count(n: &BigInt, len: usize) -> usize {
    match n.to_usize() {
        Some(m) if m < len => m,
        _ => len,
    }
}

/// `xs.take(n)`: `requires n >= 0` (a fault, stdlib S16); the first
/// `min(n, xs.len)` elements.
pub(super) fn list_take(it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    const ID: &str = "list_take";
    arity(ID, args, 2)?;
    let xs = list_arg(ID, args, 0)?;
    let n = int_arg(ID, args, 1)?;
    if n.is_negative() {
        return Err(requires_violation(it, ID, 0, args));
    }
    let k = clamp_count(n, xs.len());
    Ok(if k == xs.len() {
        Value::List(Rc::clone(xs))
    } else {
        Value::list(xs[..k].to_vec())
    })
}

/// `xs.drop(n)`: `requires n >= 0` (a fault, stdlib S16); the elements
/// after the first `min(n, xs.len)`.
pub(super) fn list_drop(it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    const ID: &str = "list_drop";
    arity(ID, args, 2)?;
    let xs = list_arg(ID, args, 0)?;
    let n = int_arg(ID, args, 1)?;
    if n.is_negative() {
        return Err(requires_violation(it, ID, 0, args));
    }
    let k = clamp_count(n, xs.len());
    Ok(if k == 0 {
        Value::List(Rc::clone(xs))
    } else {
        Value::list(xs[k..].to_vec())
    })
}

/// `xs.drop_last()`: `xs` without its last element; `[]` for `[]`.
pub(super) fn list_drop_last(_it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("list_drop_last", args, 1)?;
    let xs = list_arg("list_drop_last", args, 0)?;
    Ok(if xs.is_empty() {
        Value::List(Rc::clone(xs))
    } else {
        Value::list(xs[..xs.len() - 1].to_vec())
    })
}

/// `xs.push(v)`: a new list ending in `v`; `xs` is unchanged.
pub(super) fn list_push(_it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("list_push", args, 2)?;
    let xs = list_arg("list_push", args, 0)?;
    let mut out = Vec::with_capacity(xs.len() + 1);
    out.extend_from_slice(xs);
    out.push(args[1].clone());
    Ok(Value::list(out))
}

/// `Some(element)` or `None`.
fn element(p: &Program, xs: &[Value], index: Option<usize>) -> Value {
    match index.and_then(|i| xs.get(i)) {
        Some(x) => Value::some(p, x.clone()),
        None => Value::none(p),
    }
}

/// `xs.get(i)`: `Some` of element `i` for `0 <= i < xs.len`, else `None`
/// (a negative index included; no indexing from the end).
pub(super) fn list_get(it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("list_get", args, 2)?;
    let xs = list_arg("list_get", args, 0)?;
    let i = int_arg("list_get", args, 1)?;
    Ok(element(it.program(), xs, i.to_usize()))
}

/// `xs.first()`: `xs.get(0)`.
pub(super) fn list_first(it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("list_first", args, 1)?;
    let xs = list_arg("list_first", args, 0)?;
    Ok(element(it.program(), xs, Some(0)))
}

/// `xs.last()`: `xs.get(xs.len - 1)`; `None` for `[]`.
pub(super) fn list_last(it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("list_last", args, 1)?;
    let xs = list_arg("list_last", args, 0)?;
    Ok(element(it.program(), xs, xs.len().checked_sub(1)))
}

/// `xs.reverse()`.
pub(super) fn list_reverse(_it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("list_reverse", args, 1)?;
    let xs = list_arg("list_reverse", args, 0)?;
    Ok(Value::list(xs.iter().rev().cloned().collect()))
}

/// `xs.contains(v)`: some element `== v` (structural equality).
pub(super) fn list_contains(_it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("list_contains", args, 2)?;
    let xs = list_arg("list_contains", args, 0)?;
    Ok(Value::Bool(
        xs.iter().any(|x| x.structurally_equal(&args[1])),
    ))
}

// ---- Option ------------------------------------------------------------------------

fn option<'a>(p: &Program, id: &str, args: &'a [Value]) -> Result<Option<&'a Value>, Fault> {
    args[0]
        .as_option(p)
        .ok_or_else(|| confusion(id, 0, "an Option", &args[0]))
}

/// `o.is_some()`.
pub(super) fn option_is_some(it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("option_is_some", args, 1)?;
    Ok(Value::Bool(
        option(it.program(), "option_is_some", args)?.is_some(),
    ))
}

/// `o.is_none()`.
pub(super) fn option_is_none(it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("option_is_none", args, 1)?;
    Ok(Value::Bool(
        option(it.program(), "option_is_none", args)?.is_none(),
    ))
}

/// `o.unwrap_or(d)`: the payload of `Some`, else `d`.
pub(super) fn option_unwrap_or(it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("option_unwrap_or", args, 2)?;
    Ok(match option(it.program(), "option_unwrap_or", args)? {
        Some(v) => v.clone(),
        None => args[1].clone(),
    })
}

/// `o.ok_or(e)`: `Some(v)` gives `Ok(v)`, `None` gives `Err(e)`.
pub(super) fn option_ok_or(it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("option_ok_or", args, 2)?;
    let p = it.program();
    Ok(match option(p, "option_ok_or", args)? {
        Some(v) => Value::ok(p, v.clone()),
        None => Value::err(p, args[1].clone()),
    })
}

// ---- Result ------------------------------------------------------------------------

fn result<'a>(
    p: &Program,
    id: &str,
    args: &'a [Value],
) -> Result<Result<&'a Value, &'a Value>, Fault> {
    args[0]
        .as_result(p)
        .ok_or_else(|| confusion(id, 0, "a Result", &args[0]))
}

/// `r.is_ok()`.
pub(super) fn result_is_ok(it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("result_is_ok", args, 1)?;
    Ok(Value::Bool(
        result(it.program(), "result_is_ok", args)?.is_ok(),
    ))
}

/// `r.is_err()`.
pub(super) fn result_is_err(it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("result_is_err", args, 1)?;
    Ok(Value::Bool(
        result(it.program(), "result_is_err", args)?.is_err(),
    ))
}

/// `r.unwrap_or(d)`: the payload of `Ok`, else `d`.
pub(super) fn result_unwrap_or(it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("result_unwrap_or", args, 2)?;
    Ok(match result(it.program(), "result_unwrap_or", args)? {
        Ok(v) => v.clone(),
        Err(_) => args[1].clone(),
    })
}

/// `r.ok()`: `Ok(v)` gives `Some(v)`, `Err(_)` gives `None`.
pub(super) fn result_ok(it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("result_ok", args, 1)?;
    let p = it.program();
    Ok(match result(p, "result_ok", args)? {
        Ok(v) => Value::some(p, v.clone()),
        Err(_) => Value::none(p),
    })
}

/// `r.err()`: `Err(e)` gives `Some(e)`, `Ok(_)` gives `None`.
pub(super) fn result_err(it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("result_err", args, 1)?;
    let p = it.program();
    Ok(match result(p, "result_err", args)? {
        Ok(_) => Value::none(p),
        Err(e) => Value::some(p, e.clone()),
    })
}
