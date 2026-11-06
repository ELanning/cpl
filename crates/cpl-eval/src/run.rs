//! `cpl run`: evaluate one function of a checked program on supplied
//! arguments (`spec/toolchain.md` 1.7 and 7.2; semantics S39).
//!
//! [`run_function`] resolves the function, refuses a declaration that is
//! not a function and a generator, a function whose declared return type
//! is a `Gen<T>` (`usage_not_a_function`, T61), then generic functions
//! (`cannot_run_generic`) and functions with `using` parameters
//! (`cannot_run_effectful`), decodes every argument against its parameter
//! type, checks every refinement-typed position of every argument
//! (`refinement_violation`, S39 (a)), and calls the function in checked mode
//! with no calling declaration (S39 (c)), on the large-stack evaluation
//! thread of [`crate::stack`].
//!
//! An argument is either value JSON (`--arg`, [`crate::json::value_from_json`])
//! or a *constant expression* (a bare argument): `cpl_syntax::parse_expr`
//! parses it as the virtual file [`ARGUMENT_FILE`] and it is decoded
//! against the parameter type with the program's declarations, accepting
//! exactly the forms toolchain 7.2 lists: literals, constructors (enum
//! variants, `Some`/`None`/`Ok`/`Err`/`Unit`, wrappers; bare or qualified
//! by a suffix of the declaration's path), `[...]`, struct literals without
//! a spread, unary minus, `not`, and parentheses. Anything else (a name, a
//! call, an operator, ...) is not a constant expression; there is no
//! evaluation, so an argument never runs code. `Bytes` has no constant form
//! (grammar D8) and is supplied only as `--arg`.
//!
//! Argument indices in errors are 0-based, like the positions of the
//! `arguments` array of `RunOutput`, which the caller fills from
//! [`RunOutcome::arguments`]: the arguments as decoded, so that the two
//! forms of one value report the same value JSON (toolchain 7.2: "the same
//! values spelled two ways").

mod constant;

use crate::fault::Fault;
use crate::interp::{
    BindOptions, Budget, CapabilityImpl, EffectEvent, EvalConfig, Handle, HandleId, HostContext,
    Interpreter,
};
use crate::json::{refined_positions, value_from_json, value_to_json_untyped, ArgError};
use crate::stack::{on_eval_stack, on_eval_stack_serving};
use crate::value::Value;
use cpl_diag::{DiagCode, Diagnostic, FileId, SymbolPath};
use cpl_hir::{DefId, DefStatus, FnDef, ParamDef, Program, Ty};
use num_bigint::BigInt;
use serde_json::json;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// The [`FileId`] constant-expression arguments are parsed as: the spans of
/// an [`RunError::ArgNotConstant`] diagnostic are byte offsets into the
/// argument's own text, in this file, which no program's source map holds.
pub const ARGUMENT_FILE: FileId = FileId(u32::MAX);

/// The code of a bad `cpl run` argument (toolchain Appendix B).
pub const USAGE_BAD_ARGUMENT: DiagCode = DiagCode("usage_bad_argument");

/// One positional argument of `cpl run`: a constant expression (semantics
/// S39 and 3.5: literals, constructors, `[...]`, struct literals, unary
/// minus, `not`, parentheses), or `--arg JSON` (value JSON, toolchain 7.3).
#[derive(Clone, Debug, PartialEq)]
pub enum RunArg {
    ConstExpr(String),
    Json(serde_json::Value),
}

/// The completed call: its value or the fault it raised, the declared
/// return type (for encoding the value), the effects traced, and the
/// arguments the call was given.
#[derive(Debug)]
pub struct RunOutcome {
    pub value: Result<Value, Fault>,
    pub return_ty: Ty,
    pub effects: Vec<EffectEvent>,
    /// Every argument as decoded, in parameter order (each encodes at its
    /// parameter's type, `FnDef::params`): what `RunOutput::arguments`
    /// reports in value JSON (toolchain 7.2), whichever form spelled it. It
    /// is present when the call faulted too, a refinement violation of an
    /// argument included (the decoded value is what was checked).
    pub arguments: Vec<Value>,
    /// The steps the call took ([`crate::Interpreter::steps`]).
    pub steps: u64,
}

/// Why nothing ran (the usage outcomes of Section 7.2, exit 2).
#[derive(thiserror::Error, Debug)]
pub enum RunError {
    #[error("program has errors")]
    ProgramHasErrors,
    #[error("no function {0}")]
    NotFound(String),
    /// A declaration that is not a function, or a generator: a function
    /// whose declared return type is a `Gen<T>` (the `std.gen` stubs),
    /// which has no value outside a property's `for` clause (T61,
    /// semantics S14). Code `usage_not_a_function`.
    #[error("{0} is not a function")]
    NotAFunction(String),
    /// Code `cannot_run_generic` (semantics S39).
    #[error("{0} is generic; run a monomorphic wrapper")]
    Generic(String),
    /// Code `cannot_run_effectful` (semantics S39).
    #[error("{0} has using parameters")]
    CannotRunEffectful(String),
    #[error("expected {expected} arguments, got {got}")]
    ArgCount { expected: usize, got: usize },
    /// An ill-typed argument (JSON or constant expression); the pointer is
    /// the RFC 6901 position in the argument's value JSON.
    #[error("argument {index}: {error}")]
    Arg { index: usize, error: ArgError },
    /// The argument does not parse (the parse diagnostic) or is not a
    /// constant expression (`usage_bad_argument`, spanning the offending
    /// part of the argument in [`ARGUMENT_FILE`]).
    #[error("argument {index} is not a constant expression")]
    ArgNotConstant {
        index: usize,
        diagnostic: Diagnostic,
    },
    /// Milestone 2 (toolchain 1.7.4, semantics S78): the host of a `using`
    /// parameter could not be created or granted; the diagnostics
    /// [`RunHosts::create`] returned (`interop_bind_failed`, or a driver
    /// that cannot be built). Nothing ran.
    #[error("a using parameter could not be bound")]
    Bind(Vec<Diagnostic>),
}

/// Resolve `path`, decode `args` against the parameters
/// ([`crate::json::value_from_json`] for `RunArg::Json`, the constant
/// expression decoder for `RunArg::ConstExpr`), check every refined
/// position ([`crate::json::refined_positions`]; a failure is the fault
/// `refinement_violation`, S39 (a)), and call the function in checked mode
/// with no calling declaration (S39 (c)). The outcome holds the decoded
/// arguments beside the value ([`RunOutcome::arguments`]).
///
/// The refusals come in this order: [`RunError::ProgramHasErrors`] (no
/// definitions, or one that failed checking), [`RunError::NotFound`],
/// [`RunError::NotAFunction`] (also a generator, a function whose declared
/// return type is a `Gen<T>`, T61), [`RunError::Generic`],
/// [`RunError::CannotRunEffectful`], [`RunError::ArgCount`], then the
/// first argument that does not decode.
///
/// From Milestone 2 this is [`run_function_with_hosts`] with hosts that
/// have none, so every function with a `using` parameter is
/// `CannotRunEffectful`, as in Milestone 1 (toolchain 1.7.4).
pub fn run_function(
    p: &Program,
    path: &SymbolPath,
    args: &[RunArg],
    config: &EvalConfig,
) -> Result<RunOutcome, RunError> {
    run_function_with_hosts(p, path, args, config, &mut NoHosts)
}

// ===========================================================================
// Milestone 2: prepare_run, hosts, and engines (toolchain 1.7.4, T77)
// ===========================================================================

/// What [`run_function`] settles before it evaluates: the target, its
/// decoded arguments (in parameter order), and its declared return type
/// (`spec/toolchain.md` 1.7.4).
pub struct PreparedRun {
    pub function: DefId,
    pub arguments: Vec<Value>,
    pub return_ty: Ty,
}

/// [`run_function`]'s refusals in its order, [`RunError::CannotRunEffectful`]
/// only for a `using` parameter for which `has_host` is false (semantics
/// S78; the first such parameter, in parameter order), then the decoding of
/// the arguments, on the evaluation thread (`spec/toolchain.md` 1.7.4).
/// `has_host` has no side effect (`cpl run` answers it from the interop
/// plan), so that a refused run builds and starts nothing. Both engines
/// share it, so that they refuse and decode alike (T77).
pub fn prepare_run(
    p: &Program,
    path: &SymbolPath,
    args: &[RunArg],
    has_host: &dyn Fn(&ParamDef) -> bool,
) -> Result<PreparedRun, RunError> {
    let (f, fd) = refusals(p, path, args.len(), has_host)?;
    let decoded = on_eval_stack(|| -> Result<Vec<Portable>, RunError> {
        Ok(decode_arguments(p, fd, args)?
            .iter()
            .map(Portable::from)
            .collect())
    })?;
    Ok(PreparedRun {
        function: f,
        arguments: decoded.into_iter().map(Value::from).collect(),
        return_ty: fd.ret.clone(),
    })
}

/// The hosts of `cpl run`'s `using` parameters (semantics S78), in two
/// steps, so that a refused run starts nothing (`spec/toolchain.md` 1.7.4).
pub trait RunHosts {
    /// Whether `param`'s capability has a host: the refusal check of S78. It
    /// has no side effect.
    fn available(&self, param: &ParamDef) -> bool;
    /// The host of `param` and its options, asked for once per `using`
    /// parameter, in parameter order, after the refusals and the decoding
    /// of the arguments; it may build and start a driver. `Err`: the
    /// diagnostics of a host that cannot be created or granted
    /// (`interop_bind_failed`, or a driver that cannot be built), which `cpl
    /// run` reports with the exit codes of Section 7.4.
    fn create(
        &mut self,
        param: &ParamDef,
    ) -> Result<(Box<dyn CapabilityImpl>, BindOptions), Vec<Diagnostic>>;
}

/// What provides a [`RunEngine`]'s handle for a `using` parameter.
pub enum HostSource {
    /// A host implementation, which the evaluator binds with
    /// [`Interpreter::bind_capability_with`].
    Impl(Box<dyn CapabilityImpl>, BindOptions),
    /// The backend-`c` binding of that name, which the native engine grants
    /// in its driver.
    Binding(String),
}

/// What `cpl run` needs from an engine after [`prepare_run`]
/// (`spec/toolchain.md` 1.7.4).
pub trait RunEngine {
    /// Bind the handle of `param`; called in `using`-parameter order, so the
    /// handles are numbered from 0. `Err`: the handle cannot be provided (a
    /// grant that failed, `interop_bind_failed`; a source the engine does
    /// not take, `internal_error` kind `call_mismatch`), and then nothing
    /// runs (semantics S78).
    fn bind(&mut self, param: &ParamDef, host: HostSource) -> Result<Handle, Vec<Diagnostic>>;
    /// Semantics S39 (a), the refined positions of the arguments, then the
    /// call with no calling declaration (S39 (c)).
    fn call_checked(
        &mut self,
        f: DefId,
        args: Vec<Value>,
        using: Vec<Handle>,
    ) -> Result<Value, Fault>;
}

/// The evaluator as a [`RunEngine`]: [`HostSource::Impl`], bound with
/// [`Interpreter::bind_capability_with`]; [`HostSource::Binding`] is
/// refused (`internal_error` kind `call_mismatch`: the evaluator reaches a
/// backend-`c` binding only through a host, `CAdapter`, `spec/cabi.md`
/// 7.2). `call_checked` evaluates on the calling thread: a caller drives it
/// inside [`crate::on_eval_stack`], as [`run_function_with_hosts`] does.
impl RunEngine for Interpreter<'_> {
    fn bind(&mut self, param: &ParamDef, host: HostSource) -> Result<Handle, Vec<Diagnostic>> {
        let p = self.program();
        match (host, &param.ty) {
            (HostSource::Impl(imp, options), Ty::Capability(capability)) => {
                Ok(self.bind_capability_with(*capability, imp, options))
            }
            (HostSource::Impl(..), _) => {
                let detail = format!("{} is not a using parameter", param.name);
                Err(vec![
                    Fault::internal("call_mismatch", detail).to_diagnostic(p)
                ])
            }
            (HostSource::Binding(name), _) => {
                // No specification fixes this text: the native engine refuses
                // the other source with its own (toolchain 1.14).
                let detail = format!(
                    "the evaluator binds host implementations only, not the binding {name}"
                );
                Err(vec![
                    Fault::internal("call_mismatch", detail).to_diagnostic(p)
                ])
            }
        }
    }

    fn call_checked(
        &mut self,
        f: DefId,
        args: Vec<Value>,
        using: Vec<Handle>,
    ) -> Result<Value, Fault> {
        let p = self.program();
        if let Some(fd) = p.defs.get(f.0 as usize).and_then(|d| d.kind.as_fn()) {
            check_arguments(self, f, fd, &args)?;
        }
        self.call(f, args, using)
    }
}

/// [`run_function`] with hosts (`spec/toolchain.md` 1.7.4; semantics S78):
/// the refusals ask `hosts.available`; after the decoding of the arguments
/// the host of every `using` parameter is created ([`RunHosts::create`])
/// and bound with its options, in parameter order and numbered from 0,
/// before the refined positions of the arguments are checked; a host that
/// cannot be created is [`RunError::Bind`], and nothing runs. Then the
/// arguments are checked (S39 (a)) and the function is called with no
/// calling declaration (S39 (c)), on the evaluation thread.
///
/// The hosts and the implementations they create stay on the calling
/// thread (neither is `Send`): the evaluation thread reaches them through
/// a request channel that the calling thread serves while it waits
/// (`stack::on_eval_stack_serving`), the arguments and results of each
/// operation crossing as `Send` copies, so a host sees exactly the calls,
/// in the order, that it would see on one thread.
pub fn run_function_with_hosts(
    p: &Program,
    path: &SymbolPath,
    args: &[RunArg],
    config: &EvalConfig,
    hosts: &mut dyn RunHosts,
) -> Result<RunOutcome, RunError> {
    let (f, fd) = refusals(p, path, args.len(), &|param| hosts.available(param))?;
    let mut impls: Vec<Box<dyn CapabilityImpl>> = Vec::new();
    let mut serve = |request: HostRequest| -> HostAnswer {
        match request {
            HostRequest::Create(index) => match hosts.create(&fd.using[index]) {
                Ok((imp, options)) => {
                    impls.push(imp);
                    HostAnswer::Created(Ok(options))
                }
                Err(diagnostics) => HostAnswer::Created(Err(diagnostics)),
            },
            HostRequest::Call { host, op, args } => {
                let args = args.into_iter().map(Value::from).collect();
                let mut ctx = HostContext { program: p };
                HostAnswer::Called(
                    impls[host]
                        .call(&op, args, &mut ctx)
                        .map(|v| Portable::from(&v)),
                )
            }
            HostRequest::Fork(host) => HostAnswer::Forked(impls[host].fork()),
            HostRequest::Join(host, forked) => {
                impls[host].join(forked);
                HostAnswer::Joined
            }
            HostRequest::Resource(host) => HostAnswer::Resource(impls[host].resource()),
        }
    };
    let outcome = on_eval_stack_serving(
        |link: &mut dyn FnMut(HostRequest) -> HostAnswer| -> Result<Outcome, RunError> {
            let values = decode_arguments(p, fd, args)?;
            let arguments = values.iter().map(Portable::from).collect();
            let link = RefCell::new(link);
            let mut it = Interpreter::new(p, config.clone());
            let mut handles = Vec::with_capacity(fd.using.len());
            for (index, param) in fd.using.iter().enumerate() {
                let options = match (*link.borrow_mut())(HostRequest::Create(index)) {
                    HostAnswer::Created(Ok(options)) => options,
                    HostAnswer::Created(Err(diagnostics)) => {
                        return Err(RunError::Bind(diagnostics))
                    }
                    _ => unreachable!("a creation is answered as one"),
                };
                let capability = match &param.ty {
                    Ty::Capability(c) => *c,
                    _ => DefId(u32::MAX),
                };
                let proxy = HostProxy {
                    link: &link,
                    host: index,
                };
                handles.push(it.bind_host(capability, Box::new(proxy), options));
            }
            let value = match check_arguments(&mut it, f, fd, &values) {
                Ok(()) => it.call(f, values, handles),
                Err(fault) => Err(fault),
            };
            Ok(Outcome {
                value: value.map(|v| Portable::from(&v)),
                effects: it.effects().to_vec(),
                arguments,
                steps: it.steps(),
            })
        },
        &mut serve,
    )?;
    Ok(RunOutcome {
        value: outcome.value.map(Value::from),
        return_ty: fd.ret.clone(),
        effects: outcome.effects,
        arguments: outcome.arguments.into_iter().map(Value::from).collect(),
        steps: outcome.steps,
    })
}

/// Hosts that have none: [`run_function`]'s, which refuses every function
/// with a `using` parameter as Milestone 1 did.
struct NoHosts;

impl RunHosts for NoHosts {
    fn available(&self, _param: &ParamDef) -> bool {
        false
    }

    fn create(
        &mut self,
        _param: &ParamDef,
    ) -> Result<(Box<dyn CapabilityImpl>, BindOptions), Vec<Diagnostic>> {
        unreachable!("a run without hosts refuses every using parameter")
    }
}

/// A request of the evaluation thread to the calling thread, which holds
/// the hosts ([`run_function_with_hosts`]).
enum HostRequest {
    /// Create the host of `using` parameter `index`.
    Create(usize),
    /// Call operation `op` of host `host` (its `using` index).
    Call {
        host: usize,
        op: String,
        args: Vec<Portable>,
    },
    /// Fork host `host` for a concurrent task ([`CapabilityImpl::fork`]).
    Fork(usize),
    /// Return a fork to host `host` ([`CapabilityImpl::join`]).
    Join(usize, Box<dyn CapabilityImpl + Send>),
    /// The resource identity of host `host` ([`CapabilityImpl::resource`]).
    Resource(usize),
}

/// The calling thread's answer to a [`HostRequest`].
enum HostAnswer {
    Created(Result<BindOptions, Vec<Diagnostic>>),
    Called(Result<Portable, Fault>),
    Forked(Option<Box<dyn CapabilityImpl + Send>>),
    Joined,
    Resource(Option<String>),
}

/// The handle implementation the evaluation thread binds for a host that
/// lives on the calling thread: each call is forwarded through the link.
struct HostProxy<'l, 'f> {
    link: &'l RefCell<&'f mut dyn FnMut(HostRequest) -> HostAnswer>,
    host: usize,
}

impl CapabilityImpl for HostProxy<'_, '_> {
    fn call(
        &mut self,
        op: &str,
        args: Vec<Value>,
        _ctx: &mut HostContext<'_>,
    ) -> Result<Value, Fault> {
        let request = HostRequest::Call {
            host: self.host,
            op: op.to_string(),
            args: args.iter().map(Portable::from).collect(),
        };
        match (*self.link.borrow_mut())(request) {
            HostAnswer::Called(result) => result.map(Value::from),
            _ => unreachable!("a call is answered as one"),
        }
    }

    fn fork(&mut self) -> Option<Box<dyn CapabilityImpl + Send>> {
        match (*self.link.borrow_mut())(HostRequest::Fork(self.host)) {
            HostAnswer::Forked(forked) => forked,
            _ => unreachable!("a fork is answered as one"),
        }
    }

    fn join(&mut self, forked: Box<dyn CapabilityImpl + Send>) {
        (*self.link.borrow_mut())(HostRequest::Join(self.host, forked));
    }

    fn resource(&mut self) -> Option<String> {
        match (*self.link.borrow_mut())(HostRequest::Resource(self.host)) {
            HostAnswer::Resource(identity) => identity,
            _ => unreachable!("a resource query is answered as one"),
        }
    }
}

/// The refusals of `run_function` (toolchain 1.7, 1.7.4): the function and
/// its declaration, or the first refusal in the order of [`run_function`];
/// `CannotRunEffectful` for the first `using` parameter without a host.
fn refusals<'p>(
    p: &'p Program,
    path: &SymbolPath,
    given: usize,
    has_host: &dyn Fn(&ParamDef) -> bool,
) -> Result<(DefId, &'p FnDef), RunError> {
    if p.defs.is_empty() || p.defs.iter().any(|d| d.status == DefStatus::HasErrors) {
        return Err(RunError::ProgramHasErrors);
    }
    let name = path.display();
    let Some(f) = p.lookup(path) else {
        return Err(RunError::NotFound(name));
    };
    let Some(fd) = p.def(f).kind.as_fn() else {
        return Err(RunError::NotAFunction(name));
    };
    // T61 [D-257]: a generator is refused as no function is, before the
    // generic and effect checks (`std.gen.list` is generic, and still a
    // generator); calling one would only reach the `generator_not_a_value`
    // internal error of its builtin.
    if matches!(fd.ret, Ty::Gen(_)) {
        return Err(RunError::NotAFunction(name));
    }
    if !fd.generics.is_empty() {
        return Err(RunError::Generic(name));
    }
    // S78: a `using` parameter runs only with a host for its capability.
    if fd.using.iter().any(|param| !has_host(param)) {
        return Err(RunError::CannotRunEffectful(name));
    }
    if given != fd.params.len() {
        return Err(RunError::ArgCount {
            expected: fd.params.len(),
            got: given,
        });
    }
    Ok((f, fd))
}

/// Decode every argument against its parameter, in order (on the
/// evaluation thread: the decoders recurse with the argument's depth).
fn decode_arguments(p: &Program, fd: &FnDef, args: &[RunArg]) -> Result<Vec<Value>, RunError> {
    let mut values = Vec::with_capacity(args.len());
    for (index, (arg, param)) in args.iter().zip(&fd.params).enumerate() {
        values.push(match arg {
            RunArg::Json(j) => {
                value_from_json(p, j, &param.ty).map_err(|error| RunError::Arg { index, error })?
            }
            RunArg::ConstExpr(src) => constant::decode(p, src, &param.ty, index)?,
        });
    }
    Ok(values)
}

/// S39 (a): every refinement-typed position of every argument satisfies
/// its whole chain; the first that does not is `refinement_violation`
/// (`symbol` the function, `span` the parameter, `expected_type` the
/// position's refinement, `actual` its value, `clause` the first false
/// predicate, plus `parameter` and `pointer` naming the position). A
/// predicate that faults is reported as that fault (S34). The positions are
/// checked in the order [`refined_positions`] lists them, so a refinement's
/// predicate only ever sees a value whose base positions already hold.
fn check_arguments(
    it: &mut Interpreter<'_>,
    f: DefId,
    fd: &FnDef,
    values: &[Value],
) -> Result<(), Fault> {
    let p = it.program();
    for (param, value) in fd.params.iter().zip(values) {
        for pos in refined_positions(p, value, &param.ty) {
            if let Some(failed) = it.refinement_failure(pos.refined, &pos.value, Budget::Fresh)? {
                let clause = p
                    .def(failed)
                    .kind
                    .checked_predicate()
                    .map_or("", |r| r.predicate.text.as_str());
                let expected = p.def(pos.refined).name.display();
                return Err(Fault::refinement_violation(
                    &expected,
                    value_to_json_untyped(p, &pos.value),
                    clause,
                )
                .blamed_on(p, f)
                .with_span(param.span)
                .with_data("parameter", json!(param.name))
                .with_data("pointer", json!(pos.pointer)));
            }
        }
    }
    Ok(())
}

/// What the evaluation thread hands back: values are `Rc`-shared and so
/// not `Send`, and cross as a [`Portable`] copy.
struct Outcome {
    value: Result<Portable, Fault>,
    effects: Vec<EffectEvent>,
    arguments: Vec<Portable>,
    steps: u64,
}

/// A `Send` copy of a [`Value`]: its nodes in post-order (every node after
/// its children), a buffer that several positions share copied once and
/// referenced by index. Neither copying a value in nor rebuilding it
/// recurses, and dropping the flat node list does not either, so a result
/// nested arbitrarily deep (a loop can build one without approaching the
/// call-depth limit, S38) crosses back from the evaluation thread to a
/// caller on an ordinary stack, and its sharing survives the crossing.
#[derive(Clone)]
pub(crate) struct Portable {
    nodes: Vec<Node>,
}

/// One node of a [`Portable`]; children are indices of earlier nodes.
#[derive(Clone)]
enum Node {
    Int(BigInt),
    Bool(bool),
    Text(String),
    Bytes(Vec<u8>),
    List(Vec<usize>),
    Struct(DefId, Vec<usize>),
    Enum(DefId, u32, Vec<usize>),
    Wrapper(DefId, usize),
    Capability(HandleId),
}

/// The buffer a value's children live in, with the value's form: two
/// positions with the same key hold the same value.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
enum Shared {
    /// A list is the elements it views: their address and count.
    List(usize, usize),
    Struct(DefId, *const Value),
    Enum(DefId, u32, *const Value),
    Wrapper(DefId, *const Value),
}

/// The sharing key of a value that has children.
fn shared(v: &Value) -> Option<Shared> {
    match v {
        Value::List(xs) => {
            let (at, len) = xs.identity();
            Some(Shared::List(at, len))
        }
        Value::Struct { def, fields } => Some(Shared::Struct(*def, fields.as_ptr())),
        Value::Enum {
            def,
            variant,
            payload,
        } => Some(Shared::Enum(*def, *variant, payload.as_ptr())),
        Value::Wrapper { def, inner } => Some(Shared::Wrapper(*def, Rc::as_ptr(inner))),
        _ => None,
    }
}

/// The children of a value, in order.
fn children(v: &Value) -> &[Value] {
    match v {
        Value::List(xs) => xs,
        Value::Struct { fields, .. } => fields,
        Value::Enum { payload, .. } => payload,
        Value::Wrapper { inner, .. } => std::slice::from_ref(&**inner),
        _ => &[],
    }
}

impl From<&Value> for Portable {
    fn from(root: &Value) -> Portable {
        // Post-order on an explicit stack: a value is first entered (its
        // children are queued above it), then left (its node is written from
        // the indices its children left on `done`). Every buffer of `root`
        // lives while this runs, so a buffer address identifies its value.
        let mut nodes: Vec<Node> = Vec::new();
        let mut seen: HashMap<Shared, usize> = HashMap::new();
        let mut done: Vec<usize> = Vec::new();
        let mut work: Vec<(&Value, bool)> = vec![(root, false)];
        while let Some((v, left)) = work.pop() {
            let key = shared(v);
            if !left {
                if let Some(&index) = key.as_ref().and_then(|k| seen.get(k)) {
                    done.push(index);
                    continue;
                }
                let kids = children(v);
                if !kids.is_empty() {
                    work.push((v, true));
                    work.extend(kids.iter().rev().map(|kid| (kid, false)));
                    continue;
                }
            }
            let kids = done.split_off(done.len() - children(v).len());
            nodes.push(match v {
                Value::Int(n) => Node::Int(n.clone()),
                Value::Bool(b) => Node::Bool(*b),
                Value::Text(t) => Node::Text(t.to_string()),
                Value::Bytes(b) => Node::Bytes(b.to_vec()),
                Value::List(_) => Node::List(kids),
                Value::Struct { def, .. } => Node::Struct(*def, kids),
                Value::Enum { def, variant, .. } => Node::Enum(*def, *variant, kids),
                Value::Wrapper { def, .. } => Node::Wrapper(*def, kids[0]),
                Value::Capability(h) => Node::Capability(*h),
            });
            let index = nodes.len() - 1;
            if let Some(k) = key {
                seen.insert(k, index);
            }
            done.push(index);
        }
        Portable { nodes }
    }
}

impl From<Portable> for Value {
    fn from(portable: Portable) -> Value {
        // Every node is built from the already built values of its children
        // (an `Rc` clone shares them), so nothing recurses; the root is the
        // last node. Every built value is reachable from the root, so
        // dropping the others only releases counts.
        let mut built: Vec<Value> = Vec::with_capacity(portable.nodes.len());
        for node in portable.nodes {
            let all = |kids: &[usize]| -> Rc<[Value]> {
                kids.iter().map(|&i| built[i].clone()).collect()
            };
            let v = match node {
                Node::Int(n) => Value::Int(n),
                Node::Bool(b) => Value::Bool(b),
                Node::Text(t) => Value::Text(Rc::from(t)),
                Node::Bytes(b) => Value::Bytes(Rc::from(b)),
                Node::List(kids) => Value::list(kids.iter().map(|&i| built[i].clone()).collect()),
                Node::Struct(def, kids) => Value::Struct {
                    def,
                    fields: all(&kids),
                },
                Node::Enum(def, variant, kids) => Value::Enum {
                    def,
                    variant,
                    payload: all(&kids),
                },
                Node::Wrapper(def, kid) => Value::Wrapper {
                    def,
                    inner: Rc::new(built[kid].clone()),
                },
                Node::Capability(h) => Value::Capability(h),
            };
            built.push(v);
        }
        built.pop().expect("a portable value has a root node")
    }
}
