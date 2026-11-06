//! Which reads of a variable the Rust printer may print as a move instead of a clone.
//!
//! A CPL value is immutable and a Rust value has one owner, so the Rust printer clones a
//! variable it reads by value (an `Arc` count, a refcount on a text or a list). A read after
//! which the variable is never read again can move it instead. The analysis is liveness over
//! the structured statements of a body, backwards, with a fixpoint at each loop. A read is a
//! move when its variable is dead after the statement that reads it (after the expression,
//! for a condition), and the statement reads the variable only there, or only in fields that
//! do not overlap (`f(x.a, x.b)` moves both): no borrow of the variable can then be alive
//! when it moves. A variable a `for` loop iterates is borrowed by the loop and never moved
//! in its body.
//!
//! The result is a set of expression nodes by address: a `Var`, or the outermost `Field` or
//! `Unwrap` of a chain over a `Var` (`x.a.b`); and the match arms after which the scrutinee is
//! dead, whose bindings may move out of it. The printer still decides whether it can move one
//! (not out of an `Arc`, not through a nested pattern).

use crate::lir::*;
use std::collections::{BTreeMap, BTreeSet, HashSet};

type Live = BTreeSet<Var>;

/// The reads of each variable in an expression: the node, and the field path of a read of a
/// field (`None` for the whole variable).
type Reads = BTreeMap<Var, Vec<(usize, Option<Vec<u32>>)>>;

/// The movable reads of a body whose end nothing reads after.
pub fn movable(body: &[LStmt]) -> HashSet<usize> {
    let mut a = Analysis {
        marks: HashSet::new(),
        marking: true,
        labels: BTreeMap::new(),
        borrowed: Vec::new(),
    };
    a.stmts(body, &Live::new());
    a.marks
}

/// The variables `stmts` read before they assign them.
pub fn read_before_written(stmts: &[LStmt]) -> BTreeSet<Var> {
    let mut a = Analysis {
        marks: HashSet::new(),
        marking: false,
        labels: BTreeMap::new(),
        borrowed: Vec::new(),
    };
    a.stmts(stmts, &Live::new())
}

/// The address of a match arm, its identity in the set: an arm that may move its bindings out
/// of the scrutinee.
pub fn arm_site(a: &LArm) -> usize {
    a as *const LArm as usize
}

/// The address of an expression node, its identity in the set.
pub fn site(e: &LExpr) -> usize {
    e as *const LExpr as usize
}

struct Analysis {
    marks: HashSet<usize>,
    /// Off while a loop's fixpoint is still growing its live sets.
    marking: bool,
    /// What is live after each enclosing loop, for its `Break`s.
    labels: BTreeMap<u32, Live>,
    /// The variables enclosing `for` loops iterate.
    borrowed: Vec<Var>,
}

impl Analysis {
    fn stmts(&mut self, stmts: &[LStmt], out: &Live) -> Live {
        let mut live = out.clone();
        for s in stmts.iter().rev() {
            live = self.stmt(s, &live);
        }
        live
    }

    fn stmt(&mut self, s: &LStmt, out: &Live) -> Live {
        match s {
            LStmt::Let { var, value } | LStmt::Assign { var, value } => {
                let mut after = out.clone();
                after.remove(var);
                self.reads(value, &after, None);
                uses(value, &mut after);
                after
            }
            LStmt::Decl { var } => {
                let mut live = out.clone();
                live.remove(var);
                live
            }
            LStmt::Expr(e) => {
                self.reads(e, out, None);
                let mut live = out.clone();
                uses(e, &mut live);
                live
            }
            LStmt::Return(e) => {
                let mut live = Live::new();
                self.reads(e, &live, None);
                uses(e, &mut live);
                live
            }
            LStmt::Fault(_) => Live::new(),
            LStmt::Break { label } => self.labels.get(label).cloned().unwrap_or_default(),
            LStmt::Push { list, value } => {
                // the list is updated after the value is computed
                let mut after = out.clone();
                after.insert(*list);
                self.reads(value, &after, Some(*list));
                uses(value, &mut after);
                after
            }
            LStmt::ClauseEnter { .. } | LStmt::ClauseLeave => out.clone(),
            LStmt::If { cond, then, els } => {
                let mut after = self.stmts(then, out);
                after.extend(self.stmts(els, out));
                self.reads(cond, &after, None);
                uses(cond, &mut after);
                after
            }
            LStmt::Match { scrutinee, arms } => {
                let mut live = Live::new();
                for arm in arms {
                    let mut l = self.stmts(&arm.body, out);
                    // an arm after which the scrutinee is dead may bind by moving out of it
                    if self.marking && !l.contains(scrutinee) && !self.borrowed.contains(scrutinee)
                    {
                        self.marks.insert(arm_site(arm));
                    }
                    binds(&arm.pattern, &mut l);
                    live.extend(l);
                }
                live.insert(*scrutinee);
                live
            }
            LStmt::Loop { label, body } => {
                self.labels.insert(*label, out.clone());
                // the body's end goes back to its start
                let head = self.fixpoint(|a, head| a.stmts(body, head));
                self.labels.remove(label);
                head
            }
            LStmt::ForEach {
                label,
                var,
                list,
                body,
            } => {
                self.labels.insert(*label, out.clone());
                let mut iterated = Live::new();
                uses(list, &mut iterated);
                let depth = self.borrowed.len();
                self.borrowed.extend(iterated.iter().copied());
                // before each element: what is live after the loop, or in the next element
                let head = self.fixpoint(|a, head| {
                    let mut h = a.stmts(body, head);
                    h.remove(var);
                    h.extend(out.iter().copied());
                    h
                });
                self.borrowed.truncate(depth);
                self.labels.remove(label);
                let mut live = head;
                uses(list, &mut live);
                live
            }
        }
    }

    /// The least `head` with `step(head) == head`, then one more step that marks.
    fn fixpoint(&mut self, mut step: impl FnMut(&mut Self, &Live) -> Live) -> Live {
        let marking = std::mem::replace(&mut self.marking, false);
        let mut head = Live::new();
        loop {
            let next = step(self, &head);
            if next == head {
                break;
            }
            head = next;
        }
        self.marking = marking;
        if marking {
            head = step(self, &head);
        }
        head
    }

    /// Mark the reads of `e` that may move, `after` being live once `e` is evaluated; `extra`
    /// is a variable the statement reads besides `e`.
    fn reads(&mut self, e: &LExpr, after: &Live, extra: Option<Var>) {
        if !self.marking {
            return;
        }
        let mut found = Reads::new();
        occurrences(e, &mut found);
        for (v, occ) in found {
            if after.contains(&v) || self.borrowed.contains(&v) || extra == Some(v) {
                continue;
            }
            let disjoint = occ.len() == 1
                || occ.iter().enumerate().all(|(i, (_, a))| {
                    let Some(a) = a else { return false };
                    occ.iter().enumerate().all(|(j, (_, b))| {
                        i == j
                            || b.as_ref()
                                .is_some_and(|b| !a.starts_with(b) && !b.starts_with(a))
                    })
                });
            if disjoint {
                self.marks.extend(occ.iter().map(|(s, _)| *s));
            }
        }
    }
}

/// The variables an expression reads.
fn uses(e: &LExpr, live: &mut Live) {
    visit_expr(e, &mut |x| {
        if let LExpr::Var(v) = x {
            live.insert(*v);
        }
    });
}

/// Remove the variables a pattern binds.
fn binds(p: &LPat, live: &mut Live) {
    match p {
        LPat::Bind(v) => {
            live.remove(v);
        }
        LPat::Ctor { args, .. } => args.iter().for_each(|a| binds(a, live)),
        LPat::Unwrap { arg, .. } => binds(arg, live),
        LPat::Wild | LPat::Int(_) | LPat::Text(_) | LPat::Bool(_) => {}
    }
}

/// The field path of a chain of `Field` and `Unwrap` over a variable (`x.a.b` is `x`, [a, b];
/// an `Unwrap` is field 0).
pub fn chain(e: &LExpr) -> Option<(Var, Vec<u32>)> {
    match e {
        LExpr::Var(v) => Some((*v, Vec::new())),
        LExpr::Field { base, index, .. } => {
            let (v, mut path) = chain(base)?;
            path.push(*index);
            Some((v, path))
        }
        LExpr::Unwrap { base, .. } => {
            let (v, mut path) = chain(base)?;
            path.push(0);
            Some((v, path))
        }
        _ => None,
    }
}

/// Every read of a variable in `e`: the node, and the field path when it reads a field.
fn occurrences(e: &LExpr, found: &mut Reads) {
    match e {
        LExpr::Var(v) => found.entry(*v).or_default().push((site(e), None)),
        LExpr::Field { .. } | LExpr::Unwrap { .. } if chain(e).is_some() => {
            let (v, path) = chain(e).expect("a chain");
            found.entry(v).or_default().push((site(e), Some(path)));
        }
        _ => {
            let mut children = Vec::new();
            match e {
                LExpr::Call { args, using, .. } => children.extend(args.iter().chain(using)),
                LExpr::Builtin { args, .. } | LExpr::Op { args, .. } | LExpr::Ctor { args, .. } => {
                    children.extend(args)
                }
                LExpr::CapCall { handle, args, .. } => {
                    children.push(handle);
                    children.extend(args);
                }
                LExpr::Struct { fields, .. } => children.extend(fields),
                LExpr::List { items, .. } => children.extend(items),
                LExpr::Div { a, b, .. } => children.extend([a.as_ref(), b.as_ref()]),
                LExpr::Wrap { arg, .. } | LExpr::Pred { arg, .. } | LExpr::Valid { arg, .. } => {
                    children.push(arg)
                }
                LExpr::Field { base, .. }
                | LExpr::Payload { base, .. }
                | LExpr::IsVariant { base, .. }
                | LExpr::Unwrap { base, .. } => children.push(base),
                LExpr::CheckFailure { value, .. } => children.push(value),
                LExpr::Var(_) | LExpr::Int(_) | LExpr::Text(_) | LExpr::Bool(_) | LExpr::Unit => {}
            }
            for c in children {
                // a variable under a payload or a variant test is read whole
                occurrences(c, found);
            }
        }
    }
}
