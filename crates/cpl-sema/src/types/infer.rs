//! Inference variables, unification, substitution, and type rendering for
//! the body typer (`spec/semantics.md` 3.3, 3.7, S62).
//!
//! An inference variable is encoded as `Ty::Param(VAR_BASE + n)`: the HIR
//! type has no variable form, and a declaration's own type parameters are
//! numbered from 0, so the two ranges never meet. Variables never leave the
//! typer: every type stored in the HIR is zonked when its declaration is
//! finished, and a variable that is still unbound then (which statement-end
//! processing makes impossible) becomes `Ty::Error`.
//!
//! Unification is structural type identity (3.3): a variable binds to any
//! type, a refinement is a leaf compared by its class (S55), a type
//! parameter is rigid, and the error type unifies with everything, binding
//! the variables it meets to the error type so that no `cannot_infer_type`
//! follows an error (S62). A failed unification binds nothing.

use cpl_diag::Span;
use cpl_hir::{Program, Ty};

/// The first `Ty::Param` index that denotes an inference variable.
pub(crate) const VAR_BASE: u32 = 1 << 30;

/// The variable number of `t`, if it is an inference variable.
pub(crate) fn var_of(t: &Ty) -> Option<u32> {
    match t {
        Ty::Param(i) if *i >= VAR_BASE => Some(*i - VAR_BASE),
        _ => None,
    }
}

/// What the typer knows about one variable.
#[derive(Clone, Debug)]
pub(crate) struct VarInfo {
    pub binding: Option<Ty>,
    /// The expression that created the variable (S56/3.7: the span of
    /// `cannot_infer_type`).
    pub span: Span,
    /// The statement frame that created it (3.7: variables are scoped to
    /// their statement).
    pub frame: u32,
}

/// The inference variables of one declaration.
#[derive(Clone, Debug, Default)]
pub(crate) struct Vars {
    pub info: Vec<VarInfo>,
    trail: Vec<u32>,
}

impl Vars {
    /// A fresh variable created by the expression at `span` in `frame`.
    pub fn fresh(&mut self, span: Span, frame: u32) -> Ty {
        self.info.push(VarInfo {
            binding: None,
            span,
            frame,
        });
        Ty::Param(VAR_BASE + self.info.len() as u32 - 1)
    }

    fn bind(&mut self, v: u32, t: Ty) {
        self.info[v as usize].binding = Some(t);
        self.trail.push(v);
    }

    /// Bind `v` outside a unification (statement-end processing).
    pub fn bind_error(&mut self, v: u32) {
        self.info[v as usize].binding = Some(Ty::Error);
    }

    /// Follow bindings at the top level.
    pub fn resolve(&self, t: &Ty) -> Ty {
        let mut t = t.clone();
        while let Some(v) = var_of(&t) {
            match self.info.get(v as usize).and_then(|i| i.binding.as_ref()) {
                Some(b) => t = b.clone(),
                None => break,
            }
        }
        t
    }

    /// Resolve every position; unbound variables stay.
    pub fn zonk(&self, t: &Ty) -> Ty {
        match self.resolve(t) {
            Ty::List(e) => Ty::List(Box::new(self.zonk(&e))),
            Ty::Gen(e) => Ty::Gen(Box::new(self.zonk(&e))),
            Ty::Adt(d, args) => Ty::Adt(d, args.iter().map(|a| self.zonk(a)).collect()),
            other => other,
        }
    }

    /// [`Vars::zonk`] with every unbound variable replaced by the error
    /// type: the form stored in the HIR.
    pub fn zonk_final(&self, t: &Ty) -> Ty {
        match self.resolve(t) {
            Ty::List(e) => Ty::List(Box::new(self.zonk_final(&e))),
            Ty::Gen(e) => Ty::Gen(Box::new(self.zonk_final(&e))),
            Ty::Adt(d, args) => Ty::Adt(d, args.iter().map(|a| self.zonk_final(a)).collect()),
            t if var_of(&t).is_some() => Ty::Error,
            other => other,
        }
    }

    /// The unbound variables of `t` (after resolution), in order of
    /// appearance, without repetition.
    pub fn unbound(&self, t: &Ty, out: &mut Vec<u32>) {
        match self.resolve(t) {
            Ty::List(e) | Ty::Gen(e) => self.unbound(&e, out),
            Ty::Adt(_, args) => {
                for a in &args {
                    self.unbound(a, out);
                }
            }
            t => {
                if let Some(v) = var_of(&t) {
                    if !out.contains(&v) {
                        out.push(v);
                    }
                }
            }
        }
    }

    pub fn has_unbound(&self, t: &Ty) -> bool {
        let mut out = Vec::new();
        self.unbound(t, &mut out);
        !out.is_empty()
    }

    /// Bind every unbound variable of `t` to the error type (S62: a type
    /// that met an error is no longer something to infer).
    pub fn bind_errors(&mut self, t: &Ty) {
        let mut out = Vec::new();
        self.unbound(t, &mut out);
        for v in out {
            self.bind(v, Ty::Error);
        }
        self.trail.clear();
    }

    /// Unify `a` and `b` (3.3 type identity with variables); on failure
    /// nothing is bound.
    pub fn unify(&mut self, a: &Ty, b: &Ty) -> bool {
        let mark = self.trail.len();
        let ok = self.unify_inner(a, b);
        if !ok {
            for v in self.trail.drain(mark..) {
                self.info[v as usize].binding = None;
            }
        }
        self.trail.truncate(mark);
        ok
    }

    fn occurs(&self, v: u32, t: &Ty) -> bool {
        match self.resolve(t) {
            Ty::List(e) | Ty::Gen(e) => self.occurs(v, &e),
            Ty::Adt(_, args) => args.iter().any(|a| self.occurs(v, a)),
            t => var_of(&t) == Some(v),
        }
    }

    fn bind_errors_trailed(&mut self, t: &Ty) {
        let mut out = Vec::new();
        self.unbound(t, &mut out);
        for v in out {
            self.bind(v, Ty::Error);
        }
    }

    fn unify_inner(&mut self, a: &Ty, b: &Ty) -> bool {
        let a = self.resolve(a);
        let b = self.resolve(b);
        match (var_of(&a), var_of(&b)) {
            (Some(x), Some(y)) => {
                if x != y {
                    // The newer variable points to the older one, so a
                    // group's representative is its oldest member (the
                    // statement-end rule of 3.7 reports it once).
                    if x > y {
                        self.bind(x, b);
                    } else {
                        self.bind(y, a);
                    }
                }
                return true;
            }
            (Some(x), None) => {
                if self.occurs(x, &b) {
                    return false;
                }
                self.bind(x, b);
                return true;
            }
            (None, Some(y)) => {
                if self.occurs(y, &a) {
                    return false;
                }
                self.bind(y, a);
                return true;
            }
            (None, None) => {}
        }
        match (&a, &b) {
            (Ty::Error, other) | (other, Ty::Error) => {
                self.bind_errors_trailed(other);
                true
            }
            (Ty::Int, Ty::Int)
            | (Ty::Bool, Ty::Bool)
            | (Ty::Text, Ty::Text)
            | (Ty::Bytes, Ty::Bytes) => true,
            (Ty::List(x), Ty::List(y)) | (Ty::Gen(x), Ty::Gen(y)) => self.unify_inner(x, y),
            (Ty::Adt(d, xs), Ty::Adt(e, ys)) => {
                d == e
                    && xs.len() == ys.len()
                    && xs.iter().zip(ys).all(|(x, y)| self.unify_inner(x, y))
            }
            (Ty::Wrapper(x), Ty::Wrapper(y)) | (Ty::Capability(x), Ty::Capability(y)) => x == y,
            (Ty::Refined(x, _), Ty::Refined(y, _)) => x == y,
            (Ty::Param(x), Ty::Param(y)) => x == y,
            _ => false,
        }
    }
}

/// Replace a declaration's own type parameters `Param(j)` (`j <
/// VAR_BASE`) by `args[j]` (the instantiation of a callee's, constructor's,
/// or struct's signature, 3.7); a parameter without an argument becomes
/// the error type.
pub(crate) fn subst(t: &Ty, args: &[Ty]) -> Ty {
    match t {
        Ty::Param(j) if *j < VAR_BASE => args.get(*j as usize).cloned().unwrap_or(Ty::Error),
        Ty::List(e) => Ty::List(Box::new(subst(e, args))),
        Ty::Gen(e) => Ty::Gen(Box::new(subst(e, args))),
        Ty::Adt(d, xs) => Ty::Adt(*d, xs.iter().map(|x| subst(x, args)).collect()),
        other => other.clone(),
    }
}

/// True for the types that accept and are accepted by everything (S62):
/// the error type, and a capability, whose every value-position use is the
/// effect stage's `capability_as_value` (S27) and must not also be a type
/// diagnostic.
pub(crate) fn is_poison(t: &Ty) -> bool {
    matches!(t, Ty::Error | Ty::Capability(_))
}

/// True when `t` mentions the error type anywhere.
pub(crate) fn mentions_error(t: &Ty) -> bool {
    match t {
        Ty::Error => true,
        Ty::List(e) | Ty::Gen(e) => mentions_error(e),
        Ty::Adt(_, args) => args.iter().any(mentions_error),
        _ => false,
    }
}

/// CPL type syntax for diagnostics (3.3: "rendered as written"): named
/// types by their declared name (`qualified` gives qualified names, for
/// two distinct types that render alike, S65), type parameters by name,
/// unbound variables as `?` (9.4 `cannot_infer_type`). The error type of
/// S62, which has no spelling of its own, renders as `?` as well: it is a
/// part whose type is unknown because an earlier diagnostic failed (the
/// `expected_type` of a hole whose position came from an erroneous
/// signature, S64; a nested part of a mismatched type).
pub(crate) fn render(program: &Program, t: &Ty, generics: &[String], qualified: bool) -> String {
    let mut out = String::new();
    write(program, &mut out, t, generics, qualified);
    out
}

fn write(program: &Program, out: &mut String, t: &Ty, generics: &[String], qualified: bool) {
    let name = |d: cpl_hir::DefId| -> String {
        if (d.0 as usize) >= program.defs.len() {
            return format!("{{def {}}}", d.0);
        }
        if qualified {
            program.display_path(d)
        } else {
            program.def(d).name.display()
        }
    };
    match t {
        Ty::Int => out.push_str("Int"),
        Ty::Bool => out.push_str("Bool"),
        Ty::Text => out.push_str("Text"),
        Ty::Bytes => out.push_str("Bytes"),
        Ty::List(e) => {
            out.push_str("List<");
            write(program, out, e, generics, qualified);
            out.push('>');
        }
        Ty::Gen(e) => {
            out.push_str("Gen<");
            write(program, out, e, generics, qualified);
            out.push('>');
        }
        Ty::Adt(d, args) => {
            out.push_str(&name(*d));
            if !args.is_empty() {
                out.push('<');
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    write(program, out, a, generics, qualified);
                }
                out.push('>');
            }
        }
        Ty::Wrapper(d) | Ty::Refined(_, d) | Ty::Capability(d) => out.push_str(&name(*d)),
        Ty::Param(i) if *i >= VAR_BASE => out.push('?'),
        Ty::Param(i) => match generics.get(*i as usize) {
            Some(n) => out.push_str(n),
            None => out.push_str(&format!("${i}")),
        },
        Ty::Error => out.push('?'),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpl_diag::FileId;
    use cpl_hir::DefId;

    fn span() -> Span {
        Span::point(FileId(0), 0)
    }

    #[test]
    fn variables_bind_and_roll_back() {
        let mut v = Vars::default();
        let a = v.fresh(span(), 0);
        let b = v.fresh(span(), 0);
        // List<?a> ~ List<Int> binds ?a.
        assert!(v.unify(&Ty::list(a.clone()), &Ty::list(Ty::Int)));
        assert_eq!(v.zonk(&a), Ty::Int);
        // Adt<?b, Text> ~ Adt<Bool, Int> fails and leaves ?b unbound.
        let adt = |x: Ty, y: Ty| Ty::Adt(DefId(1), vec![x, y]);
        assert!(!v.unify(&adt(b.clone(), Ty::Text), &adt(Ty::Bool, Ty::Int)));
        assert!(v.has_unbound(&b));
        // The error type binds what it meets.
        assert!(v.unify(&Ty::Error, &Ty::list(b.clone())));
        assert_eq!(v.zonk(&b), Ty::Error);
    }

    #[test]
    fn occurs_check_and_oldest_representative() {
        let mut v = Vars::default();
        let a = v.fresh(span(), 0);
        let b = v.fresh(span(), 1);
        assert!(!v.unify(&a, &Ty::list(a.clone())));
        assert!(v.unify(&b, &a));
        // The newer ?b points to the older ?a.
        assert_eq!(v.resolve(&b), a);
        assert_eq!(v.zonk_final(&b), Ty::Error);
    }

    #[test]
    fn substitution_replaces_only_parameters() {
        let var = Ty::Param(VAR_BASE + 3);
        let t = Ty::list(Ty::Adt(
            DefId(2),
            vec![Ty::Param(0), var.clone(), Ty::Param(1)],
        ));
        let s = subst(&t, &[Ty::Int, Ty::Text]);
        assert_eq!(s, Ty::list(Ty::Adt(DefId(2), vec![Ty::Int, var, Ty::Text])));
    }
}
