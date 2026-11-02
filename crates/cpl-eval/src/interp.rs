//! The interpreter (`spec/toolchain.md` 1.7 and 1.7.1; semantics 5, 6,
//! 6.9, 7.1, 8.1): a tree-walking evaluator of the typed HIR.
//!
//! Evaluation is strict and left to right (semantics 6.2) over immutable,
//! shared values (6.1, T8); a `var` is a rebindable name. Every contract
//! is checked at its boundary exactly as the table of toolchain 1.7.1
//! says, in the order semantics 5.2-5.4 fixes:
//!
//! | Event | Check |
//! | - | - |
//! | entry of a CPL function | `requires` in source order (`attributed_to: "caller"`, `caller` = the calling declaration, absent at the top level), then the header `decreases` (S35: `>= 0`, strictly below the nearest activation of the same function) |
//! | every exit (tail, `return`, a `?` that propagates an `Err`) | `ensures` in source order with `result` bound (`attributed_to: "body"`, `actual` = the exit value) |
//! | `while` | invariants before the first test (iteration 0) and after every body; the measure after the condition held, before the body (5.3) |
//! | capability operation | its `requires` before the handle is invoked (`attributed_to: "caller"`), its `ensures` on the result (`attributed_to: "binding"`, S31) |
//! | `@builtin` call | none here: a builtin checks its own `requires` and its `ensures` are trusted (stdlib S39, semantics S58) |
//! | `assert e`, `?name`, `/` and `%` by zero | `assertion_failed`, `unresolved_hole`, `division_by_zero` |
//! | call depth, wall clock | `resource_exhaustion` (`stack`, `time`) |
//! | `check<R>(x)` | the predicates of `chain(R)` root first; `Ok(x)` or `Err(ConstraintError)`, never a fault |
//! | `Refine` (a static proof) | the predicates again, when contracts are checked: `internal_error` (`refinement_proof_unsound`) if one fails |
//!
//! A fault raised while a contract expression is being evaluated (a
//! clause, a loop clause, a refinement predicate, an operation clause, a
//! property's `for` range, semantics 5.1) is reported as that fault with
//! `facet: "contract"`, `clause` = the clause's text, and `symbol` = the
//! declaration owning the clause (S34). When contract expressions nest (a
//! clause calls a function whose own clause faults), the innermost clause
//! the fault escapes is the one reported; the outer clauses leave the fault
//! as it is. A contract violation raised by a called function's own
//! contract (not escaping one of that function's clauses) is claimed by
//! the clause whose evaluation called it, as S34's example of a nested
//! `contract_violation` says. An `internal_error` is never claimed: it
//! blames no declaration and so has no facet (6.7, toolchain 1.7.3).
//!
//! Faults carry the backtrace of the active declarations, innermost first:
//! each activation (a CPL function, a test or property body, a refinement
//! predicate, and a `@builtin` callee) appends itself, with the span of the
//! call that entered it, as the fault leaves it.
//!
//! The evaluator recurses on the host stack. [`crate::run_function`] and
//! the test runner evaluate on a dedicated large-stack thread
//! ([`crate::stack::on_eval_stack`]); a direct caller of
//! [`Interpreter::call`] should do the same for deep programs.

mod eval;
mod parallel;
mod scope;

use crate::fault::{binding_json, Fault, FaultKind, Frame, Resource};
use crate::json::value_to_json;
use crate::value::Value;
use cpl_diag::Span;
use cpl_hir::{DefId, LocalDef, LocalId, Program, Ty};
use cpl_rtspec::messages::{self, render};
use num_bigint::BigInt;
use serde::Serialize;
use serde_json::{Map, Value as Json};
use std::time::{Duration, Instant};

pub(crate) use eval::{Eval, Flow};
pub use scope::scope_at;

/// The identity of a bound capability handle: the index into the
/// interpreter's handle table.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Serialize)]
pub struct HandleId(pub u32);

/// A bound capability handle, passed to [`Interpreter::call`] for a
/// function's `using` parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Handle {
    pub id: HandleId,
    /// The capability declaration the handle implements.
    pub capability: DefId,
    /// The handle's binding name (semantics 6.9; [`BindOptions::label`]): a
    /// human label for diagnostics and receipts (`"fixture:pages"`).
    pub label: String,
}

/// How a host implementation is bound (`spec/toolchain.md` 1.7.4, T77).
///
/// `label` is the handle's binding name (semantics 6.9), which
/// [`crate::boundary::validate_result`], every fault of semantics S77 and
/// S80 that the handle's results cause (`binding`), and the `adapter`
/// detail of stdlib S45 carry verbatim: the name of the `[bindings.<name>]`
/// table for a handle that `cpl run` binds (toolchain 1.14), the label a
/// test gives its host otherwise. With `validate_results`, every result of
/// the host passes the validation of semantics S77 before the operation's
/// `ensures` clauses run; every Milestone 2 path binds this way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindOptions {
    pub label: String,
    pub validate_results: bool,
}

/// A host-provided implementation of a capability. Milestone 1 ships none
/// (semantics 6.9); the interop adapters (Milestones 2-4) implement it. The
/// evaluator calls operations only through this interface, after checking
/// the operation's `requires` clauses and before checking its `ensures`
/// clauses (semantics S31).
pub trait CapabilityImpl {
    fn call(
        &mut self,
        op: &str,
        args: Vec<Value>,
        ctx: &mut HostContext<'_>,
    ) -> Result<Value, Fault>;
}

/// What a [`CapabilityImpl`] may see of the running program.
pub struct HostContext<'a> {
    pub program: &'a Program,
}

/// Evaluation settings (`spec/toolchain.md` 1.7; semantics S38).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvalConfig {
    /// `[run] max_call_depth`, default 10 000 (semantics S38), enforced by
    /// an explicit counter of active CPL function calls (the top-level call
    /// included; a test or property body, a refinement predicate, and a
    /// `@builtin` callee are not calls): exceeding it is
    /// `resource_exhaustion` (`resource: "stack"`), never a host stack
    /// overflow.
    pub max_call_depth: u32,
    /// Default `None`; `cpl test` sets it from `[test] timeout_ms` when
    /// non-zero (`resource_exhaustion`, `resource: "time"`). The budget
    /// applies to each top-level evaluation separately: one
    /// [`Interpreter::call`] or [`Interpreter::satisfies_refinement`] from
    /// the top, and in the runner exactly what toolchain 7.2 budgets, one
    /// test body, one property case, one shrink rerun (not a property's
    /// range setup, nor the refinement checks of drawn values).
    pub time_budget: Option<Duration>,
    /// Default `true`; `false` is reserved for release builds whose
    /// obligations were proved (Milestone 5): no `requires`, `ensures`,
    /// loop `invariant`, or `decreases` clause runs, and `Refine` nodes are not
    /// re-checked. Struct constructor invariants and foreign boundary validation
    /// remain mandatory regardless of this setting (spec/state-machines.md).
    pub check_contracts: bool,
    /// Default `false`; `true` records [`EffectEvent`]s.
    pub trace_effects: bool,
}

impl EvalConfig {
    /// The default call-depth limit, `[run] max_call_depth = 10000` (S38),
    /// shared with the native engine (`cpl_rtspec::limits`, T70).
    pub const DEFAULT_MAX_CALL_DEPTH: u32 = cpl_rtspec::limits::DEFAULT_MAX_CALL_DEPTH;
}

impl Default for EvalConfig {
    fn default() -> EvalConfig {
        EvalConfig {
            max_call_depth: EvalConfig::DEFAULT_MAX_CALL_DEPTH,
            time_budget: None,
            check_contracts: true,
            trace_effects: false,
        }
    }
}

/// One capability operation performed while [`EvalConfig::trace_effects`]
/// was on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectEvent {
    pub handle: HandleId,
    pub op: String,
    /// The declaration whose body performed the operation.
    pub def: DefId,
    /// The operation call.
    pub span: Span,
}

/// A bound handle: the capability it implements, its label, whether its
/// results are validated (semantics S77), and the host implementation.
struct Binding<'p> {
    capability: DefId,
    label: String,
    validate: bool,
    imp: Box<dyn CapabilityImpl + 'p>,
}

/// The observer of [`Interpreter::set_site_probe`].
#[doc(hidden)]
pub type SiteProbe<'p> = Box<dyn FnMut(DefId, Span, &[LocalId]) + 'p>;

/// Whether a top-level evaluation runs under [`EvalConfig::time_budget`]
/// (`spec/toolchain.md` 1.7.4: public from Milestone 2, for
/// [`crate::test::TestEngine`], and re-exported at the crate root).
///
/// `Fresh`: a top-level evaluation with its own time budget (a host's call,
/// a test body, a property case, a shrink rerun, toolchain 7.2); `None`: the
/// runner's unbudgeted evaluations (a property's range setup, the
/// refinement checks of drawn values and shrink candidates).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Budget {
    /// The budget starts afresh: a host's [`Interpreter::call`] and
    /// [`Interpreter::satisfies_refinement`], and each test body, property
    /// case, and shrink rerun of the runner (toolchain 7.2).
    Fresh,
    /// No budget: the runner's own evaluations around the bodies (a
    /// property's range setup, the refinement checks of drawn values and
    /// shrink candidates), which toolchain 7.2 does not budget.
    None,
}

/// Which local table an environment indexes (`LocalId` is per table).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Table {
    /// A function, test, property, or refinement declaration.
    Def(DefId),
    /// Operation `op` of capability `capability` (its contract clauses).
    Op { capability: DefId, op: u32 },
}

/// The local environment of one activation (or of one capability
/// operation's contract): the current value of every bound local, and the
/// locals in scope in the order they were introduced.
pub(crate) struct Env {
    table: Table,
    values: Vec<Option<Value>>,
    scope: Vec<LocalId>,
}

/// The reference evaluator over one checked [`Program`] (single-threaded,
/// T8).
pub struct Interpreter<'p> {
    program: &'p Program,
    config: EvalConfig,
    /// The active declarations, outermost first: each entry is an
    /// activation of `def` entered through the call at `span` (`None` for
    /// the top-level call of `cpl run` or a test). The innermost entry is
    /// the declaration whose body or clause is being evaluated.
    frames: Vec<Frame>,
    /// Parallel to `frames`: the header `decreases` measure recorded at the
    /// entry of each activation (semantics S35).
    measures: Vec<Option<BigInt>>,
    /// The environment stack: one per activation, plus one while a
    /// capability operation's clauses are evaluated.
    envs: Vec<Env>,
    /// Index = `HandleId.0`.
    handles: Vec<Binding<'p>>,
    effects: Vec<EffectEvent>,
    /// The wall-clock deadline of the current top-level evaluation.
    deadline: Option<Instant>,
    /// The propagating fault has already been attributed to a clause (S34);
    /// outer clauses leave it unchanged.
    attributed: bool,
    /// The last `ensures`, `invariant`, or `decreases` violation raised by a
    /// declaration in the current evaluation, as raised (before any S34
    /// rewrite): the test runner's `contract_counterexample` source (S46).
    violation: Option<(DefId, Fault)>,
    /// The number of active CPL function activations: the call depth that
    /// `max_call_depth` bounds (S38).
    calls: u32,
    /// The observer of the binding sites the evaluation reaches (tests only,
    /// [`Interpreter::set_site_probe`]).
    site_probe: Option<SiteProbe<'p>>,
    parallel_depth: usize,
    parallel_cancel: Option<(std::sync::Arc<std::sync::atomic::AtomicUsize>, usize)>,
    text_length_cache: Option<(std::rc::Rc<str>, usize)>,
}

impl<'p> Interpreter<'p> {
    pub fn new(program: &'p Program, config: EvalConfig) -> Self {
        Interpreter {
            program,
            config,
            frames: Vec::new(),
            measures: Vec::new(),
            envs: Vec::new(),
            handles: Vec::new(),
            effects: Vec::new(),
            deadline: None,
            attributed: false,
            violation: None,
            calls: 0,
            site_probe: None,
            parallel_depth: 0,
            parallel_cancel: None,
            text_length_cache: None,
        }
    }

    /// The program being evaluated (the lifetime is the program's, so a
    /// caller may keep it while using the interpreter mutably).
    pub fn program(&self) -> &'p Program {
        self.program
    }

    pub(crate) fn text_length(&mut self, text: &std::rc::Rc<str>) -> usize {
        if let Some((previous, length)) = &self.text_length_cache {
            if std::rc::Rc::ptr_eq(previous, text) {
                return *length;
            }
        }
        let length = text.chars().count();
        if text.len() <= 1_048_576 {
            self.text_length_cache = Some((text.clone(), length));
        }
        length
    }

    pub fn config(&self) -> &EvalConfig {
        &self.config
    }

    /// Bind a host implementation of `capability`; the returned handle is
    /// what [`Interpreter::call`] takes for a `using` parameter. The
    /// Milestone 1 host API, trusted (semantics 6.9): it is
    /// [`Interpreter::bind_capability_with`] with `BindOptions { label,
    /// validate_results: false }`, so an ill-typed host result still
    /// surfaces as `internal_error` kind `type_confusion` where it is used.
    pub fn bind_capability(
        &mut self,
        capability: DefId,
        imp: Box<dyn CapabilityImpl>,
        label: &str,
    ) -> Handle {
        self.bind_host(
            capability,
            imp,
            BindOptions {
                label: label.to_string(),
                validate_results: false,
            },
        )
    }

    /// [`Interpreter::bind_capability`] with options (`spec/toolchain.md`
    /// 1.7.4). With `validate_results`, every result of `imp` passes the
    /// validation of semantics S77 ([`crate::boundary::validate_result`], at
    /// the operation call's span) before the operation's `ensures` clauses
    /// run; every Milestone 2 path binds this way. Handles are numbered from
    /// 0 in binding order.
    pub fn bind_capability_with(
        &mut self,
        capability: DefId,
        imp: Box<dyn CapabilityImpl>,
        options: BindOptions,
    ) -> Handle {
        self.bind_host(capability, imp, options)
    }

    /// [`Interpreter::bind_capability_with`] for an implementation that may
    /// borrow for the interpreter's lifetime (the host proxies of
    /// [`crate::run::run_function_with_hosts`]).
    pub(crate) fn bind_host(
        &mut self,
        capability: DefId,
        imp: Box<dyn CapabilityImpl + 'p>,
        options: BindOptions,
    ) -> Handle {
        let id = HandleId(self.handles.len() as u32);
        self.handles.push(Binding {
            capability,
            label: options.label.clone(),
            validate: options.validate_results,
            imp,
        });
        Handle {
            id,
            capability,
            label: options.label,
        }
    }

    /// Observe every binding site the evaluation reaches: each `assert`
    /// statement (with the span of its expression), each `while` loop (the
    /// span of its condition, when the loop is entered), and each evaluated
    /// hole, with the declaration whose environment is active and its locals
    /// in scope, outermost first. For the tests that compare the
    /// interpreter's environment with [`scope_at`] (toolchain 1.7.4); it
    /// changes no outcome.
    #[doc(hidden)]
    pub fn set_site_probe(&mut self, probe: SiteProbe<'p>) {
        self.site_probe = Some(probe);
    }

    /// The handle with this id, if it is bound.
    pub fn handle(&self, id: HandleId) -> Option<Handle> {
        self.handles.get(id.0 as usize).map(|b| Handle {
            id,
            capability: b.capability,
            label: b.label.clone(),
        })
    }

    /// Call `f` with positional `args` and handles for its `using`
    /// parameters, checking its contract (semantics 5.2-5.4): `requires` in
    /// source order with `contract_violation` (`attributed_to: "caller"`,
    /// `caller` = the calling declaration when there is one), a header
    /// `decreases` (S35), the body, and `ensures` on every exit. A builtin
    /// (`FnBody::Builtin`) is dispatched through [`crate::builtins::lookup`]
    /// with its contract clauses skipped (the builtin checks its own
    /// `requires`, stdlib S39). Faults carry the backtrace of the active
    /// declarations, innermost first.
    ///
    /// A call made while no declaration is active is a top-level
    /// evaluation: it starts the time budget afresh. A handle for another
    /// capability than the parameter's, or an argument count that does not
    /// match, is `internal_error` (`kind: "call_mismatch"`).
    pub fn call(&mut self, f: DefId, args: Vec<Value>, using: Vec<Handle>) -> Result<Value, Fault> {
        self.begin_evaluation(Budget::Fresh);
        let p = self.program;
        if let Some(fd) = p.defs.get(f.0 as usize).and_then(|d| d.kind.as_fn()) {
            let mismatch = fd.using.len() != using.len()
                || fd
                    .using
                    .iter()
                    .zip(&using)
                    .any(|(param, h)| param.ty != Ty::Capability(h.capability));
            if mismatch {
                return Err(Fault::internal(
                    "call_mismatch",
                    render(
                        &messages::DETAIL_CALL_MISMATCH_USING,
                        &[
                            ("function", &p.display_path(f)),
                            ("expected", &fd.using.len().to_string()),
                            ("found", &using.len().to_string()),
                        ],
                    ),
                ));
            }
        }
        let handles = using.into_iter().map(|h| h.id).collect();
        self.call_fn(f, args, handles, None)
            .map_err(|flow| flow.into_fault())
    }

    /// Evaluate the predicates of `chain(refined)` with `self` = `value`,
    /// root first, stopping at the first `false` (for `check<R>(x)` and for
    /// boundary checks: `cpl run` arguments, shrink candidates). A fault in
    /// a predicate is reported per semantics S34 (`symbol` the refinement,
    /// `facet: contract`, `clause` the predicate text). A declaration that
    /// is not a refinement has an empty chain and is satisfied.
    pub fn satisfies_refinement(&mut self, refined: DefId, value: &Value) -> Result<bool, Fault> {
        self.refinement_failure(refined, value, Budget::Fresh)
            .map(|r| r.is_none())
    }

    /// The effects recorded while [`EvalConfig::trace_effects`] was on.
    pub fn effects(&self) -> &[EffectEvent] {
        &self.effects
    }

    // ---- state shared with the builtins ---------------------------------------

    /// The declaration whose body or contract clause is being evaluated:
    /// the innermost active frame. A builtin's precondition fault names it
    /// as `caller` (semantics 6.10); `None` at the top level of `cpl run`.
    pub fn current_def(&self) -> Option<DefId> {
        self.frames.last().map(|f| f.def)
    }

    /// The active frames, outermost first (reverse it for a fault's
    /// backtrace, which is innermost first).
    pub fn frames(&self) -> &[Frame] {
        &self.frames
    }

    /// Enter an activation of `def` through the call at `span`. The
    /// evaluator pushes a frame on every call and pops it on every exit;
    /// tests use it to give a builtin a caller.
    #[doc(hidden)]
    pub fn push_frame(&mut self, def: DefId, span: Option<Span>) {
        self.frames.push(Frame { def, span });
        self.measures.push(None);
    }

    /// Leave the innermost activation.
    #[doc(hidden)]
    pub fn pop_frame(&mut self) -> Option<Frame> {
        self.measures.pop();
        self.frames.pop()
    }

    /// Invoke operation `op` of the bound handle `handle` (semantics 6.9):
    /// a handle the host did not bind is `internal_error` (`kind:
    /// "unbound_capability"`). Contract checks and effect tracing around the
    /// call are the evaluator's (1.7.1).
    #[doc(hidden)]
    pub fn call_handle(
        &mut self,
        handle: HandleId,
        op: &str,
        args: Vec<Value>,
    ) -> Result<Value, Fault> {
        let program = self.program;
        let Some(binding) = self.handles.get_mut(handle.0 as usize) else {
            return Err(Fault::internal(
                "unbound_capability",
                render(
                    &messages::DETAIL_UNBOUND_CAPABILITY,
                    &[("operation", op), ("handle", &handle.0.to_string())],
                ),
            ));
        };
        let mut ctx = HostContext { program };
        binding.imp.call(op, args, &mut ctx)
    }

    // ---- entry points of the runner and `cpl run` -----------------------------

    /// The first refinement of `chain(refined)`, root first, whose predicate
    /// is false for `value`, or `None` when every predicate holds, as one
    /// top-level evaluation under `budget`.
    pub(crate) fn refinement_failure(
        &mut self,
        refined: DefId,
        value: &Value,
        budget: Budget,
    ) -> Result<Option<DefId>, Fault> {
        self.begin_evaluation(budget);
        for r in crate::builtins::check_order(self.program, refined) {
            match self.eval_predicate(r, value, None) {
                Ok(true) => {}
                Ok(false) => return Ok(Some(r)),
                Err(flow) => return Err(flow.into_fault()),
            }
        }
        Ok(None)
    }

    /// Evaluate the body of the test or property `def` as one top-level
    /// evaluation, with the property's `for` binders bound to `binders`
    /// (clause order). Ok when the body completes.
    pub(crate) fn eval_body(
        &mut self,
        def: DefId,
        binders: &[(LocalId, Value)],
    ) -> Result<(), Fault> {
        self.begin_evaluation(Budget::Fresh);
        let p = self.program;
        let body = match p.defs.get(def.0 as usize).map(|d| &d.kind) {
            Some(cpl_hir::DefKind::Test(t)) => &t.body,
            Some(cpl_hir::DefKind::Property(pd)) => &pd.body,
            _ => {
                return Err(Fault::internal(
                    "not_a_test",
                    render(
                        &messages::DETAIL_NOT_A_TEST_DEF,
                        &[("def", &def.0.to_string())],
                    ),
                ))
            }
        };
        if let Err(flow) = self.enter_activation(def, None) {
            return Err(flow.into_fault());
        }
        for (local, value) in binders {
            self.bind(*local, value.clone());
        }
        let outcome = self.eval_block(body);
        self.leave_activation();
        match outcome {
            Ok(_) => Ok(()),
            Err(Flow::Fault(mut fault)) => {
                fault.push_frame(Frame { def, span: None });
                Err(*fault)
            }
            Err(Flow::Return(_)) => Err(Fault::internal(
                "return_outside_function",
                render(&messages::DETAIL_RETURN_OUTSIDE_BODY, &[]),
            )),
        }
    }

    /// Evaluate `e`, a bound of the `gen.range` in a `for` range of the
    /// property `owner`, as one top-level evaluation in a fresh activation
    /// of the property with nothing bound (stdlib 8.2: the bounds are
    /// evaluated once, before case 0, in a scope with no binders, S43).
    ///
    /// The range is a contract expression of the property (semantics 5.1,
    /// 7.4), so a fault escaping `e` is reported per S34 as a fault of that
    /// expression: `facet: "contract"`, `clause` = `range_text` (the
    /// range's clause text, T56), and `symbol` = the property, unless a
    /// clause evaluated inside `e` claimed it first. The generator's own
    /// `requires lo <= hi` is not checked here: the runner checks it after
    /// both bounds, with the shape stdlib 12 fixes for it (`symbol
    /// std.gen.range`, [`crate::builtins::check_gen_range`]).
    ///
    /// No time budget applies: `--timeout-ms` budgets each test body, each
    /// property case, and each shrink rerun (toolchain 7.2; semantics S38:
    /// a time budget applies only where the toolchain defines one), and the
    /// setup is none of them.
    pub(crate) fn eval_range_expr(
        &mut self,
        owner: DefId,
        e: &'p cpl_hir::Expr,
        range_text: &str,
    ) -> Result<Value, Fault> {
        self.begin_evaluation(Budget::None);
        if let Err(flow) = self.enter_activation(owner, None) {
            return Err(flow.into_fault());
        }
        let p = self.program;
        let owner_id = p.defs.get(owner.0 as usize).map(|d| &d.stable_id);
        let outcome = self.eval_contract_expr(e, range_text, owner_id);
        self.leave_activation();
        outcome.map_err(|flow| {
            let mut fault = flow.into_fault();
            fault.push_frame(Frame {
                def: owner,
                span: None,
            });
            fault
        })
    }

    /// The last `ensures`/`invariant`/`decreases` violation of the current
    /// evaluation, as it was raised: its declaration and the fault.
    pub(crate) fn take_recorded_violation(&mut self) -> Option<(DefId, Fault)> {
        self.violation.take()
    }

    // ---- evaluation state ---------------------------------------------------------

    /// Start an evaluation requested from outside the evaluator: no clause
    /// attribution and no recorded violation yet. When no declaration is
    /// active it is a top-level evaluation: under [`Budget::Fresh`] the
    /// time budget starts afresh, under [`Budget::None`] it does not apply.
    /// A nested request (a host callback while a declaration is active)
    /// keeps the current deadline.
    fn begin_evaluation(&mut self, budget: Budget) {
        if self.frames.is_empty() {
            self.deadline = match budget {
                Budget::Fresh => self
                    .config
                    .time_budget
                    .and_then(|budget| Instant::now().checked_add(budget)),
                Budget::None => None,
            };
        }
        self.attributed = false;
        self.violation = None;
    }

    /// Push an activation of `def` entered through the call at `span` (a
    /// function, a test or property body, a refinement predicate): the
    /// stack guard and the time budget are checked first, from the caller's
    /// side. The call-depth limit counts function activations only and is
    /// checked by the call itself.
    fn enter_activation(&mut self, def: DefId, span: Option<Span>) -> Eval<()> {
        self.check_limits(span)?;
        self.frames.push(Frame { def, span });
        self.measures.push(None);
        self.push_env(Table::Def(def));
        Ok(())
    }

    fn leave_activation(&mut self) {
        self.envs.pop();
        self.measures.pop();
        self.frames.pop();
    }

    /// The time budget of the current top-level evaluation.
    fn check_limits(&self, span: Option<Span>) -> Eval<()> {
        if crate::stack::nearly_exhausted() {
            let limit = self.config.max_call_depth;
            let mut fault = Fault::resource_exhaustion(Resource::Stack, u64::from(limit));
            fault.message = render(
                &messages::RESOURCE_STACK_GUARD,
                &[
                    ("depth", &self.calls.to_string()),
                    ("limit", &limit.to_string()),
                ],
            );
            return self.raise(fault.with_data("depth", Json::from(self.calls)), span);
        }
        self.check_time(span)?;
        Ok(())
    }

    fn check_time(&self, span: Option<Span>) -> Eval<()> {
        if self
            .parallel_cancel
            .as_ref()
            .is_some_and(|(stop, index)| *index > stop.load(std::sync::atomic::Ordering::Acquire))
        {
            return Err(Flow::Fault(Box::new(Fault::internal(
                "parallel_task_cancelled",
                render(
                    &messages::ADAPTER_CALL_CANCELLED,
                    &[("operation", "parallel task")],
                ),
            ))));
        }
        match (self.deadline, self.config.time_budget) {
            (Some(deadline), Some(budget)) if Instant::now() > deadline => {
                let ms = u64::try_from(budget.as_millis()).unwrap_or(u64::MAX);
                self.raise(Fault::resource_exhaustion(Resource::Time, ms), span)
            }
            _ => Ok(()),
        }
    }

    fn push_env(&mut self, table: Table) {
        let len = self.locals_of(table).len();
        self.envs.push(Env {
            table,
            values: vec![None; len],
            scope: Vec::new(),
        });
    }

    /// The local table `table` indexes.
    fn locals_of(&self, table: Table) -> &'p [LocalDef] {
        let p = self.program;
        match table {
            Table::Def(def) => p.defs.get(def.0 as usize).map_or(&[], |d| d.kind.locals()),
            Table::Op { capability, op } => p
                .defs
                .get(capability.0 as usize)
                .and_then(|d| d.kind.as_capability())
                .and_then(|c| c.ops.get(op as usize))
                .map_or(&[], |o| o.locals.as_slice()),
        }
    }

    /// The generic parameters types in `table` refer to by index.
    fn generics_of(&self, table: Table) -> &'p [String] {
        match table {
            Table::Def(def) => self
                .program
                .defs
                .get(def.0 as usize)
                .map_or(&[], |d| d.kind.generics()),
            Table::Op { .. } => &[],
        }
    }

    fn env(&self) -> &Env {
        self.envs.last().expect("an active environment")
    }

    fn env_mut(&mut self) -> &mut Env {
        self.envs.last_mut().expect("an active environment")
    }

    /// Bring `local` into scope with `value`.
    fn bind(&mut self, local: LocalId, value: Value) {
        let env = self.env_mut();
        if let Some(slot) = env.values.get_mut(local.0 as usize) {
            *slot = Some(value);
            env.scope.push(local);
        }
    }

    /// Rebind the `var` `local` (its scope is unchanged).
    fn assign(&mut self, local: LocalId, value: Value, span: Span) -> Eval<()> {
        match self.env_mut().values.get_mut(local.0 as usize) {
            Some(slot @ Some(_)) => {
                *slot = Some(value);
                Ok(())
            }
            _ => self.internal(
                "unbound_local",
                render(
                    &messages::DETAIL_UNBOUND_LOCAL_ASSIGN,
                    &[("local", &local.0.to_string())],
                ),
                span,
            ),
        }
    }

    fn read_local(&self, local: LocalId, span: Span) -> Eval<Value> {
        match self.env().values.get(local.0 as usize) {
            Some(Some(v)) => Ok(v.clone()),
            _ => self.internal(
                "unbound_local",
                render(
                    &messages::DETAIL_UNBOUND_LOCAL,
                    &[("local", &local.0.to_string())],
                ),
                span,
            ),
        }
    }

    /// The scope length, to restore when a block or binder scope ends.
    fn scope_mark(&self) -> usize {
        self.env().scope.len()
    }

    fn restore_scope(&mut self, mark: usize) {
        self.env_mut().scope.truncate(mark);
    }

    /// Every binding visible in the current environment, innermost first,
    /// as the `bindings` objects of `assertion_failed` and `unresolved_hole`
    /// (semantics S41, 6.7): name, type (type parameters by name), kind,
    /// `mutable` for a `var`, and the value JSON.
    fn visible_bindings(&self) -> Vec<Json> {
        let env = self.env();
        let locals = self.locals_of(env.table);
        let generics = self.generics_of(env.table);
        env.scope
            .iter()
            .rev()
            .filter_map(|id| {
                let def = locals.get(id.0 as usize)?;
                let value = env.values.get(id.0 as usize)?.as_ref()?;
                Some(binding_json(
                    &def.name,
                    &self.program.display_ty_in(&def.ty, generics),
                    def.kind,
                    def.mutable,
                    value_to_json(self.program, value, &def.ty),
                ))
            })
            .collect()
    }

    /// Every binding visible in the current environment as the
    /// `{<name>: <value JSON>}` object of a loop violation's
    /// `actual.bindings` (semantics 5.3).
    fn bindings_object(&self) -> Map<String, Json> {
        let env = self.env();
        let locals = self.locals_of(env.table);
        let mut out = Map::new();
        for id in &env.scope {
            if let (Some(def), Some(Some(value))) =
                (locals.get(id.0 as usize), env.values.get(id.0 as usize))
            {
                out.insert(
                    def.name.clone(),
                    value_to_json(self.program, value, &def.ty),
                );
            }
        }
        out
    }

    /// The `input` of a contract fault of function `def`: its positional
    /// arguments by parameter name, read from the current environment
    /// (parameters are immutable, so these are the original arguments).
    /// `None` when `def` is not a function (a test or property has no
    /// arguments).
    fn input_object(&self, def: DefId) -> Option<Json> {
        let fd = self.program.defs.get(def.0 as usize)?.kind.as_fn()?;
        let env = self.env();
        let mut out = Map::new();
        for param in &fd.params {
            if let Some(Some(v)) = env.values.get(param.local.0 as usize) {
                out.insert(
                    param.name.clone(),
                    value_to_json(self.program, v, &param.ty),
                );
            }
        }
        Some(Json::Object(out))
    }

    /// Report the binding site at `span` to the probe, if one is set
    /// ([`Interpreter::set_site_probe`]).
    fn probe_site(&mut self, span: Span) {
        if let Some(mut probe) = self.site_probe.take() {
            if let (Some(def), Some(env)) = (self.current_def(), self.envs.last()) {
                probe(def, span, &env.scope);
            }
            self.site_probe = Some(probe);
        }
    }

    /// The binding name and validation flag of the bound handle `handle`
    /// (semantics 6.9, S77); `None` for an unbound handle.
    fn binding_of(&self, handle: HandleId) -> Option<(String, bool)> {
        self.handles
            .get(handle.0 as usize)
            .map(|b| (b.label.clone(), b.validate))
    }

    /// Raise `fault` from the expression at `span`: the enclosing
    /// declaration is blamed and the span set unless the raiser set them.
    /// An `internal_error` blames no declaration (semantics 6.7).
    fn raise<T>(&self, mut fault: Fault, span: Option<Span>) -> Eval<T> {
        if fault.symbol.is_none() && fault.kind != FaultKind::Internal {
            if let Some(def) = self.current_def() {
                fault = fault.blamed_on(self.program, def);
            }
        }
        if fault.span.is_none() {
            fault.span = span;
        }
        Err(Flow::Fault(Box::new(fault)))
    }

    /// An impossible state of the typed IR (semantics 6.7: `internal_error`
    /// blames no declaration).
    fn internal<T>(
        &self,
        kind: &str,
        detail: impl Into<String>,
        span: impl Into<Option<Span>>,
    ) -> Eval<T> {
        let mut fault = Fault::internal(kind, detail);
        fault.span = span.into();
        Err(Flow::Fault(Box::new(fault)))
    }
}
