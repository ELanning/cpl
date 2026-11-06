//! The builtin registry and the standard library's runtime
//! (`spec/stdlib.md` 10.4-10.6; `spec/toolchain.md` 3.6 "Dispatch";
//! semantics 6.10 and S58).
//!
//! Every `@builtin("<id>")` declaration of `std/*.cpl` is implemented here
//! and found by its id through [`lookup`]; the interpreter calls the
//! [`BuiltinFn`] with the evaluated arguments in declaration order, the
//! receiver first. A builtin checks its own `requires` clauses before it
//! runs and raises `contract_violation` on a violation (stdlib S39,
//! semantics S58), so **the interpreter must not evaluate a builtin stub's
//! `requires` or `ensures` clauses**: the `ensures` clauses are trusted and
//! never evaluated in any mode (S39).
//!
//! A violated builtin precondition carries the shape of stdlib 10.4:
//! `symbol` and `qualified_name` the stub's qualified name
//! (`std.prelude.List.take`, S43), `facet: "contract"`, `clause_kind:
//! "requires"`, `attributed_to: "caller"`, `clause` the clause text
//! (`n >= 0`), `span` the clause in the stub file, `input` the receiver
//! under the key `self` and the other arguments by parameter name, `caller`
//! the enclosing declaration ([`Interpreter::current_def`]; absent at the
//! top level of `cpl run`), and `status: "failed"`.
//!
//! The six type ids (`int`, `bool`, `text`, `bytes`, `list`, `gen`) and the
//! seven `gen_*` ids are registered so that std loading knows them
//! (`std_builtin_missing`, stdlib 10.2) but are not callable: a builtin type
//! is never called, and a generator is lowered to `ExprKind::Gen` and
//! interpreted by the property runner (toolchain T18). Calling one is an
//! `internal_error`; `gen_range` first checks its `requires lo <= hi`
//! ([`check_gen_range`]; the property runner checks it at setup with
//! [`check_gen_range_at`]).
//!
//! The registry's ids are [`cpl_rtspec::builtins::IDS`] (tested equal,
//! toolchain 1.7.4), and a precondition's facts come from the program's
//! stub or, for a program that does not declare it, from the fallback of
//! [`cpl_rtspec::builtins`] ([`requires_spec`]; stdlib S47).
//!
//! The intrinsics of stdlib 10.6 (the `Int` operators, `==`, `++`, `not`,
//! `and`/`or`, `forall`, `check<T>`) are not in the registry; the
//! interpreter evaluates them with [`crate::arith`],
//! [`Value::structurally_equal`], [`Value::concat`], and, for `check<R>`,
//! [`check_order`] and [`check_failure`] below.

mod bytes;
mod gen;
mod prelude;
mod text;

use crate::fault::{AttributedTo, ClauseKind, Fault};
use crate::interp::Interpreter;
use crate::json::{value_text_untyped, value_to_json_untyped};
use crate::types::{chain, type_name};
use crate::value::Value;
use cpl_diag::{DeclName, Facet, Span, StableId};
use cpl_hir::{DefId, DefKind, FnBody, Program};
use cpl_rtspec::messages::{self, render};
use num_bigint::BigInt;
use serde_json::{json, Map};
use std::rc::Rc;

/// The Rust implementation of a builtin (`spec/toolchain.md` 3.6): called
/// with the evaluated arguments in declaration order, receiver first.
pub type BuiltinFn = fn(&mut Interpreter<'_>, &[Value]) -> Result<Value, Fault>;

/// The registry of stdlib 10.6, sorted by id (binary search).
static REGISTRY: [(&str, BuiltinFn); 59] = [
    ("bool", gen::type_bool),
    ("bytes", gen::type_bytes),
    ("bytes_from_list", bytes::from_list),
    ("bytes_from_text", bytes::from_text),
    ("bytes_is_empty", prelude::bytes_is_empty),
    ("bytes_len", prelude::bytes_len),
    ("bytes_to_list", bytes::to_list),
    ("bytes_to_text", bytes::to_text),
    ("flatten", prelude::flatten),
    ("gen", gen::type_gen),
    ("gen_bool", gen::gen_bool),
    ("gen_int", gen::gen_int),
    ("gen_list", gen::gen_list),
    ("gen_option", gen::gen_option),
    ("gen_positive_int", gen::gen_positive_int),
    ("gen_range", gen::gen_range),
    ("gen_text", gen::gen_text),
    ("int", gen::type_int),
    ("int_abs", prelude::int_abs),
    ("int_max", prelude::int_max),
    ("int_min", prelude::int_min),
    ("int_to_text", prelude::int_to_text),
    ("list", gen::type_list),
    ("list_contains", prelude::list_contains),
    ("list_drop", prelude::list_drop),
    ("list_drop_last", prelude::list_drop_last),
    ("list_first", prelude::list_first),
    ("list_get", prelude::list_get),
    ("list_is_empty", prelude::list_is_empty),
    ("list_last", prelude::list_last),
    ("list_len", prelude::list_len),
    ("list_push", prelude::list_push),
    ("list_reverse", prelude::list_reverse),
    ("list_take", prelude::list_take),
    ("option_is_none", prelude::option_is_none),
    ("option_is_some", prelude::option_is_some),
    ("option_ok_or", prelude::option_ok_or),
    ("option_unwrap_or", prelude::option_unwrap_or),
    ("result_err", prelude::result_err),
    ("result_is_err", prelude::result_is_err),
    ("result_is_ok", prelude::result_is_ok),
    ("result_ok", prelude::result_ok),
    ("result_unwrap_or", prelude::result_unwrap_or),
    ("text", gen::type_text),
    ("text_chars", text::chars),
    ("text_contains", text::contains),
    ("text_ends_with", text::ends_with),
    ("text_from_int", text::from_int),
    ("text_is_empty", prelude::text_is_empty),
    ("text_join", text::join),
    ("text_len", prelude::text_len),
    ("text_parse_int", text::parse_int),
    ("text_split", text::split),
    ("text_starts_with", text::starts_with),
    ("text_to_lower", text::to_lower),
    ("text_to_upper", text::to_upper),
    ("text_trim", text::trim),
    ("text_trim_end", text::trim_end),
    ("text_trim_start", text::trim_start),
];

/// The implementation of the builtin with registry id `id` (stdlib 10.6),
/// or `None` for an id the registry does not know, which fails std loading
/// as `std_builtin_missing` (stdlib 10.2).
pub fn lookup(id: &str) -> Option<BuiltinFn> {
    REGISTRY
        .binary_search_by(|(key, _)| (*key).cmp(id))
        .ok()
        .map(|i| REGISTRY[i].1)
}

/// Every registry id, in sorted order: exactly the 59 ids of stdlib 10.6.
pub fn ids() -> impl Iterator<Item = &'static str> {
    REGISTRY.iter().map(|(id, _)| *id)
}

/// The std declaration carrying `@builtin(id)`: the `fn` stub, or the
/// builtin type for a type id.
pub fn stub_of(p: &Program, id: &str) -> Option<DefId> {
    p.defs
        .iter()
        .find(|d| match &d.kind {
            DefKind::Fn(f) => matches!(&f.body, FnBody::Builtin(b) if b == id),
            DefKind::BuiltinType(t) => t.builtin == id,
            _ => false,
        })
        .map(|d| d.id)
}

// ===========================================================================
// check<R> and ConstraintError (stdlib 1.4, 1.5, S44; semantics 6.10)
// ===========================================================================

/// The refinements `check<R>(v)` evaluates, in evaluation order:
/// `chain(R)` from the root base upward (semantics 6.10), each declaration
/// as named through `R`'s declared base chain (S44, S55). For
/// `type Small = Positive where self < 10` it is `[Positive, Small]`.
/// Empty when `refined` is not a refinement declaration.
pub fn check_order(p: &Program, refined: DefId) -> Vec<DefId> {
    if p.def(refined)
        .kind
        .as_struct()
        .is_some_and(|s| s.invariant.is_some())
    {
        return vec![refined];
    }
    let mut order = chain(p, refined);
    order.reverse();
    order
}

/// `ConstraintError.type_name` for a constraint of `def` (stdlib S44): the
/// bare name of a prelude type, the qualified name of every other
/// declaration (`reader.pages.Positive`, `std.gen.Positive`).
pub fn constraint_type_name(p: &Program, def: DefId) -> String {
    type_name(p, def)
}

/// `ConstraintError { type_name, message }` (stdlib 1.5), with the fields
/// placed by name in the prelude's declaration.
pub fn constraint_error(p: &Program, type_name: &str, message: &str) -> Value {
    let def = p.std_defs.constraint_error;
    let mut fields = vec![Value::text(type_name), Value::text(message)];
    if let Some(s) = p.defs.get(def.0 as usize).and_then(|d| d.kind.as_struct()) {
        if s.fields.len() == 2 && s.fields[0].name == "message" {
            fields.swap(0, 1);
        }
    }
    Value::Struct {
        def,
        fields: Rc::from(fields),
    }
}

/// The result of `check<R>(v)` when the predicate of the refinement
/// declaration `failed` is the first of [`check_order`] to evaluate to
/// `false` (semantics 6.10): `Err(ConstraintError { type_name: <S44 name of
/// failed>, message: "<value text of v> does not satisfy <predicate text>"
/// })`.
pub fn check_failure(p: &Program, failed: DefId, value: &Value) -> Value {
    let predicate = p
        .defs
        .get(failed.0 as usize)
        .and_then(|d| d.kind.as_refinement())
        .map_or("", |r| r.predicate.text.as_str());
    let message = render(
        &messages::CHECK_FAILED,
        &[
            ("value", &value_text_untyped(p, value)),
            ("predicate", predicate),
        ],
    );
    Value::err(
        p,
        constraint_error(p, &constraint_type_name(p, failed), &message),
    )
}

/// The precondition of `std.gen.range` (`requires lo <= hi`, stdlib 8.5),
/// which the property runner checks once per property before case 0
/// (toolchain 1.7.2); a violation is the builtin `contract_violation` with
/// `input: {"lo", "hi"}` and `caller` the innermost active declaration of
/// `it`. It is [`check_gen_range_at`] with `it`'s program and
/// [`Interpreter::current_def`].
pub fn check_gen_range(it: &Interpreter<'_>, lo: &BigInt, hi: &BigInt) -> Result<(), Fault> {
    check_gen_range_at(it.program(), it.current_def(), lo, hi)
}

/// The precondition of `std.gen.range` with its `caller` given (`None` at
/// the top level), without an interpreter (toolchain 1.7.4): the generic
/// runner checks a property's ranges with the property as `caller`, for
/// either engine (toolchain 1.7.2; `spec/cabi.md` 7.3).
pub fn check_gen_range_at(
    p: &Program,
    caller: Option<DefId>,
    lo: &BigInt,
    hi: &BigInt,
) -> Result<(), Fault> {
    if lo <= hi {
        Ok(())
    } else {
        Err(requires_violation_at(
            p,
            caller,
            "gen_range",
            0,
            &[Value::Int(lo.clone()), Value::Int(hi.clone())],
        ))
    }
}

// ===========================================================================
// Preconditions (stdlib 10.4, S39; semantics 6.10)
// ===========================================================================

/// The facts of one precondition clause of a builtin (toolchain 1.7.4): the
/// symbol a violation blames (the stub's identity, its qualified name for a
/// member stub, S43), the canonical clause text, the clause's span in the
/// stub file, and the `input` key of each argument in order (`self` for a
/// member's receiver). `spec/cabi.md` 3.2: the emitter writes them into the
/// native tables with this same function.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequiresFacts {
    pub symbol: StableId,
    pub clause: String,
    pub span: Option<Span>,
    pub inputs: Vec<String>,
}

/// The facts of requires clause `clause` (0-based) of builtin `id`: from
/// the program's stub when it declares one ([`stub_of`]; every program a
/// project check produces does), else the fallback of
/// [`cpl_rtspec::builtins`] (a program built by hand), which holds the
/// stubs' own texts (stdlib S47, toolchain 1.7.4).
///
/// The symbol is the stub's identity, else the fallback symbol; the clause
/// text and span the stub's clause, else the fallback text and no span; the
/// input names the stub function's parameter names (the receiver of a
/// member stub as `self`), else the fallback inputs.
///
/// Panics when neither the stub nor the registry has clause `clause` of
/// `id`: only a toolchain defect asks for it (every caller names a clause
/// that stdlib 10.4 lists).
pub fn requires_spec(p: &Program, id: &str, clause: usize) -> RequiresFacts {
    let fallback = cpl_rtspec::builtins::spec(id);
    let fallback_clause = fallback.and_then(|b| b.requires.get(clause));
    let stub = stub_of(p, id).map(|d| p.def(d));
    let stub_fn = stub.and_then(|d| d.kind.as_fn().map(|f| (d, f)));
    let stub_clause = stub_fn.and_then(|(_, f)| f.requires.get(clause));
    let text = match (stub_clause, fallback_clause) {
        (Some(c), _) => c.text.clone(),
        (None, Some(r)) => r.clause.to_string(),
        (None, None) => panic!("requires_spec: the builtin `{id}` has no requires clause {clause}"),
    };
    let symbol = match (stub, fallback) {
        (Some(d), _) => d.stable_id.clone(),
        (None, Some(b)) => {
            StableId::parse(b.fallback_symbol).expect("a std qualified name is a stable id")
        }
        (None, None) => panic!("requires_spec: `{id}` is no builtin of the registry"),
    };
    let inputs: Vec<String> = match (stub_fn, fallback_clause) {
        (Some((d, f)), _) => f
            .params
            .iter()
            .enumerate()
            .map(|(i, param)| {
                if i == 0 && matches!(d.name, DeclName::Member { .. }) {
                    "self".to_string()
                } else {
                    param.name.clone()
                }
            })
            .collect(),
        (None, Some(r)) => r.inputs.iter().map(|s| s.to_string()).collect(),
        (None, None) => panic!("requires_spec: the builtin `{id}` has no requires clause {clause}"),
    };
    RequiresFacts {
        symbol,
        clause: text,
        span: stub_clause.map(|c| c.span),
        inputs,
    }
}

/// The `contract_violation` of requires clause `clause` (0-based) of the
/// builtin `id`, called with `args`, by the innermost active declaration of
/// `it` (stdlib 10.4): [`requires_violation_at`] with
/// [`Interpreter::current_def`].
pub(crate) fn requires_violation(
    it: &Interpreter<'_>,
    id: &str,
    clause: usize,
    args: &[Value],
) -> Fault {
    requires_violation_at(it.program(), it.current_def(), id, clause, args)
}

/// The precondition fault of stdlib 10.4 for requires clause `clause`
/// (0-based) of the builtin `id` called with `args` by `caller` (`None` at
/// the top level, where the fault has no `caller`): `symbol` and `clause`
/// from [`requires_spec`], `facet: "contract"`, `clause_kind: "requires"`,
/// `attributed_to: "caller"`, `span` the stub's clause, `input` each
/// argument as value JSON under its input name, and `caller` the qualified
/// name of the calling declaration.
pub fn requires_violation_at(
    p: &Program,
    caller: Option<DefId>,
    id: &str,
    clause: usize,
    args: &[Value],
) -> Fault {
    let facts = requires_spec(p, id, clause);
    let mut input = Map::new();
    for (name, value) in facts.inputs.iter().zip(args) {
        input.insert(name.clone(), value_to_json_untyped(p, value));
    }
    let mut fault =
        Fault::contract_violation(ClauseKind::Requires, AttributedTo::Caller, &facts.clause)
            .with_symbol(facts.symbol)
            .with_facet(Facet::Contract)
            .with_data("input", serde_json::Value::Object(input));
    if let Some(span) = facts.span {
        fault = fault.with_span(span);
    }
    if let Some(caller) = caller {
        if (caller.0 as usize) < p.defs.len() {
            fault = fault.with_data("caller", json!(p.display_path(caller)));
        }
    }
    fault
}

// ===========================================================================
// Argument access (a mismatch is a type confusion: an internal error)
// ===========================================================================

pub(crate) fn arity(id: &str, args: &[Value], n: usize) -> Result<(), Fault> {
    if args.len() == n {
        Ok(())
    } else {
        Err(Fault::internal(
            "builtin_arity",
            render(
                &messages::DETAIL_BUILTIN_ARITY,
                &[
                    ("id", id),
                    ("expected", &n.to_string()),
                    ("found", &args.len().to_string()),
                ],
            ),
        ))
    }
}

pub(crate) fn confusion(id: &str, index: usize, expected: &str, found: &Value) -> Fault {
    Fault::internal(
        "type_confusion",
        render(
            &messages::DETAIL_TYPE_CONFUSION_BUILTIN,
            &[
                ("index", &index.to_string()),
                ("id", id),
                ("found", found.kind_name()),
                ("expected", expected),
            ],
        ),
    )
}

pub(crate) fn int_arg<'a>(id: &str, args: &'a [Value], i: usize) -> Result<&'a BigInt, Fault> {
    match &args[i] {
        Value::Int(n) => Ok(n),
        other => Err(confusion(id, i, "an Int", other)),
    }
}

pub(crate) fn text_arg<'a>(id: &str, args: &'a [Value], i: usize) -> Result<&'a Rc<String>, Fault> {
    match &args[i] {
        Value::Text(t) => Ok(t),
        other => Err(confusion(id, i, "a Text", other)),
    }
}

pub(crate) fn bytes_arg<'a>(
    id: &str,
    args: &'a [Value],
    i: usize,
) -> Result<&'a Rc<Vec<u8>>, Fault> {
    match &args[i] {
        Value::Bytes(b) => Ok(b),
        other => Err(confusion(id, i, "a Bytes", other)),
    }
}

pub(crate) fn list_arg<'a>(
    id: &str,
    args: &'a [Value],
    i: usize,
) -> Result<&'a crate::value::ListRef, Fault> {
    match &args[i] {
        Value::List(xs) => Ok(xs),
        other => Err(confusion(id, i, "a List", other)),
    }
}
