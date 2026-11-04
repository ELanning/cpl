//! Quantifier-free linear integer arithmetic: linear forms over integer variables, formulas built
//! from `<= 0` and `== 0` atoms, and a decision procedure. Boolean atoms are integer variables in
//! {0, 1}. A formula is put in negation normal form and split into conjunctive cases; each case
//! eliminates equalities with unit coefficients exactly, then runs Fourier–Motzkin elimination
//! with integer tightening (every derived constraint is divided by the gcd of its coefficients and
//! its bound rounded inward, which keeps every integer solution). A derived contradiction is a
//! proof of unsatisfiability. Otherwise a model is rebuilt by back-substitution and checked
//! against the original case exactly; a case whose rational shadow is feasible but whose model
//! cannot be found is `Unknown`, never `Sat`.

use num_bigint::BigInt;
use num_traits::{One, Signed, Zero};
use std::collections::{BTreeMap, BTreeSet};

pub type Var = u32;
pub type Model = BTreeMap<Var, BigInt>;

/// `sum(coefficient * variable) + constant`, with no zero coefficients.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Lin {
    pub terms: BTreeMap<Var, BigInt>,
    pub constant: BigInt,
}

impl Lin {
    pub fn constant(c: impl Into<BigInt>) -> Lin {
        Lin {
            terms: BTreeMap::new(),
            constant: c.into(),
        }
    }
    pub fn var(v: Var) -> Lin {
        Lin {
            terms: BTreeMap::from([(v, BigInt::one())]),
            constant: BigInt::zero(),
        }
    }
    pub fn add(&self, other: &Lin) -> Lin {
        let mut out = self.clone();
        for (v, c) in &other.terms {
            let entry = out.terms.entry(*v).or_insert_with(BigInt::zero);
            *entry += c;
            if entry.is_zero() {
                out.terms.remove(v);
            }
        }
        out.constant += &other.constant;
        out
    }
    pub fn sub(&self, other: &Lin) -> Lin {
        self.add(&other.scale(&BigInt::from(-1)))
    }
    pub fn scale(&self, k: &BigInt) -> Lin {
        if k.is_zero() {
            return Lin::default();
        }
        Lin {
            terms: self.terms.iter().map(|(v, c)| (*v, c * k)).collect(),
            constant: &self.constant * k,
        }
    }
    pub fn as_constant(&self) -> Option<&BigInt> {
        self.terms.is_empty().then_some(&self.constant)
    }
    pub fn eval(&self, model: &Model) -> BigInt {
        let mut total = self.constant.clone();
        for (v, c) in &self.terms {
            if let Some(x) = model.get(v) {
                total += c * x;
            }
        }
        total
    }
    fn coefficient(&self, v: Var) -> BigInt {
        self.terms.get(&v).cloned().unwrap_or_default()
    }
    /// Replace `v` by `with`.
    fn substitute(&self, v: Var, with: &Lin) -> Lin {
        match self.terms.get(&v) {
            None => self.clone(),
            Some(c) => {
                let mut rest = self.clone();
                rest.terms.remove(&v);
                rest.add(&with.scale(c))
            }
        }
    }
}

/// A formula over linear atoms.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Form {
    True,
    False,
    /// `lin <= 0`
    Le(Lin),
    /// `lin == 0`
    Eq(Lin),
    Not(Box<Form>),
    And(Vec<Form>),
    Or(Vec<Form>),
}

impl Form {
    /// `a <= b`
    pub fn le(a: &Lin, b: &Lin) -> Form {
        let d = a.sub(b);
        match d.as_constant() {
            Some(c) => Form::bool(!c.is_positive()),
            None => Form::Le(d),
        }
    }
    /// `a < b`
    pub fn lt(a: &Lin, b: &Lin) -> Form {
        Form::le(&a.add(&Lin::constant(1)), b)
    }
    /// `a == b`
    pub fn eq(a: &Lin, b: &Lin) -> Form {
        let d = a.sub(b);
        match d.as_constant() {
            Some(c) => Form::bool(c.is_zero()),
            None => Form::Eq(d),
        }
    }
    pub fn bool(b: bool) -> Form {
        if b {
            Form::True
        } else {
            Form::False
        }
    }
    #[allow(clippy::should_implement_trait)]
    pub fn not(f: Form) -> Form {
        match f {
            Form::True => Form::False,
            Form::False => Form::True,
            Form::Not(inner) => *inner,
            other => Form::Not(Box::new(other)),
        }
    }
    pub fn and(fs: Vec<Form>) -> Form {
        let mut out = Vec::new();
        for f in fs {
            match f {
                Form::True => {}
                Form::False => return Form::False,
                Form::And(inner) => out.extend(inner),
                other => out.push(other),
            }
        }
        out.sort();
        out.dedup();
        match out.len() {
            0 => Form::True,
            1 => out.pop().unwrap(),
            _ => Form::And(out),
        }
    }
    pub fn or(fs: Vec<Form>) -> Form {
        let mut out = Vec::new();
        for f in fs {
            match f {
                Form::False => {}
                Form::True => return Form::True,
                Form::Or(inner) => out.extend(inner),
                other => out.push(other),
            }
        }
        out.sort();
        out.dedup();
        match out.len() {
            0 => Form::False,
            1 => out.pop().unwrap(),
            _ => Form::Or(out),
        }
    }
    pub fn iff(a: Form, b: Form) -> Form {
        Form::or(vec![
            Form::and(vec![a.clone(), b.clone()]),
            Form::and(vec![Form::not(a), Form::not(b)]),
        ])
    }
    pub fn eval(&self, model: &Model) -> bool {
        match self {
            Form::True => true,
            Form::False => false,
            Form::Le(l) => !l.eval(model).is_positive(),
            Form::Eq(l) => l.eval(model).is_zero(),
            Form::Not(f) => !f.eval(model),
            Form::And(fs) => fs.iter().all(|f| f.eval(model)),
            Form::Or(fs) => fs.iter().any(|f| f.eval(model)),
        }
    }
    fn vars(&self, out: &mut BTreeSet<Var>) {
        match self {
            Form::True | Form::False => {}
            Form::Le(l) | Form::Eq(l) => out.extend(l.terms.keys()),
            Form::Not(f) => f.vars(out),
            Form::And(fs) | Form::Or(fs) => {
                for f in fs {
                    f.vars(out);
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Unsat,
    Sat(Model),
    Unknown(&'static str),
}

/// Limits that keep the procedure bounded; reaching one is `Unknown`, never a wrong answer.
const MAX_CASES: usize = 4096;
const MAX_CONSTRAINTS: usize = 2000;

/// Decide `form`, whose variables in `bools` range over {0, 1}.
pub fn solve(form: &Form, bools: &BTreeSet<Var>) -> Outcome {
    let nnf = nnf(form, true);
    let mut search = Search {
        bools,
        cases: 0,
        unknown: None,
        original: form,
    };
    match search.dfs(vec![nnf], Vec::new(), Vec::new(), 0) {
        Some(model) => Outcome::Sat(model),
        None => match search.unknown {
            Some(reason) => Outcome::Unknown(reason),
            None => Outcome::Unsat,
        },
    }
}

/// Negation normal form: negations only on atoms, which become atoms again.
fn nnf(f: &Form, positive: bool) -> Form {
    match (f, positive) {
        (Form::True, p) => Form::bool(p),
        (Form::False, p) => Form::bool(!p),
        (Form::Le(l), true) => Form::Le(l.clone()),
        // not (l <= 0)  <=>  l >= 1  <=>  -l + 1 <= 0
        (Form::Le(l), false) => Form::Le(l.scale(&BigInt::from(-1)).add(&Lin::constant(1))),
        (Form::Eq(l), true) => Form::Eq(l.clone()),
        // not (l == 0)  <=>  l <= -1 or l >= 1
        (Form::Eq(l), false) => Form::Or(vec![
            Form::Le(l.add(&Lin::constant(1))),
            Form::Le(l.scale(&BigInt::from(-1)).add(&Lin::constant(1))),
        ]),
        (Form::Not(inner), p) => nnf(inner, !p),
        (Form::And(fs), true) | (Form::Or(fs), false) => {
            Form::and(fs.iter().map(|g| nnf(g, positive)).collect())
        }
        (Form::Or(fs), true) | (Form::And(fs), false) => {
            Form::or(fs.iter().map(|g| nnf(g, positive)).collect())
        }
    }
}

/// One literal of a case.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Atom {
    Le(Lin),
    Eq(Lin),
}

struct Search<'a> {
    bools: &'a BTreeSet<Var>,
    cases: usize,
    unknown: Option<&'static str>,
    original: &'a Form,
}

impl Search<'_> {
    /// Expand `pending` into `case`, then branch on the smallest disjunction left, pruning every
    /// branch whose atoms are already infeasible.
    fn dfs(
        &mut self,
        mut pending: Vec<Form>,
        mut case: Vec<Atom>,
        mut ors: Vec<Vec<Form>>,
        checked: usize,
    ) -> Option<Model> {
        while let Some(f) = pending.pop() {
            match f {
                Form::True => {}
                Form::False => return None,
                Form::Le(l) => match l.as_constant() {
                    Some(c) if c.is_positive() => return None,
                    Some(_) => {}
                    None => case.push(Atom::Le(l)),
                },
                Form::Eq(l) => match l.as_constant() {
                    Some(c) if !c.is_zero() => return None,
                    Some(_) => {}
                    None => case.push(Atom::Eq(l)),
                },
                Form::And(fs) => pending.extend(fs),
                Form::Or(fs) => ors.push(fs),
                Form::Not(_) => unreachable!("negation normal form"),
            }
        }
        if self.cases >= MAX_CASES {
            self.unknown.get_or_insert("case_limit");
            return None;
        }
        if !ors.is_empty() {
            self.cases += 1;
            if case.len() > checked && feasible(&case, self.bools) == Outcome::Unsat {
                return None;
            }
            let smallest = ors
                .iter()
                .enumerate()
                .min_by_key(|(_, o)| o.len())
                .map(|(i, _)| i)
                .unwrap_or(0);
            let alternatives = ors.swap_remove(smallest);
            for alternative in alternatives {
                let depth = case.len();
                if let Some(m) = self.dfs(vec![alternative], case.clone(), ors.clone(), depth) {
                    return Some(m);
                }
                if self.cases >= MAX_CASES {
                    self.unknown.get_or_insert("case_limit");
                    return None;
                }
            }
            return None;
        }
        self.cases += 1;
        match feasible(&case, self.bools) {
            Outcome::Sat(model) => {
                // The case is a sound strengthening of the formula; confirm on the formula itself.
                if self.original.eval(&model) {
                    Some(model)
                } else {
                    self.unknown.get_or_insert("model_check");
                    None
                }
            }
            Outcome::Unsat => None,
            Outcome::Unknown(reason) => {
                self.unknown.get_or_insert(reason);
                None
            }
        }
    }
}

fn floor_div(a: &BigInt, b: &BigInt) -> BigInt {
    let (q, r) = (a / b, a % b);
    if !r.is_zero() && (r.is_negative() != b.is_negative()) {
        q - 1
    } else {
        q
    }
}
fn ceil_div(a: &BigInt, b: &BigInt) -> BigInt {
    -floor_div(&-a, b)
}
fn gcd(a: &BigInt, b: &BigInt) -> BigInt {
    let (mut a, mut b) = (a.abs(), b.abs());
    while !b.is_zero() {
        let r = &a % &b;
        a = b;
        b = r;
    }
    a
}

/// Tighten `lin <= 0` over the integers; `Err` for a contradiction, `Ok(None)` when trivially true.
fn tighten(l: Lin) -> Result<Option<Lin>, ()> {
    if l.terms.is_empty() {
        return if l.constant.is_positive() {
            Err(())
        } else {
            Ok(None)
        };
    }
    let g = l.terms.values().fold(BigInt::zero(), |g, c| gcd(&g, c));
    if g.is_one() {
        return Ok(Some(l));
    }
    Ok(Some(Lin {
        terms: l.terms.iter().map(|(v, c)| (*v, c / &g)).collect(),
        constant: ceil_div(&l.constant, &g),
    }))
}

/// Integer feasibility of one conjunctive case.
fn feasible(case: &[Atom], bools: &BTreeSet<Var>) -> Outcome {
    let mut vars = BTreeSet::new();
    for a in case {
        let (Atom::Le(l) | Atom::Eq(l)) = a;
        vars.extend(l.terms.keys().copied());
    }
    let mut les: Vec<Lin> = Vec::new();
    let mut eqs: Vec<Lin> = Vec::new();
    for a in case {
        match a {
            Atom::Le(l) => les.push(l.clone()),
            Atom::Eq(l) => eqs.push(l.clone()),
        }
    }
    for v in vars.iter().filter(|v| bools.contains(v)) {
        les.push(Lin::var(*v).scale(&BigInt::from(-1)));
        les.push(Lin::var(*v).sub(&Lin::constant(1)));
    }
    // Exact elimination of equalities with a unit coefficient.
    let mut substitutions: Vec<(Var, Lin)> = Vec::new();
    while let Some(pos) = eqs
        .iter()
        .position(|e| e.terms.values().any(|c| c.abs().is_one()))
    {
        let e = eqs.remove(pos);
        let (v, c) = e
            .terms
            .iter()
            .find(|(_, c)| c.abs().is_one())
            .map(|(v, c)| (*v, c.clone()))
            .unwrap();
        // c*v + rest == 0  =>  v = -rest / c, with c = +-1
        let mut rest = e.clone();
        rest.terms.remove(&v);
        let value = rest.scale(&(-&c));
        for other in eqs.iter_mut().chain(les.iter_mut()) {
            *other = other.substitute(v, &value);
        }
        for (_, s) in substitutions.iter_mut() {
            *s = s.substitute(v, &value);
        }
        substitutions.push((v, value));
    }
    for e in eqs {
        if e.terms.is_empty() {
            if !e.constant.is_zero() {
                return Outcome::Unsat;
            }
            continue;
        }
        let g = e.terms.values().fold(BigInt::zero(), |g, c| gcd(&g, c));
        if !(&e.constant % &g).is_zero() {
            return Outcome::Unsat;
        }
        les.push(e.clone());
        les.push(e.scale(&BigInt::from(-1)));
    }
    let mut current: BTreeSet<Lin> = BTreeSet::new();
    for l in les {
        match tighten(l) {
            Err(()) => return Outcome::Unsat,
            Ok(Some(l)) => {
                current.insert(l);
            }
            Ok(None) => {}
        }
    }
    let mut stages: Vec<(Var, Vec<Lin>)> = Vec::new();
    loop {
        let mut counts: BTreeMap<Var, (usize, usize)> = BTreeMap::new();
        for l in &current {
            for (v, c) in &l.terms {
                let e = counts.entry(*v).or_default();
                if c.is_positive() {
                    e.0 += 1;
                } else {
                    e.1 += 1;
                }
            }
        }
        let Some((&x, _)) = counts.iter().min_by_key(|(_, (p, n))| p * n) else {
            break;
        };
        let (with, without): (Vec<Lin>, Vec<Lin>) =
            current.into_iter().partition(|l| l.terms.contains_key(&x));
        let mut next: BTreeSet<Lin> = without.into_iter().collect();
        let uppers: Vec<&Lin> = with
            .iter()
            .filter(|l| l.coefficient(x).is_positive())
            .collect();
        let lowers: Vec<&Lin> = with
            .iter()
            .filter(|l| l.coefficient(x).is_negative())
            .collect();
        for u in &uppers {
            for l in &lowers {
                let a = u.coefficient(x);
                let b = -l.coefficient(x);
                // a*x + p <= 0 and -b*x + q <= 0  =>  b*p + a*q <= 0
                let combined = u.scale(&b).add(&l.scale(&a));
                match tighten(combined) {
                    Err(()) => return Outcome::Unsat,
                    Ok(Some(c)) => {
                        next.insert(c);
                    }
                    Ok(None) => {}
                }
            }
        }
        if next.len() > MAX_CONSTRAINTS {
            return Outcome::Unknown("arithmetic_limit");
        }
        stages.push((x, with));
        current = next;
    }
    // Rebuild an integer model, latest elimination first.
    let mut model: Model = Model::new();
    for (x, constraints) in stages.iter().rev() {
        let (mut lo, mut hi): (Option<BigInt>, Option<BigInt>) = (None, None);
        for l in constraints {
            let a = l.coefficient(*x);
            let mut rest = l.clone();
            rest.terms.remove(x);
            let r = rest.eval(&model);
            if a.is_positive() {
                let bound = floor_div(&-r, &a);
                hi = Some(hi.map_or(bound.clone(), |h| h.min(bound)));
            } else {
                let bound = ceil_div(&r, &-a);
                lo = Some(lo.map_or(bound.clone(), |l| l.max(bound)));
            }
        }
        let value = match (&lo, &hi) {
            (Some(l), Some(h)) if l > h => return Outcome::Unknown("integer_gap"),
            (Some(l), _) if l.is_positive() => l.clone(),
            (_, Some(h)) if h.is_negative() => h.clone(),
            _ => BigInt::zero(),
        };
        model.insert(*x, value);
    }
    for (v, value) in substitutions.iter().rev() {
        let x = value.eval(&model);
        model.insert(*v, x);
    }
    for v in vars {
        model.entry(v).or_insert_with(BigInt::zero);
    }
    let holds = case.iter().all(|a| match a {
        Atom::Le(l) => !l.eval(&model).is_positive(),
        Atom::Eq(l) => l.eval(&model).is_zero(),
    }) && bools
        .iter()
        .all(|b| model.get(b).is_none_or(|x| x.is_zero() || x.is_one()));
    if holds {
        Outcome::Sat(model)
    } else {
        Outcome::Unknown("integer_model")
    }
}

/// The variables of a formula.
pub fn vars(f: &Form) -> BTreeSet<Var> {
    let mut out = BTreeSet::new();
    f.vars(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn x(v: Var) -> Lin {
        Lin::var(v)
    }
    fn k(n: i64) -> Lin {
        Lin::constant(n)
    }

    #[test]
    fn linear_facts_are_decided() {
        let none = BTreeSet::new();
        // x >= 0 and 10x < 0 is unsatisfiable.
        let f = Form::and(vec![
            Form::le(&k(0), &x(0)),
            Form::lt(&x(0).scale(&10.into()), &k(0)),
        ]);
        assert_eq!(solve(&f, &none), Outcome::Unsat);
        // x + 1 != 1 + x is unsatisfiable.
        let f = Form::not(Form::eq(&x(0).add(&k(1)), &k(1).add(&x(0))));
        assert_eq!(solve(&f, &none), Outcome::Unsat);
        // 2x == 1 has no integer solution (gcd test).
        let f = Form::eq(&x(0).scale(&2.into()), &k(1));
        assert_eq!(solve(&f, &none), Outcome::Unsat);
        // 1 <= 2x <= 1 has a rational but no integer solution (tightening).
        let f = Form::and(vec![
            Form::le(&k(1), &x(0).scale(&2.into())),
            Form::le(&x(0).scale(&2.into()), &k(1)),
        ]);
        assert_eq!(solve(&f, &none), Outcome::Unsat);
        // x > 5 and x != 6 has a model, which is verified.
        let f = Form::and(vec![
            Form::lt(&k(5), &x(0)),
            Form::not(Form::eq(&x(0), &k(6))),
        ]);
        match solve(&f, &none) {
            Outcome::Sat(m) => assert!(f.eval(&m)),
            other => panic!("{other:?}"),
        }
        // Transitivity over three variables.
        let f = Form::and(vec![
            Form::lt(&x(0), &x(1)),
            Form::lt(&x(1), &x(2)),
            Form::le(&x(2), &x(0)),
        ]);
        assert_eq!(solve(&f, &none), Outcome::Unsat);
        // Booleans range over {0, 1}.
        let bools = BTreeSet::from([3]);
        let f = Form::eq(&x(3), &k(2));
        assert_eq!(solve(&f, &bools), Outcome::Unsat);
    }
}
