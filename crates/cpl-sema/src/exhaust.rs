//! Match exhaustiveness and arm reachability (`spec/semantics.md` 3.13,
//! S25): the usefulness algorithm over pattern matrices (Maranget), with
//! the witnesses of a non-exhaustive match built during the search.
//!
//! The algorithm is independent of the HIR: a [`Space`] gives the
//! constructor set `Σ(T)` of a column type (`None` for an infinite set:
//! `Int`, `Text`, `Bytes`, lists, type parameters), and patterns are
//! [`Pat`]s whose literals are constructors of arity 0. The body typer
//! implements `Space` over `cpl_hir::Ty` (S25: `Bool` is `{true, false}`,
//! `Unit` `{Unit}`, a wrapper its one constructor, a struct one constructor
//! of its field count, an enum its variants).

use num_bigint::BigInt;
use std::collections::{HashMap, HashSet};

/// A constructor of a column: an enum variant (by index), a `Bool`
/// literal, the single constructor of a wrapper or struct, or an `Int` or
/// `Text` literal (constructors of an infinite type).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum CtorKey {
    Variant(u32),
    Bool(bool),
    Single,
    Int(BigInt),
    Text(String),
}

/// A pattern as the algorithm sees it: a wildcard (`_` and bindings) or a
/// constructor applied to sub-patterns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Pat {
    Wild,
    Ctor(CtorKey, Vec<Pat>),
}

/// One constructor of a finite `Σ(T)`, in declaration order.
#[derive(Clone, Debug)]
pub(crate) struct CtorSig<T> {
    pub key: CtorKey,
    /// The name a witness shows (`Missing`, `true`); `None` renders the
    /// constructor as `_` (a struct, which no pattern names, D33).
    pub name: Option<String>,
    /// The types of its payloads, instantiated.
    pub fields: Vec<T>,
}

/// The constructor sets of column types.
pub(crate) trait Space {
    type Ty: Clone;
    /// `Σ(T)`: `Some` for a finite set (possibly empty), `None` for an
    /// infinite one.
    fn sigma(&self, ty: &Self::Ty) -> Option<Vec<CtorSig<Self::Ty>>>;
}

/// The most witnesses a non-exhaustive match reports.
const MAX_WITNESSES: usize = 32;

/// The steps (calls of `useful` and `witnesses`) one `match` may take. Both are exponential in the
/// worst case: a `match` on a struct of `n` `Bool` fields whose arms keep every column's
/// constructors complete (`S { a: true, .. }`, `S { a: false, .. }`, `S { b: true, .. }`, ...)
/// branches two ways per column. Past the budget the `match` is `match_too_complex`, so a few
/// lines of source cannot keep the checker busy for hours.
pub(crate) const MATCH_STEP_BUDGET: u64 = 1_000_000;

/// The steps left to one `match`'s check.
pub(crate) struct Budget(std::cell::Cell<u64>);

impl Budget {
    pub(crate) fn new(steps: u64) -> Budget {
        Budget(std::cell::Cell::new(steps))
    }

    /// Take one step: false when none is left.
    fn step(&self) -> bool {
        let left = self.0.get();
        if left == 0 {
            return false;
        }
        self.0.set(left - 1);
        true
    }

    pub(crate) fn exhausted(&self) -> bool {
        self.0.get() == 0
    }
}

/// A witness pattern of a non-exhaustive match.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Witness {
    Wild,
    Ctor(String, Vec<Witness>),
}

impl Witness {
    /// `Err(Unavailable)`, `Some(_)`, `true`, `_`.
    pub fn render(&self) -> String {
        match self {
            Witness::Wild => "_".to_string(),
            Witness::Ctor(name, args) if args.is_empty() => name.clone(),
            Witness::Ctor(name, args) => format!(
                "{name}({})",
                args.iter()
                    .map(Witness::render)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

#[cfg(test)]
fn arity<T>(sigma: Option<&[CtorSig<T>]>, key: &CtorKey) -> usize {
    match key {
        CtorKey::Int(_) | CtorKey::Text(_) | CtorKey::Bool(_) => 0,
        _ => sigma
            .and_then(|s| s.iter().find(|c| &c.key == key))
            .map_or(0, |c| c.fields.len()),
    }
}

#[cfg(test)]
fn fields_of<T: Clone>(sigma: Option<&[CtorSig<T>]>, key: &CtorKey) -> Vec<T> {
    sigma
        .and_then(|s| s.iter().find(|c| &c.key == key))
        .map(|c| c.fields.clone())
        .unwrap_or_default()
}

/// `specialize(c, P)`: rows whose head is `c(r1..rk)` become `r1..rk ++
/// rest`, rows whose head is a wildcard become `_^k ++ rest`, others go.
fn specialize(rows: &[Vec<Pat>], key: &CtorKey, k: usize) -> Vec<Vec<Pat>> {
    let mut out = Vec::new();
    for row in rows {
        match &row[0] {
            Pat::Ctor(c, args) if c == key => {
                let mut r: Vec<Pat> = args.clone();
                // A malformed row (wrong payload count) is padded or cut so
                // that the matrix stays rectangular.
                r.resize(k, Pat::Wild);
                r.extend(row[1..].iter().cloned());
                out.push(r);
            }
            Pat::Ctor(..) => {}
            Pat::Wild => {
                let mut r = vec![Pat::Wild; k];
                r.extend(row[1..].iter().cloned());
                out.push(r);
            }
        }
    }
    out
}

/// `default(P)`: rows whose head is a wildcard, with the head removed.
fn default(rows: &[Vec<Pat>]) -> Vec<Vec<Pat>> {
    rows.iter()
        .filter(|r| r[0] == Pat::Wild)
        .map(|r| r[1..].to_vec())
        .collect()
}

/// The constructors at the heads of the rows (a set: a `match` may have
/// thousands of literal arms).
fn heads(rows: &[Vec<Pat>]) -> HashSet<&CtorKey> {
    rows.iter()
        .filter_map(|r| match &r[0] {
            Pat::Ctor(c, _) => Some(c),
            Pat::Wild => None,
        })
        .collect()
}

/// `useful(P, q)`: is there a value matched by `q` and by no row of `P`?
/// `tys` are the column types. The reference [`Matrix::useful`] is tested
/// against.
#[cfg(test)]
pub(crate) fn useful<S: Space>(space: &S, rows: &[Vec<Pat>], q: &[Pat], tys: &[S::Ty]) -> bool {
    if q.is_empty() {
        return rows.is_empty();
    }
    let sigma = space.sigma(&tys[0]);
    match &q[0] {
        Pat::Ctor(c, args) => {
            let k = arity(sigma.as_deref(), c).max(args.len());
            let mut fields = fields_of(sigma.as_deref(), c);
            fields.resize(k, tys[0].clone());
            let mut q2: Vec<Pat> = args.clone();
            q2.resize(k, Pat::Wild);
            q2.extend(q[1..].iter().cloned());
            let mut t2 = fields;
            t2.extend(tys[1..].iter().cloned());
            useful(space, &specialize(rows, c, k), &q2, &t2)
        }
        Pat::Wild => {
            let hs = heads(rows);
            match &sigma {
                Some(ctors) if ctors.iter().all(|c| hs.contains(&c.key)) => ctors.iter().any(|c| {
                    let k = c.fields.len();
                    let mut q2 = vec![Pat::Wild; k];
                    q2.extend(q[1..].iter().cloned());
                    let mut t2 = c.fields.clone();
                    t2.extend(tys[1..].iter().cloned());
                    useful(space, &specialize(rows, &c.key, k), &q2, &t2)
                }),
                _ => useful(space, &default(rows), &q[1..], &tys[1..]),
            }
        }
    }
}

/// Every witness vector (one pattern per column) that no row matches, in
/// the declaration order of the missing constructors, depth-first (3.13);
/// empty when the rows are exhaustive.
pub(crate) fn witnesses<S: Space>(
    space: &S,
    rows: &[Vec<Pat>],
    tys: &[S::Ty],
    budget: &Budget,
) -> Vec<Vec<Witness>> {
    if !budget.step() {
        return Vec::new();
    }
    if tys.is_empty() {
        return if rows.is_empty() {
            vec![Vec::new()]
        } else {
            Vec::new()
        };
    }
    let sigma = space.sigma(&tys[0]);
    let hs = heads(rows);
    let mut out: Vec<Vec<Witness>> = Vec::new();
    match &sigma {
        Some(ctors) if ctors.iter().all(|c| hs.contains(&c.key)) => {
            for c in ctors {
                let k = c.fields.len();
                let mut t2 = c.fields.clone();
                t2.extend(tys[1..].iter().cloned());
                for w in witnesses(space, &specialize(rows, &c.key, k), &t2, budget) {
                    let (args, rest) = w.split_at(k);
                    let head = match &c.name {
                        Some(name) => Witness::Ctor(name.clone(), args.to_vec()),
                        None => Witness::Wild,
                    };
                    let mut v = vec![head];
                    v.extend(rest.iter().cloned());
                    out.push(v);
                    if out.len() >= MAX_WITNESSES {
                        return out;
                    }
                }
            }
        }
        _ => {
            let sub = witnesses(space, &default(rows), &tys[1..], budget);
            if sub.is_empty() {
                return out;
            }
            let firsts: Vec<Witness> = match &sigma {
                Some(ctors) => ctors
                    .iter()
                    .filter(|c| !hs.contains(&c.key))
                    .map(|c| match &c.name {
                        Some(name) => {
                            Witness::Ctor(name.clone(), vec![Witness::Wild; c.fields.len()])
                        }
                        None => Witness::Wild,
                    })
                    .collect(),
                None => vec![Witness::Wild],
            };
            for f in firsts {
                for w in &sub {
                    let mut v = vec![f.clone()];
                    v.extend(w.iter().cloned());
                    out.push(v);
                    if out.len() >= MAX_WITNESSES {
                        return out;
                    }
                }
            }
        }
    }
    out
}

/// The rows of the arms so far, with the matrices `useful` derives from them kept and extended
/// as rows arrive: its specialization by each constructor it was asked about, and its default
/// matrix, each indexed by the constructors at the heads of its rows. Deciding an arm then
/// touches only the rows its own constructors select, so a `match` of thousands of arms (a
/// table of literals, even under a constructor, `Some(1)`, `Some(2)`, ...) is checked in time
/// linear in its size, where rebuilding the matrices for every arm was quadratic. The answers
/// are `useful`'s: a specialization holds the same rows, and usefulness does not depend on
/// their order.
struct Matrix<T> {
    tys: Vec<T>,
    /// `Σ` of the first column, and each constructor's position in it.
    sigma: Option<Vec<CtorSig<T>>>,
    sigma_at: HashMap<CtorKey, usize>,
    rows: Vec<Vec<Pat>>,
    by_head: HashMap<CtorKey, Vec<usize>>,
    wild: Vec<usize>,
    /// By constructor, then by arity (a malformed pattern may give one a second).
    special: HashMap<CtorKey, Vec<(usize, Matrix<T>)>>,
    default: Option<Box<Matrix<T>>>,
}

impl<T: Clone> Matrix<T> {
    fn new<S: Space<Ty = T>>(space: &S, tys: Vec<T>) -> Matrix<T> {
        let sigma = tys.first().and_then(|t| space.sigma(t));
        let sigma_at = sigma
            .iter()
            .flatten()
            .enumerate()
            .map(|(i, c)| (c.key.clone(), i))
            .collect();
        Matrix {
            tys,
            sigma,
            sigma_at,
            rows: Vec::new(),
            by_head: HashMap::new(),
            wild: Vec::new(),
            special: HashMap::new(),
            default: None,
        }
    }

    fn arity(&self, key: &CtorKey) -> usize {
        match key {
            CtorKey::Int(_) | CtorKey::Text(_) | CtorKey::Bool(_) => 0,
            _ => self.fields(key).len(),
        }
    }

    fn fields(&self, key: &CtorKey) -> Vec<T> {
        match (&self.sigma, self.sigma_at.get(key)) {
            (Some(ctors), Some(&i)) => ctors[i].fields.clone(),
            _ => Vec::new(),
        }
    }

    /// `specialize` of one row by `(key, k)`, when it is kept.
    fn specialized(row: &[Pat], key: &CtorKey, k: usize) -> Option<Vec<Pat>> {
        let mut r = match &row[0] {
            Pat::Ctor(c, args) if c == key => {
                let mut r = args.clone();
                r.resize(k, Pat::Wild);
                r
            }
            Pat::Ctor(..) => return None,
            Pat::Wild => vec![Pat::Wild; k],
        };
        r.extend(row[1..].iter().cloned());
        Some(r)
    }

    /// Add a row (one pattern per column), to this matrix and to every one derived from it.
    fn push(&mut self, row: Vec<Pat>) {
        if row.is_empty() {
            self.rows.push(row);
            return;
        }
        match &row[0] {
            Pat::Ctor(c, _) => {
                self.by_head
                    .entry(c.clone())
                    .or_default()
                    .push(self.rows.len());
                for (k, m) in self.special.get_mut(c).into_iter().flatten() {
                    if let Some(r) = Self::specialized(&row, c, *k) {
                        m.push(r);
                    }
                }
            }
            Pat::Wild => {
                self.wild.push(self.rows.len());
                for (key, ms) in self.special.iter_mut() {
                    for (k, m) in ms {
                        if let Some(r) = Self::specialized(&row, key, *k) {
                            m.push(r);
                        }
                    }
                }
                if let Some(d) = &mut self.default {
                    d.push(row[1..].to_vec());
                }
            }
        }
        self.rows.push(row);
    }

    /// The specialization by `(key, k)`, built from the rows its head selects the first time.
    fn special<S: Space<Ty = T>>(&mut self, space: &S, key: &CtorKey, k: usize) -> &mut Matrix<T> {
        let built = self
            .special
            .get(key)
            .is_some_and(|ms| ms.iter().any(|(j, _)| *j == k));
        if !built {
            let mut tys = self.fields(key);
            tys.resize(k, self.tys[0].clone());
            tys.extend(self.tys[1..].iter().cloned());
            let mut m = Matrix::new(space, tys);
            let mut selected: Vec<usize> = self.by_head.get(key).cloned().unwrap_or_default();
            selected.extend(&self.wild);
            selected.sort_unstable();
            for i in selected {
                if let Some(r) = Self::specialized(&self.rows[i], key, k) {
                    m.push(r);
                }
            }
            self.special.entry(key.clone()).or_default().push((k, m));
        }
        self.special
            .get_mut(key)
            .and_then(|ms| ms.iter_mut().find(|(j, _)| *j == k))
            .map(|(_, m)| m)
            .expect("just built")
    }

    /// The default matrix, built from the wildcard rows the first time.
    fn default<S: Space<Ty = T>>(&mut self, space: &S) -> &mut Matrix<T> {
        if self.default.is_none() {
            let mut m = Matrix::new(space, self.tys[1..].to_vec());
            for &i in &self.wild {
                m.push(self.rows[i][1..].to_vec());
            }
            self.default = Some(Box::new(m));
        }
        self.default.as_mut().expect("just built")
    }

    /// `useful(P, q)` for this matrix's rows `P`; true once the budget is spent (the caller
    /// then reports `match_too_complex`, not an unreachable arm).
    fn useful<S: Space<Ty = T>>(&mut self, space: &S, q: &[Pat], budget: &Budget) -> bool {
        if !budget.step() {
            return true;
        }
        if q.is_empty() {
            return self.rows.is_empty();
        }
        match &q[0] {
            Pat::Ctor(c, args) => {
                let k = self.arity(c).max(args.len());
                let mut q2: Vec<Pat> = args.clone();
                q2.resize(k, Pat::Wild);
                q2.extend(q[1..].iter().cloned());
                self.special(space, c, k).useful(space, &q2, budget)
            }
            Pat::Wild => {
                let complete = match &self.sigma {
                    Some(ctors) => ctors.iter().all(|c| self.by_head.contains_key(&c.key)),
                    None => false,
                };
                if complete {
                    let ctors: Vec<(CtorKey, usize)> = self
                        .sigma
                        .iter()
                        .flatten()
                        .map(|c| (c.key.clone(), c.fields.len()))
                        .collect();
                    ctors.into_iter().any(|(key, k)| {
                        let mut q2 = vec![Pat::Wild; k];
                        q2.extend(q[1..].iter().cloned());
                        self.special(space, &key, k).useful(space, &q2, budget)
                    })
                } else {
                    self.default(space).useful(space, &q[1..], budget)
                }
            }
        }
    }
}

/// The outcome of checking one `match`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MatchCheck {
    /// The witnesses of the values no arm matches (empty: exhaustive).
    pub missing: Vec<String>,
    /// The indices of the arms no value reaches (`unreachable_arm`).
    pub unreachable: Vec<usize>,
    /// The check ran out of `MATCH_STEP_BUDGET` (`match_too_complex`): `missing` and
    /// `unreachable` are then empty.
    pub too_complex: bool,
}

/// Check the arms of a `match` on a scrutinee of type `ty`: `arms[i]` is
/// `None` for an arm whose pattern is erroneous, which takes part in
/// neither check (S62); exhaustiveness is checked only when every arm's
/// pattern is well-typed.
pub(crate) fn check_match<S: Space>(space: &S, ty: &S::Ty, arms: &[Option<Pat>]) -> MatchCheck {
    check_match_within(space, ty, arms, &Budget::new(MATCH_STEP_BUDGET))
}

/// `check_match` within `budget`.
pub(crate) fn check_match_within<S: Space>(
    space: &S,
    ty: &S::Ty,
    arms: &[Option<Pat>],
    budget: &Budget,
) -> MatchCheck {
    let tys = vec![ty.clone()];
    let mut rows: Vec<Vec<Pat>> = Vec::new();
    let mut matrix = Matrix::new(space, tys.clone());
    let mut unreachable = Vec::new();
    for (i, arm) in arms.iter().enumerate() {
        let Some(p) = arm else { continue };
        let q = vec![p.clone()];
        if !matrix.useful(space, &q, budget) {
            unreachable.push(i);
        }
        if budget.exhausted() {
            break;
        }
        matrix.push(q.clone());
        rows.push(q);
    }
    let missing = if !budget.exhausted() && arms.iter().all(Option::is_some) {
        witnesses(space, &rows, &tys, budget)
            .into_iter()
            .map(|w| w[0].render())
            .collect()
    } else {
        Vec::new()
    };
    if budget.exhausted() {
        return MatchCheck {
            missing: Vec::new(),
            unreachable: Vec::new(),
            too_complex: true,
        };
    }
    MatchCheck {
        missing,
        unreachable,
        too_complex: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A toy type language for the tests.
    #[derive(Clone, Debug, PartialEq)]
    enum T {
        Bool,
        Int,
        Unit,
        /// `Option<T>`.
        Opt(Box<T>),
        /// `Result<A, B>`.
        Res(Box<T>, Box<T>),
        /// `enum StoreError { Missing(Text) Unavailable }`.
        Store,
        /// An enum with no variants.
        Never,
        /// `struct Page { a: Int, b: Bool }`.
        Page,
        /// `type PageId(Int)`.
        Id,
    }

    struct Toy;

    fn sig(key: CtorKey, name: &str, fields: Vec<T>) -> CtorSig<T> {
        CtorSig {
            key,
            name: Some(name.to_string()),
            fields,
        }
    }

    impl Space for Toy {
        type Ty = T;
        fn sigma(&self, ty: &T) -> Option<Vec<CtorSig<T>>> {
            Some(match ty {
                T::Bool => vec![
                    sig(CtorKey::Bool(true), "true", vec![]),
                    sig(CtorKey::Bool(false), "false", vec![]),
                ],
                T::Int => return None,
                T::Unit => vec![sig(CtorKey::Variant(0), "Unit", vec![])],
                T::Opt(t) => vec![
                    sig(CtorKey::Variant(0), "Some", vec![(**t).clone()]),
                    sig(CtorKey::Variant(1), "None", vec![]),
                ],
                T::Res(a, b) => vec![
                    sig(CtorKey::Variant(0), "Ok", vec![(**a).clone()]),
                    sig(CtorKey::Variant(1), "Err", vec![(**b).clone()]),
                ],
                T::Store => vec![
                    sig(CtorKey::Variant(0), "Missing", vec![T::Int]),
                    sig(CtorKey::Variant(1), "Unavailable", vec![]),
                ],
                T::Never => vec![],
                T::Page => vec![CtorSig {
                    key: CtorKey::Single,
                    name: None,
                    fields: vec![T::Int, T::Bool],
                }],
                T::Id => vec![sig(CtorKey::Single, "PageId", vec![T::Int])],
            })
        }
    }

    fn v(i: u32, args: Vec<Pat>) -> Pat {
        Pat::Ctor(CtorKey::Variant(i), args)
    }

    fn b(x: bool) -> Pat {
        Pat::Ctor(CtorKey::Bool(x), vec![])
    }

    fn n(x: i64) -> Pat {
        Pat::Ctor(CtorKey::Int(BigInt::from(x)), vec![])
    }

    fn check(ty: T, arms: Vec<Pat>) -> MatchCheck {
        let arms: Vec<Option<Pat>> = arms.into_iter().map(Some).collect();
        check_match(&Toy, &ty, &arms)
    }

    #[test]
    fn bool_and_unit_are_finite() {
        assert_eq!(
            check(T::Bool, vec![b(true), b(false)]).missing,
            Vec::<String>::new()
        );
        assert_eq!(check(T::Bool, vec![b(true)]).missing, ["false"]);
        assert_eq!(
            check(T::Unit, vec![v(0, vec![])]).missing,
            Vec::<String>::new()
        );
        let c = check(T::Bool, vec![b(true), b(false), Pat::Wild]);
        assert_eq!(c.unreachable, [2]);
    }

    #[test]
    fn literals_of_infinite_types_need_a_wildcard() {
        let c = check(T::Int, vec![n(0), n(1), n(-1)]);
        assert_eq!(c.missing, ["_"]);
        let c = check(T::Int, vec![n(0), Pat::Wild]);
        assert!(c.missing.is_empty());
        assert!(c.unreachable.is_empty());
        let c = check(T::Int, vec![n(0), n(0), Pat::Wild]);
        assert_eq!(c.unreachable, [1]);
    }

    #[test]
    fn missing_variants_are_listed_in_declaration_order() {
        let c = check(T::Store, vec![v(0, vec![Pat::Wild])]);
        assert_eq!(c.missing, ["Unavailable"]);
        // No arms: every constructor is missing.
        let c = check(T::Store, vec![]);
        assert_eq!(c.missing, ["Missing(_)", "Unavailable"]);
        let c = check(T::Int, vec![]);
        assert_eq!(c.missing, ["_"]);
    }

    #[test]
    fn nested_witnesses() {
        // match result { Ok(page) => ..  Err(Missing(_)) => .. }
        let ty = T::Res(Box::new(T::Page), Box::new(T::Store));
        let c = check(
            ty,
            vec![v(0, vec![Pat::Wild]), v(1, vec![v(0, vec![Pat::Wild])])],
        );
        assert_eq!(c.missing, ["Err(Unavailable)"]);
        // Option<Option<Bool>> without Some(Some(false)).
        let ty = T::Opt(Box::new(T::Opt(Box::new(T::Bool))));
        let c = check(
            ty.clone(),
            vec![
                v(0, vec![v(0, vec![b(true)])]),
                v(0, vec![v(1, vec![])]),
                v(1, vec![]),
            ],
        );
        assert_eq!(c.missing, ["Some(Some(false))"]);
        // All four arms: exhaustive, nothing unreachable.
        let c = check(
            ty,
            vec![
                v(0, vec![v(0, vec![b(true)])]),
                v(0, vec![v(0, vec![b(false)])]),
                v(0, vec![v(1, vec![])]),
                v(1, vec![]),
            ],
        );
        assert!(c.missing.is_empty());
        assert!(c.unreachable.is_empty());
    }

    #[test]
    fn empty_enums_wrappers_and_structs() {
        // A match with no arms on an enum with no variants is exhaustive.
        assert!(check(T::Never, vec![]).missing.is_empty());
        // A single-arm match on a wrapper is exhaustive.
        let c = check(T::Id, vec![Pat::Ctor(CtorKey::Single, vec![Pat::Wild])]);
        assert!(c.missing.is_empty());
        let c = check(T::Id, vec![]);
        assert_eq!(c.missing, ["PageId(_)"]);
        // A struct is matched only by `_` or a binding; its witness is `_`.
        assert_eq!(check(T::Page, vec![]).missing, ["_"]);
        assert!(check(T::Page, vec![Pat::Wild]).missing.is_empty());
    }

    #[test]
    fn reachability() {
        // _ then Go: Go is unreachable.
        let c = check(T::Store, vec![Pat::Wild, v(1, vec![])]);
        assert_eq!(c.unreachable, [1]);
        // Some(v), None, Some(_): the last is unreachable.
        let ty = T::Opt(Box::new(T::Int));
        let c = check(
            ty,
            vec![v(0, vec![Pat::Wild]), v(1, vec![]), v(0, vec![Pat::Wild])],
        );
        assert_eq!(c.unreachable, [2]);
        assert!(c.missing.is_empty());
    }

    #[test]
    fn erroneous_arms_take_no_part() {
        // Some(_) on a Result (erroneous), Ok(v), Err(_): no warning, no
        // exhaustiveness report.
        let ty = T::Res(Box::new(T::Int), Box::new(T::Int));
        let arms = vec![
            None,
            Some(v(0, vec![Pat::Wild])),
            Some(v(1, vec![Pat::Wild])),
        ];
        let c = check_match(&Toy, &ty, &arms);
        assert!(c.unreachable.is_empty());
        assert!(c.missing.is_empty());
        // Missing(_) then an erroneous Unavailable(_): not reported missing.
        let arms = vec![Some(v(0, vec![Pat::Wild])), None];
        assert!(check_match(&Toy, &T::Store, &arms).missing.is_empty());
    }

    /// A random pattern of type `ty`, `depth` levels deep at most.
    fn random_pat(r: &mut u64, ty: &T, depth: u32) -> Pat {
        let mut next = |n: u64| {
            *r ^= *r << 13;
            *r ^= *r >> 7;
            *r ^= *r << 17;
            *r % n
        };
        if depth == 0 || next(4) == 0 {
            return Pat::Wild;
        }
        match ty {
            T::Int => n(next(6) as i64),
            T::Bool => b(next(2) == 0),
            _ => match Toy.sigma(ty) {
                Some(ctors) if !ctors.is_empty() => {
                    let c = &ctors[next(ctors.len() as u64) as usize];
                    let args = c
                        .fields
                        .iter()
                        .map(|f| random_pat(r, f, depth - 1))
                        .collect();
                    Pat::Ctor(c.key.clone(), args)
                }
                _ => Pat::Wild,
            },
        }
    }

    #[test]
    fn the_incremental_matrix_agrees_with_the_reference() {
        // Arm by arm, the matrix kept between arms answers as `useful` on all
        // the rows before, for random matches over nested types.
        let types = [
            T::Opt(Box::new(T::Int)),
            T::Res(Box::new(T::Opt(Box::new(T::Bool))), Box::new(T::Store)),
            T::Page,
            T::Opt(Box::new(T::Page)),
            T::Id,
            T::Int,
            T::Bool,
        ];
        let mut r = 0x2545_F491_4F6C_DD1Du64;
        for case in 0..3000 {
            let ty = &types[case % types.len()];
            let tys = vec![ty.clone()];
            let mut matrix = Matrix::new(&Toy, tys.clone());
            let mut rows: Vec<Vec<Pat>> = Vec::new();
            for _ in 0..(1 + case % 12) {
                let q = vec![random_pat(&mut r, ty, 3)];
                assert_eq!(
                    matrix.useful(&Toy, &q, &Budget::new(MATCH_STEP_BUDGET)),
                    useful(&Toy, &rows, &q, &tys),
                    "{ty:?}: {q:?} after {rows:?}"
                );
                matrix.push(q.clone());
                rows.push(q);
            }
        }
    }

    #[test]
    fn a_match_past_the_budget_is_too_complex() {
        // Option<Option<...<Bool>>> is small; a budget of a few steps is still too few for it
        let ty = T::Opt(Box::new(T::Opt(Box::new(T::Bool))));
        let some = |p: Pat| Pat::Ctor(CtorKey::Variant(0), vec![p]);
        let arms = vec![Some(some(some(Pat::Ctor(CtorKey::Bool(true), vec![])))), Some(Pat::Wild)];
        let c = check_match_within(&Toy, &ty, &arms, &Budget::new(3));
        assert!(c.too_complex && c.missing.is_empty() && c.unreachable.is_empty(), "{c:?}");
        let c = check_match(&Toy, &ty, &arms);
        assert!(!c.too_complex && c.missing.is_empty(), "{c:?}");
    }
}
