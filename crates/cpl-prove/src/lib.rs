//! The automatic verifier (`spec/verification.md`).
//!
//! [`verify`] decides the proof obligations of a function (`spec/toolchain.md` 8.1): it executes
//! the function symbolically over all inputs that satisfy its preconditions, collects one
//! verification condition per obligation and path, and decides each with the procedure for
//! quantifier-free linear integer arithmetic of [`solver`]. An unsatisfiable condition is a proof;
//! a model of a condition on a path that abstracted nothing is a counterexample, reported with the
//! inputs that reach it; anything else is `unknown`, with the reason. [`equivalent`] decides
//! whether a new version of a function behaves like the old one on every input in the old one's
//! domain, the same way.
//!
//! The procedure is sound: `proved` is never reported for an obligation that fails on some input,
//! and `failed` only with inputs on which it fails. It is complete for the fragment it decides
//! exactly (integer and boolean arithmetic with `+`, `-`, multiplication and division by
//! constants, comparisons, conditionals, `match`, structs, enums, wrappers, lists of known length,
//! refinements and invariants over them, and calls to functions it can inline); outside it,
//! calls are abstracted by their contracts, loops by their invariants, and unsupported values by
//! fresh ones, so a proof can still go through.

pub mod exec;
pub mod render;
mod sites;
pub mod solver;

use cpl_diag::{Span, StableId};
use cpl_hir::{DefId, DefKind, DefStatus, FnBody, Program, RefKind};
use exec::{Canon, Check, Exec, Flow, Mode, Shared, Sym, ABSTRACT};
use num_bigint::BigInt;
use num_traits::Zero;
use render::Renderer;
use serde_json::{json, Map, Value};
use solver::{Form, Lin, Model, Outcome, Var};
use std::collections::{BTreeMap, BTreeSet};

/// Where an obligation arises: the keys `cpl-index` maps its obligation ids onto.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Site {
    /// The i-th `ensures` clause (0-based, canonical order).
    Ensures(usize),
    /// The function's `decreases` measure.
    Decreases,
    /// The i-th invariant of the loop whose condition has this span, on entry.
    InvariantInit(Span, usize),
    /// ... after each iteration.
    InvariantPreserve(Span, usize),
    /// The measure of the loop whose condition has this span.
    LoopDecreases(Span),
    /// The i-th precondition of the callee of the call with this span.
    CallRequires(Span, usize),
    /// The divisor of the `/` or `%` with this span.
    Division(Span),
    /// The `assert` whose condition has this span.
    Assert(Span),
    /// The invariant of the struct literal with this span.
    Constructor(Span),
}

impl Site {
    /// The obligation kind of `spec/toolchain.md` 8.1.
    pub fn kind(&self) -> &'static str {
        match self {
            Site::Ensures(_) => "ensures",
            Site::Decreases => "decreases",
            Site::InvariantInit(..) => "invariant_init",
            Site::InvariantPreserve(..) => "invariant_preserve",
            Site::LoopDecreases(_) => "loop_decreases",
            Site::CallRequires(..) => "call_requires",
            Site::Division(_) => "division",
            Site::Assert(_) => "assert",
            Site::Constructor(_) => "constructor_invariant",
        }
    }

    /// Where the obligation is stated in `f`.
    pub fn span(&self, f: &cpl_hir::FnDef) -> Option<Span> {
        match self {
            Site::Ensures(i) => f.ensures.get(*i).map(|c| c.span),
            Site::Decreases => f.decreases.as_ref().map(|c| c.span),
            Site::InvariantInit(s, _)
            | Site::InvariantPreserve(s, _)
            | Site::LoopDecreases(s)
            | Site::CallRequires(s, _)
            | Site::Division(s)
            | Site::Assert(s)
            | Site::Constructor(s) => Some(*s),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Proved,
    Failed,
    Unknown,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Proved => "proved",
            Status::Failed => "failed",
            Status::Unknown => "unknown",
        }
    }
}

/// The verdict on one obligation.
#[derive(Clone, Debug, PartialEq)]
pub struct Verdict {
    pub status: Status,
    /// For `unknown`: why (`spec/verification.md` lists the reasons).
    pub reason: Option<&'static str>,
    /// For `proved`: `{"by": ...}`; for `failed`: `{"model": inputs, "concrete": true}`; for
    /// `unknown`: `{"candidate": inputs}` when a model exists that the prover could not confirm.
    pub evidence: Option<Value>,
}

impl Verdict {
    fn unknown(reason: &'static str) -> Verdict {
        Verdict {
            status: Status::Unknown,
            reason: Some(reason),
            evidence: None,
        }
    }
}

/// The verdicts on a function's obligations and the function's status.
#[derive(Clone, Debug)]
pub struct Report {
    /// One verdict per site, in walk order.
    pub verdicts: Vec<(Site, Verdict)>,
    /// The suffix of each site's obligation id (`ensures/1`, `call/3/requires/1`, ...), as
    /// `spec/toolchain.md` 8.1 and T62 number them: `<stable id>#<suffix>` is the id.
    pub ids: BTreeMap<Site, String>,
    /// `proved` when every obligation is proved, termination is known (every recursion and loop
    /// has a measure), and no typed hole is left; `failed` when an obligation failed; `unknown`
    /// otherwise.
    pub status: Status,
    pub reason: Option<&'static str>,
}

impl Report {
    /// The verdict at `site`: `unknown` (`not_analyzed`) for a site the prover does not know.
    pub fn verdict(&self, site: &Site) -> Verdict {
        self.verdicts
            .iter()
            .find(|(s, _)| s == site)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| Verdict::unknown("not_analyzed"))
    }
}

/// The name of the decision procedure, for receipts.
pub const VERIFIER: &str = "cpl-prove";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Decide the obligations of function `def`; `None` for anything but a function with a body.
pub fn verify(p: &Program, def: DefId) -> Option<Report> {
    let d = p.def(def);
    let f = d.kind.as_fn()?;
    if !matches!(f.body, FnBody::Block(_)) {
        return None;
    }
    let (numbered, unmeasured_loop) = sites::of(p, f);
    let termination_unknown = unmeasured_loop || (f.decreases.is_none() && is_recursive(p, def));
    let ids: BTreeMap<Site, String> = numbered.iter().cloned().collect();
    let sites: Vec<Site> = numbered.into_iter().map(|(s, _)| s).collect();
    if d.status == DefStatus::HasErrors {
        let verdicts = sites
            .into_iter()
            .map(|s| (s, Verdict::unknown("has_errors")))
            .collect();
        return Some(Report {
            verdicts,
            ids,
            status: Status::Unknown,
            reason: Some("has_errors"),
        });
    }
    if sites.is_empty() {
        // Nothing to decide: the status is termination's and the holes'.
        let (mut status, mut reason) = summary(&[], termination_unknown);
        if has_hole(f) {
            (status, reason) = (Status::Unknown, Some("unresolved_hole"));
        }
        return Some(Report {
            verdicts: Vec::new(),
            ids,
            status,
            reason,
        });
    }
    let indirect = f.decreases.is_some() && indirect_recursion(p, def);
    let mut verdicts = verdicts_of(p, def, f, &sites, indirect, false)?;
    // Loops cut by too weak invariants: again, with the invariants inference proves inductive.
    // Their extra assumptions only strengthen what a run can show, so a site keeps the second
    // run's verdict when it is decided there.
    if verdicts
        .iter()
        .any(|(_, v)| v.reason == Some("invariant_too_weak"))
    {
        if let Some(again) = verdicts_of(p, def, f, &sites, indirect, true) {
            for ((_, v), (_, w)) in verdicts.iter_mut().zip(again) {
                if v.status == Status::Unknown && w.status != Status::Unknown {
                    *v = w;
                }
            }
        }
    }
    let (mut status, mut reason) = summary(&verdicts, termination_unknown);
    // A function with a typed hole is unfinished: its proofs cover every completion, but the
    // function itself is not proved until the hole is filled.
    if status != Status::Failed && has_hole(f) {
        (status, reason) = (Status::Unknown, Some("unresolved_hole"));
    }
    Some(Report {
        verdicts,
        ids,
        status,
        reason,
    })
}

/// The sites whose obligations a run of `def` on the inputs `inputs` (value JSON by parameter
/// name) violates: the function executed on those values with loops run iteration by iteration
/// and calls executed in place, keeping only paths that abstracted nothing. `None` when the
/// inputs are not all concrete values or the run is cut short.
pub fn run_concretely(p: &Program, def: DefId, inputs: &Value) -> Option<BTreeSet<Site>> {
    let f = p.def(def).kind.as_fn()?;
    if !f.using.is_empty() || !f.generics.is_empty() {
        return None;
    }
    let mut shared = Shared::default();
    let (checks, aborted) = {
        let mut x = Exec::new(p, &mut shared, def, Mode::Verify);
        x.concrete = true;
        let mut params = Vec::with_capacity(f.params.len());
        for param in &f.params {
            params.push(x.from_json(&param.ty, inputs.get(param.name.as_str())?)?);
        }
        let (path, _) = exec::enter(&mut x, Some(&params))?;
        let paths = exec::assume_requires(&mut x, path);
        let paths = exec::enter_measure(&mut x, paths);
        let outs = exec::body(&mut x, paths);
        exec::check_ensures(&mut x, outs);
        (std::mem::take(&mut x.checks), x.aborted)
    };
    if aborted.is_some() {
        return None;
    }
    let mut violated = BTreeSet::new();
    for (site, check) in checks {
        if check.inexact != 0 || violated.contains(&site) {
            continue;
        }
        if let Outcome::Sat(model) = solver::solve(&check.cond(), &shared.bools) {
            // Concrete values leave nothing to choose but what no condition mentions.
            if check.cond().eval(&model) {
                violated.insert(site);
            }
        }
    }
    Some(violated)
}

/// One symbolic run of `def` and the verdict of each of `sites`; `infer`: with loop invariant
/// inference.
fn verdicts_of(
    p: &Program,
    def: DefId,
    f: &cpl_hir::FnDef,
    sites: &[Site],
    indirect: bool,
    infer: bool,
) -> Option<Vec<(Site, Verdict)>> {
    let mut shared = Shared::default();
    let (checks, params, aborted) = {
        let mut x = Exec::new(p, &mut shared, def, Mode::Verify);
        x.infer = infer;
        let (path, params) = exec::enter(&mut x, None)?;
        let paths = exec::assume_requires(&mut x, path);
        let paths = exec::enter_measure(&mut x, paths);
        let outs = exec::body(&mut x, paths);
        exec::check_ensures(&mut x, outs);
        (std::mem::take(&mut x.checks), params, x.aborted)
    };
    let inputs: Vec<(String, Sym)> = f
        .params
        .iter()
        .zip(params)
        .map(|(param, v)| (param.name.clone(), v))
        .collect();
    let mut by_site: BTreeMap<Site, Vec<Check>> = BTreeMap::new();
    for (s, c) in checks {
        by_site.entry(s).or_default().push(c);
    }
    let mut memo = Memo::new();
    let mut verdicts = Vec::new();
    let mut sources = Vec::new();
    for site in sites {
        let (verdict, source) = if let Some(reason) = aborted {
            (Verdict::unknown(reason), None)
        } else if *site == Site::Decreases && indirect {
            (Verdict::unknown("indirect_recursion"), None)
        } else {
            let checks = by_site.get(site).map(Vec::as_slice).unwrap_or(&[]);
            decide(p, &shared, &mut memo, checks, &inputs)
        };
        verdicts.push((*site, verdict));
        sources.push(source);
    }
    // Candidates the abstractions could not settle: run them concretely (a function with
    // generic or capability parameters cannot be).
    if f.generics.is_empty() && f.using.is_empty() {
        let mut runs = Runs::default();
        for (i, source) in sources.into_iter().enumerate() {
            let Some((check, model)) = source else {
                continue;
            };
            if verdicts[i].1.status != Status::Unknown {
                continue;
            }
            let Some(first) = verdicts[i]
                .1
                .evidence
                .as_ref()
                .and_then(|e| e.get("candidate"))
                .cloned()
            else {
                continue;
            };
            if runs.run(p, def, &first, &mut verdicts) || runs.exhausted() {
                continue;
            }
            // Others: models of the same condition with different inputs.
            let mut vars = BTreeSet::new();
            for (_, v) in &inputs {
                input_vars(&shared, v, &mut vars);
            }
            if vars.is_empty() {
                continue;
            }
            let c = &by_site[&verdicts[i].0][check];
            let mut cond = c.cond();
            let mut last = model;
            for _ in 0..Runs::MORE {
                if runs.exhausted() {
                    break;
                }
                let Some(model) = other_candidate(&shared, &mut cond, &last, &vars) else {
                    break;
                };
                let candidate = render_inputs(p, &shared, &model, &inputs);
                if runs.run(p, def, &candidate, &mut verdicts) {
                    break;
                }
                last = model;
            }
        }
    }
    Some(verdicts)
}

/// Concrete runs of candidates (`run_concretely`), bounded per function.
#[derive(Default)]
struct Runs {
    tried: Vec<Value>,
}

impl Runs {
    const LIMIT: usize = 12;
    const MORE: usize = 3;

    fn exhausted(&self) -> bool {
        self.tried.len() >= Self::LIMIT
    }

    /// Run `def` on `candidate`; every `unknown` site the run violates fails with it as its
    /// model. Whether one did.
    fn run(
        &mut self,
        p: &Program,
        def: DefId,
        candidate: &Value,
        verdicts: &mut [(Site, Verdict)],
    ) -> bool {
        if self.exhausted() || self.tried.contains(candidate) {
            return false;
        }
        self.tried.push(candidate.clone());
        let Some(violated) = run_concretely(p, def, candidate) else {
            return false;
        };
        let mut any = false;
        for (site, v) in verdicts.iter_mut() {
            if v.status == Status::Unknown && violated.contains(site) {
                *v = Verdict {
                    status: Status::Failed,
                    reason: None,
                    evidence: Some(json!({ "model": candidate.clone(), "concrete": true })),
                };
                any = true;
            }
        }
        any
    }
}

/// Whether a function's clauses or body contain a typed hole.
pub fn has_hole(f: &cpl_hir::FnDef) -> bool {
    let mut found = false;
    let mut visit = |e: &cpl_hir::Expr| {
        if matches!(e.kind, cpl_hir::ExprKind::Hole { .. }) {
            found = true;
        }
    };
    for c in f.requires.iter().chain(&f.ensures).chain(&f.decreases) {
        c.expr.walk(&mut visit);
    }
    if let FnBody::Block(b) = &f.body {
        b.walk(&mut visit);
    }
    found
}

/// A function's status from its verdicts (see [`Report::status`]).
pub fn summary(
    verdicts: &[(Site, Verdict)],
    termination_unknown: bool,
) -> (Status, Option<&'static str>) {
    if verdicts.iter().any(|(_, v)| v.status == Status::Failed) {
        return (Status::Failed, None);
    }
    if let Some((_, v)) = verdicts.iter().find(|(_, v)| v.status == Status::Unknown) {
        return (Status::Unknown, v.reason);
    }
    if termination_unknown {
        return (Status::Unknown, Some("no_measure"));
    }
    (Status::Proved, None)
}

/// Rank of an unknown reason: the most informative one is reported.
fn rank(reason: &str) -> u8 {
    match reason {
        "limit" => 4,
        "nonlinear_arithmetic" => 3,
        "integer_reasoning" => 2,
        "unsupported_expression" => 1,
        _ => 0,
    }
}

/// Solver outcomes of the conditions of one query, which repeat across paths and sites.
type Memo = std::collections::HashMap<Form, Outcome>;

fn solve_memo(memo: &mut Memo, form: Form, bools: &BTreeSet<Var>) -> Outcome {
    if let Some(o) = memo.get(&form) {
        return o.clone();
    }
    let o = solver::solve(&form, bools);
    memo.insert(form, o.clone());
    o
}

/// Decide one condition group by group (see [`Check::components`]): the obligation's group
/// first, which alone suffices for a proof; any unsatisfiable group makes the path infeasible,
/// and a model is the union of the groups' models.
fn solve_check(memo: &mut Memo, c: &Check, bools: &BTreeSet<Var>) -> Outcome {
    let mut model = Model::new();
    let mut unknown = None;
    for part in c.components() {
        match solve_memo(memo, part, bools) {
            Outcome::Unsat => return Outcome::Unsat,
            Outcome::Sat(m) => model.extend(m),
            Outcome::Unknown(r) => {
                unknown.get_or_insert(r);
            }
        }
    }
    match unknown {
        Some(r) => Outcome::Unknown(r),
        None => Outcome::Sat(model),
    }
}

/// The verdict on one obligation from its conditions, one per path.
/// The verdict of one site from its checks, and for an `unknown` one with a candidate, the
/// check and model it came from (to look for other candidates).
fn decide(
    p: &Program,
    shared: &Shared,
    memo: &mut Memo,
    checks: &[Check],
    inputs: &[(String, Sym)],
) -> (Verdict, Option<(usize, Model)>) {
    let mut source = None;
    let verdict = decide_into(p, shared, memo, checks, inputs, &mut source);
    (verdict, source)
}

/// Inputs a candidate may stand for, as solver variables: what a different candidate must differ in.
fn input_vars(shared: &Shared, v: &Sym, out: &mut BTreeSet<Var>) {
    match v {
        Sym::Int(l) => out.extend(l.terms.keys().copied()),
        Sym::Bool(f) => out.extend(solver::vars(f)),
        Sym::Data { fields, .. } => fields.iter().for_each(|f| input_vars(shared, f, out)),
        Sym::Seq(items) => items.iter().for_each(|f| input_vars(shared, f, out)),
        Sym::Opaque { id, .. } => {
            if let Some(t) = shared.tags.get(id) {
                out.insert(*t);
            }
            for ((owner, _), (fields, _)) in shared.payloads.range((*id, 0)..=(*id, u32::MAX)) {
                debug_assert_eq!(owner, id);
                fields.iter().for_each(|f| input_vars(shared, f, out));
            }
        }
    }
}

/// Another model of `cond` whose inputs differ from `last`'s; `cond` is strengthened to exclude
/// `last`'s inputs.
fn other_candidate(
    shared: &Shared,
    cond: &mut Form,
    last: &Model,
    vars: &BTreeSet<Var>,
) -> Option<Model> {
    let differs = Form::or(
        vars.iter()
            .map(|v| {
                let value = last.get(v).cloned().unwrap_or_default();
                Form::not(Form::eq(&Lin::var(*v), &Lin::constant(value)))
            })
            .collect(),
    );
    *cond = Form::and(vec![cond.clone(), differs]);
    let Outcome::Sat(model) = solver::solve(cond, &shared.bools) else {
        return None;
    };
    confirm(shared, cond, model).ok()
}

fn decide_into(
    p: &Program,
    shared: &Shared,
    memo: &mut Memo,
    checks: &[Check],
    inputs: &[(String, Sym)],
    source: &mut Option<(usize, Model)>,
) -> Verdict {
    let mut unknown: Option<(&'static str, Option<Value>)> = None;
    let mut note = |reason: &'static str, candidate: Option<Value>| {
        let better = match &unknown {
            None => true,
            Some((r, _)) => rank(reason) > rank(r),
        };
        if better {
            unknown = Some((reason, candidate));
        }
    };
    for (index, c) in checks.iter().enumerate() {
        match solve_check(memo, c, &shared.bools) {
            Outcome::Unsat => {}
            Outcome::Sat(model) => {
                let confirmed = confirm(shared, &c.cond(), model);
                match confirmed {
                    Ok(model) if c.inexact == 0 => {
                        return Verdict {
                            status: Status::Failed,
                            reason: None,
                            evidence: Some(json!({
                                "model": render_inputs(p, shared, &model, inputs),
                                "concrete": true,
                            })),
                        };
                    }
                    Ok(model) => {
                        let reason = if c.inexact & ABSTRACT != 0 {
                            "unsupported_expression"
                        } else {
                            "invariant_too_weak"
                        };
                        let rendered = render_inputs(p, shared, &model, inputs);
                        if source.is_none() {
                            *source = Some((index, model));
                        }
                        note(reason, Some(rendered));
                    }
                    Err(()) => note("nonlinear_arithmetic", None),
                }
            }
            Outcome::Unknown(r) => note(
                if r == "case_limit" || r == "arithmetic_limit" {
                    "limit"
                } else {
                    "integer_reasoning"
                },
                None,
            ),
        }
    }
    match unknown {
        Some((reason, candidate)) => Verdict {
            status: Status::Unknown,
            reason: Some(reason),
            evidence: candidate.map(|c| json!({ "candidate": c })),
        },
        None => Verdict {
            status: Status::Proved,
            reason: None,
            evidence: Some(json!({
                "by": if checks.is_empty() { "unreachable" } else { "linear_arithmetic" },
                "paths": checks.len(),
            })),
        },
    }
}

fn render_inputs(p: &Program, shared: &Shared, model: &Model, inputs: &[(String, Sym)]) -> Value {
    let r = Renderer { p, shared, model };
    let mut m = Map::new();
    for (name, v) in inputs {
        m.insert(name.clone(), r.value(v));
    }
    Value::Object(m)
}

/// A model consistent with the products and quotients the linear encoding leaves free: the
/// solver's own when it is, else one found by fixing the offending factors to their values and
/// solving again. `Err` when none is found.
fn confirm(shared: &Shared, form: &Form, model: Model) -> Result<Model, ()> {
    let mut form = form.clone();
    let mut model = model;
    for _ in 0..4 {
        let vars = solver::vars(&form);
        let mut fixes = Vec::new();
        let value = |l: &Lin, m: &Model| l.eval(m);
        for (p, a, b) in &shared.products {
            if !vars.contains(p) {
                continue;
            }
            let (va, vb) = (value(a, &model), value(b, &model));
            let product = &va * &vb;
            if model.get(p).cloned().unwrap_or_default() != product {
                fixes.push(Form::eq(a, &Lin::constant(va)));
                fixes.push(Form::eq(b, &Lin::constant(vb)));
                fixes.push(Form::eq(&Lin::var(*p), &Lin::constant(product)));
            }
        }
        for (q, r, a, b) in &shared.quotients {
            if !vars.contains(q) && !vars.contains(r) {
                continue;
            }
            let (va, vb) = (value(a, &model), value(b, &model));
            if vb.is_zero() {
                continue;
            }
            let (qv, rv): (BigInt, BigInt) = (&va / &vb, &va % &vb);
            let get = |v: &Var| model.get(v).cloned().unwrap_or_default();
            if get(q) != qv || get(r) != rv {
                fixes.push(Form::eq(a, &Lin::constant(va)));
                fixes.push(Form::eq(b, &Lin::constant(vb)));
                fixes.push(Form::eq(&Lin::var(*q), &Lin::constant(qv)));
                fixes.push(Form::eq(&Lin::var(*r), &Lin::constant(rv)));
            }
        }
        if fixes.is_empty() {
            return Ok(model);
        }
        fixes.push(form);
        form = Form::and(fixes);
        match solver::solve(&form, &shared.bools) {
            Outcome::Sat(m) => model = m,
            _ => return Err(()),
        }
    }
    Err(())
}

/// Whether `def` lies on a cycle of the call graph (semantics 5.4), a function's direct calls of
/// itself from its own clauses excepted.
pub fn is_recursive(p: &Program, def: DefId) -> bool {
    reaches(p, def, def, true)
}

/// Whether `from` reaches `to` through calls; `first` counts `from` itself as not yet visited.
fn reaches(p: &Program, from: DefId, to: DefId, _first: bool) -> bool {
    let mut seen = BTreeSet::new();
    let mut stack = vec![from];
    while let Some(d) = stack.pop() {
        for r in p.references_from(d) {
            if r.kind != RefKind::Call || (r.interface && r.from == r.to) {
                continue;
            }
            if r.to == to {
                return true;
            }
            if seen.insert(r.to) {
                stack.push(r.to);
            }
        }
    }
    false
}

/// Whether `def` reaches itself through some other function.
fn indirect_recursion(p: &Program, def: DefId) -> bool {
    p.references_from(def)
        .iter()
        .filter(|r| r.kind == RefKind::Call && r.to != def)
        .any(|r| reaches(p, r.to, def, false))
}

// ---- equivalence ------------------------------------------------------------------------

/// The outcome of an equivalence query.
#[derive(Clone, Debug, PartialEq)]
pub struct Equivalence {
    pub status: Status,
    pub reason: Option<&'static str>,
    /// For `proved`: `{"by": "symbolic_equivalence", "pairs": n, "partial": bool}` (`partial`:
    /// a recursive call or a shared loop was summarized, so the proof assumes both versions
    /// terminate); for `failed`: `{"inputs": ...}`.
    pub evidence: Value,
    /// For `unknown`: inputs on which the versions may differ, to be confirmed by running both.
    pub candidates: Vec<Value>,
}

impl Equivalence {
    fn unknown(reason: &'static str) -> Equivalence {
        Equivalence {
            status: Status::Unknown,
            reason: Some(reason),
            evidence: Value::Null,
            candidates: Vec::new(),
        }
    }
}

/// Pairs of paths decided per equivalence query (after the infeasible ones are skipped), and
/// pairs examined for being infeasible.
const MAX_PAIRS: usize = 5_000;
const MAX_EXAMINED: usize = 1_000_000;
const MAX_CANDIDATES: usize = 8;

/// Whether function `new_def` of `new` behaves like `old_def` of `old` on every input that
/// satisfies the old preconditions: the same result or the same fault, after the same effects.
/// Contracts are part of the behavior (checked mode). `same` tells which callees are identical in
/// both programs, transitively: those are uninterpreted functions shared by both sides (with
/// their contracts), while every other callee is inlined or abstracted per side. When that
/// leaves the answer `unknown` because of an abstraction, the query is repeated with the
/// identical callees executed in place where they can be (not recursive, no effects, not too
/// deep), which decides refactorings that inline or extract a helper.
pub fn equivalent(
    old: &Program,
    old_def: DefId,
    new: &Program,
    new_def: DefId,
    same: &dyn Fn(&StableId) -> bool,
) -> Equivalence {
    let first = equivalent_in(old, old_def, new, new_def, same, false);
    if first.status != Status::Unknown || first.reason != Some("unsupported_expression") {
        return first;
    }
    let second = equivalent_in(old, old_def, new, new_def, same, true);
    match second.status {
        Status::Unknown => first,
        _ => second,
    }
}

fn equivalent_in(
    old: &Program,
    old_def: DefId,
    new: &Program,
    new_def: DefId,
    same: &dyn Fn(&StableId) -> bool,
    inline: bool,
) -> Equivalence {
    let (Some(fo), Some(fnew)) = (old.def(old_def).kind.as_fn(), new.def(new_def).kind.as_fn())
    else {
        return Equivalence::unknown("not_a_function");
    };
    if !matches!(fo.body, FnBody::Block(_)) || !matches!(fnew.body, FnBody::Block(_)) {
        return Equivalence::unknown("not_a_function");
    }
    if old.def(old_def).status == DefStatus::HasErrors
        || new.def(new_def).status == DefStatus::HasErrors
    {
        return Equivalence::unknown("has_errors");
    }
    // The signature must be the same for inputs to be shared.
    let types = |p: &Program, f: &cpl_hir::FnDef| -> Vec<String> {
        f.params
            .iter()
            .chain(&f.using)
            .map(|x| p.display_ty_in(&x.ty, &f.generics))
            .chain(std::iter::once(p.display_ty_in(&f.ret, &f.generics)))
            .collect()
    };
    if types(old, fo) != types(new, fnew) {
        return Equivalence::unknown("signature_changed");
    }
    let canon = canon(old, new);
    // Inputs and results cross sides: every type they mention must be the same definition.
    let mut mentioned = BTreeSet::new();
    for t in fnew
        .params
        .iter()
        .chain(&fnew.using)
        .map(|x| &x.ty)
        .chain([&fnew.ret])
    {
        type_defs(new, t, &mut mentioned);
    }
    if mentioned.iter().any(|d| !canon.to.contains_key(d)) {
        return Equivalence::unknown("type_changed");
    }
    let mut shared = Shared::default();
    let (old_outs, params, summarized_old, aborted_old) = {
        let mut x = Exec::new(
            old,
            &mut shared,
            old_def,
            Mode::Equivalence {
                side: "old",
                same,
                canon: None,
                inline,
            },
        );
        let Some((path, params)) = exec::enter(&mut x, None) else {
            return Equivalence::unknown("not_a_function");
        };
        let paths = exec::assume_requires(&mut x, path);
        let outs = exec::body(&mut x, paths);
        let mut outs = exec::ensure(&mut x, outs);
        outs.append(&mut x.faulted);
        (outs, params, x.summarized, x.aborted)
    };
    let (new_outs, summarized_new, aborted_new) = {
        let mut y = Exec::new(
            new,
            &mut shared,
            new_def,
            Mode::Equivalence {
                side: "new",
                same,
                canon: Some(&canon),
                inline,
            },
        );
        let Some((path, _)) = exec::enter(&mut y, Some(&params)) else {
            return Equivalence::unknown("not_a_function");
        };
        let mut outs = Vec::new();
        for (q, flow) in exec::require(&mut y, path) {
            match flow {
                Flow::Val(_) => outs.extend(exec::body(&mut y, vec![q])),
                other => outs.push((q, other)),
            }
        }
        let mut outs = exec::ensure(&mut y, outs);
        outs.append(&mut y.faulted);
        (outs, y.summarized, y.aborted)
    };
    if let Some(reason) = aborted_old.or(aborted_new) {
        return Equivalence::unknown(reason);
    }
    if old_outs.len() * new_outs.len() > MAX_EXAMINED {
        return Equivalence::unknown("limit");
    }
    let inputs: Vec<(String, Sym)> = fo
        .params
        .iter()
        .zip(params)
        .map(|(param, v)| (param.name.clone(), v))
        .collect();
    let mut z = Exec::new(
        old,
        &mut shared,
        old_def,
        Mode::Equivalence {
            side: "old",
            same,
            canon: None,
            inline,
        },
    );
    let mut pairs = Vec::new();
    for (po, a) in &old_outs {
        // A pair whose sides took opposite branches of one condition is infeasible: most pairs
        // of two similar versions are, and spotting them here saves solving them.
        let opposite: std::collections::HashSet<Form> =
            po.cond.iter().map(|c| Form::not(c.clone())).collect();
        for (pn, b) in &new_outs {
            if pn.cond.iter().any(|c| opposite.contains(c)) {
                continue;
            }
            let mut scratch = exec::Path::new(0);
            let differ = match (a, b) {
                (Flow::Val(x) | Flow::Ret(x), Flow::Val(y) | Flow::Ret(y)) => {
                    Form::not(z.eq(&mut scratch, x, y))
                }
                (Flow::Fault(x), Flow::Fault(y)) => Form::bool(x != y),
                _ => Form::True,
            };
            let traces = if po.trace.len() != pn.trace.len()
                || po
                    .trace
                    .iter()
                    .zip(&pn.trace)
                    .any(|(x, y)| x.0 != y.0 || x.1.len() != y.1.len())
            {
                Form::True
            } else {
                let mut parts = Vec::new();
                for (x, y) in po.trace.iter().zip(&pn.trace) {
                    for (u, v) in x.1.iter().zip(&y.1) {
                        parts.push(Form::not(z.eq(&mut scratch, u, v)));
                    }
                }
                Form::or(parts)
            };
            let mut all = po.cond.clone();
            all.extend(pn.cond.iter().cloned());
            all.extend(scratch.cond);
            all.push(Form::or(vec![differ, traces]));
            pairs.push((Form::and(all), po.inexact | pn.inexact | scratch.inexact));
            if pairs.len() > MAX_PAIRS {
                return Equivalence::unknown("limit");
            }
        }
    }
    drop(z);
    let mut unknown: Option<&'static str> = None;
    let mut candidates = Vec::new();
    for (form, inexact) in &pairs {
        match solver::solve(form, &shared.bools) {
            Outcome::Unsat => {}
            Outcome::Sat(model) => match confirm(&shared, form, model) {
                Ok(model) if *inexact == 0 => {
                    return Equivalence {
                        status: Status::Failed,
                        reason: None,
                        evidence: json!({ "inputs": render_inputs(old, &shared, &model, &inputs) }),
                        candidates: Vec::new(),
                    };
                }
                Ok(model) => {
                    unknown.get_or_insert("unsupported_expression");
                    if candidates.len() < MAX_CANDIDATES {
                        candidates.push(render_inputs(old, &shared, &model, &inputs));
                    }
                }
                Err(()) => {
                    unknown = Some("nonlinear_arithmetic");
                }
            },
            Outcome::Unknown(r) => {
                unknown = Some(if r == "case_limit" || r == "arithmetic_limit" {
                    "limit"
                } else {
                    "integer_reasoning"
                });
            }
        }
    }
    match unknown {
        Some(reason) => Equivalence {
            status: Status::Unknown,
            reason: Some(reason),
            evidence: Value::Null,
            candidates,
        },
        None => Equivalence {
            status: Status::Proved,
            reason: None,
            evidence: json!({
                "by": "symbolic_equivalence",
                "pairs": pairs.len(),
                "partial": summarized_old || summarized_new,
            }),
            candidates: Vec::new(),
        },
    }
}

/// The definitions a type mentions, those of its definitions' parts included.
fn type_defs(p: &Program, ty: &cpl_hir::Ty, out: &mut BTreeSet<DefId>) {
    use cpl_hir::Ty;
    let visit = |d: DefId, out: &mut BTreeSet<DefId>| {
        if !out.insert(d) {
            return;
        }
        let parts: Vec<Ty> = match &p.def(d).kind {
            DefKind::Struct(s) => s.fields.iter().map(|f| f.ty.clone()).collect(),
            DefKind::Enum(e) => e.variants.iter().flat_map(|v| v.payload.clone()).collect(),
            DefKind::Wrapper(w) => vec![w.inner.clone()],
            DefKind::Refinement(r) => vec![r.base.clone()],
            _ => Vec::new(),
        };
        for t in &parts {
            type_defs(p, t, out);
        }
    };
    match ty {
        Ty::Adt(d, args) => {
            visit(*d, out);
            for a in args {
                type_defs(p, a, out);
            }
        }
        Ty::Wrapper(d) | Ty::Refined(_, d) | Ty::Capability(d) => visit(*d, out),
        Ty::List(t) | Ty::Gen(t) => type_defs(p, t, out),
        _ => {}
    }
}

/// The new program's definitions that have a counterpart of the same shape in the old one.
fn canon(old: &Program, new: &Program) -> Canon {
    let mut c = Canon::default();
    for d in new.defs() {
        let Some(o) = old.by_stable_id(&d.stable_id) else {
            continue;
        };
        if shape(old, o) == shape(new, d.id) {
            c.to.insert(d.id, o);
            c.from.insert(o, d.id);
        }
    }
    c
}

/// What a value of a type definition looks like, in source terms.
fn shape(p: &Program, def: DefId) -> String {
    let d = p.def(def);
    match &d.kind {
        DefKind::Struct(s) => {
            let fields: Vec<String> = s
                .fields
                .iter()
                .map(|f| format!("{}:{}", f.name, p.display_ty_in(&f.ty, &s.generics)))
                .collect();
            let inv = s.invariant.as_ref().map(|i| i.predicate.text.clone());
            format!("struct{{{}}}{inv:?}", fields.join(","))
        }
        DefKind::Enum(e) => {
            let variants: Vec<String> = e
                .variants
                .iter()
                .map(|v| {
                    let payload: Vec<String> = v
                        .payload
                        .iter()
                        .map(|t| p.display_ty_in(t, &e.generics))
                        .collect();
                    format!("{}({})", v.name, payload.join(","))
                })
                .collect();
            format!("enum{{{}}}", variants.join(","))
        }
        DefKind::Wrapper(w) => format!("wrapper({})", p.display_ty(&w.inner)),
        DefKind::Refinement(r) => {
            format!("refined({}:{})", p.display_ty(&r.base), r.predicate.text)
        }
        other => other.keyword().to_string(),
    }
}
