//! Symbolic execution of the typed HIR (`spec/verification.md`).
//!
//! Every path carries its condition, its locals, and the variants it resolved for opaque enum
//! values. Constructs outside the decidable fragment (effects, text and list operations other than
//! literals, loop iterations, calls abstracted by their contracts) are over-approximated by fresh
//! values constrained by everything the program guarantees about them, and mark the path inexact:
//! a proof over the over-approximation is a proof of the program, while a model is a
//! counterexample only on an exact path.
//!
//! Values are kept in one *canonical* definition space so that two versions of a program can be
//! executed against shared inputs: the space of the program being verified, or in an equivalence
//! query the old program's, with the new program's definitions mapped through [`Canon`].

use crate::solver::{self, Form, Lin, Var};
use crate::Site;
use cpl_diag::{Span, StableId};
use cpl_hir::{
    Arm, Block, Clause, DefId, DefKind, Expr, ExprKind, FnBody, Lit, LocalId, Pattern, PatternKind,
    Program, Stmt, Ty,
};
use cpl_syntax::ast::{BinaryOp, UnaryOp};
use num_bigint::BigInt;
use num_traits::{One, Signed, ToPrimitive, Zero};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// The path abstracted a loop iteration by its invariants.
pub const HAVOC: u8 = 1;
/// The path abstracted a construct outside the fragment.
pub const ABSTRACT: u8 = 2;

/// `b == 1` for a boolean variable.
pub fn truth(v: Var) -> Form {
    Form::Eq(Lin::var(v).sub(&Lin::constant(1)))
}

#[derive(Clone, Debug)]
pub enum Sym {
    Int(Lin),
    Bool(Form),
    /// A struct (variant 0), a wrapper (variant 0, one field), or an enum variant. `def` is
    /// canonical.
    Data {
        def: DefId,
        variant: u32,
        fields: Vec<Sym>,
    },
    /// A list of known length.
    Seq(Vec<Sym>),
    /// A value known only by identity: text, bytes, an unknown list, a generic value, a handle,
    /// or an enum value whose variant is its tag variable. `ty` is canonical.
    Opaque {
        id: u32,
        ty: Ty,
    },
}

/// State shared by every execution of one query.
#[derive(Default)]
pub struct Shared {
    next_var: Var,
    pub bools: BTreeSet<Var>,
    next_opaque: u32,
    texts: HashMap<String, u32>,
    pub literal_texts: BTreeMap<u32, String>,
    /// Uninterpreted applications: the value and the assumptions that come with it.
    uf: HashMap<String, (Sym, Vec<Form>)>,
    eq_atoms: BTreeMap<(u32, u32), Var>,
    /// The variant index variable of an opaque enum value.
    pub tags: BTreeMap<u32, Var>,
    /// The payload of an opaque enum value in a variant, and its assumptions.
    pub payloads: BTreeMap<(u32, u32), (Vec<Sym>, Vec<Form>)>,
    /// `p == a * b`.
    pub products: Vec<(Var, Lin, Lin)>,
    divisions: HashMap<(Lin, Lin), (Var, Var)>,
    /// The value of `abs`, `min`, or `max` at its arguments: one variable for every path and
    /// side that applies it to the same arguments, so their conditions coincide syntactically.
    cases: HashMap<(&'static str, Lin, Lin), Var>,
    /// `(q, r, a, b)`: `q == a / b` and `r == a % b` (truncating) for a nonconstant `b`.
    pub quotients: Vec<(Var, Var, Lin, Lin)>,
    /// The canonical type of every opaque value, for rendering models.
    pub opaque_types: BTreeMap<u32, Ty>,
}

impl Shared {
    pub fn var(&mut self) -> Var {
        self.next_var += 1;
        self.next_var
    }
    fn bool_var(&mut self) -> Var {
        let v = self.var();
        self.bools.insert(v);
        v
    }
}

/// Maps a program's definitions into the canonical space (equivalence queries).
#[derive(Default)]
pub struct Canon {
    pub to: HashMap<DefId, DefId>,
    pub from: HashMap<DefId, DefId>,
}

/// Canonical ids of definitions only the mapped program has.
const FOREIGN: u32 = 0x8000_0000;

#[derive(Clone, Copy)]
pub enum Mode<'a> {
    /// Record the obligations of the root.
    Verify,
    /// Execute one side of an equivalence query: callees `same` judges identical are
    /// uninterpreted functions shared by both sides (or, with `inline`, executed in place like
    /// any other callee where that is possible), and so are the root's recursive calls.
    Equivalence {
        side: &'static str,
        same: &'a dyn Fn(&StableId) -> bool,
        canon: Option<&'a Canon>,
        inline: bool,
    },
}

#[derive(Clone, Debug)]
pub struct Path {
    pub cond: Vec<Form>,
    pub env: Vec<Option<Sym>>,
    /// Opaque enum values resolved on this path (also stated in `cond` by their tags).
    pub variants: BTreeMap<u32, u32>,
    pub inexact: u8,
    /// The capability operations and effectful calls performed, in order, with their arguments.
    pub trace: Vec<(String, Vec<Sym>)>,
    /// The root's `decreases` measure on entry, on this path.
    pub measure: Option<Lin>,
}

impl Path {
    /// Add a fact to the condition, once.
    pub fn add(&mut self, f: Form) {
        if f != Form::True && !self.cond.contains(&f) {
            self.cond.push(f);
        }
    }

    pub fn new(locals: usize) -> Path {
        Path {
            cond: Vec::new(),
            env: vec![None; locals],
            variants: BTreeMap::new(),
            inexact: 0,
            trace: Vec::new(),
            measure: None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Flow {
    Val(Sym),
    Ret(Sym),
    /// The path ends in a fault with this code.
    Fault(String),
}

pub type Outs = Vec<(Path, Flow)>;

/// One recorded obligation on one path: `cond` is the path condition with the goal negated.
#[derive(Clone, Debug)]
pub struct Check {
    /// The path condition, one fact per element.
    pub facts: Vec<Form>,
    /// The obligation, negated.
    pub negated: Form,
    pub inexact: u8,
}

impl Check {
    /// The whole condition: a model is a path that violates the obligation.
    pub fn cond(&self) -> Form {
        let mut all = self.facts.clone();
        all.push(self.negated.clone());
        Form::and(all)
    }

    /// The condition split into groups of facts that share no variable: the group of the
    /// negated obligation first. The condition is satisfiable exactly when every group is, and
    /// a model of it is the union of the groups' models.
    pub fn components(&self) -> Vec<Form> {
        let all: Vec<&Form> = std::iter::once(&self.negated).chain(&self.facts).collect();
        let mut parent: Vec<usize> = (0..all.len()).collect();
        fn find(parent: &mut [usize], i: usize) -> usize {
            let mut r = i;
            while parent[r] != r {
                r = parent[r];
            }
            let mut i = i;
            while parent[i] != r {
                let next = parent[i];
                parent[i] = r;
                i = next;
            }
            r
        }
        let mut owner: HashMap<Var, usize> = HashMap::new();
        for (i, f) in all.iter().enumerate() {
            for v in crate::solver::vars(f) {
                match owner.get(&v) {
                    Some(&j) => {
                        let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                        if a != b {
                            parent[a.max(b)] = a.min(b);
                        }
                    }
                    None => {
                        owner.insert(v, i);
                    }
                }
            }
        }
        let mut groups: BTreeMap<usize, Vec<Form>> = BTreeMap::new();
        for (i, f) in all.iter().enumerate() {
            let root = find(&mut parent, i);
            groups.entry(root).or_default().push((*f).clone());
        }
        // Root 0 is the negated obligation's group, and comes first.
        groups.into_values().map(Form::and).collect()
    }
}

const MAX_PATHS: usize = 4096;
const MAX_INLINE: usize = 5;
const MAX_AXIOM_DEPTH: u32 = 2;
const MAX_DEPTH: u32 = 8;
/// Concrete runs: iterations of one loop, and nested calls executed in place.
const MAX_UNROLL: usize = 10_000;
const MAX_CONCRETE_DEPTH: usize = 64;
/// Concrete runs: the expressions evaluated in all. A run forks nothing, so the path budget
/// never ends it, and a call in place inside a loop multiplies the work at every level (a
/// recursive function looping `y` times runs `y` to the depth of the recursion).
const MAX_CONCRETE_STEPS: usize = 200_000;

pub struct Exec<'p, 's> {
    pub p: &'p Program,
    pub shared: &'s mut Shared,
    pub root: DefId,
    pub mode: Mode<'s>,
    pub checks: Vec<(Site, Check)>,
    pub aborted: Option<&'static str>,
    paths: usize,
    /// The expressions a concrete run has evaluated.
    steps: usize,
    stack: Vec<DefId>,
    /// The generic arguments of each frame of `stack` (the root's: none).
    subst: Vec<Vec<Ty>>,
    /// Sites are recorded only for the root's own syntax.
    recording: bool,
    axiom_depth: u32,
    /// An equivalence side summarized a recursive call or a loop (partial equivalence).
    pub summarized: bool,
    /// Equivalence: paths that ended in a fault where no output stream could take them (a
    /// clause whose own evaluation faults); they are behavior like any other fault.
    pub faulted: Outs,
    /// Infer loop invariants (verification only).
    pub infer: bool,
    /// Concrete inputs: loops are run iteration by iteration and every call is executed in
    /// place (up to a depth), confirming a candidate counterexample.
    pub concrete: bool,
    /// Inferring a loop's invariants: loops nested in it use their declared invariants only.
    inferring: bool,
}

fn int(s: &Sym) -> Option<&Lin> {
    match s {
        Sym::Int(l) => Some(l),
        _ => None,
    }
}

pub fn boolean(s: &Sym) -> Form {
    match s {
        Sym::Bool(f) => f.clone(),
        _ => Form::False,
    }
}

impl<'p, 's> Exec<'p, 's> {
    pub fn new(p: &'p Program, shared: &'s mut Shared, root: DefId, mode: Mode<'s>) -> Self {
        Exec {
            p,
            shared,
            root,
            mode,
            checks: Vec::new(),
            aborted: None,
            paths: 0,
            steps: 0,
            stack: vec![root],
            subst: vec![Vec::new()],
            recording: matches!(mode, Mode::Verify),
            axiom_depth: 0,
            summarized: false,
            faulted: Vec::new(),
            infer: false,
            concrete: false,
            inferring: false,
        }
    }

    fn budget(&mut self, n: usize) -> bool {
        self.paths += n;
        if self.paths > MAX_PATHS {
            self.aborted.get_or_insert("limit");
            return false;
        }
        true
    }

    // ---- definition spaces -------------------------------------------------------------

    fn canon_map(&self) -> Option<&'s Canon> {
        match self.mode {
            Mode::Equivalence { canon, .. } => canon,
            Mode::Verify => None,
        }
    }

    /// A definition of this program in the canonical space.
    pub fn canon(&self, def: DefId) -> DefId {
        match self.canon_map() {
            Some(c) => c.to.get(&def).copied().unwrap_or(DefId(def.0 | FOREIGN)),
            None => def,
        }
    }

    /// A canonical definition in this program; `None` when it has no counterpart here.
    fn local(&mut self, def: DefId) -> Option<DefId> {
        match self.canon_map() {
            Some(c) => {
                if def.0 & FOREIGN != 0 {
                    Some(DefId(def.0 & !FOREIGN))
                } else {
                    let d = c.from.get(&def).copied();
                    if d.is_none() {
                        self.aborted.get_or_insert("type_changed");
                    }
                    d
                }
            }
            // The canonical space is this program's: only its own definitions are in it.
            None => (def.0 & FOREIGN == 0 && (def.0 as usize) < self.p.defs.len()).then_some(def),
        }
    }

    fn map_def(&mut self, d: DefId, to_canon: bool) -> DefId {
        if to_canon {
            self.canon(d)
        } else {
            self.local(d).unwrap_or(d)
        }
    }

    fn map_ty(&mut self, ty: &Ty, to_canon: bool) -> Ty {
        if self.canon_map().is_none() {
            return ty.clone();
        }
        match ty {
            Ty::List(t) => Ty::List(Box::new(self.map_ty(t, to_canon))),
            Ty::Gen(t) => Ty::Gen(Box::new(self.map_ty(t, to_canon))),
            Ty::Adt(d, args) => {
                let d = self.map_def(*d, to_canon);
                Ty::Adt(d, args.iter().map(|a| self.map_ty(a, to_canon)).collect())
            }
            Ty::Wrapper(d) => Ty::Wrapper(self.map_def(*d, to_canon)),
            Ty::Refined(class, d) => Ty::Refined(*class, self.map_def(*d, to_canon)),
            Ty::Capability(d) => Ty::Capability(self.map_def(*d, to_canon)),
            other => other.clone(),
        }
    }

    // ---- recording -----------------------------------------------------------------------

    pub fn assume(&self, mut path: Path, f: Form) -> Option<Path> {
        match f {
            Form::True => Some(path),
            Form::False => None,
            other => {
                path.add(other);
                Some(path)
            }
        }
    }

    pub fn check(&mut self, path: &Path, site: Site, goal: &Form) {
        if !self.recording {
            return;
        }
        self.checks.push((
            site,
            Check {
                facts: path.cond.clone(),
                negated: Form::not(goal.clone()),
                inexact: path.inexact,
            },
        ));
    }

    /// An obligation the prover cannot state on this path.
    fn check_unknown(&mut self, site: Site) {
        if self.recording {
            self.checks.push((
                site,
                Check {
                    facts: Vec::new(),
                    negated: Form::True,
                    inexact: ABSTRACT,
                },
            ));
        }
    }

    fn quiet<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        let recording = std::mem::replace(&mut self.recording, false);
        let out = f(self);
        self.recording = recording;
        out
    }

    /// Whether runtime-checked clauses are part of the behavior (equivalence queries).
    fn faults(&self) -> bool {
        matches!(self.mode, Mode::Equivalence { .. })
    }

    /// A runtime-checked clause: its obligation at `site`, then, when verifying, an assumption;
    /// in an equivalence query a path where it fails ends in `fault`, pushed to `out`.
    fn checked(
        &mut self,
        q: Path,
        site: Option<Site>,
        form: Form,
        fault: &str,
        out: &mut Outs,
    ) -> Option<Path> {
        if let Some(site) = site {
            self.check(&q, site, &form);
        }
        if self.faults() {
            let (yes, no) = self.split(q, &form);
            if let Some(n) = no {
                out.push((n, Flow::Fault(fault.to_string())));
            }
            yes
        } else {
            self.assume(q, form)
        }
    }

    // ---- values -------------------------------------------------------------------------

    /// A fresh value of `ty` (this program's space), with the assumptions its type guarantees.
    pub fn fresh(&mut self, path: &mut Path, ty: &Ty) -> Sym {
        // A type of an inlined generic callee is in terms of its own parameters.
        let ty = match self.subst.last() {
            Some(args) if !args.is_empty() => instantiate(ty, args),
            _ => ty.clone(),
        };
        self.fresh_depth(path, &ty, 0)
    }

    fn opaque(&mut self, ty: &Ty) -> Sym {
        let ty = self.map_ty(ty, true);
        self.shared.next_opaque += 1;
        let id = self.shared.next_opaque;
        self.shared.opaque_types.insert(id, ty.clone());
        Sym::Opaque { id, ty }
    }

    fn fresh_depth(&mut self, path: &mut Path, ty: &Ty, depth: u32) -> Sym {
        let p = self.p;
        match ty {
            Ty::Int => Sym::Int(Lin::var(self.shared.var())),
            Ty::Bool => Sym::Bool(truth(self.shared.bool_var())),
            Ty::Refined(_, def) if (def.0 as usize) < p.defs.len() => {
                let base = p
                    .def(*def)
                    .kind
                    .as_refinement()
                    .map(|r| r.base.clone())
                    .unwrap_or(Ty::Error);
                let value = self.fresh_depth(path, &base, depth + 1);
                if let Some((holds, inexact)) = self.predicate_holds(path, *def, &value) {
                    path.add(holds);
                    path.inexact |= inexact;
                }
                value
            }
            Ty::Wrapper(def) if depth < MAX_DEPTH && (def.0 as usize) < p.defs.len() => {
                let inner = p
                    .def(*def)
                    .kind
                    .as_wrapper()
                    .map(|w| w.inner.clone())
                    .unwrap_or(Ty::Error);
                let v = self.fresh_depth(path, &inner, depth + 1);
                Sym::Data {
                    def: self.canon(*def),
                    variant: 0,
                    fields: vec![v],
                }
            }
            Ty::Adt(def, args) if depth < MAX_DEPTH && (def.0 as usize) < p.defs.len() => match &p
                .def(*def)
                .kind
            {
                DefKind::Struct(s) => {
                    let tys: Vec<Ty> = s.fields.iter().map(|f| instantiate(&f.ty, args)).collect();
                    let fields = tys
                        .iter()
                        .map(|t| self.fresh_depth(path, t, depth + 1))
                        .collect();
                    let value = Sym::Data {
                        def: self.canon(*def),
                        variant: 0,
                        fields,
                    };
                    if s.invariant.is_some() {
                        if let Some((holds, inexact)) = self.predicate_holds(path, *def, &value) {
                            path.add(holds);
                            path.inexact |= inexact;
                        }
                    }
                    value
                }
                DefKind::Enum(e) if e.variants.len() == 1 && e.variants[0].payload.is_empty() => {
                    Sym::Data {
                        def: self.canon(*def),
                        variant: 0,
                        fields: Vec::new(),
                    }
                }
                _ => self.opaque(ty),
            },
            _ => self.opaque(ty),
        }
    }

    /// The condition under which `value` satisfies the predicate of refinement or struct `def`
    /// (this program's space), and whether stating it abstracted anything; `None` when the
    /// predicate cannot be evaluated.
    fn predicate_holds(&mut self, path: &mut Path, def: DefId, value: &Sym) -> Option<(Form, u8)> {
        let r = self.p.defs.get(def.0 as usize)?.kind.checked_predicate()?;
        let mut probe = path.clone();
        probe.env = vec![None; r.locals.len()];
        probe.env[r.self_local.0 as usize] = Some(value.clone());
        let start = probe.cond.len();
        self.stack.push(def);
        self.subst.push(Vec::new());
        let outs = self.quiet(|x| x.expr(probe, &r.predicate.expr));
        self.subst.pop();
        self.stack.pop();
        let mut alternatives = Vec::new();
        let mut inexact = 0;
        for (mut q, flow) in outs {
            match flow {
                Flow::Val(v) => {
                    let c = self.truth_of(&mut q, &v);
                    let mut parts = q.cond[start.min(q.cond.len())..].to_vec();
                    parts.push(c);
                    alternatives.push(Form::and(parts));
                    inexact |= q.inexact;
                }
                // A predicate that faults raises the fault itself: that input is not a member.
                _ => inexact |= ABSTRACT,
            }
        }
        if self.aborted.is_some() {
            return None;
        }
        Some((Form::or(alternatives), inexact))
    }

    fn text(&mut self, t: &str) -> Sym {
        let id = match self.shared.texts.get(t) {
            Some(id) => *id,
            None => {
                self.shared.next_opaque += 1;
                let id = self.shared.next_opaque;
                self.shared.texts.insert(t.to_string(), id);
                self.shared.literal_texts.insert(id, t.to_string());
                self.shared.opaque_types.insert(id, Ty::Text);
                id
            }
        };
        Sym::Opaque { id, ty: Ty::Text }
    }

    /// A value used as a condition. Every `Bool` value is a formula; anything else here is a value
    /// the prover could not type precisely, which is a boolean it does not know.
    pub fn truth_of(&mut self, path: &mut Path, v: &Sym) -> Form {
        match v {
            Sym::Bool(f) => f.clone(),
            other => {
                path.inexact |= ABSTRACT;
                let key = format!("bool:{}", self.key(other));
                match self.uf(path, key, &Ty::Bool) {
                    Sym::Bool(f) => f,
                    _ => truth(self.shared.bool_var()),
                }
            }
        }
    }

    /// The canonical key of a value, for uninterpreted applications.
    pub fn key(&self, s: &Sym) -> String {
        match s {
            Sym::Int(l) => format!("i{l:?}"),
            Sym::Bool(f) => format!("b{f:?}"),
            Sym::Data {
                def,
                variant,
                fields,
            } => {
                let parts: Vec<String> = fields.iter().map(|f| self.key(f)).collect();
                format!("d{}.{variant}({})", def.0, parts.join(","))
            }
            Sym::Seq(items) => {
                let parts: Vec<String> = items.iter().map(|f| self.key(f)).collect();
                format!("[{}]", parts.join(","))
            }
            Sym::Opaque { id, .. } => format!("o{id}"),
        }
    }

    /// The tag variable of an opaque enum value.
    /// The variable of a value defined by cases (`abs`, `min`, `max`) at its arguments.
    fn case_var(&mut self, name: &'static str, x: &Lin, y: &Lin) -> Var {
        let key = (name, x.clone(), y.clone());
        if let Some(v) = self.shared.cases.get(&key) {
            return *v;
        }
        let v = self.shared.var();
        self.shared.cases.insert(key, v);
        v
    }

    /// The variant tag of opaque enum value `id`, one of its enum's `count` variants: the range
    /// is stated on `path`, so that a model never names a variant that does not exist.
    fn tag(&mut self, path: &mut Path, id: u32, count: usize) -> Var {
        let v = match self.shared.tags.get(&id) {
            Some(v) => *v,
            None => {
                let v = self.shared.var();
                self.shared.tags.insert(id, v);
                v
            }
        };
        path.add(Form::le(&Lin::constant(0), &Lin::var(v)));
        path.add(Form::lt(&Lin::var(v), &Lin::constant(count as u32)));
        v
    }

    /// The enum definition (this program's space) and its type arguments of an opaque value.
    fn enum_of(&mut self, ty: &Ty) -> Option<(DefId, Vec<Ty>)> {
        let Ty::Adt(def, args) = ty else { return None };
        let local = self.local(*def)?;
        self.p.def(local).kind.as_enum()?;
        let args = args.iter().map(|a| self.map_ty(a, false)).collect();
        Some((local, args))
    }

    /// The payload of opaque enum value `id` in `variant`, shared by every path and side.
    fn payload(
        &mut self,
        path: &mut Path,
        id: u32,
        def: DefId,
        args: &[Ty],
        variant: u32,
    ) -> Vec<Sym> {
        if let Some((fields, assumptions)) = self.shared.payloads.get(&(id, variant)).cloned() {
            for a in assumptions {
                path.add(a);
            }
            return fields;
        }
        let tys: Vec<Ty> = self
            .p
            .def(def)
            .kind
            .as_enum()
            .and_then(|e| e.variants.get(variant as usize))
            .map(|v| v.payload.iter().map(|t| instantiate(t, args)).collect())
            .unwrap_or_default();
        let mut tmp = path.clone();
        tmp.cond.clear();
        // The payload types come from the value's own type: no frame's substitution applies.
        let fields: Vec<Sym> = tys
            .iter()
            .map(|t| self.fresh_depth(&mut tmp, t, 0))
            .collect();
        path.inexact |= tmp.inexact;
        for a in &tmp.cond {
            path.add(a.clone());
        }
        self.shared
            .payloads
            .insert((id, variant), (fields.clone(), tmp.cond));
        fields
    }

    /// Structural equality as a formula.
    pub fn eq(&mut self, path: &mut Path, a: &Sym, b: &Sym) -> Form {
        self.eq_depth(path, a, b, 0)
    }

    fn eq_depth(&mut self, path: &mut Path, a: &Sym, b: &Sym, depth: u32) -> Form {
        match (a, b) {
            (Sym::Int(x), Sym::Int(y)) => Form::eq(x, y),
            (Sym::Bool(x), Sym::Bool(y)) => Form::iff(x.clone(), y.clone()),
            (
                Sym::Data {
                    def: d1,
                    variant: v1,
                    fields: f1,
                },
                Sym::Data {
                    def: d2,
                    variant: v2,
                    fields: f2,
                },
            ) => {
                if d1 != d2 || v1 != v2 || f1.len() != f2.len() {
                    return Form::False;
                }
                let parts = f1
                    .iter()
                    .zip(f2)
                    .map(|(x, y)| self.eq_depth(path, x, y, depth + 1))
                    .collect();
                Form::and(parts)
            }
            (Sym::Seq(x), Sym::Seq(y)) => {
                if x.len() != y.len() {
                    return Form::False;
                }
                let parts = x
                    .iter()
                    .zip(y)
                    .map(|(a, b)| self.eq_depth(path, a, b, depth + 1))
                    .collect();
                Form::and(parts)
            }
            (Sym::Opaque { id: x, ty }, Sym::Opaque { id: y, .. }) => {
                if x == y {
                    return Form::True;
                }
                let lits = (
                    self.shared.literal_texts.get(x),
                    self.shared.literal_texts.get(y),
                );
                if let (Some(s), Some(t)) = lits {
                    return Form::bool(s == t);
                }
                // Two enum values are equal exactly when they agree on the variant and its payload.
                if depth < 3 {
                    if let Some((def, args)) = self.enum_of(ty) {
                        let count = self
                            .p
                            .def(def)
                            .kind
                            .as_enum()
                            .map_or(0, |e| e.variants.len());
                        let (tx, ty_) = (self.tag(path, *x, count), self.tag(path, *y, count));
                        let mut cases = Vec::new();
                        for k in 0..count as u32 {
                            let px = self.payload(path, *x, def, &args, k);
                            let py = self.payload(path, *y, def, &args, k);
                            let mut parts = vec![
                                Form::eq(&Lin::var(tx), &Lin::constant(k)),
                                Form::eq(&Lin::var(ty_), &Lin::constant(k)),
                            ];
                            for (a, b) in px.iter().zip(&py) {
                                parts.push(self.eq_depth(path, a, b, depth + 1));
                            }
                            cases.push(Form::and(parts));
                        }
                        return Form::or(cases);
                    }
                }
                path.inexact |= ABSTRACT;
                let key = ((*x).min(*y), (*x).max(*y));
                let v = match self.shared.eq_atoms.get(&key) {
                    Some(v) => *v,
                    None => {
                        let v = self.shared.bool_var();
                        self.shared.eq_atoms.insert(key, v);
                        v
                    }
                };
                truth(v)
            }
            (
                Sym::Opaque { id, ty },
                Sym::Data {
                    variant, fields, ..
                },
            )
            | (
                Sym::Data {
                    variant, fields, ..
                },
                Sym::Opaque { id, ty },
            ) => {
                if let Some((def, args)) = self.enum_of(ty) {
                    let count = self
                        .p
                        .def(def)
                        .kind
                        .as_enum()
                        .map_or(0, |e| e.variants.len());
                    let t = self.tag(path, *id, count);
                    let payload = self.payload(path, *id, def, &args, *variant);
                    let mut parts = vec![Form::eq(&Lin::var(t), &Lin::constant(*variant))];
                    for (a, b) in payload.iter().zip(fields) {
                        parts.push(self.eq_depth(path, a, b, depth + 1));
                    }
                    return Form::and(parts);
                }
                path.inexact |= ABSTRACT;
                truth(self.shared.bool_var())
            }
            _ => {
                path.inexact |= ABSTRACT;
                truth(self.shared.bool_var())
            }
        }
    }

    // ---- control flow helpers ------------------------------------------------------------

    /// Fork on a boolean: the paths where it holds and where it fails, infeasible ones dropped.
    pub fn split(&mut self, path: Path, c: &Form) -> (Option<Path>, Option<Path>) {
        match c {
            Form::True => return (Some(path), None),
            Form::False => return (None, Some(path)),
            _ => {}
        }
        if !self.budget(1) {
            return (None, None);
        }
        (
            self.assume(path.clone(), c.clone()),
            self.assume(path, Form::not(c.clone())),
        )
    }

    /// Evaluate expressions left to right across every path.
    fn seq(&mut self, path: Path, es: &[&Expr]) -> Vec<(Path, Result<Vec<Sym>, Flow>)> {
        let mut states = vec![(path, Ok(Vec::new()))];
        for e in es {
            let mut next = Vec::new();
            for (p, acc) in states {
                match acc {
                    Err(flow) => next.push((p, Err(flow))),
                    Ok(values) => {
                        for (q, flow) in self.expr(p, e) {
                            match flow {
                                Flow::Val(v) => {
                                    let mut vs = values.clone();
                                    vs.push(v);
                                    next.push((q, Ok(vs)));
                                }
                                other => next.push((q, Err(other))),
                            }
                        }
                    }
                }
            }
            states = next;
            if self.aborted.is_some() {
                return Vec::new();
            }
        }
        states
    }

    pub fn unit(&self) -> Sym {
        Sym::Data {
            def: self.canon(self.p.std_defs().unit),
            variant: 0,
            fields: Vec::new(),
        }
    }

    fn variant_index(&self, def: DefId, name: &str) -> u32 {
        self.p
            .def(def)
            .kind
            .as_enum()
            .and_then(|e| e.variants.iter().position(|v| v.name == name))
            .unwrap_or(0) as u32
    }

    /// The type of a local of the definition being executed.
    fn local_def_ty(&self, local: LocalId) -> Ty {
        let def = self.stack.last().copied().unwrap_or(self.root);
        self.p
            .def(def)
            .kind
            .locals()
            .get(local.0 as usize)
            .map(|l| l.ty.clone())
            .unwrap_or(Ty::Error)
    }

    // ---- statements and blocks -------------------------------------------------------------

    pub fn block(&mut self, path: Path, b: &Block) -> Outs {
        let mut paths = vec![path];
        let mut done: Outs = Vec::new();
        for s in &b.stmts {
            let mut next = Vec::new();
            for p in paths {
                for (q, flow) in self.stmt(p, s) {
                    match flow {
                        Flow::Val(_) => next.push(q),
                        other => done.push((q, other)),
                    }
                }
            }
            paths = next;
            if self.aborted.is_some() {
                return Vec::new();
            }
        }
        for p in paths {
            match &b.tail {
                Some(t) => done.extend(self.expr(p, t)),
                None => {
                    let unit = self.unit();
                    done.push((p, Flow::Val(unit)));
                }
            }
        }
        done
    }

    fn stmt(&mut self, path: Path, s: &Stmt) -> Outs {
        let unit = self.unit();
        match s {
            Stmt::Let { local, value } | Stmt::Assign { local, value } => self
                .expr(path, value)
                .into_iter()
                .map(|(mut q, flow)| match flow {
                    Flow::Val(v) => {
                        if let Some(slot) = q.env.get_mut(local.0 as usize) {
                            *slot = Some(v);
                        }
                        (q, Flow::Val(unit.clone()))
                    }
                    other => (q, other),
                })
                .collect(),
            Stmt::Discard(e) | Stmt::Expr(e) => self
                .expr(path, e)
                .into_iter()
                .map(|(q, flow)| match flow {
                    Flow::Val(_) => (q, Flow::Val(unit.clone())),
                    other => (q, other),
                })
                .collect(),
            Stmt::Return(e) => match e {
                Some(e) => self
                    .expr(path, e)
                    .into_iter()
                    .map(|(q, flow)| match flow {
                        Flow::Val(v) => (q, Flow::Ret(v)),
                        other => (q, other),
                    })
                    .collect(),
                None => vec![(path, Flow::Ret(unit))],
            },
            Stmt::Assert { expr, .. } => {
                let mut out = Vec::new();
                for (mut q, flow) in self.expr(path, expr) {
                    match flow {
                        Flow::Val(v) => {
                            let c = self.truth_of(&mut q, &v);
                            self.check(&q, Site::Assert(expr.span), &c);
                            let (yes, no) = self.split(q, &c);
                            if let Some(y) = yes {
                                out.push((y, Flow::Val(unit.clone())));
                            }
                            if let Some(n) = no {
                                out.push((n, Flow::Fault("assertion_failed".into())));
                            }
                        }
                        other => out.push((q, other)),
                    }
                }
                out
            }
            Stmt::While {
                cond,
                invariants,
                decreases,
                body,
            } => {
                if let Some(outs) =
                    self.summarize_loop(&path, cond, invariants, decreases.as_ref(), body)
                {
                    return outs;
                }
                self.exec_while(path, cond, invariants, decreases.as_ref(), body)
            }
            Stmt::Concurrent(tasks) => {
                // The result is that of running the tasks in order (spec/concurrency.md).
                let mut paths = vec![path];
                let mut done = Vec::new();
                for t in tasks {
                    let mut next = Vec::new();
                    for p in paths {
                        for (q, flow) in self.stmt(p, &t.stmt) {
                            match flow {
                                Flow::Val(_) => next.push(q),
                                other => done.push((q, other)),
                            }
                        }
                    }
                    paths = next;
                }
                done.extend(paths.into_iter().map(|p| (p, Flow::Val(unit.clone()))));
                done
            }
        }
    }

    /// Evaluate a clause in the current frame; one formula per resulting path.
    fn clause_here(&mut self, path: Path, c: &Clause) -> Vec<(Path, Form)> {
        let outs = self.expr(path, &c.expr);
        let mut out = Vec::with_capacity(outs.len());
        for (mut q, flow) in outs {
            // A clause that faults raises the fault itself; its paths end there.
            match flow {
                Flow::Val(v) => {
                    let f = self.truth_of(&mut q, &v);
                    out.push((q, f));
                }
                other if self.faults() => self.faulted.push((q, other)),
                _ => {}
            }
        }
        out
    }

    /// Assume `clauses` in order on every path (the later ones only where the earlier hold).
    fn assume_clauses(&mut self, paths: Vec<Path>, clauses: &[Clause]) -> Vec<Path> {
        let mut paths = paths;
        for c in clauses {
            let mut next = Vec::new();
            for p in paths {
                for (q, f) in self.clause_here(p, c) {
                    if let Some(q) = self.assume(q, f) {
                        next.push(q);
                    }
                }
            }
            paths = next;
        }
        paths
    }

    fn exec_while(
        &mut self,
        path: Path,
        cond: &Expr,
        invariants: &[Clause],
        decreases: Option<&Clause>,
        body: &Block,
    ) -> Outs {
        if self.concrete {
            return self.unroll(path, cond, invariants, decreases, body);
        }
        let site = cond.span;
        let unit = self.unit();
        // Invariants on entry, each assuming the previous ones.
        let mut paths = vec![path];
        let mut out = Vec::new();
        for (i, inv) in invariants.iter().enumerate() {
            let mut next = Vec::new();
            for p in paths {
                for (q, f) in self.clause_here(p, inv) {
                    if let Some(q) = self.checked(
                        q,
                        Some(Site::InvariantInit(site, i)),
                        f,
                        "contract_violation:invariant",
                        &mut out,
                    ) {
                        next.push(q);
                    }
                }
            }
            paths = next;
        }
        // An arbitrary iteration: every local the loop assigns takes an arbitrary value that
        // satisfies the invariants (they are checked on entry and after every iteration).
        let mut assigned = BTreeSet::new();
        assigned_locals(body, &mut assigned);
        for mut p in paths {
            // Facts the loop maintains beyond its declared invariants, proved inductive.
            let inferred = if self.infer && matches!(self.mode, Mode::Verify) && !self.inferring {
                self.infer_invariants(&p, cond, invariants, body, &assigned)
            } else {
                None
            };
            p.inexact |= HAVOC;
            let mut head: BTreeMap<LocalId, Lin> = BTreeMap::new();
            for local in &assigned {
                let ty = self.local_def_ty(*local);
                let v = self.fresh(&mut p, &ty);
                if let Sym::Int(l) = &v {
                    head.insert(*local, l.clone());
                }
                if let Some(slot) = p.env.get_mut(local.0 as usize) {
                    *slot = Some(v);
                }
            }
            if let Some((vars, facts)) = inferred {
                let map: BTreeMap<Var, Lin> = vars
                    .iter()
                    .filter_map(|(local, v)| head.get(local).map(|l| (*v, l.clone())))
                    .collect();
                for f in facts {
                    p.add(f.substitute_all(&map));
                }
            }
            let heads = self.quiet(|x| x.assume_clauses(vec![p], invariants));
            for h in heads {
                for (mut q, flow) in self.expr(h, cond) {
                    let c = match flow {
                        Flow::Val(v) => self.truth_of(&mut q, &v),
                        other => {
                            out.push((q, other));
                            continue;
                        }
                    };
                    let (enter, exit) = self.split(q, &c);
                    if let Some(e) = exit {
                        out.push((e, Flow::Val(unit.clone())));
                    }
                    let Some(e) = enter else { continue };
                    // The measure is nonnegative before each iteration.
                    let mut starts = Vec::new();
                    match decreases {
                        Some(m) => {
                            for (q2, flow) in self.expr(e, &m.expr) {
                                match flow {
                                    Flow::Val(Sym::Int(v)) => {
                                        let nonnegative = Form::le(&Lin::default(), &v);
                                        if let Some(q2) = self.checked(
                                            q2,
                                            Some(Site::LoopDecreases(site)),
                                            nonnegative,
                                            "contract_violation:decreases",
                                            &mut out,
                                        ) {
                                            starts.push((q2, Some(v)));
                                        }
                                    }
                                    Flow::Val(_) => {
                                        self.check_unknown(Site::LoopDecreases(site));
                                        starts.push((q2, None));
                                    }
                                    other => out.push((q2, other)),
                                }
                            }
                        }
                        None => starts.push((e, None)),
                    }
                    for (start, measure) in starts {
                        for (after, flow) in self.block(start, body) {
                            match flow {
                                Flow::Val(_) => {
                                    let mut ends = vec![after];
                                    for (i, inv) in invariants.iter().enumerate() {
                                        let mut next = Vec::new();
                                        for x in ends {
                                            for (y, f) in self.clause_here(x, inv) {
                                                if let Some(y) = self.checked(
                                                    y,
                                                    Some(Site::InvariantPreserve(site, i)),
                                                    f,
                                                    "contract_violation:invariant",
                                                    &mut out,
                                                ) {
                                                    next.push(y);
                                                }
                                            }
                                        }
                                        ends = next;
                                    }
                                    // Another iteration must see a smaller measure.
                                    let (Some(m0), Some(mc)) = (&measure, decreases) else {
                                        continue;
                                    };
                                    for x in ends {
                                        for (mut y, again) in self.quiet(|s| s.expr(x, cond)) {
                                            let Flow::Val(again) = again else { continue };
                                            let again = self.truth_of(&mut y, &again);
                                            let Some(y) = self.assume(y, again) else {
                                                continue;
                                            };
                                            for (z, m1) in self.quiet(|s| s.expr(y, &mc.expr)) {
                                                match m1 {
                                                    Flow::Val(Sym::Int(m1)) => {
                                                        let smaller = Form::lt(&m1, m0);
                                                        // The next iteration is the arbitrary one: this path ends here.
                                                        let _ = self.checked(
                                                            z,
                                                            Some(Site::LoopDecreases(site)),
                                                            smaller,
                                                            "contract_violation:decreases",
                                                            &mut out,
                                                        );
                                                    }
                                                    Flow::Val(_) => self
                                                        .check_unknown(Site::LoopDecreases(site)),
                                                    _ => {}
                                                }
                                            }
                                        }
                                    }
                                }
                                other => out.push((after, other)),
                            }
                        }
                    }
                }
            }
        }
        out
    }

    /// A loop on concrete values: its iterations one by one, with the checks of checked mode
    /// (semantics 5.3): the invariants before the first test and after every iteration, the
    /// measure after the condition held (nonnegative, and below the previous iteration's).
    /// Values the inputs do not settle abort the run.
    fn unroll(
        &mut self,
        path: Path,
        cond: &Expr,
        invariants: &[Clause],
        decreases: Option<&Clause>,
        body: &Block,
    ) -> Outs {
        let site = cond.span;
        let unit = self.unit();
        let mut out = Vec::new();
        let mut current = vec![path];
        let mut previous: Option<Lin> = None;
        for iteration in 0..MAX_UNROLL {
            let mut heads = Vec::new();
            for p in current {
                let mut ps = vec![p];
                for (i, inv) in invariants.iter().enumerate() {
                    let at = if iteration == 0 {
                        Site::InvariantInit(site, i)
                    } else {
                        Site::InvariantPreserve(site, i)
                    };
                    let mut next = Vec::new();
                    for q in ps {
                        for (r, f) in self.clause_here(q, inv) {
                            if let Some(r) = self.checked(
                                r,
                                Some(at),
                                f,
                                "contract_violation:invariant",
                                &mut out,
                            ) {
                                next.push(r);
                            }
                        }
                    }
                    ps = next;
                }
                heads.extend(ps);
            }
            let mut entering = Vec::new();
            for h in heads {
                for (mut q, flow) in self.expr(h, cond) {
                    let c = match flow {
                        Flow::Val(v) => self.truth_of(&mut q, &v),
                        other => {
                            out.push((q, other));
                            continue;
                        }
                    };
                    let (enter, exit) = self.split(q, &c);
                    // On concrete inputs the condition is decided; a loop that can both enter
                    // and exit depends on something abstract (a hole, an opaque value), and
                    // unrolling it would add an exit and a fresh value on every iteration up
                    // to MAX_UNROLL, its paths' conditions growing each time.
                    if enter.is_some() && exit.is_some() {
                        self.aborted.get_or_insert("limit");
                        return out;
                    }
                    if let Some(e) = exit {
                        out.push((e, Flow::Val(unit.clone())));
                    }
                    entering.extend(enter);
                }
            }
            if entering.len() > 1 {
                self.aborted.get_or_insert("limit");
                return out;
            }
            let Some(e) = entering.pop() else {
                return out;
            };
            let mut starts = Vec::new();
            match decreases {
                Some(m) => {
                    for (q, flow) in self.expr(e, &m.expr) {
                        match flow {
                            Flow::Val(Sym::Int(v)) => {
                                let mut ok = vec![Form::le(&Lin::default(), &v)];
                                if let Some(prev) = &previous {
                                    ok.push(Form::lt(&v, prev));
                                }
                                if let Some(q) = self.checked(
                                    q,
                                    Some(Site::LoopDecreases(site)),
                                    Form::and(ok),
                                    "contract_violation:decreases",
                                    &mut out,
                                ) {
                                    previous = Some(v);
                                    starts.push(q);
                                }
                            }
                            Flow::Val(_) => {
                                self.aborted.get_or_insert("limit");
                                return out;
                            }
                            other => out.push((q, other)),
                        }
                    }
                }
                None => starts.push(e),
            }
            current = Vec::new();
            for start in starts {
                for (after, flow) in self.block(start, body) {
                    match flow {
                        Flow::Val(_) => current.push(after),
                        other => out.push((after, other)),
                    }
                }
            }
            if current.is_empty() {
                return out;
            }
        }
        self.aborted.get_or_insert("limit");
        out
    }

    /// A value of `ty` (this program's space) from its value JSON (toolchain 7.3); `None` for a
    /// type or value it does not represent concretely.
    pub fn from_json(&mut self, ty: &Ty, v: &serde_json::Value) -> Option<Sym> {
        let p = self.p;
        match ty {
            Ty::Int => {
                let n: BigInt = v.as_number()?.to_string().parse().ok()?;
                Some(Sym::Int(Lin::constant(n)))
            }
            Ty::Bool => Some(Sym::Bool(Form::bool(v.as_bool()?))),
            Ty::Text => Some(self.text(v.as_str()?)),
            Ty::List(t) => {
                let items = v.as_array()?;
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    out.push(self.from_json(t, item)?);
                }
                Some(Sym::Seq(out))
            }
            Ty::Refined(_, def) => {
                let base = p.def(*def).kind.as_refinement()?.base.clone();
                self.from_json(&base, v)
            }
            Ty::Wrapper(def) => {
                let inner = p.def(*def).kind.as_wrapper()?.inner.clone();
                let payload = v.get("$payload")?.as_array()?.first()?;
                let inner = self.from_json(&inner, payload)?;
                Some(Sym::Data {
                    def: self.canon(*def),
                    variant: 0,
                    fields: vec![inner],
                })
            }
            Ty::Adt(def, _) if *def == p.std_defs().unit => Some(self.unit()),
            Ty::Adt(def, args) => match &p.def(*def).kind {
                DefKind::Struct(s) => {
                    let mut fields = Vec::with_capacity(s.fields.len());
                    for f in &s.fields {
                        let t = instantiate(&f.ty, args);
                        fields.push(self.from_json(&t, v.get(f.name.as_str())?)?);
                    }
                    Some(Sym::Data {
                        def: self.canon(*def),
                        variant: 0,
                        fields,
                    })
                }
                DefKind::Enum(e) => {
                    let name = v.get("$variant")?.as_str()?;
                    let k = e.variants.iter().position(|x| x.name == name)?;
                    let payload = match v.get("$payload") {
                        Some(items) => items.as_array()?.clone(),
                        None => Vec::new(),
                    };
                    if payload.len() != e.variants[k].payload.len() {
                        return None;
                    }
                    let mut fields = Vec::with_capacity(payload.len());
                    for (t, item) in e.variants[k].payload.iter().zip(&payload) {
                        fields.push(self.from_json(&instantiate(t, args), item)?);
                    }
                    Some(Sym::Data {
                        def: self.canon(*def),
                        variant: k as u32,
                        fields,
                    })
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// Invariant inference (Houdini) for a loop entered on `entry`. The candidates are facts
    /// about the `Int` locals the loop assigns, stated over fresh head variables: each local is
    /// at least (at most) its entry value; two locals that change by constant steps keep their
    /// ratio of changes; an atom of the condition, relaxed by a step, holds (or the local is
    /// still at its entry value). A candidate survives when it holds on entry and every
    /// iteration that ends normally preserves it, assuming the declared invariants (run-time
    /// checks enforce them), the surviving candidates, and the condition; the survivors are
    /// inductive, so assuming them at the head is sound. `None` when nothing survives or the
    /// inference exceeds its budget.
    fn infer_invariants(
        &mut self,
        entry: &Path,
        cond: &Expr,
        invariants: &[Clause],
        body: &Block,
        assigned: &BTreeSet<LocalId>,
    ) -> Option<(BTreeMap<LocalId, Var>, Vec<Form>)> {
        const CANDIDATES: usize = 32;
        const PATHS: usize = 512;
        let entry_values: BTreeMap<LocalId, Lin> = assigned
            .iter()
            .filter_map(|local| match entry.env.get(local.0 as usize) {
                Some(Some(Sym::Int(l))) => Some((*local, l.clone())),
                _ => None,
            })
            .collect();
        if entry_values.is_empty() {
            return None;
        }
        let saved = (self.paths, self.aborted, self.inferring);
        self.inferring = true;
        let result = self.quiet(|x| {
            // The head of an arbitrary iteration.
            let mut head = entry.clone();
            let mut vars: BTreeMap<LocalId, Var> = BTreeMap::new();
            for local in assigned {
                let ty = x.local_def_ty(*local);
                let v = x.fresh(&mut head, &ty);
                if let Sym::Int(l) = &v {
                    if let ([(var, c)], true) = (
                        l.terms.iter().collect::<Vec<_>>().as_slice(),
                        l.constant.is_zero(),
                    ) {
                        if c.is_one() && entry_values.contains_key(local) {
                            vars.insert(*local, **var);
                        }
                    }
                }
                if let Some(slot) = head.env.get_mut(local.0 as usize) {
                    *slot = Some(v);
                }
            }
            if vars.is_empty() {
                return None;
            }
            let at_head = |local: &LocalId| Lin::var(vars[local]);
            let mut candidates: Vec<Form> = Vec::new();
            for (local, e) in &entry_values {
                if vars.contains_key(local) {
                    candidates.push(Form::le(e, &at_head(local)));
                    candidates.push(Form::le(&at_head(local), e));
                }
            }
            // One iteration: the steps of the locals and the atoms of the condition.
            let mut steps: BTreeMap<LocalId, Option<BigInt>> = BTreeMap::new();
            let mut atoms: Vec<Lin> = Vec::new();
            for after in x.iterate(&head, cond, invariants, body, &[], Some(&mut atoms))? {
                for (local, v) in &vars {
                    let step = match after.env.get(local.0 as usize) {
                        Some(Some(Sym::Int(l))) => {
                            let d = l.sub(&Lin::var(*v));
                            d.terms.is_empty().then_some(d.constant)
                        }
                        _ => None,
                    };
                    let known = steps.entry(*local).or_insert_with(|| step.clone());
                    if *known != step {
                        *known = None;
                    }
                }
            }
            let stepping: Vec<(LocalId, BigInt)> = steps
                .iter()
                .filter_map(|(l, d)| d.clone().filter(|d| !d.is_zero()).map(|d| (*l, d)))
                .collect();
            for (i, (v, dv)) in stepping.iter().enumerate() {
                for (w, dw) in &stepping[i + 1..] {
                    let change = |l: &LocalId| at_head(l).sub(&entry_values[l]);
                    candidates.push(Form::eq(&change(v).scale(dw), &change(w).scale(dv)));
                }
            }
            for atom in &atoms {
                let mentioned: Vec<(&LocalId, &Var)> = vars
                    .iter()
                    .filter(|(_, v)| atom.terms.contains_key(v))
                    .collect();
                let [(local, var)] = mentioned.as_slice() else {
                    continue;
                };
                if !atom.terms[var].abs().is_one() {
                    continue;
                }
                let step = steps
                    .get(local)
                    .cloned()
                    .flatten()
                    .map_or(BigInt::one(), |d| d.abs().max(BigInt::one()));
                let relaxed = Form::Le(atom.sub(&Lin::constant(step)));
                candidates.push(relaxed.clone());
                candidates.push(Form::or(vec![
                    relaxed,
                    Form::eq(&at_head(local), &entry_values[local]),
                ]));
            }
            candidates.sort();
            candidates.dedup();
            candidates.truncate(CANDIDATES);
            // On entry, the head variables are the entry values.
            let at_entry: BTreeMap<Var, Lin> = vars
                .iter()
                .map(|(local, v)| (*v, entry_values[local].clone()))
                .collect();
            candidates.retain(|c| x.implied(&entry.cond, &c.substitute_all(&at_entry)));
            // Drop candidates some iteration breaks, until the rest are inductive.
            while !candidates.is_empty() {
                let afters = x.iterate(&head, cond, invariants, body, &candidates, None)?;
                if x.paths > saved.0 + PATHS {
                    return None;
                }
                let mut broken = BTreeSet::new();
                for after in &afters {
                    let now: BTreeMap<Var, Lin> = vars
                        .iter()
                        .filter_map(|(local, v)| match after.env.get(local.0 as usize) {
                            Some(Some(Sym::Int(l))) => Some((*v, l.clone())),
                            _ => None,
                        })
                        .collect();
                    for (i, c) in candidates.iter().enumerate() {
                        if !broken.contains(&i) && !x.implied(&after.cond, &c.substitute_all(&now))
                        {
                            broken.insert(i);
                        }
                    }
                }
                if broken.is_empty() {
                    break;
                }
                let mut i = 0;
                candidates.retain(|_| {
                    i += 1;
                    !broken.contains(&(i - 1))
                });
            }
            (!candidates.is_empty()).then_some((vars, candidates))
        });
        let failed = self.aborted.is_some() && saved.1.is_none();
        self.paths = saved.0;
        self.aborted = saved.1;
        self.inferring = saved.2;
        if failed {
            return None;
        }
        result
    }

    /// The paths of one iteration from `head` that end normally (quietly): the declared
    /// invariants and `facts` assumed, the condition holding, the body run. The atoms of the
    /// condition, as `l <= 0`, are added to `atoms`. `None` past the inference's path budget.
    fn iterate(
        &mut self,
        head: &Path,
        cond: &Expr,
        invariants: &[Clause],
        body: &Block,
        facts: &[Form],
        mut atoms: Option<&mut Vec<Lin>>,
    ) -> Option<Vec<Path>> {
        let mut start = head.clone();
        for f in facts {
            start.add(f.clone());
        }
        let mut out = Vec::new();
        for h in self.assume_clauses(vec![start], invariants) {
            for (mut q, flow) in self.expr(h, cond) {
                let Flow::Val(v) = flow else { continue };
                let c = self.truth_of(&mut q, &v);
                if let Some(atoms) = atoms.as_deref_mut() {
                    condition_atoms(&c, true, atoms);
                }
                let Some(e) = self.assume(q, c) else { continue };
                for (after, flow) in self.block(e, body) {
                    if matches!(flow, Flow::Val(_)) {
                        out.push(after);
                    }
                }
                if out.len() > 256 || self.aborted.is_some() {
                    return None;
                }
            }
        }
        Some(out)
    }

    /// Whether `facts` imply `goal` (unsatisfiability of the facts with the goal negated).
    fn implied(&self, facts: &[Form], goal: &Form) -> bool {
        if *goal == Form::True {
            return true;
        }
        let mut all = facts.to_vec();
        all.push(Form::not(goal.clone()));
        solver::solve(&Form::and(all), &self.shared.bools) == solver::Outcome::Unsat
    }

    /// In an equivalence query, a loop whose text, callees, and inputs are the same on both sides
    /// has the same outcome on both: an uninterpreted function of its inputs.
    fn summarize_loop(
        &mut self,
        path: &Path,
        cond: &Expr,
        invariants: &[Clause],
        decreases: Option<&Clause>,
        body: &Block,
    ) -> Option<Outs> {
        let Mode::Equivalence { same, .. } = self.mode else {
            return None;
        };
        let p = self.p;
        let sm = p.source_map();
        let (start, end) = (cond.span.start, body.span.end);
        if cond.span.file != body.span.file || end < start {
            return None;
        }
        let mut text = String::from(sm.text(cond.span));
        for c in invariants.iter().chain(decreases) {
            text.push_str(" ;");
            text.push_str(&c.text);
        }
        text.push_str(sm.text(body.span));
        // Every callee must be one the query shares, and no effect may happen inside.
        let mut ok = true;
        let mut callees = BTreeSet::new();
        let mut reads = BTreeSet::new();
        // The type definitions the loop builds or inspects: the same text means the same
        // behavior only if they are the same definitions on both sides.
        let mut types = BTreeSet::new();
        let mut visit = |e: &Expr| match &e.kind {
            ExprKind::Ctor { def, .. }
            | ExprKind::Wrap { def, .. }
            | ExprKind::StructLit { def, .. }
            | ExprKind::Field { def, .. }
            | ExprKind::Widen { from: def, .. }
            | ExprKind::Refine { refined: def, .. }
            | ExprKind::Check { refined: def, .. } => {
                types.insert(*def);
            }
            ExprKind::Call { callee, .. } => {
                let d = p.def(*callee);
                let builtin = matches!(d.kind.as_fn().map(|f| &f.body), Some(FnBody::Builtin(_)));
                if !builtin && !same(&d.stable_id) {
                    ok = false;
                }
                callees.insert(d.stable_id.as_str().to_string());
            }
            ExprKind::CapCall { .. } | ExprKind::Hole { .. } | ExprKind::Error => ok = false,
            ExprKind::Local(l) => {
                reads.insert(*l);
            }
            _ => {}
        };
        cond.walk(&mut visit);
        for c in invariants.iter().chain(decreases) {
            c.expr.walk(&mut visit);
        }
        body.walk(&mut visit);
        if !ok {
            return None;
        }
        let mut returns = false;
        for s in &body.stmts {
            s.walk_stmts(&mut |s| {
                if matches!(s, Stmt::Return(_)) {
                    returns = true;
                }
            });
        }
        let def = self.stack.last().copied().unwrap_or(self.root);
        let locals = p.def(def).kind.locals();
        let outside = |l: &LocalId| {
            locals.get(l.0 as usize).is_some_and(|d| {
                d.span.file != cond.span.file || d.span.start < start || d.span.start > end
            })
        };
        let mut inputs: Vec<(String, LocalId)> = reads
            .iter()
            .filter(|l| outside(l))
            .map(|l| (locals[l.0 as usize].name.clone(), *l))
            .collect();
        inputs.sort();
        let mut assigned = BTreeSet::new();
        assigned_locals(body, &mut assigned);
        let mut outputs: Vec<(String, LocalId)> = assigned
            .iter()
            .filter(|l| outside(l))
            .map(|l| (locals[l.0 as usize].name.clone(), *l))
            .collect();
        outputs.sort();
        let types: Vec<u32> = types.iter().map(|d| self.canon(*d).0).collect();
        let mut key = format!("loop:{text}:{callees:?}:{types:?}");
        for (name, l) in &inputs {
            let v = path.env.get(l.0 as usize).cloned().flatten()?;
            let ty = self.map_ty(&locals[l.0 as usize].ty, true);
            key.push_str(&format!(":{name}:{ty:?}={}", self.key(&v)));
        }
        for (name, l) in &outputs {
            let ty = self.map_ty(&locals[l.0 as usize].ty, true);
            key.push_str(&format!(":{name}:{ty:?}"));
        }
        self.summarized = true;
        let mut path = path.clone();
        path.inexact |= ABSTRACT;
        let mut out = Vec::new();
        let fault = self.uf(&mut path, format!("{key}:fault"), &Ty::Bool);
        let (faults, completes) = self.split(path, &boolean(&fault));
        if let Some(f) = faults {
            out.push((f, Flow::Fault(format!("loop_fault:{key}"))));
        }
        let mut path = completes?;
        if returns {
            let ret_ty = self.fn_ret();
            let flag = self.uf(&mut path, format!("{key}:returns"), &Ty::Bool);
            let (r, c) = self.split(path, &boolean(&flag));
            if let Some(mut r) = r {
                let v = self.uf(&mut r, format!("{key}:result"), &ret_ty);
                out.push((r, Flow::Ret(v)));
            }
            let Some(c) = c else { return Some(out) };
            path = c;
        }
        for (name, l) in &outputs {
            let ty = locals[l.0 as usize].ty.clone();
            let v = self.uf(&mut path, format!("{key}:out:{name}"), &ty);
            path.env[l.0 as usize] = Some(v);
        }
        let unit = self.unit();
        out.push((path, Flow::Val(unit)));
        Some(out)
    }

    // ---- expressions -----------------------------------------------------------------------

    pub fn expr(&mut self, path: Path, e: &Expr) -> Outs {
        if self.aborted.is_some() {
            return Vec::new();
        }
        // A concrete run keeps only the paths that abstracted nothing, and a path stays inexact
        // once it is: it is dropped here, before it is explored. Explored, a call run in place
        // on an abstract value (a list builtin's result) recurses to the depth limit, forking
        // at every level.
        if self.concrete && path.inexact != 0 {
            return Vec::new();
        }
        if self.concrete {
            self.steps += 1;
            if self.steps > MAX_CONCRETE_STEPS {
                self.aborted.get_or_insert("limit");
                return Vec::new();
            }
        }
        match &e.kind {
            ExprKind::Lit(Lit::Int(n)) => {
                vec![(path, Flow::Val(Sym::Int(Lin::constant(n.clone()))))]
            }
            ExprKind::Lit(Lit::Bool(b)) => vec![(path, Flow::Val(Sym::Bool(Form::bool(*b))))],
            ExprKind::Lit(Lit::Text(t)) => {
                let v = self.text(t);
                vec![(path, Flow::Val(v))]
            }
            ExprKind::Local(id) => {
                let mut path = path;
                let v = match path.env.get(id.0 as usize).cloned().flatten() {
                    Some(v) => v,
                    None => {
                        path.inexact |= ABSTRACT;
                        self.fresh(&mut path, &e.ty)
                    }
                };
                vec![(path, Flow::Val(v))]
            }
            ExprKind::Ctor { def, variant, args } => {
                let refs: Vec<&Expr> = args.iter().collect();
                let def = self.canon(*def);
                self.seq(path, &refs)
                    .into_iter()
                    .map(|(q, r)| match r {
                        Ok(fields) => (
                            q,
                            Flow::Val(Sym::Data {
                                def,
                                variant: *variant,
                                fields,
                            }),
                        ),
                        Err(flow) => (q, flow),
                    })
                    .collect()
            }
            ExprKind::Wrap { def, arg } => {
                let def = self.canon(*def);
                self.expr(path, arg)
                    .into_iter()
                    .map(|(q, flow)| match flow {
                        Flow::Val(v) => (
                            q,
                            Flow::Val(Sym::Data {
                                def,
                                variant: 0,
                                fields: vec![v],
                            }),
                        ),
                        other => (q, other),
                    })
                    .collect()
            }
            ExprKind::Field { base, index, .. } => {
                let mut out = Vec::new();
                for (mut q, flow) in self.expr(path, base) {
                    match flow {
                        Flow::Val(Sym::Data { fields, .. }) if (*index as usize) < fields.len() => {
                            out.push((q, Flow::Val(fields[*index as usize].clone())));
                        }
                        Flow::Val(Sym::Opaque { id, .. }) => {
                            q.inexact |= ABSTRACT;
                            let v = self.uf(&mut q, format!("field:{id}:{index}"), &e.ty);
                            out.push((q, Flow::Val(v)));
                        }
                        Flow::Val(_) => {
                            q.inexact |= ABSTRACT;
                            let v = self.fresh(&mut q, &e.ty);
                            out.push((q, Flow::Val(v)));
                        }
                        other => out.push((q, other)),
                    }
                }
                out
            }
            ExprKind::Call {
                callee,
                generic_args,
                args,
                using,
            } => {
                let refs: Vec<&Expr> = args.iter().chain(using.iter()).collect();
                let positional = args.len();
                let mut out = Vec::new();
                for (q, r) in self.seq(path, &refs) {
                    match r {
                        Ok(mut values) => {
                            let handles = values.split_off(positional);
                            out.extend(self.call(
                                q,
                                e.span,
                                *callee,
                                generic_args,
                                values,
                                handles,
                                &e.ty,
                            ))
                        }
                        Err(flow) => out.push((q, flow)),
                    }
                }
                out
            }
            ExprKind::CapCall {
                handle,
                capability,
                op,
                args,
            } => {
                let mut refs: Vec<&Expr> = vec![handle];
                refs.extend(args.iter());
                let mut out = Vec::new();
                for (q, r) in self.seq(path, &refs) {
                    match r {
                        Ok(values) => out.extend(self.cap_call(q, e, *capability, *op, values)),
                        Err(flow) => out.push((q, flow)),
                    }
                }
                out
            }
            ExprKind::Check { refined, arg } => {
                let mut out = Vec::new();
                for (mut q, flow) in self.expr(path, arg) {
                    let Flow::Val(v) = flow else {
                        out.push((q, flow));
                        continue;
                    };
                    let result = self.p.std_defs().result;
                    let Some((holds, inexact)) = self.predicate_holds(&mut q, *refined, &v) else {
                        q.inexact |= ABSTRACT;
                        let r = self.fresh(&mut q, &e.ty);
                        out.push((q, Flow::Val(r)));
                        continue;
                    };
                    q.inexact |= inexact;
                    let (yes, no) = self.split(q, &holds);
                    let ok = self.variant_index(result, "Ok");
                    let err = self.variant_index(result, "Err");
                    let result = self.canon(result);
                    if let Some(y) = yes {
                        out.push((
                            y,
                            Flow::Val(Sym::Data {
                                def: result,
                                variant: ok,
                                fields: vec![v.clone()],
                            }),
                        ));
                    }
                    if let Some(mut n) = no {
                        // The error's fields (the refinement's name, the value) are not modeled.
                        n.inexact |= ABSTRACT;
                        let ce = Ty::Adt(self.p.std_defs().constraint_error, vec![]);
                        let error = self.fresh(&mut n, &ce);
                        out.push((
                            n,
                            Flow::Val(Sym::Data {
                                def: result,
                                variant: err,
                                fields: vec![error],
                            }),
                        ));
                    }
                }
                out
            }
            ExprKind::Propagate(inner) => {
                let result = self.p.std_defs().result;
                let ok = self.variant_index(result, "Ok");
                let mut out = Vec::new();
                for (q, flow) in self.expr(path, inner) {
                    let Flow::Val(v) = flow else {
                        out.push((q, flow));
                        continue;
                    };
                    for (mut r, v) in self.resolve(q, v) {
                        match v {
                            Sym::Data {
                                variant, fields, ..
                            } if variant == ok && !fields.is_empty() => {
                                out.push((r, Flow::Val(fields[0].clone())))
                            }
                            Sym::Data {
                                def,
                                variant,
                                fields,
                            } => out.push((
                                r,
                                Flow::Ret(Sym::Data {
                                    def,
                                    variant,
                                    fields,
                                }),
                            )),
                            _ => {
                                r.inexact |= ABSTRACT;
                                let value = self.fresh(&mut r, &e.ty);
                                let ret = self.fn_ret();
                                let mut early = r.clone();
                                let error = self.fresh(&mut early, &ret);
                                out.push((early, Flow::Ret(error)));
                                out.push((r, Flow::Val(value)));
                            }
                        }
                    }
                }
                out
            }
            ExprKind::Unary { op, operand } => self
                .expr(path, operand)
                .into_iter()
                .map(|(q, flow)| match (op, flow) {
                    (UnaryOp::Neg, Flow::Val(Sym::Int(l))) => {
                        (q, Flow::Val(Sym::Int(l.scale(&BigInt::from(-1)))))
                    }
                    (UnaryOp::Not, Flow::Val(Sym::Bool(f))) => {
                        (q, Flow::Val(Sym::Bool(Form::not(f))))
                    }
                    (_, other) => (q, other),
                })
                .collect(),
            ExprKind::Binary { op, lhs, rhs } => self.binary(path, *op, lhs, rhs, e),
            ExprKind::Forall {
                binder,
                collection,
                predicate,
            } => self.forall(path, e, *binder, collection, predicate),
            ExprKind::Parallel { .. } | ExprKind::Gen(_) | ExprKind::Hole { .. } => {
                // A hole is any expression of its type: a proof covers every completion.
                let mut path = path;
                path.inexact |= ABSTRACT;
                let v = self.fresh(&mut path, &e.ty);
                vec![(path, Flow::Val(v))]
            }
            ExprKind::Error => vec![(path, Flow::Fault("internal_error".into()))],
            ExprKind::List(items) => {
                let refs: Vec<&Expr> = items.iter().collect();
                self.seq(path, &refs)
                    .into_iter()
                    .map(|(q, r)| match r {
                        Ok(values) => (q, Flow::Val(Sym::Seq(values))),
                        Err(flow) => (q, flow),
                    })
                    .collect()
            }
            ExprKind::StructLit {
                def,
                invariant_proven,
                spread,
                fields,
            } => self.struct_lit(path, e, *def, *invariant_proven, spread.as_deref(), fields),
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => {
                let mut out = Vec::new();
                for (mut q, flow) in self.expr(path, cond) {
                    let c = match flow {
                        Flow::Val(v) => self.truth_of(&mut q, &v),
                        other => {
                            out.push((q, other));
                            continue;
                        }
                    };
                    let (yes, no) = self.split(q, &c);
                    if let Some(y) = yes {
                        out.extend(self.block(y, then_block));
                    }
                    if let Some(n) = no {
                        match else_block {
                            Some(b) => out.extend(self.block(n, b)),
                            None => {
                                let unit = self.unit();
                                out.push((n, Flow::Val(unit)));
                            }
                        }
                    }
                }
                out
            }
            ExprKind::Match { scrutinee, arms } => {
                let mut out = Vec::new();
                for (q, flow) in self.expr(path, scrutinee) {
                    let Flow::Val(v) = flow else {
                        out.push((q, flow));
                        continue;
                    };
                    out.extend(self.exec_match(q, v, arms));
                }
                out
            }
            ExprKind::Widen { expr, .. } | ExprKind::Refine { expr, .. } => self.expr(path, expr),
        }
    }

    /// `forall x in xs: p`: over a list of known length, the conjunction in element order
    /// (stopping at the first false element); otherwise an abstraction.
    fn forall(
        &mut self,
        path: Path,
        e: &Expr,
        binder: LocalId,
        collection: &Expr,
        predicate: &Expr,
    ) -> Outs {
        let mut out = Vec::new();
        for (mut q, flow) in self.expr(path, collection) {
            match flow {
                Flow::Val(Sym::Seq(items)) => {
                    let mut pending = vec![(q, 0usize)];
                    while let Some((r, i)) = pending.pop() {
                        let Some(item) = items.get(i) else {
                            out.push((r, Flow::Val(Sym::Bool(Form::True))));
                            continue;
                        };
                        let mut r = r;
                        let saved = r.env.get(binder.0 as usize).cloned().flatten();
                        if let Some(slot) = r.env.get_mut(binder.0 as usize) {
                            *slot = Some(item.clone());
                        }
                        for (mut s, flow) in self.expr(r, predicate) {
                            if let Some(slot) = s.env.get_mut(binder.0 as usize) {
                                *slot = saved.clone();
                            }
                            match flow {
                                Flow::Val(v) => {
                                    let c = self.truth_of(&mut s, &v);
                                    let (yes, no) = self.split(s, &c);
                                    if let Some(n) = no {
                                        out.push((n, Flow::Val(Sym::Bool(Form::False))));
                                    }
                                    if let Some(y) = yes {
                                        pending.push((y, i + 1));
                                    }
                                }
                                other => out.push((s, other)),
                            }
                        }
                    }
                }
                Flow::Val(_) => {
                    q.inexact |= ABSTRACT;
                    let v = self.fresh(&mut q, &e.ty);
                    out.push((q, Flow::Val(v)));
                }
                other => out.push((q, other)),
            }
        }
        out
    }

    /// The return type of the function being executed.
    fn fn_ret(&self) -> Ty {
        let def = self.stack.last().copied().unwrap_or(self.root);
        self.p
            .def(def)
            .kind
            .as_fn()
            .map(|f| f.ret.clone())
            .unwrap_or(Ty::Error)
    }

    fn struct_lit(
        &mut self,
        path: Path,
        e: &Expr,
        def: DefId,
        invariant_proven: bool,
        spread: Option<&Expr>,
        fields: &[(u32, Expr)],
    ) -> Outs {
        let p = self.p;
        let Some(s) = p.def(def).kind.as_struct() else {
            return vec![(path, Flow::Fault("internal_error".into()))];
        };
        let count = s.fields.len();
        let mut refs: Vec<&Expr> = Vec::new();
        if let Some(s) = spread {
            refs.push(s);
        }
        refs.extend(fields.iter().map(|(_, f)| f));
        let canonical = self.canon(def);
        let mut out = Vec::new();
        for (mut q, r) in self.seq(path, &refs) {
            let values = match r {
                Ok(v) => v,
                Err(flow) => {
                    out.push((q, flow));
                    continue;
                }
            };
            let mut values = values.into_iter();
            let mut slots: Vec<Option<Sym>> = vec![None; count];
            if spread.is_some() {
                match values.next() {
                    Some(Sym::Data { fields: f, .. }) => {
                        for (i, v) in f.into_iter().enumerate().take(count) {
                            slots[i] = Some(v);
                        }
                    }
                    Some(Sym::Opaque { id, .. }) => {
                        q.inexact |= ABSTRACT;
                        for (i, slot) in slots.iter_mut().enumerate() {
                            let ty = instantiate(&s.fields[i].ty, struct_args(&e.ty));
                            *slot = Some(self.uf(&mut q, format!("field:{id}:{i}"), &ty));
                        }
                    }
                    _ => q.inexact |= ABSTRACT,
                }
            }
            for ((index, _), v) in fields.iter().zip(values) {
                if let Some(slot) = slots.get_mut(*index as usize) {
                    *slot = Some(v);
                }
            }
            let mut filled = Vec::with_capacity(count);
            for (i, slot) in slots.into_iter().enumerate() {
                filled.push(match slot {
                    Some(v) => v,
                    None => {
                        q.inexact |= ABSTRACT;
                        let ty = instantiate(&s.fields[i].ty, struct_args(&e.ty));
                        self.fresh(&mut q, &ty)
                    }
                });
            }
            let value = Sym::Data {
                def: canonical,
                variant: 0,
                fields: filled,
            };
            if s.invariant.is_none() || invariant_proven {
                out.push((q, Flow::Val(value)));
                continue;
            }
            match self.predicate_holds(&mut q, def, &value) {
                Some((holds, inexact)) => {
                    q.inexact |= inexact;
                    self.check(&q, Site::Constructor(e.span), &holds);
                    let (yes, no) = self.split(q, &holds);
                    if let Some(y) = yes {
                        out.push((y, Flow::Val(value)));
                    }
                    if let Some(n) = no {
                        out.push((n, Flow::Fault("contract_violation:invariant".into())));
                    }
                }
                None => {
                    self.check_unknown(Site::Constructor(e.span));
                    q.inexact |= ABSTRACT;
                    out.push((q, Flow::Val(value)));
                }
            }
        }
        out
    }

    fn binary(&mut self, path: Path, op: BinaryOp, lhs: &Expr, rhs: &Expr, e: &Expr) -> Outs {
        if matches!(op, BinaryOp::And | BinaryOp::Or) {
            // Short-circuit operators evaluate the right operand only on the paths that need it.
            let mut out = Vec::new();
            for (mut q, flow) in self.expr(path, lhs) {
                let a = match flow {
                    Flow::Val(v) => self.truth_of(&mut q, &v),
                    other => {
                        out.push((q, other));
                        continue;
                    }
                };
                // A right operand that evaluates to one value without a fault, a fork, or a new
                // fact needs no fork: `a and b` is the formula.
                let mut probe = q.clone();
                probe.inexact = 0;
                // Unguarded, so it records nothing: an operand with obligations is not plain.
                let rhs_outs = self.quiet(|x| x.expr(probe, rhs));
                if let [(r, Flow::Val(b))] = rhs_outs.as_slice() {
                    if r.cond.len() == q.cond.len()
                        && r.variants == q.variants
                        && r.trace.len() == q.trace.len()
                    {
                        let mut r = r.clone();
                        let b = self.truth_of(&mut r, b);
                        let mut q = q;
                        q.inexact |= r.inexact;
                        let f = if op == BinaryOp::And {
                            Form::and(vec![a, b])
                        } else {
                            Form::or(vec![a, b])
                        };
                        out.push((q, Flow::Val(Sym::Bool(f))));
                        continue;
                    }
                }
                let (yes, no) = self.split(q, &a);
                let (decided, rest, value) = if op == BinaryOp::And {
                    (no, yes, false)
                } else {
                    (yes, no, true)
                };
                if let Some(d) = decided {
                    out.push((d, Flow::Val(Sym::Bool(Form::bool(value)))));
                }
                if let Some(r) = rest {
                    out.extend(self.expr(r, rhs));
                }
            }
            return out;
        }
        let mut out = Vec::new();
        for (mut q, r) in self.seq(path, &[lhs, rhs]) {
            let values = match r {
                Ok(v) => v,
                Err(flow) => {
                    out.push((q, flow));
                    continue;
                }
            };
            let (a, b) = (&values[0], &values[1]);
            let value = match (op, int(a), int(b)) {
                (BinaryOp::Add, Some(x), Some(y)) => Sym::Int(x.add(y)),
                (BinaryOp::Sub, Some(x), Some(y)) => Sym::Int(x.sub(y)),
                (BinaryOp::Mul, Some(x), Some(y)) => {
                    let (x, y) = (x.clone(), y.clone());
                    Sym::Int(self.mul(&mut q, &x, &y))
                }
                (BinaryOp::Lt, Some(x), Some(y)) => Sym::Bool(Form::lt(x, y)),
                (BinaryOp::Le, Some(x), Some(y)) => Sym::Bool(Form::le(x, y)),
                (BinaryOp::Gt, Some(x), Some(y)) => Sym::Bool(Form::lt(y, x)),
                (BinaryOp::Ge, Some(x), Some(y)) => Sym::Bool(Form::le(y, x)),
                (BinaryOp::Div | BinaryOp::Rem, Some(x), Some(y)) => {
                    let (x, y) = (x.clone(), y.clone());
                    let nonzero = Form::not(Form::eq(&y, &Lin::default()));
                    self.check(&q, Site::Division(e.span), &nonzero);
                    let (yes, zero) = self.split(q, &nonzero);
                    if let Some(z) = zero {
                        out.push((z, Flow::Fault("division_by_zero".into())));
                    }
                    if let Some(mut n) = yes {
                        let (quotient, remainder) = self.divide(&mut n, &x, &y);
                        let v = if op == BinaryOp::Div {
                            quotient
                        } else {
                            remainder
                        };
                        out.push((n, Flow::Val(Sym::Int(v))));
                    }
                    continue;
                }
                (BinaryOp::Eq, _, _) => {
                    let (a, b) = (a.clone(), b.clone());
                    Sym::Bool(self.eq(&mut q, &a, &b))
                }
                (BinaryOp::Ne, _, _) => {
                    let (a, b) = (a.clone(), b.clone());
                    Sym::Bool(Form::not(self.eq(&mut q, &a, &b)))
                }
                (BinaryOp::Concat, _, _) => match (a, b) {
                    (Sym::Seq(x), Sym::Seq(y)) => {
                        let mut all = x.clone();
                        all.extend(y.iter().cloned());
                        Sym::Seq(all)
                    }
                    (Sym::Seq(x), other) | (other, Sym::Seq(x)) if x.is_empty() => other.clone(),
                    _ => {
                        q.inexact |= ABSTRACT;
                        let key = format!("concat({},{})", self.key(a), self.key(b));
                        self.uf(&mut q, key, &e.ty)
                    }
                },
                _ => {
                    q.inexact |= ABSTRACT;
                    let key = format!("{op:?}({},{})", self.key(a), self.key(b));
                    self.uf(&mut q, key, &e.ty)
                }
            };
            out.push((q, Flow::Val(value)));
        }
        out
    }

    fn mul(&mut self, path: &mut Path, x: &Lin, y: &Lin) -> Lin {
        if let Some(k) = x.as_constant() {
            return y.scale(k);
        }
        if let Some(k) = y.as_constant() {
            return x.scale(k);
        }
        let (a, b) = if x <= y { (x, y) } else { (y, x) };
        let v = match self
            .shared
            .products
            .iter()
            .find(|(_, p, q)| p == a && q == b)
        {
            Some((v, _, _)) => *v,
            None => {
                let v = self.shared.var();
                self.shared.products.push((v, a.clone(), b.clone()));
                v
            }
        };
        let p = Lin::var(v);
        // Sign and magnitude facts of the product, which linear reasoning cannot derive: one case
        // per sign of the factors.
        let zero = Lin::default();
        let one = Lin::constant(1);
        let neg = |l: &Lin| l.scale(&BigInt::from(-1));
        let pos = |l: &Lin| Form::le(&one, l);
        let negative = |l: &Lin| Form::le(l, &neg(&one));
        let cases = if a == b {
            vec![
                Form::and(vec![Form::eq(a, &zero), Form::eq(&p, &zero)]),
                Form::and(vec![pos(a), Form::le(a, &p)]),
                Form::and(vec![negative(a), Form::le(&neg(a), &p)]),
            ]
        } else {
            vec![
                Form::and(vec![Form::eq(a, &zero), Form::eq(&p, &zero)]),
                Form::and(vec![Form::eq(b, &zero), Form::eq(&p, &zero)]),
                Form::and(vec![pos(a), pos(b), Form::le(a, &p), Form::le(b, &p)]),
                Form::and(vec![
                    pos(a),
                    negative(b),
                    Form::le(&p, b),
                    Form::le(&p, &neg(a)),
                ]),
                Form::and(vec![
                    negative(a),
                    pos(b),
                    Form::le(&p, a),
                    Form::le(&p, &neg(b)),
                ]),
                Form::and(vec![
                    negative(a),
                    negative(b),
                    Form::le(&neg(a), &p),
                    Form::le(&neg(b), &p),
                ]),
            ]
        };
        path.add(Form::or(cases));
        p
    }

    /// Truncating division `x / y` and remainder, `y` nonzero on this path.
    fn divide(&mut self, path: &mut Path, x: &Lin, y: &Lin) -> (Lin, Lin) {
        // Truncating division of constants (the divisor is not 0 here).
        if let (Some(a), Some(b)) = (x.as_constant(), y.as_constant()) {
            if !b.is_zero() {
                return (Lin::constant(a / b), Lin::constant(a % b));
            }
        }
        let key = (x.clone(), y.clone());
        let (q, r) = match self.shared.divisions.get(&key) {
            Some(qr) => *qr,
            None => {
                let qr = (self.shared.var(), self.shared.var());
                self.shared.divisions.insert(key, qr);
                if y.as_constant().is_none() {
                    self.shared
                        .quotients
                        .push((qr.0, qr.1, x.clone(), y.clone()));
                }
                qr
            }
        };
        let (ql, rl) = (Lin::var(q), Lin::var(r));
        let zero = Lin::default();
        // The remainder has the dividend's sign and is smaller than the divisor in magnitude.
        path.add(Form::or(vec![
            Form::and(vec![Form::le(&zero, x), Form::le(&zero, &rl)]),
            Form::and(vec![Form::lt(x, &zero), Form::le(&rl, &zero)]),
        ]));
        match y.as_constant() {
            Some(c) => {
                let bound = Lin::constant(c.abs() - BigInt::one());
                path.add(Form::eq(x, &ql.scale(c).add(&rl)));
                path.add(Form::le(&rl, &bound));
                path.cond
                    .push(Form::le(&bound.scale(&BigInt::from(-1)), &rl));
            }
            None => {
                let product = self.mul(path, y, &ql);
                path.add(Form::eq(x, &product.add(&rl)));
                let minus_y = y.scale(&BigInt::from(-1));
                path.add(Form::or(vec![
                    Form::and(vec![
                        Form::lt(&zero, y),
                        Form::lt(&rl, y),
                        Form::lt(&minus_y, &rl),
                    ]),
                    Form::and(vec![
                        Form::lt(y, &zero),
                        Form::lt(y, &rl),
                        Form::lt(&rl, &minus_y),
                    ]),
                ]));
            }
        }
        (ql, rl)
    }

    /// An uninterpreted application: equal keys give equal values on every path and side.
    pub fn uf(&mut self, path: &mut Path, key: String, ty: &Ty) -> Sym {
        if let Some((v, assumptions)) = self.shared.uf.get(&key).cloned() {
            for a in assumptions {
                path.add(a);
            }
            return v;
        }
        let mut tmp = path.clone();
        tmp.cond.clear();
        let v = self.fresh(&mut tmp, ty);
        path.inexact |= tmp.inexact;
        for a in &tmp.cond {
            path.add(a.clone());
        }
        self.shared.uf.insert(key, (v.clone(), tmp.cond));
        v
    }

    /// Run `f` in a frame of `def` with locals `env` and generic arguments `generics` (in the
    /// caller's terms), restoring the caller's locals afterwards.
    fn in_frame<T>(
        &mut self,
        def: DefId,
        mut path: Path,
        env: Vec<Option<Sym>>,
        generics: &[Ty],
        f: impl FnOnce(&mut Self, Path) -> Vec<(Path, T)>,
    ) -> Vec<(Path, T)> {
        let saved = std::mem::replace(&mut path.env, env);
        let outer = self.subst.last().cloned().unwrap_or_default();
        let generics: Vec<Ty> = generics.iter().map(|t| instantiate(t, &outer)).collect();
        self.stack.push(def);
        self.subst.push(generics);
        let out = f(self, path);
        self.subst.pop();
        self.stack.pop();
        out.into_iter()
            .map(|(mut q, t)| {
                q.env = saved.clone();
                (q, t)
            })
            .collect()
    }

    fn cap_call(
        &mut self,
        path: Path,
        e: &Expr,
        capability: DefId,
        op: u32,
        values: Vec<Sym>,
    ) -> Outs {
        let p = self.p;
        let Some(opdef) = p
            .def(capability)
            .kind
            .as_capability()
            .and_then(|c| c.ops.get(op as usize))
        else {
            let mut path = path;
            path.inexact |= ABSTRACT;
            let v = self.fresh(&mut path, &e.ty);
            return vec![(path, Flow::Val(v))];
        };
        let args = &values[1..];
        let frame = |result: Option<&Sym>| {
            let mut env = vec![None; opdef.locals.len()];
            for (param, v) in opdef.params.iter().zip(args) {
                if let Some(slot) = env.get_mut(param.local.0 as usize) {
                    *slot = Some(v.clone());
                }
            }
            if let (Some(r), Some(v)) = (opdef.result_local, result) {
                if let Some(slot) = env.get_mut(r.0 as usize) {
                    *slot = Some(v.clone());
                }
            }
            env
        };
        // An operation's preconditions are the caller's obligations.
        let identity = format!("{}.{}", p.def(capability).stable_id.as_str(), opdef.name);
        let violation = format!("contract_violation:requires:{identity}");
        let mut paths = vec![path];
        let mut faults = Vec::new();
        for (i, clause) in opdef.requires.iter().enumerate() {
            let mut next = Vec::new();
            for q in paths {
                let forms = self.quiet(|x| {
                    x.in_frame(capability, q, frame(None), &[], |x, q| {
                        x.clause_here(q, clause)
                    })
                });
                for (r, f) in forms {
                    if let Some(r) = self.checked(
                        r,
                        Some(Site::CallRequires(e.span, i)),
                        f,
                        &violation,
                        &mut faults,
                    ) {
                        next.push(r);
                    }
                }
            }
            paths = next;
        }
        let mut out = faults;
        for mut q in paths {
            q.inexact |= ABSTRACT;
            // The environment answers each operation as a function of everything performed so
            // far: two runs that perform the same operations receive the same answers.
            let mut key = String::from("env");
            for (name, args) in &q.trace {
                key.push_str(&format!(";{name}("));
                for a in args {
                    key.push_str(&self.key(a));
                    key.push(',');
                }
                key.push(')');
            }
            key.push_str(&format!(";{identity}("));
            for a in &values {
                key.push_str(&self.key(a));
                key.push(',');
            }
            key.push(')');
            q.trace.push((identity.clone(), values.clone()));
            let ret = opdef.ret.clone();
            let v = self.uf(&mut q, key, &ret);
            // The operation's postconditions are its implementation's promise.
            let mut ends = vec![q];
            for clause in &opdef.ensures {
                let mut next = Vec::new();
                for x in ends {
                    let env = frame(Some(&v));
                    let forms = self.quiet(|s| {
                        s.in_frame(capability, x, env, &[], |s, x| s.clause_here(x, clause))
                    });
                    for (y, form) in forms {
                        if let Some(y) = self.assume(y, form) {
                            next.push(y);
                        }
                    }
                }
                ends = next;
            }
            out.extend(ends.into_iter().map(|x| (x, Flow::Val(v.clone()))));
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn call(
        &mut self,
        path: Path,
        span: Span,
        callee: DefId,
        generic_args: &[Ty],
        args: Vec<Sym>,
        handles: Vec<Sym>,
        ty: &Ty,
    ) -> Outs {
        let p = self.p;
        let Some(f) = p.def(callee).kind.as_fn() else {
            let mut path = path;
            path.inexact |= ABSTRACT;
            let v = self.fresh(&mut path, ty);
            return vec![(path, Flow::Val(v))];
        };
        let frame = |result: Option<&Sym>| -> Vec<Option<Sym>> {
            let mut env = vec![None; f.locals.len()];
            for (param, v) in f
                .params
                .iter()
                .zip(&args)
                .chain(f.using.iter().zip(&handles))
            {
                if let Some(slot) = env.get_mut(param.local.0 as usize) {
                    *slot = Some(v.clone());
                }
            }
            if let (Some(r), Some(v)) = (f.result_local, result) {
                if let Some(slot) = env.get_mut(r.0 as usize) {
                    *slot = Some(v.clone());
                }
            }
            env
        };
        let axioms = self.axiom_depth < MAX_AXIOM_DEPTH;
        let stable = &p.def(callee).stable_id;
        let mut out = Vec::new();
        // Preconditions: the caller's obligations, then assumptions.
        let mut paths = vec![path];
        if axioms || self.recording || self.faults() {
            let fault = format!("contract_violation:requires:{}", stable.as_str());
            for (i, clause) in f.requires.iter().enumerate() {
                let mut next = Vec::new();
                for q in paths {
                    self.axiom_depth += 1;
                    let forms = self.quiet(|x| {
                        x.in_frame(callee, q, frame(None), generic_args, |x, q| {
                            x.clause_here(q, clause)
                        })
                    });
                    self.axiom_depth -= 1;
                    for (r, form) in forms {
                        if let Some(r) = self.checked(
                            r,
                            Some(Site::CallRequires(span, i)),
                            form,
                            &fault,
                            &mut out,
                        ) {
                            next.push(r);
                        }
                    }
                }
                paths = next;
            }
        }
        // A recursive call of the root must decrease its measure (semantics S35).
        if callee == self.root && self.recording {
            if let Some(dc) = f.decreases.as_ref() {
                let mut next = Vec::new();
                for q in paths {
                    let Some(m0) = q.measure.clone() else {
                        self.check_unknown(Site::Decreases);
                        next.push(q);
                        continue;
                    };
                    let measures = self.quiet(|x| {
                        x.in_frame(callee, q, frame(None), generic_args, |x, q| {
                            x.expr(q, &dc.expr)
                        })
                    });
                    for (r, m) in measures {
                        match &m {
                            Flow::Val(Sym::Int(m1)) => {
                                self.check(&r, Site::Decreases, &Form::lt(m1, &m0))
                            }
                            Flow::Val(_) => self.check_unknown(Site::Decreases),
                            _ => continue,
                        }
                        next.push(r);
                    }
                }
                paths = next;
            }
        }
        let builtin = match &f.body {
            FnBody::Builtin(id) => Some(id.as_str()),
            FnBody::Block(_) => None,
        };
        let (shared_key, side, equivalence, inline_shared) = match self.mode {
            Mode::Equivalence {
                same, side, inline, ..
            } => (callee == self.root || same(stable), side, true, inline),
            Mode::Verify => (false, "", false, false),
        };
        let effectful = !handles.is_empty();
        // The root is on the stack, so its recursive calls are never executed in place, except on
        // concrete inputs, where every call is (up to a depth).
        let inline = builtin.is_none()
            && !(equivalence && shared_key && !inline_shared)
            && !effectful
            && if self.concrete {
                self.stack.len() < MAX_CONCRETE_DEPTH
            } else {
                !self.stack.contains(&callee) && self.stack.len() <= MAX_INLINE
            };
        let instantiated = instantiate(&f.ret, generic_args);
        for q in paths {
            let results: Outs = if let Some(id) = builtin {
                self.builtin(q, id, &args, &instantiated)
            } else if inline {
                let FnBody::Block(body) = &f.body else {
                    unreachable!()
                };
                self.quiet(|x| {
                    x.in_frame(callee, q, frame(None), generic_args, |x, q| {
                        x.block(q, body)
                    })
                })
                .into_iter()
                .map(|(q, flow)| match flow {
                    Flow::Ret(v) => (q, Flow::Val(v)),
                    other => (q, other),
                })
                .collect()
            } else {
                let mut q = q;
                q.inexact |= ABSTRACT;
                if equivalence && callee == self.root {
                    self.summarized = true;
                }
                let mut key = if shared_key {
                    format!("call:{}(", stable.as_str())
                } else {
                    format!("call:{side}:{}(", stable.as_str())
                };
                for a in args.iter().chain(&handles) {
                    key.push_str(&self.key(a));
                    key.push(',');
                }
                key.push(')');
                if effectful {
                    // An effectful call answers as a function of the effects so far.
                    for (name, args) in &q.trace {
                        key.push_str(&format!(";{name}("));
                        for a in args {
                            key.push_str(&self.key(a));
                            key.push(',');
                        }
                        key.push(')');
                    }
                    let mut performed = args.clone();
                    performed.extend(handles.iter().cloned());
                    q.trace
                        .push((format!("call:{}", stable.as_str()), performed));
                }
                if self.faults() {
                    // The callee may fault inside, the same way for the same arguments.
                    let fault = self.uf(&mut q, format!("fault:{key}"), &Ty::Bool);
                    let (faults, returns) = self.split(q, &boolean(&fault));
                    if let Some(f) = faults {
                        out.push((f, Flow::Fault(format!("fault:{key}"))));
                    }
                    match returns {
                        Some(r) => {
                            q = r;
                        }
                        None => continue,
                    }
                }
                let v = self.uf(&mut q, key, &instantiated);
                vec![(q, Flow::Val(v))]
            };
            // A callee's postconditions hold whenever it returns (checked mode; for a builtin they
            // are its axioms, semantics S61): assume them.
            for (q, flow) in results {
                let Flow::Val(v) = flow else {
                    out.push((q, flow));
                    continue;
                };
                if !axioms {
                    out.push((q, Flow::Val(v)));
                    continue;
                }
                let mut ends = vec![q];
                let fault = format!("contract_violation:ensures:{}", stable.as_str());
                for clause in &f.ensures {
                    let mut next = Vec::new();
                    for x in ends {
                        self.axiom_depth += 1;
                        let env = frame(Some(&v));
                        let forms = self.quiet(|s| {
                            s.in_frame(callee, x, env, generic_args, |s, x| {
                                s.clause_here(x, clause)
                            })
                        });
                        self.axiom_depth -= 1;
                        for (y, form) in forms {
                            // An inlined body's postconditions are checked like any call's; the
                            // others are what the callee promises.
                            let y = if inline && builtin.is_none() {
                                self.checked(y, None, form, &fault, &mut out)
                            } else {
                                self.assume(y, form)
                            };
                            if let Some(y) = y {
                                next.push(y);
                            }
                        }
                    }
                    ends = next;
                }
                out.extend(ends.into_iter().map(|x| (x, Flow::Val(v.clone()))));
            }
        }
        out
    }

    /// A builtin on a value whose shape is known, exactly: on a list of known length, `first`,
    /// `last`, `get` by a constant index, `take` and `drop` by a constant count, and
    /// `drop_last` (stdlib 4.1); on an `Option` or `Result` of known variant, its tests and
    /// accessors. `None` for anything else, which is abstracted.
    fn exact_builtin(&mut self, id: &str, args: &[Sym], ty: &Ty) -> Option<Sym> {
        if let Some(Sym::Data {
            def,
            variant,
            fields,
        }) = args.first()
        {
            let name = self
                .p
                .def(*def)
                .kind
                .as_enum()?
                .variants
                .get(*variant as usize)?
                .name
                .clone();
            let payload = fields.first().cloned();
            let is = |n: &str| Some(Sym::Bool(Form::bool(name == n)));
            return match id {
                "option_is_some" => is("Some"),
                "option_is_none" => is("None"),
                "result_is_ok" => is("Ok"),
                "result_is_err" => is("Err"),
                "option_unwrap_or" if name == "Some" => payload,
                "option_unwrap_or" if name == "None" => args.get(1).cloned(),
                "result_unwrap_or" if name == "Ok" => payload,
                "result_unwrap_or" if name == "Err" => args.get(1).cloned(),
                "result_ok" if name == "Ok" || name == "Err" => {
                    self.option(ty, payload.filter(|_| name == "Ok"))
                }
                "result_err" if name == "Ok" || name == "Err" => {
                    self.option(ty, payload.filter(|_| name == "Err"))
                }
                _ => None,
            };
        }
        let Some(Sym::Seq(items)) = args.first() else {
            return None;
        };
        let constant = match args.get(1) {
            Some(Sym::Int(n)) if n.terms.is_empty() => Some(n.constant.clone()),
            _ => None,
        };
        // a count `n >= 0` as an index into the list: `min(n, len)`
        let count = |n: &BigInt| n.to_usize().unwrap_or(usize::MAX).min(items.len());
        match id {
            "list_first" => self.option(ty, items.first().cloned()),
            "list_last" => self.option(ty, items.last().cloned()),
            "list_get" => {
                let i = constant?;
                let item = i.to_usize().and_then(|i| items.get(i)).cloned();
                self.option(ty, item)
            }
            "list_take" => {
                let n = constant.filter(|n| !n.is_negative())?;
                Some(Sym::Seq(items[..count(&n)].to_vec()))
            }
            "list_drop" => {
                let n = constant.filter(|n| !n.is_negative())?;
                Some(Sym::Seq(items[count(&n)..].to_vec()))
            }
            "list_drop_last" => Some(Sym::Seq(items[..items.len().saturating_sub(1)].to_vec())),
            _ => None,
        }
    }

    /// `Some(item)`, or `None` without one, of the `Option` type `ty`.
    fn option(&mut self, ty: &Ty, item: Option<Sym>) -> Option<Sym> {
        let (def, _) = self.enum_of(ty)?;
        let name = if item.is_some() { "Some" } else { "None" };
        let variant = self
            .p
            .def(def)
            .kind
            .as_enum()?
            .variants
            .iter()
            .position(|v| v.name == name)? as u32;
        Some(Sym::Data {
            def: self.canon(def),
            variant,
            fields: item.into_iter().collect(),
        })
    }

    fn builtin(&mut self, mut path: Path, id: &str, args: &[Sym], ty: &Ty) -> Outs {
        if let Some(v) = self.exact_builtin(id, args, ty) {
            return vec![(path, Flow::Val(v))];
        }
        let ints: Vec<Option<&Lin>> = args.iter().map(int).collect();
        match (id, ints.as_slice()) {
            // A value defined by cases is a fresh variable with a disjunctive definition, not a
            // fork: the solver splits the cases only where an obligation depends on them.
            ("int_abs", [Some(x)]) => {
                let x = (*x).clone();
                if x.terms.is_empty() {
                    let c = Lin::constant(x.constant.abs());
                    return vec![(path, Flow::Val(Sym::Int(c)))];
                }
                let v = Lin::var(self.case_var("int_abs", &x, &Lin::default()));
                let zero = Lin::default();
                path.add(Form::or(vec![
                    Form::and(vec![Form::le(&zero, &x), Form::eq(&v, &x)]),
                    Form::and(vec![
                        Form::lt(&x, &zero),
                        Form::eq(&v, &x.scale(&BigInt::from(-1))),
                    ]),
                ]));
                vec![(path, Flow::Val(Sym::Int(v)))]
            }
            ("int_min" | "int_max", [Some(x), Some(y)]) => {
                let (x, y) = ((*x).clone(), (*y).clone());
                let (lo, hi) = if id == "int_min" { (&x, &y) } else { (&y, &x) };
                if x.terms.is_empty() && y.terms.is_empty() {
                    let v = if x.constant <= y.constant { lo } else { hi };
                    return vec![(path, Flow::Val(Sym::Int(v.clone())))];
                }
                if x == y {
                    return vec![(path, Flow::Val(Sym::Int(x)))];
                }
                let name = if id == "int_min" {
                    "int_min"
                } else {
                    "int_max"
                };
                let v = Lin::var(self.case_var(name, &x, &y));
                path.add(Form::or(vec![
                    Form::and(vec![Form::le(&x, &y), Form::eq(&v, lo)]),
                    Form::and(vec![Form::lt(&y, &x), Form::eq(&v, hi)]),
                ]));
                vec![(path, Flow::Val(Sym::Int(v)))]
            }
            ("list_len", _) if matches!(args.first(), Some(Sym::Seq(_))) => {
                let Some(Sym::Seq(items)) = args.first() else {
                    unreachable!()
                };
                vec![(path, Flow::Val(Sym::Int(Lin::constant(items.len()))))]
            }
            ("list_is_empty", _) if matches!(args.first(), Some(Sym::Seq(_))) => {
                let Some(Sym::Seq(items)) = args.first() else {
                    unreachable!()
                };
                vec![(path, Flow::Val(Sym::Bool(Form::bool(items.is_empty()))))]
            }
            ("list_push", [_, _]) if matches!(args.first(), Some(Sym::Seq(_))) => {
                let Some(Sym::Seq(items)) = args.first() else {
                    unreachable!()
                };
                let mut items = items.clone();
                items.push(args[1].clone());
                vec![(path, Flow::Val(Sym::Seq(items)))]
            }
            ("text_len", _) => {
                if let Some(Sym::Opaque { id: o, .. }) = args.first() {
                    if let Some(t) = self.shared.literal_texts.get(o) {
                        let n = t.chars().count();
                        return vec![(path, Flow::Val(Sym::Int(Lin::constant(n))))];
                    }
                }
                path.inexact |= ABSTRACT;
                let key = format!(
                    "builtin:{id}({})",
                    args.first().map(|a| self.key(a)).unwrap_or_default()
                );
                let v = self.uf(&mut path, key, ty);
                vec![(path, Flow::Val(v))]
            }
            _ => {
                path.inexact |= ABSTRACT;
                let mut key = format!("builtin:{id}(");
                for a in args {
                    key.push_str(&self.key(a));
                    key.push(',');
                }
                key.push(')');
                let v = self.uf(&mut path, key, ty);
                vec![(path, Flow::Val(v))]
            }
        }
    }

    /// Resolve an opaque enum value into each of its variants, recording the choice on the path.
    fn resolve(&mut self, path: Path, v: Sym) -> Vec<(Path, Sym)> {
        let Sym::Opaque { id, ty } = &v else {
            return vec![(path, v)];
        };
        let Some((def, args)) = self.enum_of(ty) else {
            return vec![(path, v)];
        };
        let canonical = self.canon(def);
        if let Some(variant) = path.variants.get(id).copied() {
            let mut path = path;
            let fields = self.payload(&mut path, *id, def, &args, variant);
            return vec![(
                path,
                Sym::Data {
                    def: canonical,
                    variant,
                    fields,
                },
            )];
        }
        let count = self
            .p
            .def(def)
            .kind
            .as_enum()
            .map_or(0, |e| e.variants.len());
        if !self.budget(count) {
            return Vec::new();
        }
        let mut path = path;
        let tag = self.tag(&mut path, *id, count);
        let mut out = Vec::new();
        for k in 0..count as u32 {
            let mut q = path.clone();
            q.add(Form::eq(&Lin::var(tag), &Lin::constant(k)));
            q.variants.insert(*id, k);
            let fields = self.payload(&mut q, *id, def, &args, k);
            out.push((
                q,
                Sym::Data {
                    def: canonical,
                    variant: k,
                    fields,
                },
            ));
        }
        out
    }

    fn exec_match(&mut self, path: Path, value: Sym, arms: &[Arm]) -> Outs {
        let pats: Vec<&Pattern> = arms.iter().map(|a| &a.pattern).collect();
        let mut out = Vec::new();
        for (q, v) in self.concretize(path, value, &pats, 0) {
            let mut earlier: Vec<Form> = Vec::new();
            for arm in arms {
                let mut r = q.clone();
                let mut binds = Vec::new();
                let m = self.pattern(&mut r, &v, &arm.pattern, &mut binds);
                let mut guard = vec![m.clone()];
                guard.extend(earlier.iter().map(|f| Form::not(f.clone())));
                earlier.push(m);
                let Some(mut r) = self.assume(r, Form::and(guard)) else {
                    continue;
                };
                if !self.budget(1) {
                    return Vec::new();
                }
                for (local, value) in binds {
                    if let Some(slot) = r.env.get_mut(local.0 as usize) {
                        *slot = Some(value);
                    }
                }
                out.extend(self.block(r, &arm.body));
            }
            // A value no arm is known to match (the checker makes matches exhaustive, so only
            // an abstraction leaves one): its behavior is unknown, never dropped.
            if self.faults() {
                let rest = Form::and(earlier.iter().map(|f| Form::not(f.clone())).collect());
                if let Some(mut r) = self.assume(q, rest) {
                    r.inexact |= ABSTRACT;
                    out.push((r, Flow::Fault("unmatched".into())));
                }
            }
        }
        out
    }

    /// Resolve the opaque enum values that constructor patterns inspect, nested payloads included.
    fn concretize(
        &mut self,
        path: Path,
        value: Sym,
        pats: &[&Pattern],
        depth: u32,
    ) -> Vec<(Path, Sym)> {
        let inspects = pats
            .iter()
            .any(|p| matches!(p.kind, PatternKind::Ctor { .. }));
        if !inspects || depth > 3 {
            return vec![(path, value)];
        }
        let mut out = Vec::new();
        for (q, v) in self.resolve(path, value) {
            let Sym::Data {
                def,
                variant,
                fields,
            } = v
            else {
                out.push((q, v));
                continue;
            };
            // Resolve each payload field against the sub-patterns of the arms of this variant.
            let mut states = vec![(q, Vec::new())];
            for (i, field) in fields.iter().enumerate() {
                let subs: Vec<&Pattern> = pats
                    .iter()
                    .filter_map(|p| match &p.kind {
                        PatternKind::Ctor {
                            variant: w, args, ..
                        } if *w == variant => args.get(i),
                        _ => None,
                    })
                    .collect();
                let mut next = Vec::new();
                for (s, done) in states {
                    for (t, f) in self.concretize(s, field.clone(), &subs, depth + 1) {
                        let mut d: Vec<Sym> = done.clone();
                        d.push(f);
                        next.push((t, d));
                    }
                }
                states = next;
            }
            for (s, fields) in states {
                out.push((
                    s,
                    Sym::Data {
                        def,
                        variant,
                        fields,
                    },
                ));
            }
        }
        out
    }

    fn pattern(
        &mut self,
        path: &mut Path,
        v: &Sym,
        pat: &Pattern,
        binds: &mut Vec<(LocalId, Sym)>,
    ) -> Form {
        match (&pat.kind, v) {
            (PatternKind::Wildcard, _) => Form::True,
            (PatternKind::Bind(local), _) => {
                binds.push((*local, v.clone()));
                Form::True
            }
            (PatternKind::Int(n), Sym::Int(l)) => Form::eq(l, &Lin::constant(n.clone())),
            (PatternKind::Bool(b), Sym::Bool(f)) => {
                if *b {
                    f.clone()
                } else {
                    Form::not(f.clone())
                }
            }
            (PatternKind::Text(t), _) => {
                let lit = self.text(t);
                self.eq(path, v, &lit)
            }
            (
                PatternKind::Ctor { def, variant, args },
                Sym::Data {
                    def: d,
                    variant: w,
                    fields,
                },
            ) => {
                if self.canon(*def) != *d || variant != w {
                    return Form::False;
                }
                let mut parts = Vec::new();
                for (sub, field) in args.iter().zip(fields) {
                    parts.push(self.pattern(path, field, sub, binds));
                }
                Form::and(parts)
            }
            (PatternKind::Ctor { variant, args, .. }, Sym::Opaque { id, ty }) => {
                // Deeper than `concretize` resolves: state the variant by the tag.
                match self.enum_of(ty) {
                    Some((def, targs)) => {
                        let count = self
                            .p
                            .def(def)
                            .kind
                            .as_enum()
                            .map_or(0, |e| e.variants.len());
                        let t = self.tag(path, *id, count);
                        let payload = self.payload(path, *id, def, &targs, *variant);
                        let mut parts = vec![Form::eq(&Lin::var(t), &Lin::constant(*variant))];
                        for (sub, field) in args.iter().zip(&payload) {
                            parts.push(self.pattern(path, field, sub, binds));
                        }
                        Form::and(parts)
                    }
                    None => self.unknown_pattern(path, v, pat, binds),
                }
            }
            (PatternKind::Unwrap { arg, .. }, Sym::Data { fields, .. }) if !fields.is_empty() => {
                let inner = fields[0].clone();
                self.pattern(path, &inner, arg, binds)
            }
            _ => self.unknown_pattern(path, v, pat, binds),
        }
    }

    fn unknown_pattern(
        &mut self,
        path: &mut Path,
        v: &Sym,
        pat: &Pattern,
        binds: &mut Vec<(LocalId, Sym)>,
    ) -> Form {
        path.inexact |= ABSTRACT;
        collect_binds(self, path, v, pat, binds);
        truth(self.shared.bool_var())
    }
}

/// Bind every variable of a pattern matched against a value the prover cannot decompose.
fn collect_binds(
    x: &mut Exec<'_, '_>,
    path: &mut Path,
    v: &Sym,
    pat: &Pattern,
    binds: &mut Vec<(LocalId, Sym)>,
) {
    match &pat.kind {
        PatternKind::Bind(local) => binds.push((*local, v.clone())),
        PatternKind::Ctor { args, .. } => {
            for a in args {
                let fresh = x.fresh(path, &a.ty);
                collect_binds(x, path, &fresh, a, binds);
            }
        }
        PatternKind::Unwrap { arg, .. } => {
            let fresh = x.fresh(path, &arg.ty);
            collect_binds(x, path, &fresh, arg, binds);
        }
        _ => {}
    }
}

/// Every local a block assigns, at any depth.
/// The atoms of a loop condition (`positive`: as it holds when the loop is entered), each as
/// `l <= 0`; equalities and atoms under a disjunction are skipped.
fn condition_atoms(f: &Form, positive: bool, out: &mut Vec<Lin>) {
    match (f, positive) {
        (Form::Le(l), true) => out.push(l.clone()),
        // not (l <= 0)  <=>  -l + 1 <= 0
        (Form::Le(l), false) => out.push(l.scale(&BigInt::from(-1)).add(&Lin::constant(1))),
        (Form::Not(g), p) => condition_atoms(g, !p, out),
        (Form::And(gs), true) | (Form::Or(gs), false) => {
            for g in gs {
                condition_atoms(g, positive, out);
            }
        }
        _ => {}
    }
}

fn assigned_locals(b: &Block, out: &mut BTreeSet<LocalId>) {
    let mut visit = |s: &Stmt| {
        if let Stmt::Assign { local, .. } = s {
            out.insert(*local);
        }
    };
    for s in &b.stmts {
        s.walk_stmts(&mut visit);
    }
    if let Some(t) = &b.tail {
        t.walk(&mut |e| match &e.kind {
            ExprKind::If {
                then_block,
                else_block,
                ..
            } => {
                for b in std::iter::once(then_block).chain(else_block) {
                    for s in &b.stmts {
                        s.walk_stmts(&mut visit);
                    }
                }
            }
            ExprKind::Match { arms, .. } => {
                for arm in arms {
                    for s in &arm.body.stmts {
                        s.walk_stmts(&mut visit);
                    }
                }
            }
            _ => {}
        });
    }
}

fn struct_args(ty: &Ty) -> &[Ty] {
    match ty {
        Ty::Adt(_, args) => args,
        _ => &[],
    }
}

/// Substitute generic arguments into a type.
pub fn instantiate(ty: &Ty, args: &[Ty]) -> Ty {
    if args.is_empty() {
        return ty.clone();
    }
    match ty {
        Ty::Param(i) => args.get(*i as usize).cloned().unwrap_or(Ty::Param(*i)),
        Ty::List(t) => Ty::List(Box::new(instantiate(t, args))),
        Ty::Gen(t) => Ty::Gen(Box::new(instantiate(t, args))),
        Ty::Adt(def, targs) => Ty::Adt(*def, targs.iter().map(|t| instantiate(t, args)).collect()),
        other => other.clone(),
    }
}

/// The root's entry: parameters then `using` handles, fresh or `given` (canonical).
pub fn enter(x: &mut Exec<'_, '_>, given: Option<&[Sym]>) -> Option<(Path, Vec<Sym>)> {
    let f = x.p.def(x.root).kind.as_fn()?;
    let FnBody::Block(_) = &f.body else {
        return None;
    };
    let mut path = Path::new(f.locals.len());
    let mut values = Vec::new();
    for (i, param) in f.params.iter().chain(&f.using).enumerate() {
        let v = match given.and_then(|g| g.get(i)) {
            Some(v) => v.clone(),
            None => x.fresh(&mut path, &param.ty),
        };
        path.env[param.local.0 as usize] = Some(v.clone());
        values.push(v);
    }
    Some((path, values))
}

/// The root's preconditions, assumed in order; their own obligations are recorded.
pub fn assume_requires(x: &mut Exec<'_, '_>, path: Path) -> Vec<Path> {
    let f = x.p.def(x.root).kind.as_fn().expect("a function");
    x.assume_clauses(vec![path], &f.requires)
}

/// The root's preconditions as a call evaluates them: a path where one fails ends in a
/// `contract_violation`.
pub fn require(x: &mut Exec<'_, '_>, path: Path) -> Outs {
    let f = x.p.def(x.root).kind.as_fn().expect("a function");
    let mut paths = vec![path];
    let mut out = Vec::new();
    for c in &f.requires {
        let mut next = Vec::new();
        for p in paths {
            for (mut q, flow) in x.expr(p, &c.expr) {
                match flow {
                    Flow::Val(v) => {
                        let c = x.truth_of(&mut q, &v);
                        let (yes, no) = x.split(q, &c);
                        if let Some(n) = no {
                            out.push((n, Flow::Fault("contract_violation:requires".into())));
                        }
                        if let Some(y) = yes {
                            next.push(y);
                        }
                    }
                    other => out.push((q, other)),
                }
            }
        }
        paths = next;
    }
    let unit = x.unit();
    out.extend(paths.into_iter().map(|p| (p, Flow::Val(unit.clone()))));
    out
}

/// Evaluate the root's measure and check that it is nonnegative; record it for recursive calls.
pub fn enter_measure(x: &mut Exec<'_, '_>, paths: Vec<Path>) -> Vec<Path> {
    let f = x.p.def(x.root).kind.as_fn().expect("a function");
    let Some(dc) = &f.decreases else {
        return paths;
    };
    let mut out = Vec::new();
    for q in paths {
        for (mut r, flow) in x.expr(q, &dc.expr) {
            match flow {
                Flow::Val(Sym::Int(m)) => {
                    x.check(&r, Site::Decreases, &Form::le(&Lin::default(), &m));
                    r.measure = Some(m);
                    out.push(r);
                }
                Flow::Val(_) => {
                    x.check_unknown(Site::Decreases);
                    out.push(r);
                }
                _ => {}
            }
        }
    }
    out
}

/// Run the root's body.
pub fn body(x: &mut Exec<'_, '_>, paths: Vec<Path>) -> Outs {
    let f = x.p.def(x.root).kind.as_fn().expect("a function");
    let FnBody::Block(b) = &f.body else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for p in paths {
        out.extend(x.block(p, b));
    }
    out
}

/// The root's postconditions as checked mode evaluates them on every normal return: a path where
/// one fails ends in a `contract_violation`.
pub fn ensure(x: &mut Exec<'_, '_>, outs: Outs) -> Outs {
    let f = x.p.def(x.root).kind.as_fn().expect("a function");
    let mut done = Vec::new();
    for (mut q, flow) in outs {
        let v = match flow {
            Flow::Val(v) | Flow::Ret(v) => v,
            other => {
                done.push((q, other));
                continue;
            }
        };
        if let Some(r) = f.result_local {
            if let Some(slot) = q.env.get_mut(r.0 as usize) {
                *slot = Some(v.clone());
            }
        }
        let mut paths = vec![q];
        for clause in &f.ensures {
            let mut next = Vec::new();
            for p in paths {
                for (r, form) in x.clause_here(p, clause) {
                    if let Some(r) =
                        x.checked(r, None, form, "contract_violation:ensures", &mut done)
                    {
                        next.push(r);
                    }
                }
            }
            paths = next;
        }
        done.extend(paths.into_iter().map(|p| (p, Flow::Val(v.clone()))));
    }
    done
}

/// Check the root's postconditions on every normal return.
pub fn check_ensures(x: &mut Exec<'_, '_>, outs: Outs) {
    let f = x.p.def(x.root).kind.as_fn().expect("a function");
    for (mut q, flow) in outs {
        let v = match flow {
            Flow::Val(v) | Flow::Ret(v) => v,
            Flow::Fault(_) => continue,
        };
        if let Some(r) = f.result_local {
            if let Some(slot) = q.env.get_mut(r.0 as usize) {
                *slot = Some(v.clone());
            }
        }
        let mut paths = vec![q];
        for (i, clause) in f.ensures.iter().enumerate() {
            let mut next = Vec::new();
            for p in paths {
                for (r, form) in x.clause_here(p, clause) {
                    x.check(&r, Site::Ensures(i), &form);
                    if let Some(r) = x.assume(r, form) {
                        next.push(r);
                    }
                }
            }
            paths = next;
        }
    }
}
