//! The fault and value messages whose text a specification fixes, as `{name}` templates (`spec/toolchain.md` 1.11,
//! T70; `spec/stdlib.md` S47; `spec/cabi.md` 2.9).
//!
//! Each constant is byte for byte the text the Milestone 1 evaluator writes (`crates/cpl-eval/src`), with each
//! formatted part replaced by a placeholder; the templates of what Milestone 1 never produced (the binding faults of
//! semantics S77 and S80 and the `ForeignError` messages of stdlib S45) are the texts those decisions fix. The
//! evaluator renders every one of them through [`render`], and the native runtime receives them as the `CPLRT_MSG_*`
//! macros of the generated `cpl_rt_gen.h`, named `CPLRT_MSG_` followed by [`Template::name`] (`spec/cabi.md` B.2), so
//! the two engines cannot drift (semantics S75 compares `message` byte for byte).
//!
//! # The placeholder rule
//!
//! A placeholder is `{`, a name of a lowercase ASCII letter followed by lowercase ASCII letters, digits, and `_`, and
//! `}`. [`render`] scans the template once, left to right, replaces each placeholder by the value of the argument of
//! that name (the first one, when a name is given twice), never rescans a substituted value, ignores arguments that
//! the template does not name, and copies every other byte, a `{` or `}` that is not part of a placeholder included,
//! unchanged. A placeholder without an argument is a toolchain defect: [`render`] panics, and the C renderer of the
//! runtime, which follows the same rule (`spec/cabi.md` 2.9), reports a runtime invariant failure.
//!
//! # Values of the placeholders
//!
//! Values are inserted as given: a value text (semantics S40) where a template shows a value (`{value}`, `{lhs}`,
//! `{quoted}`, `{arguments}`), a decimal integer where it shows a count, an index, or a limit, and the qualified or
//! display names the evaluator computes (`cpl-eval`'s `display_path`, `display_ty_in`, `type_name`,
//! `operation_identity`) where it shows a name. The documentation of each constant names its source in the evaluator
//! or the decision that fixes it.

/// A message with `{name}` placeholders (the placeholder rule of the module documentation).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Template {
    /// The constant's name, `CONTRACT_VIOLATION`: the C macro is `CPLRT_MSG_<name>` (`spec/cabi.md` B.2).
    pub name: &'static str,
    /// The text, with its placeholders.
    pub text: &'static str,
}

const fn t(name: &'static str, text: &'static str) -> Template {
    Template { name, text }
}

// ---- fault messages (semantics 6.7; toolchain 1.7.3; `cpl_eval::fault`) ---------------------------------------------

/// `contract_violation`: `{clause_kind}` is `requires`, `ensures`, `invariant`, or `decreases`, `{clause}` the
/// canonical clause text (`Fault::contract_violation`).
pub const CONTRACT_VIOLATION: Template = t(
    "CONTRACT_VIOLATION",
    "{clause_kind} clause failed: {clause}",
);
/// `division_by_zero` (`Fault::division_by_zero`; stdlib S8).
pub const DIVISION_BY_ZERO: Template = t("DIVISION_BY_ZERO", "division by zero");
/// `internal_error`: `{kind}` the free-form identifier, `{detail}` its explanation, one of the `DETAIL_*` templates
/// when the kind has one (`Fault::internal`; semantics 6.7, S80).
pub const INTERNAL_ERROR: Template = t("INTERNAL_ERROR", "internal error ({kind}): {detail}");
/// `assertion_failed`: `{clause}` the canonical text of the asserted expression (`Fault::assertion_failed`).
pub const ASSERTION_FAILED: Template = t("ASSERTION_FAILED", "assertion failed: {clause}");
/// `unresolved_hole`: `{hole}` the hole's name without the `?` (`Fault::unresolved_hole`).
pub const UNRESOLVED_HOLE: Template = t(
    "UNRESOLVED_HOLE",
    "evaluation reached the unresolved hole ?{hole}",
);
/// `refinement_violation` of a boundary value, a `cpl run` or C export argument or a validated capability result:
/// `{expected_type}` the declared type at the failing position, `{clause}` the first false predicate
/// (`Fault::refinement_violation`; semantics S39, S77, S81).
pub const REFINEMENT_VIOLATION: Template = t(
    "REFINEMENT_VIOLATION",
    "the value does not satisfy {expected_type}: {clause}",
);
/// `resource_exhaustion` with `resource: "stack"` from the call-depth limit: `{limit}` the limit in activations
/// (`Fault::resource_exhaustion`; semantics S38).
pub const RESOURCE_STACK: Template = t(
    "RESOURCE_STACK",
    "the call depth limit of {limit} activations was exceeded",
);
/// `resource_exhaustion` with `resource: "stack"` from the stack guard: `{depth}` the active calls, `{limit}` the
/// call-depth limit (`Interpreter::enter_activation`; `spec/cabi.md` 3.4 uses the same text natively).
pub const RESOURCE_STACK_GUARD: Template = t(
    "RESOURCE_STACK_GUARD",
    "the evaluator's stack is exhausted at call depth {depth} (limit {limit})",
);
/// `resource_exhaustion` with `resource: "time"`: `{limit}` the budget in whole milliseconds.
pub const RESOURCE_TIME: Template = t(
    "RESOURCE_TIME",
    "the time budget of {limit} ms was exceeded",
);
/// `resource_exhaustion` with `resource: "memory"` (semantics S76; raised natively, and by the evaluator for a failure
/// its adapter host reports).
pub const RESOURCE_MEMORY: Template = t("RESOURCE_MEMORY", "memory was exhausted");
/// `resource_exhaustion` with `resource: "steps"`, reserved for the opt-in step budget of semantics S38.
pub const RESOURCE_STEPS: Template = t(
    "RESOURCE_STEPS",
    "the step budget of {limit} steps was exhausted",
);

// ---- internal_error details of the evaluator (`cpl_eval::interp`, `builtins`, `arith`, `test`) ----------------------

/// `builtin_arity`: `{id}` the registry id, `{expected}` and `{found}` the argument counts (`builtins::arity`).
pub const DETAIL_BUILTIN_ARITY: Template = t(
    "DETAIL_BUILTIN_ARITY",
    "the builtin {id} takes {expected} argument(s), it was called with {found}",
);
/// `builtin_body` (`Interpreter::run_fn`).
pub const DETAIL_BUILTIN_BODY: Template =
    t("DETAIL_BUILTIN_BODY", "a builtin has no body to evaluate");
/// `builtin_type_called`: `{id}` the type id (`builtins::gen`; `spec/cabi.md` 1.4, `cplrt_bi_uncallable`).
pub const DETAIL_BUILTIN_TYPE_CALLED: Template = t(
    "DETAIL_BUILTIN_TYPE_CALLED",
    "the builtin type `{id}` is not callable",
);
/// `call_mismatch` of a call: `{function}` the qualified name, `{params}` and `{using}` the declared counts, `{args}`
/// and `{handles}` the given ones (`Interpreter::call_fn`; `spec/cabi.md` 1.4).
pub const DETAIL_CALL_MISMATCH: Template = t(
    "DETAIL_CALL_MISMATCH",
    "{function} takes {params} argument(s) and {using} handle(s), it was called with {args} and {handles}",
);
/// `call_mismatch` of a host's top-level call: `{function}` the qualified name, `{expected}` the `using` parameters,
/// `{found}` the handles given (`Interpreter::call`).
pub const DETAIL_CALL_MISMATCH_USING: Template = t(
    "DETAIL_CALL_MISMATCH_USING",
    "{function} takes {expected} using parameter(s) of its declared capabilities, it was given {found} handle(s)",
);
/// `control_flow_in_contract` (`Interpreter::eval_contract_expr`).
pub const DETAIL_CONTROL_FLOW_IN_CONTRACT: Template = t(
    "DETAIL_CONTROL_FLOW_IN_CONTRACT",
    "a `return` or `?` inside a contract expression",
);
/// `error_node` (`ExprKind::Error`).
pub const DETAIL_ERROR_NODE: Template = t(
    "DETAIL_ERROR_NODE",
    "the expression did not typecheck; a program with errors cannot run",
);
/// `generator_draw_rejected`: `{value}` the drawn value's text, `{refinement}` the refinement's qualified name (the
/// property runner).
pub const DETAIL_GENERATOR_DRAW_REJECTED: Template = t(
    "DETAIL_GENERATOR_DRAW_REJECTED",
    "the generator drew {value} for {refinement}, which does not satisfy it",
);
/// `generator_not_a_value` of a call of a generator builtin: `{id}` the registry id (`builtins::gen`;
/// `spec/cabi.md` 1.4, `cplrt_bi_uncallable`).
pub const DETAIL_GENERATOR_NOT_A_VALUE: Template = t(
    "DETAIL_GENERATOR_NOT_A_VALUE",
    "the generator builtin {id} has no value outside a property's `for` clause; the property runner interprets it",
);
/// `generator_not_a_value` of an evaluated `ExprKind::Gen` (`spec/cabi.md` 1.4: the lowering of `Gen`).
pub const DETAIL_GENERATOR_NOT_A_VALUE_EXPR: Template = t(
    "DETAIL_GENERATOR_NOT_A_VALUE_EXPR",
    "a generator expression is interpreted only as a property's `for` range",
);
/// `missing_field` of a field access: `{index}` the field index.
pub const DETAIL_MISSING_FIELD: Template = t(
    "DETAIL_MISSING_FIELD",
    "the struct value has no field {index}",
);
/// `missing_field` of a struct literal's initializer: `{struct}` the struct's qualified name.
pub const DETAIL_MISSING_FIELD_LITERAL: Template = t(
    "DETAIL_MISSING_FIELD_LITERAL",
    "{struct} has no field {index}",
);
/// `missing_field` of a struct literal that leaves a field without a value.
pub const DETAIL_MISSING_FIELD_UNSET: Template = t(
    "DETAIL_MISSING_FIELD_UNSET",
    "field {index} of {struct} has no value",
);
/// `no_matching_arm`: `{value}` the value text of the scrutinee (`Interpreter::eval_match`; `spec/cabi.md` 1.4,
/// `cplrt_no_match`).
pub const DETAIL_NO_MATCHING_ARM: Template = t(
    "DETAIL_NO_MATCHING_ARM",
    "no arm of the match matches {value}",
);
/// `not_a_function`: `{def}` the `DefId.0` (`Interpreter::call_fn`; `spec/cabi.md` 1.4).
pub const DETAIL_NOT_A_FUNCTION: Template =
    t("DETAIL_NOT_A_FUNCTION", "DefId({def}) is not a function");
/// `not_a_refinement` of a declaration that is no refinement: `{name}` its qualified name.
pub const DETAIL_NOT_A_REFINEMENT: Template =
    t("DETAIL_NOT_A_REFINEMENT", "{name} is not a refinement");
/// `not_a_refinement` of an id with no declaration: `{def}` the `DefId.0`.
pub const DETAIL_NOT_A_REFINEMENT_DEF: Template = t(
    "DETAIL_NOT_A_REFINEMENT_DEF",
    "DefId({def}) is not a declaration",
);
/// `not_a_struct`: `{def}` the `DefId.0`.
pub const DETAIL_NOT_A_STRUCT: Template = t("DETAIL_NOT_A_STRUCT", "DefId({def}) is not a struct");
/// `not_a_test` of the runner: `{name}` the qualified name (`run_test`).
pub const DETAIL_NOT_A_TEST: Template = t("DETAIL_NOT_A_TEST", "{name} is not a test or property");
/// `not_a_test` of a body evaluation: `{def}` the `DefId.0` (`Interpreter::run_body`).
pub const DETAIL_NOT_A_TEST_DEF: Template = t(
    "DETAIL_NOT_A_TEST_DEF",
    "DefId({def}) is not a test or property",
);
/// `refinement_proof_unsound` (T39): `{value}` the value text, `{refinement}` the refinement's `type_name`,
/// `{predicate}` its predicate text (`Interpreter::eval_refine`; `spec/cabi.md` 1.4, `cplrt_refine_unsound`).
pub const DETAIL_REFINEMENT_PROOF_UNSOUND: Template = t(
    "DETAIL_REFINEMENT_PROOF_UNSOUND",
    "{value} was statically proved to be a {refinement}, but it does not satisfy {predicate}",
);
/// `return_outside_function` of a test or property body (`Interpreter::run_body`).
pub const DETAIL_RETURN_OUTSIDE_BODY: Template = t(
    "DETAIL_RETURN_OUTSIDE_BODY",
    "a `return` or `?` left a test or property body",
);
/// `return_outside_function` of a function exit that escaped every function boundary (`Flow::into_fault`).
pub const DETAIL_RETURN_OUTSIDE_FUNCTION: Template = t(
    "DETAIL_RETURN_OUTSIDE_FUNCTION",
    "a `return` or `?` escaped its function",
);
/// `type_confusion` of a value of the wrong form: `{found}` its kind name, `{expected}` the form wanted (`a Bool`,
/// `a struct`, ...).
pub const DETAIL_TYPE_CONFUSION: Template = t(
    "DETAIL_TYPE_CONFUSION",
    "found {found} where {expected} was expected",
);
/// `type_confusion` of a builtin argument: `{index}` its position, `{id}` the registry id, `{found}` the kind name,
/// `{expected}` the form wanted (`builtins::confusion`).
pub const DETAIL_TYPE_CONFUSION_BUILTIN: Template = t(
    "DETAIL_TYPE_CONFUSION_BUILTIN",
    "argument {index} of the builtin {id} is {found} where {expected} is expected",
);
/// `type_confusion` of `++`: `{lhs}` and `{rhs}` the operands' kind names.
pub const DETAIL_TYPE_CONFUSION_CONCAT: Template =
    t("DETAIL_TYPE_CONFUSION_CONCAT", "`++` of {lhs} and {rhs}");
/// `type_confusion` of an `Int` operator applied to another operator (`arith::binary`): `{operator}` its symbol.
pub const DETAIL_TYPE_CONFUSION_NOT_INT: Template = t(
    "DETAIL_TYPE_CONFUSION_NOT_INT",
    "`{operator}` is not an Int operator",
);
/// `type_confusion` of an operator's operands: `{operator}` its symbol, `{lhs}` and `{rhs}` the kind names.
pub const DETAIL_TYPE_CONFUSION_OPERATOR: Template = t(
    "DETAIL_TYPE_CONFUSION_OPERATOR",
    "`{operator}` of {lhs} and {rhs}",
);
/// `type_confusion` of a `gen.range` bound: `{found}` its kind name (the property runner).
pub const DETAIL_TYPE_CONFUSION_RANGE_BOUND: Template = t(
    "DETAIL_TYPE_CONFUSION_RANGE_BOUND",
    "a gen.range bound is {found} where an Int was expected",
);
/// `unbound_capability`: `{operation}` the operation's name, `{handle}` the handle number (`Interpreter::call_handle`).
pub const DETAIL_UNBOUND_CAPABILITY: Template = t(
    "DETAIL_UNBOUND_CAPABILITY",
    "operation {operation} was called on handle {handle}, which the host did not bind",
);
/// `unbound_local` of a read: `{local}` the `LocalId.0`.
pub const DETAIL_UNBOUND_LOCAL: Template = t("DETAIL_UNBOUND_LOCAL", "local {local} is not bound");
/// `unbound_local` of an assignment: `{local}` the `LocalId.0`.
pub const DETAIL_UNBOUND_LOCAL_ASSIGN: Template = t(
    "DETAIL_UNBOUND_LOCAL_ASSIGN",
    "assignment to unbound local {local}",
);
/// `unknown_builtin`: `{id}` the id the registry lacks.
pub const DETAIL_UNKNOWN_BUILTIN: Template = t(
    "DETAIL_UNKNOWN_BUILTIN",
    "the builtin registry has no `{id}`",
);
/// `unknown_operation`: `{def}` the capability's `DefId.0`, `{op}` the operation index.
pub const DETAIL_UNKNOWN_OPERATION: Template = t(
    "DETAIL_UNKNOWN_OPERATION",
    "DefId({def}) has no operation {op}",
);

// ---- binding faults and foreign failures (semantics S77, S80; stdlib S45; Milestone 2) ----------------------------

/// The `detail` of `binding_result_ill_typed` (semantics S80) and the `message` of an `InvalidResult` whose shape
/// failed or whose C result could not be converted (stdlib S45): `{operation}` the operation's qualified name,
/// `{expected_type}` the display text of the type at the failing position, `{pointer}` its RFC 6901 position in the
/// result. [`RESULT_ILL_TYPED_WHOLE`] is the form for the whole result (the pointer `""`).
pub const RESULT_ILL_TYPED: Template = t(
    "RESULT_ILL_TYPED",
    "the result of {operation} is not a value of {expected_type} at {pointer}",
);
/// [`RESULT_ILL_TYPED`] with ` at <pointer>` left out, for a failing position that is the whole result.
pub const RESULT_ILL_TYPED_WHOLE: Template = t(
    "RESULT_ILL_TYPED_WHOLE",
    "the result of {operation} is not a value of {expected_type}",
);
/// The `message` of an `InvalidResult` whose refinement failed (stdlib S45): `{expected_type}` the refinement named
/// at the position, `{clause}` the canonical text of the false predicate. [`RESULT_REFINEMENT_WHOLE`] is the form for
/// the whole result.
pub const RESULT_REFINEMENT: Template = t(
    "RESULT_REFINEMENT",
    "the result of {operation} does not satisfy {expected_type} at {pointer}: {clause}",
);
/// [`RESULT_REFINEMENT`] with ` at <pointer>` left out, for a failing position that is the whole result.
pub const RESULT_REFINEMENT_WHOLE: Template = t(
    "RESULT_REFINEMENT_WHOLE",
    "the result of {operation} does not satisfy {expected_type}: {clause}",
);
/// The `detail` of `binding_error_unrepresentable` (semantics S80): `{binding}` the binding name, `{kind}` the
/// reported `ForeignErrorKind` variant's name, `{message}` the reported message as value text (S40).
pub const DETAIL_BINDING_ERROR_UNREPRESENTABLE: Template = t(
    "DETAIL_BINDING_ERROR_UNREPRESENTABLE",
    "{binding} reported {kind} for {operation}: {message}",
);
/// The `detail` of `binding_protocol` for a status the callback contract does not allow: `{code}` the status in
/// decimal (semantics S80; `spec/cabi.md` 5.7).
pub const DETAIL_BINDING_PROTOCOL_STATUS: Template = t(
    "DETAIL_BINDING_PROTOCOL_STATUS",
    "{binding} returned status {code} from {operation}",
);
/// The `detail` of `binding_protocol` for `CPL_OK` without a valid owned result handle (semantics S80).
pub const DETAIL_BINDING_PROTOCOL_NO_RESULT: Template = t(
    "DETAIL_BINDING_PROTOCOL_NO_RESULT",
    "{binding} returned CPL_OK from {operation} without a valid result",
);
/// The `detail` of `binding_protocol` for `CPL_FOREIGN_ERROR` without a reported failure (semantics S80).
pub const DETAIL_BINDING_PROTOCOL_NO_FAILURE: Template = t(
    "DETAIL_BINDING_PROTOCOL_NO_FAILURE",
    "{binding} returned CPL_FOREIGN_ERROR from {operation} without reporting a failure",
);
/// The `detail` of `binding_protocol` for a scripted host without a response for the call: `{arguments}` the value
/// texts of the call's arguments separated by `, ` (semantics S80; `spec/cabi.md` 3.8, `bind_scripted`).
pub const DETAIL_BINDING_PROTOCOL_NO_RESPONSE: Template = t(
    "DETAIL_BINDING_PROTOCOL_NO_RESPONSE",
    "{binding} has no scripted response for {operation}({arguments})",
);
/// The `detail` of `binding_host_failed` (semantics S80) and the `message` of a `Transport` failure (stdlib S45):
/// `{status}` is [`ADAPTER_HOST_STATUS`] or [`ADAPTER_HOST_SIGNAL`] rendered.
pub const ADAPTER_HOST_EXITED: Template = t(
    "ADAPTER_HOST_EXITED",
    "the adapter host for {binding} exited with {status}",
);
/// The `<status>` of [`ADAPTER_HOST_EXITED`] when the adapter host exited: `{code}` its exit status in decimal.
pub const ADAPTER_HOST_STATUS: Template = t("ADAPTER_HOST_STATUS", "status {code}");
/// The `<status>` of [`ADAPTER_HOST_EXITED`] when a signal ended the adapter host: `{signal}` its number in decimal.
pub const ADAPTER_HOST_SIGNAL: Template = t("ADAPTER_HOST_SIGNAL", "signal {signal}");
/// The `message` of a worker's `Timeout` failure (stdlib S48; `spec/python.md` 6.4): `{ms}` the binding's
/// `timeout_ms` in decimal.
pub const ADAPTER_HOST_TIMEOUT: Template = t(
    "ADAPTER_HOST_TIMEOUT",
    "the adapter host for {binding} did not answer within {ms} ms",
);
/// The `message` of a worker's `Cancelled` failure (stdlib S48; `spec/python.md` 6.4).
pub const ADAPTER_CALL_CANCELLED: Template = t(
    "ADAPTER_CALL_CANCELLED",
    "the call of {operation} was cancelled",
);
/// The `detail` of `binding_protocol` for a worker answer that is not cpl-worker protocol 1 (semantics S82):
/// `{reason}` one of `not JSON`, `not a response to cap_call`, `a value that is not a node list`.
pub const DETAIL_BINDING_PROTOCOL_WORKER: Template = t(
    "DETAIL_BINDING_PROTOCOL_WORKER",
    "{binding} sent an answer that is not cpl-worker protocol 1 for {operation}: {reason}",
);
/// The `message` of a declarative binding's error status when it declares no `message` function, or that function
/// returns NULL or text that is not strict UTF-8 (stdlib S45): `{function}` the C function's name, `{code}` the status
/// in decimal.
pub const FOREIGN_RETURNED: Template = t("FOREIGN_RETURNED", "{function} returned {code}");
/// The `message` of a declarative binding's `InvalidArgument` for an `Int` outside its C type (stdlib S45):
/// `{argument}` the parameter's name followed by `.<field>` per struct field on the way to the value, `{c_type}` the
/// value's binding type (`int32`).
pub const FOREIGN_ARGUMENT_DOES_NOT_FIT: Template = t(
    "FOREIGN_ARGUMENT_DOES_NOT_FIT",
    "argument {argument} of {operation} does not fit {c_type}",
);
/// The `message` of a declarative binding's `InvalidArgument` for a text with an interior U+0000 passed as a
/// NUL-terminated string (stdlib S45).
pub const FOREIGN_ARGUMENT_CONTAINS_NUL: Template = t(
    "FOREIGN_ARGUMENT_CONTAINS_NUL",
    "argument {argument} of {operation} contains U+0000",
);

// ---- value messages (stdlib 1.5, 3.4, 6.2; semantics 6.10) ---------------------------------------------------------

/// `ConstraintError.message` of `check<R>(v)`: `{value}` the value text of `v`, `{predicate}` the text of the first
/// false predicate of `check_order(R)` (`builtins::check_failure`; `spec/cabi.md` 2.7, `cplrt_check_failure`).
pub const CHECK_FAILED: Template = t("CHECK_FAILED", "{value} does not satisfy {predicate}");
/// `ConstraintError.message` of `std.text.parse_int` (`type_name` `"Int"`): `{quoted}` the `quote_text` of the input
/// (stdlib S25).
pub const PARSE_INT_FAILED: Template = t("PARSE_INT_FAILED", "{quoted} is not a decimal integer");
/// `ConstraintError.message` of `std.bytes.to_text` (`type_name` `"Text"`): `{offset}` the `valid_up_to` of the
/// first invalid sequence.
pub const BYTES_TO_TEXT_FAILED: Template =
    t("BYTES_TO_TEXT_FAILED", "invalid UTF-8 at byte {offset}");

/// Every template, in the order of the generated `CPLRT_MSG_*` macros (`spec/cabi.md` B.2): the fault messages, the
/// `internal_error` details of the evaluator, the binding and foreign-failure messages, and the value messages.
pub const ALL: &[Template] = &[
    CONTRACT_VIOLATION,
    DIVISION_BY_ZERO,
    INTERNAL_ERROR,
    ASSERTION_FAILED,
    UNRESOLVED_HOLE,
    REFINEMENT_VIOLATION,
    RESOURCE_STACK,
    RESOURCE_STACK_GUARD,
    RESOURCE_TIME,
    RESOURCE_MEMORY,
    RESOURCE_STEPS,
    DETAIL_BUILTIN_ARITY,
    DETAIL_BUILTIN_BODY,
    DETAIL_BUILTIN_TYPE_CALLED,
    DETAIL_CALL_MISMATCH,
    DETAIL_CALL_MISMATCH_USING,
    DETAIL_CONTROL_FLOW_IN_CONTRACT,
    DETAIL_ERROR_NODE,
    DETAIL_GENERATOR_DRAW_REJECTED,
    DETAIL_GENERATOR_NOT_A_VALUE,
    DETAIL_GENERATOR_NOT_A_VALUE_EXPR,
    DETAIL_MISSING_FIELD,
    DETAIL_MISSING_FIELD_LITERAL,
    DETAIL_MISSING_FIELD_UNSET,
    DETAIL_NO_MATCHING_ARM,
    DETAIL_NOT_A_FUNCTION,
    DETAIL_NOT_A_REFINEMENT,
    DETAIL_NOT_A_REFINEMENT_DEF,
    DETAIL_NOT_A_STRUCT,
    DETAIL_NOT_A_TEST,
    DETAIL_NOT_A_TEST_DEF,
    DETAIL_REFINEMENT_PROOF_UNSOUND,
    DETAIL_RETURN_OUTSIDE_BODY,
    DETAIL_RETURN_OUTSIDE_FUNCTION,
    DETAIL_TYPE_CONFUSION,
    DETAIL_TYPE_CONFUSION_BUILTIN,
    DETAIL_TYPE_CONFUSION_CONCAT,
    DETAIL_TYPE_CONFUSION_NOT_INT,
    DETAIL_TYPE_CONFUSION_OPERATOR,
    DETAIL_TYPE_CONFUSION_RANGE_BOUND,
    DETAIL_UNBOUND_CAPABILITY,
    DETAIL_UNBOUND_LOCAL,
    DETAIL_UNBOUND_LOCAL_ASSIGN,
    DETAIL_UNKNOWN_BUILTIN,
    DETAIL_UNKNOWN_OPERATION,
    RESULT_ILL_TYPED,
    RESULT_ILL_TYPED_WHOLE,
    RESULT_REFINEMENT,
    RESULT_REFINEMENT_WHOLE,
    DETAIL_BINDING_ERROR_UNREPRESENTABLE,
    DETAIL_BINDING_PROTOCOL_STATUS,
    DETAIL_BINDING_PROTOCOL_NO_RESULT,
    DETAIL_BINDING_PROTOCOL_NO_FAILURE,
    DETAIL_BINDING_PROTOCOL_NO_RESPONSE,
    ADAPTER_HOST_EXITED,
    ADAPTER_HOST_STATUS,
    ADAPTER_HOST_SIGNAL,
    ADAPTER_HOST_TIMEOUT,
    ADAPTER_CALL_CANCELLED,
    DETAIL_BINDING_PROTOCOL_WORKER,
    FOREIGN_RETURNED,
    FOREIGN_ARGUMENT_DOES_NOT_FIT,
    FOREIGN_ARGUMENT_CONTAINS_NUL,
    CHECK_FAILED,
    PARSE_INT_FAILED,
    BYTES_TO_TEXT_FAILED,
];

/// `t` with every placeholder replaced by the value of the argument of its name, by the placeholder rule of the
/// module documentation. Panics on a placeholder without an argument (a toolchain defect, caught by
/// `crates/cpl-rtspec/tests/tables.rs`); the C runtime renders the same templates with its own substitution routine,
/// which `crates/cpl-rt/tests/generated_tables.rs` compares with this function for every template.
pub fn render(t: &Template, args: &[(&str, &str)]) -> String {
    let text = t.text;
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len() + 32);
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if let Some(len) = placeholder_len(&bytes[i + 1..]) {
                let name = &text[i + 1..i + 1 + len];
                let Some(&(_, value)) = args.iter().find(|(n, _)| *n == name) else {
                    panic!(
                        "message template {}: no argument for the placeholder {{{name}}}",
                        t.name
                    );
                };
                out.push_str(&text[copied..i]);
                out.push_str(value);
                i += len + 2;
                copied = i;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&text[copied..]);
    out
}

/// The length of the placeholder name that `rest` (the bytes after a `{`) begins with, when the name is followed by
/// `}`: a lowercase ASCII letter, then lowercase ASCII letters, digits, and `_`.
fn placeholder_len(rest: &[u8]) -> Option<usize> {
    match rest.first() {
        Some(b) if b.is_ascii_lowercase() => {}
        _ => return None,
    }
    let len = rest
        .iter()
        .take_while(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || **b == b'_')
        .count();
    (rest.get(len) == Some(&b'}')).then_some(len)
}
