//! Stand-ins for the Milestone 2 items of `cpl-eval` (`spec/toolchain.md` 1.7.4, T77) that this crate's signatures
//! name before track A adds them. The contracts engineer's file: each stand-in has the declaration toolchain 1.7.4
//! gives the real item, and the track that owns the `use` of it switches that `use` to the real path when it lands
//! (track N in `session`, track F1 in `adapter`); an item nothing uses any more is deleted, and the module with the
//! last one. The traits are declared with exactly their specified methods, so that the impls of `NativeSession` and
//! `ManifestHosts` keep their bodies when the real traits replace these.

use cpl_diag::Diagnostic;
use cpl_eval::{CapabilityImpl, Fault, Handle, Value};
use cpl_hir::{DefId, LocalId, ParamDef, Program};

/// Stand-in for `cpl_eval::BindOptions` (toolchain 1.7.4): `label` is the handle's binding name (semantics 6.9),
/// which `boundary::validate_result`, every fault of semantics S77 and S80 that the handle's results cause
/// (`binding`), and the `adapter` detail of stdlib S45 carry verbatim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindOptions {
    pub label: String,
    pub validate_results: bool,
}

/// Stand-in for `cpl_eval::Budget` (toolchain 1.7.4, public and re-exported at the crate root from Milestone 2).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Budget {
    /// A top-level evaluation with its own time budget.
    Fresh,
    /// The runner's unbudgeted evaluations.
    None,
}

/// Stand-in for `cpl_eval::test::RangeSide` (toolchain 1.7.4).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RangeSide {
    Lo,
    Hi,
}

/// Stand-in for `cpl_eval::test::TestEngine` (toolchain 1.7.4): what the runner needs from an engine; each method is
/// one top-level evaluation of the engine.
pub trait TestEngine<'p> {
    fn program(&self) -> &'p Program;
    /// A test or property body, the `for` binders bound in clause order; Ok when the body completes.
    fn run_body(&mut self, test: DefId, binders: &[(LocalId, Value)]) -> Result<(), Fault>;
    /// The last `ensures`, `invariant`, or `decreases` violation of the last evaluation, as raised (semantics S46).
    fn take_violation(&mut self) -> Option<(DefId, Fault)>;
    /// The first refinement of `chain(refined)`, root first, whose predicate is false for `value`; None when all hold.
    fn failing_refinement(
        &mut self,
        refined: DefId,
        value: &Value,
        budget: Budget,
    ) -> Result<Option<DefId>, Fault>;
    /// Bound `side` of the `gen.range` of binder `binder` (clause order) of `property`, reported per semantics S71
    /// with `clause_text`, the range's clause text.
    fn eval_range_bound(
        &mut self,
        property: DefId,
        binder: usize,
        side: RangeSide,
        clause_text: &str,
    ) -> Result<Value, Fault>;
}

/// Stand-in for `cpl_eval::run::RunHosts` (toolchain 1.7.4): the hosts of `cpl run`'s `using` parameters (semantics
/// S78), in two steps, so that a refused run starts nothing.
pub trait RunHosts {
    /// Whether `param`'s capability has a host: the refusal check of S78. It has no side effect.
    fn available(&self, param: &ParamDef) -> bool;
    /// The host of `param` and its options, asked for once per `using` parameter, in parameter order, after the
    /// refusals and the decoding of the arguments; it may build and start a driver.
    fn create(
        &mut self,
        param: &ParamDef,
    ) -> Result<(Box<dyn CapabilityImpl>, BindOptions), Vec<Diagnostic>>;
}

/// Stand-in for `cpl_eval::run::HostSource` (toolchain 1.7.4): what provides a `RunEngine`'s handle for a `using`
/// parameter.
pub enum HostSource {
    /// A host implementation, which the evaluator binds with `bind_capability_with`.
    Impl(Box<dyn CapabilityImpl>, BindOptions),
    /// The backend-`c` binding of that name, which the native engine grants in its driver.
    Binding(String),
}

/// Stand-in for `cpl_eval::run::RunEngine` (toolchain 1.7.4): what `cpl run` needs from an engine after
/// `prepare_run`.
pub trait RunEngine {
    /// Bind the handle of `param`; called in `using`-parameter order, so the handles are numbered from 0.
    fn bind(&mut self, param: &ParamDef, host: HostSource) -> Result<Handle, Vec<Diagnostic>>;
    /// Semantics S39 (a), the refined positions of the arguments, then the call with no calling declaration (S39 (c)).
    fn call_checked(
        &mut self,
        f: DefId,
        args: Vec<Value>,
        using: Vec<Handle>,
    ) -> Result<Value, Fault>;
}
