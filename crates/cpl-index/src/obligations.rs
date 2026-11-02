//! Proof obligations of a function (`spec/toolchain.md` 8.1): the
//! enumeration behind `Index::obligations`, whose documentation gives the
//! ids, the numbering in body order ([`crate::walk`]), and the Milestone 1
//! statuses; the recursion test of `termination`; and what the syntax
//! supplies that the HIR does not keep ([`crate::syntax`]: the canonical
//! text of a divisor and the span of an `assert` statement, Section 3.3:
//! statements carry no span).

use crate::query::Index;
use crate::syntax;
use crate::walk::{self, Visitor};
use cpl_diag::json::{Obligation, ObligationStatus};
use cpl_diag::{SourceMap, Span};
use cpl_sema::hir::{Clause, DefId, DefKind, Expr, ExprKind, FnBody, RefKind, Stmt};
use cpl_syntax::ast::{self, BinaryOp, Declaration};
use cpl_syntax::{format_fragment, Fragment, FragmentMode};
use serde_json::json;
use std::collections::BTreeSet;

/// The reason of every obligation Milestone 1 leaves open (S36).
pub(crate) const VERIFIER_UNAVAILABLE: &str = "verifier_unavailable";
/// The reason of a recursive function's termination without a measure.
const NO_MEASURE: &str = "no_measure";

/// The obligations of `def` (Section 8.1; module docs): empty for every
/// declaration that is not a function with a body.
pub(crate) fn of(index: &Index<'_>, def: DefId) -> Vec<Obligation> {
    let program = index.program();
    let d = program.def(def);
    let DefKind::Fn(f) = &d.kind else {
        return Vec::new();
    };
    if !matches!(f.body, FnBody::Block(_)) {
        return Vec::new();
    }
    let sm = program.source_map();
    let decl = program.ast(def);
    let id = index.version(def).id.as_str().to_string();
    let mut out = Vec::new();
    for (i, c) in f.ensures.iter().enumerate() {
        out.push(open(sm, format!("{id}#ensures/{}", i + 1), "ensures", c));
    }
    if let Some(c) = &f.decreases {
        out.push(open(sm, format!("{id}#decreases"), "decreases", c));
    } else if index.is_recursive(def) {
        out.push(Obligation {
            id: format!("{id}#termination"),
            kind: "termination".to_string(),
            clause: String::new(),
            location: sm.location(crate::query::name_span(decl)),
            status: ObligationStatus::Unknown,
            reason: Some(NO_MEASURE.to_string()),
            evidence: None,
            assumptions: Vec::new(),
        });
    }

    let mut sites = Sites::default();
    walk::definition(d, &mut sites);
    for (n, (invariants, decreases)) in sites.loops.iter().enumerate() {
        let n = n + 1;
        for (i, c) in invariants.iter().enumerate() {
            let i = i + 1;
            out.push(open(
                sm,
                format!("{id}#loop/{n}/invariant/{i}/init"),
                "invariant_init",
                c,
            ));
            out.push(open(
                sm,
                format!("{id}#loop/{n}/invariant/{i}/preserve"),
                "invariant_preserve",
                c,
            ));
        }
        if let Some(c) = decreases {
            out.push(open(
                sm,
                format!("{id}#loop/{n}/decreases"),
                "loop_decreases",
                c,
            ));
        }
    }
    for (n, call) in sites.calls.iter().enumerate() {
        for (i, c) in callee_requires(index, call).iter().enumerate() {
            out.push(Obligation {
                id: format!("{id}#call/{}/requires/{}", n + 1, i + 1),
                kind: "call_requires".to_string(),
                clause: c.text.clone(),
                location: sm.location(call.span),
                status: ObligationStatus::Unknown,
                reason: Some(VERIFIER_UNAVAILABLE.to_string()),
                evidence: None,
                assumptions: Vec::new(),
            });
        }
    }
    for (n, division) in sites.divisions.iter().enumerate() {
        out.push(Obligation {
            id: format!("{id}#div/{}", n + 1),
            kind: "division".to_string(),
            clause: format!("{} != 0", divisor_text(sm, decl, division)),
            location: sm.location(division.span),
            status: ObligationStatus::Unknown,
            reason: Some(VERIFIER_UNAVAILABLE.to_string()),
            evidence: None,
            assumptions: Vec::new(),
        });
    }
    for (n, (refine, refined)) in sites.refines.iter().enumerate() {
        let predicate = program
            .def(*refined)
            .kind
            .as_refinement()
            .map(|r| r.predicate.text.clone())
            .unwrap_or_default();
        out.push(Obligation {
            id: format!("{id}#refine/{}", n + 1),
            kind: "refinement".to_string(),
            clause: predicate,
            location: sm.location(refine.span),
            status: ObligationStatus::Proved,
            reason: None,
            evidence: Some(json!({ "by": "literal" })),
            assumptions: Vec::new(),
        });
    }
    for (n, (condition, text)) in sites.asserts.iter().enumerate() {
        let span = assert_span(decl, condition.span).unwrap_or(condition.span);
        out.push(Obligation {
            id: format!("{id}#assert/{}", n + 1),
            kind: "assert".to_string(),
            clause: text.clone(),
            location: sm.location(span),
            status: ObligationStatus::Unknown,
            reason: Some(VERIFIER_UNAVAILABLE.to_string()),
            evidence: None,
            assumptions: Vec::new(),
        });
    }
    for (n, (construction, target, proved)) in sites
        .constructors
        .iter()
        .filter(|(_, target, _)| program.def(*target).kind.checked_predicate().is_some())
        .enumerate()
    {
        let predicate = program.def(*target).kind.checked_predicate().unwrap();
        out.push(Obligation {
            id: format!("{id}#constructor/{}", n + 1),
            kind: "constructor_invariant".into(),
            clause: predicate.predicate.text.clone(),
            location: sm.location(construction.span),
            status: if *proved {
                ObligationStatus::Proved
            } else {
                ObligationStatus::Unknown
            },
            reason: (!proved).then(|| "runtime_check_required".into()),
            evidence: proved.then(|| json!({"by":"bounded_constant_evaluation"})),
            assumptions: Vec::new(),
        });
    }
    out
}

/// Whether the termination of `def` is unknown whatever a verifier proves
/// (semantics 5.4, S36 (b)): a function with a body that is recursive
/// without a header `decreases` (its `termination` obligation), or that
/// has a `while` loop without a `decreases` clause (no `loop_decreases`
/// obligation states that loop's termination). These are the two
/// conditions of the `missing_decreases` warning, which the checker raises
/// for `@prove` functions; `cpl prove` notes them on its
/// `verification_result` for every function it reports on (5.5: "has a
/// note when the function's termination is also unknown").
pub(crate) fn termination_unknown(index: &Index<'_>, def: DefId) -> bool {
    let d = index.program().def(def);
    let DefKind::Fn(f) = &d.kind else {
        return false;
    };
    if !matches!(f.body, FnBody::Block(_)) {
        return false;
    }
    if f.decreases.is_none() && index.is_recursive(def) {
        return true;
    }
    let mut sites = Sites::default();
    walk::definition(d, &mut sites);
    sites.loops.iter().any(|(_, decreases)| decreases.is_none())
}

/// An obligation on a clause the verifier would have to discharge.
fn open(sm: &SourceMap, id: String, kind: &str, clause: &Clause) -> Obligation {
    Obligation {
        id,
        kind: kind.to_string(),
        clause: clause.text.clone(),
        location: sm.location(clause.span),
        status: ObligationStatus::Unknown,
        reason: Some(VERIFIER_UNAVAILABLE.to_string()),
        evidence: None,
        assumptions: Vec::new(),
    }
}

/// The sites of a function in body order.
#[derive(Default)]
struct Sites<'h> {
    /// Every `while`: its invariants and its measure.
    loops: Vec<(&'h [Clause], Option<&'h Clause>)>,
    /// Every `Call` and `CapCall`.
    calls: Vec<&'h Expr>,
    /// Every `/` and `%`.
    divisions: Vec<&'h Expr>,
    /// Every `Refine` coercion with its refinement declaration.
    refines: Vec<(&'h Expr, DefId)>,
    /// Every `assert`: the condition and its canonical text.
    asserts: Vec<(&'h Expr, String)>,
    constructors: Vec<(&'h Expr, DefId, bool)>,
}

impl<'h> Visitor<'h> for Sites<'h> {
    fn expr(&mut self, e: &'h Expr) {
        match &e.kind {
            ExprKind::Call { .. } | ExprKind::CapCall { .. } => self.calls.push(e),
            ExprKind::Binary {
                op: BinaryOp::Div | BinaryOp::Rem,
                ..
            } => self.divisions.push(e),
            ExprKind::Refine { refined, .. } => self.refines.push((e, *refined)),
            ExprKind::StructLit {
                def,
                invariant_proven,
                ..
            } => self.constructors.push((e, *def, *invariant_proven)),
            _ => {}
        }
    }

    fn stmt(&mut self, s: &'h Stmt) {
        match s {
            Stmt::While {
                invariants,
                decreases,
                ..
            } => self.loops.push((invariants, decreases.as_ref())),
            Stmt::Assert { expr, text } => self.asserts.push((expr, text.clone())),
            _ => {}
        }
    }
}

/// The preconditions of the callee of a call site: a function's `requires`
/// clauses, or a capability operation's.
fn callee_requires<'h>(index: &Index<'h>, call: &Expr) -> &'h [Clause] {
    let program = index.program();
    match &call.kind {
        ExprKind::Call { callee, .. } => program
            .def(*callee)
            .kind
            .as_fn()
            .map(|f| f.requires.as_slice())
            .unwrap_or_default(),
        ExprKind::CapCall { capability, op, .. } => program
            .def(*capability)
            .kind
            .as_capability()
            .and_then(|c| c.ops.get(*op as usize))
            .map(|op| op.requires.as_slice())
            .unwrap_or_default(),
        _ => &[],
    }
}

/// The canonical text of a division's divisor, read from the declaration's
/// syntax (canonical text is a rendering of the AST, Section 5.2); the
/// source text of the HIR operand when no AST division has that span.
fn divisor_text(sm: &SourceMap, decl: &Declaration, division: &Expr) -> String {
    let mut found = None;
    syntax::exprs(decl, &mut |e| {
        if found.is_some() {
            return;
        }
        if let ast::ExprKind::Binary(b) = &e.kind {
            if matches!(b.op, BinaryOp::Div | BinaryOp::Rem)
                && (e.span == division.span || b.span == division.span)
            {
                found = Some(clause_text(&b.rhs));
            }
        }
    });
    found.unwrap_or_else(|| match &division.kind {
        ExprKind::Binary { rhs, .. } => sm.text(rhs.span).to_string(),
        _ => String::new(),
    })
}

/// The span of the `assert` statement whose condition has `condition`'s
/// span, from the `assert` keyword.
fn assert_span(decl: &Declaration, condition: Span) -> Option<Span> {
    let mut found = None;
    syntax::stmts(decl, &mut |s| {
        if let ast::Statement::Assert(a) = s {
            if found.is_none() && a.condition.span == condition {
                found = Some(a.span);
            }
        }
    });
    found
}

/// A clause's one-line canonical text (T56): the canonical rendering with
/// every line break and the indentation after it replaced by one space.
pub(crate) fn clause_text(e: &ast::Expr) -> String {
    one_line(&format_fragment(Fragment::Expr(e), FragmentMode::Canonical))
}

/// `text` with every line break and the indentation after it replaced by
/// one space.
pub(crate) fn one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            out.push(' ');
            out.push_str(line.trim_start());
        } else {
            out.push_str(line);
        }
    }
    out
}

impl<'p> Index<'p> {
    /// True when `def` is a function on a cycle of the call graph
    /// (semantics 5.4 "recursive"): the `Call` references of every
    /// declaration, from bodies and clauses alike, except a function's
    /// direct calls of itself from its own clauses, which are
    /// `recursive_contract` (S33) or, for a `@builtin`, axioms (S61).
    pub fn is_recursive(&self, def: DefId) -> bool {
        let program = self.program();
        let mut seen = BTreeSet::new();
        let mut stack = vec![def];
        while let Some(d) = stack.pop() {
            for r in program.references_from(d) {
                if r.kind != RefKind::Call || (r.interface && r.from == r.to) {
                    continue;
                }
                if r.to == def {
                    return true;
                }
                if seen.insert(r.to) {
                    stack.push(r.to);
                }
            }
        }
        false
    }
}
