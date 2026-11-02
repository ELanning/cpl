//! The registry entries that are not callable: the builtin types of
//! `std/prelude.cpl` and `std/gen.cpl` (`int`, `bool`, `text`, `bytes`,
//! `list`, `gen`) and the generators of `std.gen` (stdlib 8.5). They are
//! registered so that std loading knows every id (stdlib 10.2); a call is
//! an impossible state of the typed IR, reported as `internal_error`.
//! Generator expressions are lowered to `ExprKind::Gen` (toolchain T18) and
//! drawn by the property runner, never called; `gen_range` still checks its
//! precondition first (stdlib 8.2: `lo <= hi` faults the property).

use super::{arity, check_gen_range, int_arg};
use crate::fault::Fault;
use crate::interp::Interpreter;
use crate::value::Value;
use cpl_rtspec::messages::{self, render};

type Out = Result<Value, Fault>;

fn builtin_type(id: &str) -> Fault {
    Fault::internal(
        "builtin_type_called",
        render(&messages::DETAIL_BUILTIN_TYPE_CALLED, &[("id", id)]),
    )
}

fn generator(id: &str) -> Fault {
    Fault::internal(
        "generator_not_a_value",
        render(&messages::DETAIL_GENERATOR_NOT_A_VALUE, &[("id", id)]),
    )
}

pub(super) fn type_int(_it: &mut Interpreter<'_>, _args: &[Value]) -> Out {
    Err(builtin_type("int"))
}

pub(super) fn type_bool(_it: &mut Interpreter<'_>, _args: &[Value]) -> Out {
    Err(builtin_type("bool"))
}

pub(super) fn type_text(_it: &mut Interpreter<'_>, _args: &[Value]) -> Out {
    Err(builtin_type("text"))
}

pub(super) fn type_bytes(_it: &mut Interpreter<'_>, _args: &[Value]) -> Out {
    Err(builtin_type("bytes"))
}

pub(super) fn type_list(_it: &mut Interpreter<'_>, _args: &[Value]) -> Out {
    Err(builtin_type("list"))
}

pub(super) fn type_gen(_it: &mut Interpreter<'_>, _args: &[Value]) -> Out {
    Err(builtin_type("gen"))
}

pub(super) fn gen_int(_it: &mut Interpreter<'_>, _args: &[Value]) -> Out {
    Err(generator("gen_int"))
}

pub(super) fn gen_positive_int(_it: &mut Interpreter<'_>, _args: &[Value]) -> Out {
    Err(generator("gen_positive_int"))
}

pub(super) fn gen_bool(_it: &mut Interpreter<'_>, _args: &[Value]) -> Out {
    Err(generator("gen_bool"))
}

pub(super) fn gen_text(_it: &mut Interpreter<'_>, _args: &[Value]) -> Out {
    Err(generator("gen_text"))
}

pub(super) fn gen_list(_it: &mut Interpreter<'_>, _args: &[Value]) -> Out {
    Err(generator("gen_list"))
}

pub(super) fn gen_option(_it: &mut Interpreter<'_>, _args: &[Value]) -> Out {
    Err(generator("gen_option"))
}

/// `range(lo, hi)`: `requires lo <= hi` is checked (the builtin
/// `contract_violation`), then the call is an internal error like every
/// generator's.
pub(super) fn gen_range(it: &mut Interpreter<'_>, args: &[Value]) -> Out {
    arity("gen_range", args, 2)?;
    let lo = int_arg("gen_range", args, 0)?;
    let hi = int_arg("gen_range", args, 1)?;
    check_gen_range(it, lo, hi)?;
    Err(generator("gen_range"))
}
