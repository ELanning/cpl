//! Body typing: the `type` stage (`spec/toolchain.md` 3.1, 3.3, 3.5, 6.5;
//! `spec/semantics.md` 3.4-3.14, 8.1, 8.3).
//!
//! Every function body, test and property body, contract clause (`requires`,
//! `ensures`, `decreases`, loop `invariant` and `decreases`, capability
//! operation clauses), refinement `where` predicate, and property `for`
//! range is typed into the HIR of toolchain 3.3:
//!
//! | Rule | Where |
//! | - | - |
//! | local bidirectional inference (3.7, S19, S20, S56, S62), statement-scoped variables, `cannot_infer_type` | [`infer`], `typer` (frames) |
//! | assignability (3.4) with `Widen`/`Refine`, the compile-time proof rule (3.5, S16, S65) | `typer::Typer::coerce`, [`crate::consteval`] |
//! | expressions (3.8): literals, names, constructors (S10 step 1 via `Resolver::resolve_ctor`), calls with generic instantiation, `check<R>` (3.7), members (3.9, T19), operators (3.6, S17, S18, S21), `?`, holes, `forall`, lists, struct literals, `if`, `match`, generators (S14, T18) | `expr` |
//! | blocks, divergence, statements (3.10, 3.11, S24, S68) | `stmt` |
//! | patterns (3.12) and exhaustiveness (3.13, S25) | `pattern`, [`crate::exhaust`] |
//! | hole reports (8.3, S48) and the `hole`, `hole_type_unknown`, and `hole_in_release` diagnostics (S47, S64) | `holes` |
//! | references (toolchain 6.5, T17): `Call`, `Ctor`, `FieldAccess`, `CapabilityOp`, `Check` | throughout |
//!
//! # Division of labour with the later stages
//!
//! The typer reports the codes of the `type` and `hole` categories (and
//! `hole_in_release`, which replaces `hole` under `--release`, S64). It
//! leaves to the effect stage (`crate::effects`) every code of the `effect`
//! category but one, `using_argument_not_capability` (S28), which only the
//! syntax decides (a bare name or not) and which the typer reports while it
//! matches `using` arguments by position; it leaves to the contract stage
//! (`crate::contracts`) every code of the `contract` category. The HIR it
//! builds is what those stages walk (`crate::bodies::visit_def` gives each
//! expression its contract clause and position):
//!
//! * A `using` parameter in any position other than an operation receiver
//!   (`ExprKind::CapCall::handle`) or a `using` argument (`Call::using`)
//!   is an `ExprKind::Local` whose `ty` is `Ty::Capability` (S27's
//!   `capability_as_value`; an operand of `==`/`!=` is
//!   `capability_equality`). A call whose callee is a handle (`db(x)`) is
//!   that `Local` alone. The typer accepts such an expression everywhere
//!   without a diagnostic (it is "poison", like the error type).
//! * `Call::using[j]` is `ExprKind::Local` for a bare name (its type says
//!   whether it is a handle), `ExprKind::Error` for a name that did not
//!   resolve (already reported), and any other expression, typed without
//!   diagnostics, for a `using` argument that is not a bare name (already
//!   `using_argument_not_capability`, also when it is a handle in another
//!   shape: `(db)` and `db(key)` type as the handle's `Local`, so the
//!   effect stage tells bare arguments apart by their AST). In a contract
//!   expression every `using` argument is typed without diagnostics and is
//!   the typer's `capability_as_value` alone, whatever its form (5.1).
//! * Holes in contract expressions get a `HoleId` but no report (S64:
//!   `hole_in_contract` alone, or, for a hole in a `using` argument, that
//!   argument's `capability_as_value`); `?`, `return`, and the statements
//!   of S32 inside contract expressions are typed but not checked against
//!   the enclosing function.
//!
//! The typed predicate of every refinement is computed on demand (the
//! proof rule needs it while another declaration is typed) and cached in
//! [`TypeCx`].

mod expr;
mod holes;
pub(crate) mod infer;
mod pattern;
mod stmt;
mod typer;

use crate::check::HoleReport;
use crate::checker::{Checker, Stage};
use cpl_diag::Diagnostic;
use cpl_hir::{
    Binder, Block, DefId, DefKind, Expr, FnBody, HoleId, LocalDef, LocalId, RefKind, Reference,
};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;

/// Type every body, clause, predicate, and range (see the module docs),
/// then report the holes (S47, S48, S64).
pub(crate) fn type_bodies(cx: &mut Checker<'_>) {
    let outputs = {
        let tcx = TypeCx::new(cx);
        let mut outputs: Vec<(DefId, DefOutput)> = Vec::new();
        for index in 0..cx.program.defs.len() {
            let def = DefId(index as u32);
            match &cx.program.def(def).kind {
                DefKind::Refinement(_) => {
                    tcx.predicate(def);
                }
                DefKind::Struct(s) if s.invariant.is_some() => {
                    tcx.predicate(def);
                }
                DefKind::Fn(_)
                | DefKind::Test(_)
                | DefKind::Property(_)
                | DefKind::Capability(_) => {
                    outputs.push((def, tcx.type_def(def)));
                }
                _ => {}
            }
        }
        outputs.extend(tcx.take_predicates());
        outputs.sort_by_key(|(d, _)| *d);
        outputs
    };
    let mut holes: Vec<(HoleReport, holes::HoleSite, Diagnostic)> = Vec::new();
    for (def, out) in outputs {
        apply(cx, def, out, &mut holes);
    }
    holes::report(cx, holes);
}

/// The shared state of one typing run.
pub(crate) struct TypeCx<'c, 'a> {
    pub cx: &'c Checker<'a>,
    /// The typed predicate of every refinement typed so far.
    predicates: RefCell<HashMap<DefId, Rc<Predicate>>>,
    /// The refinements whose predicate is being typed (a proof that needs
    /// one of them is a cycle).
    in_progress: RefCell<BTreeSet<DefId>>,
    /// The next `HoleId` (per program, toolchain 3.3).
    next_hole: Cell<u32>,
}

/// The typed `where` predicate of one refinement, and the output of typing
/// it.
pub(crate) struct Predicate {
    pub expr: Expr,
    pub self_local: LocalId,
    /// The predicate typed without an error: the proof rule may evaluate it.
    pub ok: bool,
    pub output: RefCell<Option<DefOutput>>,
}

impl<'c, 'a> TypeCx<'c, 'a> {
    fn new(cx: &'c Checker<'a>) -> TypeCx<'c, 'a> {
        TypeCx {
            cx,
            predicates: RefCell::new(HashMap::new()),
            in_progress: RefCell::new(BTreeSet::new()),
            next_hole: Cell::new(0),
        }
    }

    pub fn hole_id(&self) -> HoleId {
        let id = self.next_hole.get();
        self.next_hole.set(id + 1);
        HoleId(id)
    }

    /// The typed predicate of refinement `def`, typing it now if needed;
    /// `None` while it is being typed (a cyclic proof) or for a definition
    /// that is not a refinement.
    pub fn predicate(&self, def: DefId) -> Option<Rc<Predicate>> {
        if let Some(p) = self.predicates.borrow().get(&def) {
            return Some(p.clone());
        }
        self.cx.program.def(def).kind.checked_predicate()?;
        if !self.in_progress.borrow_mut().insert(def) {
            return None;
        }
        let output = typer::type_refinement(self, def);
        self.in_progress.borrow_mut().remove(&def);
        let (expr, self_local) = match &output.kind {
            KindOutput::Refinement { predicate, .. } => (
                predicate.clone(),
                self.cx
                    .program
                    .def(def)
                    .kind
                    .checked_predicate()
                    .map_or(LocalId(0), |r| r.self_local),
            ),
            _ => unreachable!("a refinement's output"),
        };
        let ok = !output.failed;
        let p = Rc::new(Predicate {
            expr,
            self_local,
            ok,
            output: RefCell::new(Some(output)),
        });
        self.predicates.borrow_mut().insert(def, p.clone());
        Some(p)
    }

    fn take_predicates(&self) -> Vec<(DefId, DefOutput)> {
        let mut out = Vec::new();
        for (def, p) in self.predicates.borrow().iter() {
            if let Some(o) = p.output.borrow_mut().take() {
                out.push((*def, o));
            }
        }
        out
    }

    fn type_def(&self, def: DefId) -> DefOutput {
        match &self.cx.program.def(def).kind {
            DefKind::Fn(_) => typer::type_fn(self, def),
            DefKind::Test(_) => typer::type_test(self, def),
            DefKind::Property(_) => typer::type_property(self, def),
            DefKind::Capability(_) => typer::type_capability(self, def),
            _ => DefOutput::default(),
        }
    }
}

/// What typing one definition produced.
#[derive(Default)]
pub(crate) struct DefOutput {
    pub kind: KindOutput,
    pub diags: Vec<(Stage, Diagnostic)>,
    pub refs: Vec<Reference>,
    /// Complete hole reports except `dependencies`, with where each hole
    /// stands and its development-mode `hole` diagnostic.
    pub holes: Vec<(HoleReport, holes::HoleSite, Diagnostic)>,
    /// Some diagnostic of the definition is an error.
    pub failed: bool,
}

/// The typed parts of one definition.
#[derive(Default)]
pub(crate) enum KindOutput {
    #[default]
    Nothing,
    Fn {
        requires: Vec<Expr>,
        ensures: Vec<Expr>,
        decreases: Option<Expr>,
        body: Option<Block>,
        locals: Vec<LocalDef>,
    },
    Refinement {
        predicate: Expr,
        locals: Vec<LocalDef>,
    },
    Capability {
        /// Per operation: its `requires`, its `ensures`, its local table.
        ops: Vec<(Vec<Expr>, Vec<Expr>, Vec<LocalDef>)>,
    },
    Test {
        body: Block,
        locals: Vec<LocalDef>,
    },
    Property {
        binders: Vec<Binder>,
        body: Block,
        locals: Vec<LocalDef>,
    },
}

/// Store one definition's typed parts, diagnostics, references, and holes.
fn apply(
    cx: &mut Checker<'_>,
    def: DefId,
    out: DefOutput,
    holes: &mut Vec<(HoleReport, holes::HoleSite, Diagnostic)>,
) {
    let program = &mut cx.program;
    match (&mut program.defs[def.0 as usize].kind, out.kind) {
        (
            DefKind::Fn(f),
            KindOutput::Fn {
                requires,
                ensures,
                decreases,
                body,
                locals,
            },
        ) => {
            for (c, e) in f.requires.iter_mut().zip(requires) {
                c.expr = e;
            }
            for (c, e) in f.ensures.iter_mut().zip(ensures) {
                c.expr = e;
            }
            if let (Some(c), Some(e)) = (&mut f.decreases, decreases) {
                c.expr = e;
            }
            if let (FnBody::Block(b), Some(typed)) = (&mut f.body, body) {
                *b = typed;
            }
            f.locals = locals;
        }
        (DefKind::Refinement(r), KindOutput::Refinement { predicate, locals }) => {
            r.predicate.expr = predicate.clone();
            r.locals = locals;
            // The class's predicate is its first member's (refine module docs).
            if let Some(class) = program
                .refinement_classes
                .iter_mut()
                .find(|c| c.members.first() == Some(&def))
            {
                class.predicate = predicate;
            }
        }
        (DefKind::Struct(s), KindOutput::Refinement { predicate, locals }) => {
            if let Some(inv) = &mut s.invariant {
                inv.predicate.expr = predicate;
                inv.locals = locals;
            }
        }
        (DefKind::Capability(c), KindOutput::Capability { ops }) => {
            for (op, (requires, ensures, locals)) in c.ops.iter_mut().zip(ops) {
                for (cl, e) in op.requires.iter_mut().zip(requires) {
                    cl.expr = e;
                }
                for (cl, e) in op.ensures.iter_mut().zip(ensures) {
                    cl.expr = e;
                }
                op.locals = locals;
            }
        }
        (DefKind::Test(t), KindOutput::Test { body, locals }) => {
            t.body = body;
            t.locals = locals;
        }
        (
            DefKind::Property(p),
            KindOutput::Property {
                binders,
                body,
                locals,
            },
        ) => {
            p.binders = binders;
            p.body = body;
            p.locals = locals;
        }
        _ => {}
    }
    program.references.extend(out.refs);
    for (stage, d) in out.diags {
        cx.emit(stage, d);
    }
    if out.failed {
        cx.fail(def);
    }
    if out.holes.is_empty() {
        return;
    }
    // A hole's dependencies: every definition its declaration references,
    // itself included when it is recursive (toolchain 1.6, as
    // `Index::dependencies`, Section 6.5), sorted by qualified name.
    let mut deps: BTreeMap<String, DefId> = BTreeMap::new();
    for r in &cx.program.references {
        if r.from == def {
            deps.entry(cx.program.display_path(r.to)).or_insert(r.to);
        }
    }
    let deps: Vec<DefId> = deps.into_values().collect();
    for (mut report, site, d) in out.holes {
        report.dependencies = deps.clone();
        holes.push((report, site, d));
    }
}

/// A reference recorded by the typer.
pub(crate) fn reference(
    from: DefId,
    to: DefId,
    kind: RefKind,
    span: cpl_diag::Span,
    interface: bool,
) -> Reference {
    Reference {
        from,
        to,
        kind,
        span,
        interface,
    }
}
