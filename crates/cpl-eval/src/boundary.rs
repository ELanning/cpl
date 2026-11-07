//! The capability boundary of Milestone 2 in Rust (`spec/toolchain.md`
//! 1.7.4, T77): the validation of a host's result (semantics S77) and the
//! value or fault of a failure a binding reports (stdlib S45, semantics
//! S80). The native runtime does the same in C (`spec/cabi.md` 6.4, 6.5),
//! from the same templates (`cpl_rtspec::messages`), so the two engines
//! produce the same `ForeignError` values and faults byte for byte
//! (semantics S75).
//!
//! # Validation (semantics S77)
//!
//! [`validate_result`] runs for every result of a handle bound with
//! `BindOptions { validate_results: true, .. }`
//! ([`Interpreter::bind_capability_with`]), after the host returned and
//! before the operation's `ensures` clauses, in every contract mode:
//!
//! 1. *Shape*: the value is walked against the operation's declared return
//!    type in value order (list elements by index, struct fields in
//!    declaration order, payloads by position, a wrapper's inner value at
//!    `/$payload/0`), each position at the type its parent declares for it;
//!    a refinement-typed position is checked at its root base `strip(R)`.
//!    The first position whose value is not of the form its type requires
//!    (another kind of value, a struct, enum, or wrapper of another
//!    declaration, a variant index or a field or payload count the
//!    declaration does not have) fails, with its declared type
//!    (`Program::display_ty`) as `<expected_type>`.
//! 2. *Refinements*: every refined position, in the order of semantics S74
//!    (`cpl_eval::json::refined_positions`: siblings in value order, the
//!    positions inside a refinement's base before the refinement, each
//!    chain root first, a refinement whose base is a refinement one
//!    position), each predicate an activation of its refinement entered
//!    through the operation call's span; the first false predicate fails,
//!    with the refinement named at the position (`DeclName::display`, as for
//!    a `cpl run` argument) as `<expected_type>`. A predicate that faults is
//!    reported per S34, as for an argument.
//! 3. *A failure* is decided by the return type: with a foreign-error
//!    return type (`Result<T, std.foreign.ForeignError>`), the value is
//!    replaced by `Err(ForeignError { kind: InvalidResult, .. })` with the
//!    message and details of stdlib S45; otherwise a failed refinement is
//!    the fault `refinement_violation` (`symbol` the operation, `facet:
//!    "body"`, `attributed_to: "binding"`, `binding`, `pointer`) and a
//!    failed shape the fault `internal_error` kind
//!    `binding_result_ill_typed` (semantics S80).
//!
//! The walk runs on an explicit work list, so a result nested arbitrarily
//! deep is validated without recursing on the host stack.

use crate::builtins::check_order;
use crate::fault::{AttributedTo, Fault};
use crate::interp::Interpreter;
use crate::json::value_to_json_untyped;
use crate::types::{instantiate, operation_identity, strip};
use crate::value::Value;
use cpl_diag::{quote_text, Span};
use cpl_hir::{DefId, DefKind, OpDef, Program, Ty};
use cpl_rtspec::messages::{self, render};
use serde_json::json;

/// The `kind` of the `internal_error` of a result of the wrong shape
/// (semantics S80).
pub const BINDING_RESULT_ILL_TYPED: &str = "binding_result_ill_typed";
/// The `kind` of the `internal_error` of a failure that the operation's
/// return type cannot carry (semantics S80).
pub const BINDING_ERROR_UNREPRESENTABLE: &str = "binding_error_unrepresentable";
/// The `kind` of the fault of semantics S80 (and S82) when the process that hosts an adapter (the evaluator's C
/// adapter host, or a worker) ends during the call of an operation whose return type cannot carry a `ForeignError`.
pub const BINDING_HOST_FAILED: &str = "binding_host_failed";

/// The validation of `v`, the result of operation `op` (declaration order)
/// of `capability`, called at `span` on a handle whose binding name is
/// `binding` (its `BindOptions::label`): `Ok(v)`, or `Ok` of the
/// `Err(ForeignError { kind: InvalidResult, .. })` that replaces it, or the
/// fault S77 assigns (`spec/toolchain.md` 1.7.4; the module documentation
/// gives the rules). It runs inside the current evaluation and starts none
/// (its time budget, clause attribution, and violation record are kept);
/// each predicate is an activation of its refinement entered through `span`,
/// and every fault it builds spans `span`. An operation that `capability`
/// does not have is `internal_error` kind `unknown_operation`.
pub fn validate_result(
    it: &mut Interpreter<'_>,
    capability: DefId,
    op: u32,
    binding: &str,
    span: Span,
    v: Value,
) -> Result<Value, Fault> {
    let p = it.program();
    let opdef = operation(p, capability, op).map_err(|f| f.with_span(span))?;
    let positions = match walk(p, &v, &opdef.ret) {
        Ok(positions) => positions,
        Err(shape) => {
            return reject(
                p,
                capability,
                opdef,
                binding,
                span,
                Failure::Shape {
                    pointer: shape.pointer,
                    expected_type: p.display_ty(&shape.expected),
                },
            )
        }
    };
    for pos in positions {
        for r in check_order(p, pos.refined) {
            match it.eval_predicate(r, &pos.value, Some(span)) {
                Ok(true) => {}
                Ok(false) => {
                    let clause = p
                        .def(r)
                        .kind
                        .checked_predicate()
                        .map_or(String::new(), |rd| rd.predicate.text.clone());
                    return reject(
                        p,
                        capability,
                        opdef,
                        binding,
                        span,
                        Failure::Refinement {
                            pointer: pos.pointer,
                            expected_type: p.def(pos.refined).name.display(),
                            clause,
                            actual: pos.value,
                        },
                    );
                }
                Err(flow) => return Err(flow.into_fault()),
            }
        }
    }
    Ok(v)
}

/// A failure that the binding named `binding` reported for operation `op`
/// of `capability`, with its `ForeignErrorKind` name `kind` (`"Exception"`,
/// ..., `"Unavailable"`), its message, and its details in the order it gave
/// them (stdlib S45; `spec/toolchain.md` 1.7.4).
///
/// When the operation's return type is a foreign-error return type
/// (`Result<T, std.foreign.ForeignError>`), the result is the value
/// `Err(ForeignError { kind, message, details })` whose details are the
/// runtime's `adapter` (the binding name), `capability` (its qualified
/// name), and `operation` (the operation's qualified name), then `details`
/// as given. Otherwise it is the fault `internal_error` kind
/// `binding_error_unrepresentable` of semantics S80, with `operation` and
/// `binding`; it has no span, which the interpreter sets to the operation
/// call's as the fault leaves the host. `CAdapter` and `ScriptHost`
/// (toolchain 1.14) build every reported failure with it, as the C runtime
/// builds it natively.
///
/// Panics when `kind` is not a variant of `std.foreign.ForeignErrorKind`
/// and the return type carries a `ForeignError` (a defect of the caller,
/// which reads the kind from a validated protocol message).
pub fn foreign_failure(
    p: &Program,
    capability: DefId,
    op: u32,
    binding: &str,
    kind: &str,
    message: &str,
    details: &[(String, String)],
) -> Result<Value, Fault> {
    let opdef = operation(p, capability, op)?;
    let operation = operation_identity(p, capability, &opdef.name);
    if is_foreign_error_return(p, &opdef.ret) {
        let mut all = runtime_details(p, capability, binding, operation.as_str());
        all.extend(details.iter().cloned());
        let error = foreign_error(p, kind, message, &all)
            .unwrap_or_else(|| panic!("foreign_failure: `{kind}` is no ForeignErrorKind"));
        return Ok(Value::err(p, error));
    }
    Err(Fault::internal(
        BINDING_ERROR_UNREPRESENTABLE,
        render(
            &messages::DETAIL_BINDING_ERROR_UNREPRESENTABLE,
            &[
                ("binding", binding),
                ("kind", kind),
                ("operation", operation.as_str()),
                ("message", &quote_text(message)),
            ],
        ),
    )
    .with_data("operation", json!(operation.as_str()))
    .with_data("binding", json!(binding)))
}

/// How an adapter host ended (`spec/stdlib.md` S45's `<status>`): its exit status, or the signal that ended it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum HostEnd {
    Status(i32),
    Signal(i32),
}

impl HostEnd {
    /// `status <n>` or `signal <n>`.
    pub fn render(self) -> String {
        match self {
            HostEnd::Status(code) => render(
                &messages::ADAPTER_HOST_STATUS,
                &[("code", &code.to_string())],
            ),
            HostEnd::Signal(signal) => render(
                &messages::ADAPTER_HOST_SIGNAL,
                &[("signal", &signal.to_string())],
            ),
        }
    }
}

/// The outcome of an operation call whose adapter host ended during the call (`spec/stdlib.md` S45 and S48;
/// semantics S80 and S82): `Err(ForeignError { kind: Transport, message: "the adapter host for <binding> exited with
/// <status>", details: [adapter, capability, operation] })` when the operation's return type carries a
/// `ForeignError`, else the fault `internal_error` kind `binding_host_failed` with `operation` and `binding`.
pub fn host_ended(
    p: &Program,
    capability: DefId,
    op: u32,
    binding: &str,
    end: HostEnd,
) -> Result<Value, Fault> {
    let message = render(
        &messages::ADAPTER_HOST_EXITED,
        &[("binding", binding), ("status", &end.render())],
    );
    host_failed(p, capability, op, binding, &message)
}

/// A call on an unavailable adapter host. Workers preserve the first failure's message on later calls, even when
/// that first failure was a timeout or a protocol violation (spec/python.md 6.4).
pub fn host_failed(
    p: &Program,
    capability: DefId,
    op: u32,
    binding: &str,
    message: &str,
) -> Result<Value, Fault> {
    let opdef = operation(p, capability, op)?;
    let operation = operation_identity(p, capability, &opdef.name);
    if is_foreign_error_return(p, &opdef.ret) {
        let all = runtime_details(p, capability, binding, operation.as_str());
        let error = foreign_error(p, "Transport", message, &all)
            .expect("std.foreign.ForeignErrorKind has Transport");
        return Ok(Value::err(p, error));
    }
    Err(Fault::internal(BINDING_HOST_FAILED, message)
        .with_data("operation", json!(operation.as_str()))
        .with_data("binding", json!(binding)))
}

/// Whether `ty` is a foreign-error return type, `Result<T,
/// std.foreign.ForeignError>` for some `T` (semantics S77; the design's
/// adapter pattern, line 526): the return types whose operations receive a
/// failure as an `Err(ForeignError)` value rather than as a fault.
pub fn is_foreign_error_return(p: &Program, ty: &Ty) -> bool {
    matches!(ty, Ty::Adt(result, args)
        if *result == p.std_defs.result
            && args.len() == 2
            && matches!(&args[1], Ty::Adt(e, a) if *e == p.std_defs.foreign_error && a.is_empty()))
}

/// The value `ForeignError { kind, message, details }` of `std.foreign`
/// (stdlib 7), its fields placed by name in the declaration and each detail
/// a `ForeignDetail { key, value }`; `None` when the program has no
/// `std.foreign.ForeignError` of that shape or `kind` names no variant of
/// `ForeignErrorKind`.
pub fn foreign_error(
    p: &Program,
    kind: &str,
    message: &str,
    details: &[(String, String)],
) -> Option<Value> {
    let error = p.std_defs.foreign_error;
    let fields = &p.defs.get(error.0 as usize)?.kind.as_struct()?.fields;
    let field_ty = |name: &str| fields.iter().find(|f| f.name == name).map(|f| &f.ty);
    let Some(Ty::Adt(kind_def, _)) = field_ty("kind") else {
        return None;
    };
    let Some(Ty::List(detail_ty)) = field_ty("details") else {
        return None;
    };
    let Ty::Adt(detail_def, _) = &**detail_ty else {
        return None;
    };
    let kind_value = Value::variant(p, *kind_def, kind, Vec::new())?;
    let detail_fields = &p.defs.get(detail_def.0 as usize)?.kind.as_struct()?.fields;
    let mut list = Vec::with_capacity(details.len());
    for (key, value) in details {
        let entry = detail_fields
            .iter()
            .map(|f| match f.name.as_str() {
                "key" => Some(Value::text(key)),
                "value" => Some(Value::text(value)),
                _ => None,
            })
            .collect::<Option<Vec<Value>>>()?;
        list.push(Value::new_struct(*detail_def, entry));
    }
    let mut kind_value = Some(kind_value);
    let mut message_value = Some(Value::text(message));
    let mut list_value = Some(Value::list(list));
    let values = fields
        .iter()
        .map(|f| match f.name.as_str() {
            "kind" => kind_value.take(),
            "message" => message_value.take(),
            "details" => list_value.take(),
            _ => None,
        })
        .collect::<Option<Vec<Value>>>()?;
    Some(Value::new_struct(error, values))
}

/// The details the runtime puts first in a C binding's `ForeignError`
/// (stdlib S45, `cpl_rtspec::foreign::DETAIL_ORDER`): `adapter`,
/// `capability`, `operation`.
fn runtime_details(
    p: &Program,
    capability: DefId,
    binding: &str,
    operation: &str,
) -> Vec<(String, String)> {
    vec![
        ("adapter".to_string(), binding.to_string()),
        ("capability".to_string(), p.display_path(capability)),
        ("operation".to_string(), operation.to_string()),
    ]
}

/// The qualified name of operation `op` (declaration order) of `capability`
/// (`reader.images.ImageCodec.dimensions`), or `"<capability>.#<op>"` for an
/// index the capability does not have.
pub fn operation_identity_of(p: &Program, capability: DefId, op: u32) -> String {
    match operation(p, capability, op) {
        Ok(opdef) => operation_identity(p, capability, &opdef.name)
            .as_str()
            .to_string(),
        Err(_) => format!("{}.#{op}", p.display_path(capability)),
    }
}

/// Operation `op` of `capability`, or `internal_error` kind
/// `unknown_operation`.
fn operation(p: &Program, capability: DefId, op: u32) -> Result<&OpDef, Fault> {
    p.defs
        .get(capability.0 as usize)
        .and_then(|d| d.kind.as_capability())
        .and_then(|c| c.ops.get(op as usize))
        .ok_or_else(|| {
            Fault::internal(
                "unknown_operation",
                render(
                    &messages::DETAIL_UNKNOWN_OPERATION,
                    &[("def", &capability.0.to_string()), ("op", &op.to_string())],
                ),
            )
        })
}

/// Why a result failed validation.
enum Failure {
    Shape {
        pointer: String,
        expected_type: String,
    },
    Refinement {
        pointer: String,
        expected_type: String,
        clause: String,
        actual: Value,
    },
}

/// Step 3 of semantics S77: the replacing `Err(ForeignError)` value, or the
/// fault.
fn reject(
    p: &Program,
    capability: DefId,
    opdef: &OpDef,
    binding: &str,
    span: Span,
    failure: Failure,
) -> Result<Value, Fault> {
    let operation = operation_identity(p, capability, &opdef.name);
    let op = operation.as_str();
    let (pointer, expected_type) = match &failure {
        Failure::Shape {
            pointer,
            expected_type,
        }
        | Failure::Refinement {
            pointer,
            expected_type,
            ..
        } => (pointer.clone(), expected_type.clone()),
    };
    if is_foreign_error_return(p, &opdef.ret) {
        // stdlib S45: the runtime's InvalidResult, ` at <pointer>` left out
        // for the whole result.
        let message = match &failure {
            Failure::Shape { .. } => result_ill_typed(op, &expected_type, &pointer),
            Failure::Refinement { clause, .. } => {
                let args = [
                    ("operation", op),
                    ("expected_type", expected_type.as_str()),
                    ("pointer", pointer.as_str()),
                    ("clause", clause.as_str()),
                ];
                if pointer.is_empty() {
                    render(&messages::RESULT_REFINEMENT_WHOLE, &args)
                } else {
                    render(&messages::RESULT_REFINEMENT, &args)
                }
            }
        };
        let mut details = runtime_details(p, capability, binding, op);
        details.push(("pointer".to_string(), pointer));
        details.push(("expected_type".to_string(), expected_type));
        let error = foreign_error(p, "InvalidResult", &message, &details)
            .expect("a program whose operation returns a ForeignError declares std.foreign");
        return Ok(Value::err(p, error));
    }
    Err(match failure {
        Failure::Refinement { clause, actual, .. } => {
            Fault::refinement_violation(&expected_type, value_to_json_untyped(p, &actual), &clause)
                .with_symbol(operation.clone())
                .with_span(span)
                .with_data("pointer", json!(pointer))
                .with_data("attributed_to", json!(AttributedTo::Binding.as_str()))
                .with_data("binding", json!(binding))
        }
        Failure::Shape { .. } => Fault::internal(
            BINDING_RESULT_ILL_TYPED,
            result_ill_typed(op, &expected_type, &pointer),
        )
        .with_span(span)
        .with_data("operation", json!(op))
        .with_data("binding", json!(binding))
        .with_data("pointer", json!(pointer)),
    })
}

/// The `detail` of `binding_result_ill_typed` and the message of a shape's
/// `InvalidResult` (semantics S80, stdlib S45).
fn result_ill_typed(operation: &str, expected_type: &str, pointer: &str) -> String {
    let args = [
        ("operation", operation),
        ("expected_type", expected_type),
        ("pointer", pointer),
    ];
    if pointer.is_empty() {
        render(&messages::RESULT_ILL_TYPED_WHOLE, &args)
    } else {
        render(&messages::RESULT_ILL_TYPED, &args)
    }
}

// ===========================================================================
// The walk: shape and refined positions
// ===========================================================================

/// A refined position of a validated value (semantics S74), as
/// `cpl_eval::json::RefinedPosition`.
pub(crate) struct Position {
    pub(crate) pointer: String,
    pub(crate) refined: DefId,
    pub(crate) value: Value,
}

/// The first position of the wrong shape: its pointer and its declared type.
pub(crate) struct ShapeError {
    pub(crate) pointer: String,
    pub(crate) expected: Ty,
}

/// The pointer segments a position adds to its parent's.
enum Segment {
    /// The root, and a refinement's base (the same position).
    None,
    /// A list element.
    Index(usize),
    /// A struct field.
    Field(String),
    /// `/$payload/<i>`: an enum payload, or a wrapper's inner value (`0`).
    Payload(usize),
}

/// One step of the walk.
enum Work<'v> {
    /// Check `value` against `ty` at the position `segment` names; a
    /// mismatch there reports `declared` (the type the parent declares for
    /// the position, which for a refinement's base is the refinement).
    Visit {
        value: &'v Value,
        ty: Ty,
        declared: Ty,
        segment: Segment,
    },
    /// Leave a position: pop this many segments.
    Leave(usize),
    /// Record the refined position of `refined` at the current pointer.
    Refined { refined: DefId, value: &'v Value },
}

/// Walk `v` against `ty` in value order: the first position of the wrong
/// shape, or every refined position in the order of semantics S74 (the
/// order of `crate::json::refined_positions`, whose recursion this walk
/// replaces by a work list, so that a result of any depth is safe).
pub(crate) fn walk(p: &Program, v: &Value, ty: &Ty) -> Result<Vec<Position>, ShapeError> {
    let mut path: Vec<String> = Vec::new();
    let mut out = Vec::new();
    let mut work = vec![Work::Visit {
        value: v,
        ty: ty.clone(),
        declared: ty.clone(),
        segment: Segment::None,
    }];
    while let Some(step) = work.pop() {
        let (value, ty, declared, segment) = match step {
            Work::Leave(n) => {
                path.truncate(path.len() - n);
                continue;
            }
            Work::Refined { refined, value } => {
                out.push(Position {
                    pointer: render_pointer(&path),
                    refined,
                    value: value.clone(),
                });
                continue;
            }
            Work::Visit {
                value,
                ty,
                declared,
                segment,
            } => (value, ty, declared, segment),
        };
        let pushed = match segment {
            Segment::None => 0,
            Segment::Index(i) => {
                path.push(i.to_string());
                1
            }
            Segment::Field(name) => {
                path.push(name);
                1
            }
            Segment::Payload(i) => {
                path.push(PAYLOAD.to_string());
                path.push(i.to_string());
                2
            }
        };
        if pushed > 0 {
            work.push(Work::Leave(pushed));
        }
        let mismatch = |path: &[String]| ShapeError {
            pointer: render_pointer(path),
            expected: declared.clone(),
        };
        match &ty {
            Ty::Refined(_, def) => {
                // One position: its root base's shape and positions first,
                // then its whole chain (S74, `refined_positions`).
                let base = strip(p, &ty);
                if matches!(base, Ty::Refined(..) | Ty::Error) {
                    return Err(mismatch(&path));
                }
                work.push(Work::Refined {
                    refined: *def,
                    value,
                });
                work.push(Work::Visit {
                    value,
                    ty: base,
                    declared: declared.clone(),
                    segment: Segment::None,
                });
            }
            Ty::Int => {
                if !matches!(value, Value::Int(_)) {
                    return Err(mismatch(&path));
                }
            }
            Ty::Bool => {
                if !matches!(value, Value::Bool(_)) {
                    return Err(mismatch(&path));
                }
            }
            Ty::Text => {
                if !matches!(value, Value::Text(_)) {
                    return Err(mismatch(&path));
                }
            }
            Ty::Bytes => {
                if !matches!(value, Value::Bytes(_)) {
                    return Err(mismatch(&path));
                }
            }
            Ty::List(elem) => {
                let Value::List(items) = value else {
                    return Err(mismatch(&path));
                };
                for (i, item) in items.iter().enumerate().rev() {
                    work.push(Work::Visit {
                        value: item,
                        ty: (**elem).clone(),
                        declared: (**elem).clone(),
                        segment: Segment::Index(i),
                    });
                }
            }
            Ty::Adt(def, args) => match (p.defs.get(def.0 as usize).map(|d| &d.kind), value) {
                (Some(DefKind::Struct(s)), Value::Struct { def: vd, fields })
                    if vd == def && fields.len() == s.fields.len() =>
                {
                    if s.invariant.is_some() {
                        work.push(Work::Refined {
                            refined: *def,
                            value,
                        });
                    }
                    for (field, value) in s.fields.iter().zip(fields.iter()).rev() {
                        let t = instantiate(&field.ty, args);
                        work.push(Work::Visit {
                            value,
                            ty: t.clone(),
                            declared: t,
                            segment: Segment::Field(field.name.clone()),
                        });
                    }
                }
                (
                    Some(DefKind::Enum(e)),
                    Value::Enum {
                        def: vd,
                        variant,
                        payload,
                    },
                ) if vd == def
                    && e.variants
                        .get(*variant as usize)
                        .is_some_and(|v| v.payload.len() == payload.len()) =>
                {
                    let declared = &e.variants[*variant as usize].payload;
                    for (i, (t, value)) in declared.iter().zip(payload.iter()).enumerate().rev() {
                        let t = instantiate(t, args);
                        work.push(Work::Visit {
                            value,
                            ty: t.clone(),
                            declared: t,
                            segment: Segment::Payload(i),
                        });
                    }
                }
                _ => return Err(mismatch(&path)),
            },
            Ty::Wrapper(def) => match (
                p.defs.get(def.0 as usize).and_then(|d| d.kind.as_wrapper()),
                value,
            ) {
                (Some(w), Value::Wrapper { def: vd, inner }) if vd == def => {
                    work.push(Work::Visit {
                        value: inner,
                        ty: w.inner.clone(),
                        declared: w.inner.clone(),
                        segment: Segment::Payload(0),
                    });
                }
                _ => return Err(mismatch(&path)),
            },
            // No value of 0.1 contains a handle (semantics S27), and no
            // operation is generic; both are checked only by their form.
            Ty::Capability(_) => {
                if !matches!(value, Value::Capability(_)) {
                    return Err(mismatch(&path));
                }
            }
            Ty::Context(_) => {
                if !matches!(value, Value::Context(_)) {
                    return Err(mismatch(&path));
                }
            }
            Ty::Param(_) => {}
            Ty::Gen(_) | Ty::Error => return Err(mismatch(&path)),
        }
    }
    Ok(out)
}

/// The `$payload` segment of an enum payload or a wrapper's inner value.
const PAYLOAD: &str = "$payload";

/// The RFC 6901 pointer of a path of segments (`~` as `~0`, `/` as `~1`),
/// as `crate::json` writes it.
fn render_pointer(path: &[String]) -> String {
    let mut s = String::new();
    for segment in path {
        s.push('/');
        s.push_str(&segment.replace('~', "~0").replace('/', "~1"));
    }
    s
}
