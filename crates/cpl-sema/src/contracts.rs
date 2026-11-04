//! Contract checking: the `contract` stage (`spec/toolchain.md` 3.1, 3.4;
//! `spec/semantics.md` 5.1, 5.4, 5.5, S32, S33, S35, S36, S61, S63, S64).
//!
//! | Rule | Diagnostic | Span (S63) |
//! | - | - | - |
//! | no hole in a contract expression (5.1, S64: the error alone) | `hole_in_contract` | the hole |
//! | no `?` in a contract expression (5.1; it takes precedence over `propagate_outside_function`) | `propagate_in_contract` | the `?` expression |
//! | a block inside a contract expression holds only `let` statements and a tail (S32) | `statement_in_contract` | the statement |
//! | no capability operation call in a contract expression (4.5) | `effect_in_contract` | the clause |
//! | a clause of a declaration does not call the declaration itself, unless it is a `@builtin` (S33, S61) | `recursive_contract` | the clause |
//! | a header `decreases` only on a recursive function (5.4) | `unused_decreases` (W) | the clause |
//! | a recursive `@prove` function has a header `decreases`, and every `while` of a `@prove` body has one (5.4) | `missing_decreases` (W) | the function's name, or the loop |
//! | every `@prove` function has a verification status (S36) | `verification_result` (I) | the function's name |
//!
//! The rules about what a contract expression *writes* (holes, `?`,
//! statements, self-calls) and the loops of 5.4 read the AST
//! ([`crate::ast_walk`]) with each callee's resolution
//! (`Checker::names`), so they hold whether or not the expression could
//! be typed; `effect_in_contract` needs to know which names are handles
//! and reads the typed HIR ([`crate::bodies::visit_def`]); recursion is a
//! cycle of the call graph `Checker::calls`. `private_in_public_contract`
//! (S4, category `resolve`) and the S9 names (`result_outside_ensures`,
//! `self_outside_where`) are reported by the resolution walk
//! (`crate::names`), the `Bool`/`Int` clause types by the typer, and so is
//! a `using` argument in a contract (`capability_as_value`, 5.1, in the
//! effect stage: the only report for that argument, so a hole in it is
//! not also `hole_in_contract`, S64). `effect_in_contract` is reported
//! here (toolchain 3.4) under its 9.4 category, `effect`.
//!
//! The contract diagnostics carry `symbol` (with `qualified_name`) and
//! `facet: "contract"`; those about a clause carry `clause`, the
//! canonical text of the clause's expression (9.2), and
//! `effect_in_contract` adds `effect` (9.4). A clause of a capability
//! operation belongs to the capability's `contract` facet (toolchain 5.1),
//! so its `symbol` is the capability.
//!
//! # Verification statuses (5.5, S36)
//!
//! Milestone 1 has no verifier: every `@prove` function's status is
//! `unknown` with `reason: "verifier_unavailable"`, one
//! [`VerificationStatus`] in `CheckResult::verification` and one
//! `verification_result` (`facet: "verification_policy"`, `status`,
//! `reason`) per declaration, in declaration order. A function with a
//! `missing_decreases` warning is additionally unknown for termination,
//! which its `verification_result` states in a note. The S59 `receipt`
//! needs the checkout's revision and the per-module content hashes of
//! `cpl-index`, so it is added by the caller, as for `test_result` records
//! and the hole report's `dependencies` (see the report's readings).

use crate::ast_walk::{self, ContractClause, SyntaxVisitor};
use crate::bodies::{visit_def, Site, Visitor};
use crate::check::{ProofStatus, VerificationStatus};
use crate::checker::{blame, diag, Checker, Stage};
use crate::codes;
use crate::resolve::Res;
use cpl_diag::{Diagnostic, Facet, Span};
use cpl_hir::{BindingKind, Def, DefId, DefKind, Expr, ExprKind, Ty};
use cpl_syntax::ast;
use serde_json::json;
use std::collections::BTreeSet;

/// The reason of every Milestone 1 verification status (S36).
const VERIFIER_UNAVAILABLE: &str = "verifier_unavailable";

/// Check every declaration's contracts (see the module docs).
pub(crate) fn check_contracts(cx: &mut Checker<'_>) {
    let recursive = recursive_functions(cx);
    let mut diags: Vec<Diagnostic> = Vec::new();
    let mut failed: BTreeSet<DefId> = BTreeSet::new();
    let mut verification: Vec<VerificationStatus> = Vec::new();
    for def in cx.program.defs() {
        let (module, index) = cx.decl_ast(def.id);
        let decl = &module.declarations[index];
        let mut syntax = Syntax {
            cx,
            def: def.id,
            builtin: cx.decls[def.id.0 as usize].annots.builtin.is_some(),
            using_args: Vec::new(),
            reported: Vec::new(),
            recursive: BTreeSet::new(),
            loops: Vec::new(),
            diags: Vec::new(),
        };
        ast_walk::visit_decl(cx, decl, &mut syntax);
        let mut hir = Hir {
            cx,
            def,
            effects: BTreeSet::new(),
            diags: Vec::new(),
        };
        visit_def(def, &mut hir);
        let mut out = syntax.diags;
        out.append(&mut hir.diags);
        if let DefKind::Fn(f) = &def.kind {
            let is_recursive = recursive.contains(&def.id);
            if let Some(clause) = &f.decreases {
                if !is_recursive {
                    let d = diag(
                        codes::UNUSED_DECREASES,
                        Some(clause.span),
                        format!(
                            "`{}` is not recursive, so its `decreases` measure is never checked",
                            def.name.display()
                        ),
                    );
                    out.push(blame(d, cx, def.id, Some(Facet::Contract)));
                }
            }
            if def.policy.prove {
                let name_span = cx.decls[def.id.0 as usize].name_span;
                let mut termination_unknown = false;
                if is_recursive && f.decreases.is_none() {
                    let d = diag(
                        codes::MISSING_DECREASES,
                        Some(name_span),
                        format!(
                            "`@prove` function `{}` is recursive but has no `decreases` measure, so its termination cannot be proved",
                            def.name.display()
                        ),
                    );
                    out.push(blame(d, cx, def.id, Some(Facet::Contract)));
                    termination_unknown = true;
                }
                for span in &syntax.loops {
                    let d = diag(
                        codes::MISSING_DECREASES,
                        Some(*span),
                        format!(
                            "this loop of `@prove` function `{}` has no `decreases` measure, so its termination cannot be proved",
                            def.name.display()
                        ),
                    );
                    out.push(blame(d, cx, def.id, Some(Facet::Body)));
                    termination_unknown = true;
                }
                let mut d = diag(
                    codes::VERIFICATION_RESULT,
                    Some(name_span),
                    format!("{}: unknown (verifier unavailable)", def.name.display()),
                )
                .with_data("status", json!("unknown"))
                .with_data("reason", json!(VERIFIER_UNAVAILABLE));
                if termination_unknown {
                    d = d.with_note(
                        "termination is unknown as well: a `decreases` measure is missing (`missing_decreases`)",
                        None,
                    );
                }
                out.push(blame(d, cx, def.id, Some(Facet::VerificationPolicy)));
                verification.push(VerificationStatus {
                    def: def.id,
                    status: ProofStatus::Unknown,
                    reason: Some(VERIFIER_UNAVAILABLE.to_string()),
                });
            }
        }
        if out.iter().any(Diagnostic::is_error) {
            failed.insert(def.id);
        }
        diags.append(&mut out);
    }
    for def in failed {
        cx.fail(def);
    }
    cx.verification = verification;
    cx.emit_all(Stage::Contract, diags);
}

/// The functions on a cycle of the call graph (5.4: "recursive"), whose
/// edges are every function each declaration calls by name, from its body
/// and its clauses (`Checker::calls`: a call in a clause runs in checked
/// mode, so a cycle through one does not terminate either), except a
/// direct self-call from the function's own clauses: that is
/// `recursive_contract` (or, for a `@builtin`, an axiom, S61), and with it
/// fixed the function is not recursive, so it decides no `decreases`
/// warning (S62).
fn recursive_functions(cx: &Checker<'_>) -> BTreeSet<DefId> {
    let mut edges = cx.calls.clone();
    for (def, callees) in edges.iter_mut() {
        if callees.contains(def) && !calls_itself_outside_clauses(cx, *def) {
            callees.remove(def);
        }
    }
    let nodes: Vec<DefId> = edges.keys().copied().collect();
    let mut recursive = BTreeSet::new();
    for scc in crate::refine::tarjan(&nodes, &edges) {
        if scc.len() > 1 || edges[&scc[0]].contains(&scc[0]) {
            recursive.extend(scc);
        }
    }
    recursive
}

/// The function a call names, as the resolution walk resolved its callee
/// (the resolution `Checker::calls` records).
fn callee(cx: &Checker<'_>, c: &ast::CallExpr) -> Option<DefId> {
    match cx.names.exprs.get(&c.callee.span) {
        Some(Res::Fn(d)) => Some(*d),
        _ => None,
    }
}

/// Whether a definition's body (not a contract expression of it) calls
/// the definition itself.
fn calls_itself_outside_clauses(cx: &Checker<'_>, def: DefId) -> bool {
    struct SelfCall<'c, 'a> {
        cx: &'c Checker<'a>,
        def: DefId,
        found: bool,
    }
    impl<'x> SyntaxVisitor<'x> for SelfCall<'_, '_> {
        fn expr(&mut self, e: &'x ast::Expr, clause: Option<ContractClause<'x>>) {
            if let (None, ast::ExprKind::Call(c)) = (clause, &e.kind) {
                self.found |= callee(self.cx, c) == Some(self.def);
            }
        }
    }
    let (module, index) = cx.decl_ast(def);
    let mut v = SelfCall {
        cx,
        def,
        found: false,
    };
    ast_walk::visit_decl(cx, &module.declarations[index], &mut v);
    v.found
}

/// The canonical text of a clause (9.2 `clause`).
fn clause_text(c: &ContractClause<'_>) -> String {
    crate::collect::clause_text(c.expr)
}

/// `a` contains `b` (both in one file).
fn contains(a: Span, b: Span) -> bool {
    a.file == b.file && a.start <= b.start && b.end <= a.end
}

/// The syntactic rules of 5.1 and S33 and the loops of 5.4 for one
/// declaration.
struct Syntax<'c, 'a> {
    cx: &'c Checker<'a>,
    def: DefId,
    /// A `@builtin` declaration: exempt from `recursive_contract` (S61).
    builtin: bool,
    /// The `using` arguments of the calls in contract expressions met so
    /// far (pre-order: a call comes before its arguments). Each is the
    /// typer's `capability_as_value` alone (5.1), which is also the one
    /// report of a hole in it (S64).
    using_args: Vec<Span>,
    /// The statements reported `statement_in_contract`: a statement inside
    /// one is part of the same mistake (S62).
    reported: Vec<Span>,
    /// The clauses already reported `recursive_contract` (one per clause).
    recursive: BTreeSet<Span>,
    /// The `while` loops of the bodies (not of contract expressions)
    /// without a `decreases` clause.
    loops: Vec<Span>,
    diags: Vec<Diagnostic>,
}

impl Syntax<'_, '_> {
    /// Attach `clause` and blame the declaration's contract facet.
    fn contract(&mut self, d: Diagnostic, clause: &ContractClause<'_>) {
        let d = d.with_data("clause", json!(clause_text(clause)));
        self.diags
            .push(blame(d, self.cx, self.def, Some(Facet::Contract)));
    }
}

impl<'a> SyntaxVisitor<'a> for Syntax<'_, '_> {
    fn expr(&mut self, e: &'a ast::Expr, clause: Option<ContractClause<'a>>) {
        let Some(clause) = clause else { return };
        if let ast::ExprKind::Call(c) = &e.kind {
            self.using_args
                .extend(c.args.using.iter().map(|u| u.expr.span));
        }
        match &e.kind {
            ast::ExprKind::Hole(_) if self.using_args.iter().any(|u| contains(*u, e.span)) => {
                // 5.1: the `using` argument's `capability_as_value` is this
                // hole's one report (S64).
            }
            ast::ExprKind::Hole(n) => {
                let d = diag(
                    codes::HOLE_IN_CONTRACT,
                    Some(e.span),
                    format!(
                        "a contract expression cannot contain the hole `?{}`: contracts are never unfinished",
                        n.text
                    ),
                )
                .with_data("hole", json!(n.text));
                self.contract(d, &clause);
            }
            ast::ExprKind::Propagate(_) => {
                let d = diag(
                    codes::PROPAGATE_IN_CONTRACT,
                    Some(e.span),
                    "`?` cannot appear in a contract expression: a contract is evaluated, it never returns early; match the `Result` instead",
                );
                self.contract(d, &clause);
            }
            ast::ExprKind::Call(c) => {
                if self.builtin
                    || callee(self.cx, c) != Some(self.def)
                    || !self.recursive.insert(clause.span)
                {
                    return;
                }
                let name = self.cx.program.def(self.def).name.display();
                let d = diag(
                    codes::RECURSIVE_CONTRACT,
                    Some(clause.span),
                    format!(
                        "this clause of `{name}` calls `{name}` itself: checking it would re-enter the clause forever"
                    ),
                );
                self.contract(d, &clause);
            }
            _ => {}
        }
    }

    fn stmt(&mut self, s: &'a ast::Statement, clause: Option<ContractClause<'a>>) {
        let Some(clause) = clause else {
            if let ast::Statement::While(w) = s {
                if w.decreases().is_none() {
                    self.loops.push(w.span);
                }
            }
            return;
        };
        let what = match s {
            ast::Statement::Let(_) => return,
            ast::Statement::Var(_) => "a `var`",
            ast::Statement::Assign(_) => "an assignment",
            ast::Statement::Return(_) => "a `return`",
            ast::Statement::Assert(_) => "an `assert`",
            ast::Statement::While(_) => "a `while` loop",
            ast::Statement::Concurrent(_) => "a `concurrent` block",
            ast::Statement::Expr(_) => "an expression statement",
        };
        let span = s.span();
        if self.reported.iter().any(|r| contains(*r, span)) {
            return;
        }
        self.reported.push(span);
        let d = diag(
            codes::STATEMENT_IN_CONTRACT,
            Some(span),
            format!(
                "{what} cannot appear in a contract expression: a block inside a contract holds only `let` statements and a tail expression"
            ),
        );
        self.contract(d, &clause);
    }
}

/// `effect_in_contract` (4.5) for one declaration: an operation call
/// through a handle inside a contract expression.
struct Hir<'c, 'a, 'h> {
    cx: &'c Checker<'a>,
    def: &'h Def,
    /// The (clause, effect) pairs already reported.
    effects: BTreeSet<(Span, String)>,
    diags: Vec<Diagnostic>,
}

impl<'h> Visitor<'h> for Hir<'_, '_, 'h> {
    fn expr(&mut self, e: &'h Expr, site: Site<'h>) {
        let ExprKind::CapCall {
            handle,
            capability,
            op,
            ..
        } = &e.kind
        else {
            return;
        };
        if !site.contract {
            return;
        }
        let Some(f) = self.def.kind.as_fn() else {
            return;
        };
        let ExprKind::Local(id) = handle.kind else {
            return;
        };
        let Some(local) = f.locals.get(id.0 as usize) else {
            return;
        };
        if local.kind != BindingKind::Using || !matches!(local.ty, Ty::Capability(_)) {
            // A positional parameter of a capability type was
            // `capability_in_value_position` (S62).
            return;
        }
        let Some(op_def) = self
            .cx
            .program
            .def(*capability)
            .kind
            .as_capability()
            .and_then(|c| c.ops.get(*op as usize))
        else {
            return;
        };
        let effect = format!("{}.{}", local.name, op_def.name);
        // A property's `for` range has no HIR clause; no handle can occur
        // there (S30), so the expression stands in for it.
        let (span, text) = match site.clause {
            Some(c) => (c.span, Some(c.text.clone())),
            None => (e.span, None),
        };
        if !self.effects.insert((span, effect.clone())) {
            return;
        }
        let mut d = diag(
            codes::EFFECT_IN_CONTRACT,
            Some(span),
            format!(
                "a contract expression cannot call the operation `{effect}`: contracts are pure, checked with an empty effect bound"
            ),
        )
        .with_data("effect", json!(effect));
        if let Some(text) = text {
            d = d.with_data("clause", json!(text));
        }
        self.diags
            .push(blame(d, self.cx, self.def.id, Some(Facet::Contract)));
    }
}
