//! Blocks, divergence, and statements (`spec/semantics.md` 3.10, 3.11,
//! S24, S68).

use super::typer::{How, Owner, Typer, UnitOperand};
use crate::checker::diag;
use crate::codes;
use crate::resolve::Res;
use cpl_hir::{Block, Clause, Stmt, Ty};
use cpl_syntax::ast;
use serde_json::json;

/// One typed block.
pub(crate) struct BlockOut {
    pub block: Block,
    /// The block diverges (S24).
    pub div: bool,
    /// The tail's `≤ E` test failed (the `else`-less `if` of 3.8 reports
    /// there, S68).
    pub tail_failed: bool,
}

impl<'t, 'c, 'a> Typer<'t, 'c, 'a> {
    /// `type_block` of 3.10: statements in order (each its own frame, 3.7),
    /// `unreachable_statement` once after a diverging statement (the tail
    /// included: it is an expression statement too, S24), then the tail
    /// against `exp`, or the missing tail, which stands for the constant
    /// expression `Unit`: rule 1 of 3.4, the proof rule of 3.5 for a
    /// refinement of `Unit`, or the "missing tail expression" mismatch
    /// spanning the block.
    pub fn block(&mut self, b: &ast::Block, exp: Option<&Ty>) -> BlockOut {
        let mut stmts = Vec::new();
        let mut diverged = false;
        let mut warned = false;
        let mut last_div = false;
        for s in &b.statements {
            if diverged && !warned {
                warned = true;
                self.unreachable(s.span());
            }
            self.push_frame();
            let (st, div) = self.stmt(s);
            self.pop_frame();
            stmts.push(st);
            diverged |= div;
            last_div = div;
        }
        if let Some(t) = &b.tail {
            // 3.10: "if a previous statement diverged and the block has no
            // warning yet: warn `unreachable_statement` at t".
            if diverged && !warned {
                self.unreachable(t.span);
            }
        }
        let mut tail_failed = false;
        let (tail, ty, div) = match &b.tail {
            Some(t) => {
                let (e, div) = match exp {
                    Some(x) => {
                        let out = self.expr(&t.expr, Some(x));
                        self.last_coerce_failed = false;
                        let div = out.div;
                        let e = if out.done {
                            out.e
                        } else {
                            let e = self.coerce(out.e, &t.expr, x, How::Normal);
                            tail_failed = self.last_coerce_failed;
                            e
                        };
                        (e, div)
                    }
                    None => {
                        let out = self.expr(&t.expr, None);
                        (out.e, out.div)
                    }
                };
                let ty = match exp {
                    Some(x) => x.clone(),
                    None if div => Ty::Error,
                    None => e.ty.clone(),
                };
                (Some(Box::new(e)), ty, div)
            }
            None => {
                if last_div {
                    (None, exp.cloned().unwrap_or(Ty::Error), true)
                } else {
                    let unit = self.unit();
                    let mut ty = unit.clone();
                    if let Some(x) = exp {
                        // The missing tail stands for the constant expression
                        // `Unit`, checked against E (3.10): for a refinement
                        // of `Unit` the proof rule of 3.5 decides, so `where
                        // false` is `refinement_false`, spanning the block
                        // as the mismatch does (S63 (1)).
                        match self.unit_operand(
                            b.span,
                            x,
                            "missing tail expression: the block has no tail, which stands for `Unit`",
                        ) {
                            UnitOperand::Unit => {}
                            UnitOperand::Refinement { proved } => {
                                ty = self.zonk(x);
                                tail_failed = !proved;
                            }
                            UnitOperand::Mismatch => {
                                let expected = self.render(x);
                                self.error(
                                    diag(
                                        codes::TYPE_MISMATCH,
                                        Some(b.span),
                                        format!("expected `{expected}`, found `Unit`: the block has no tail expression"),
                                    )
                                    .with_data("expected", json!(expected))
                                    .with_data("actual", json!("Unit"))
                                    .with_note("missing tail expression", None),
                                );
                                tail_failed = true;
                            }
                        }
                    }
                    (None, ty, false)
                }
            }
        };
        BlockOut {
            block: Block {
                stmts,
                tail,
                ty,
                span: b.span,
            },
            div,
            tail_failed,
        }
    }

    /// One statement (3.11); `true` when it diverges (S24).
    fn stmt(&mut self, s: &ast::Statement) -> (Stmt, bool) {
        match s {
            ast::Statement::Let(l) => {
                let value = self.binding_value(&l.value, l.ty.as_ref());
                match &l.binding {
                    ast::LetBinding::Name(n) => {
                        match self.tcx.cx.names.binders.get(&n.span).copied() {
                            Some(local) => {
                                let ty = binding_type(
                                    &value.ty,
                                    l.ty.as_ref().map(|t| self.annotation(t)),
                                );
                                self.set_local_ty(local, ty);
                                (Stmt::Let { local, value }, false)
                            }
                            None => (Stmt::Discard(value), false),
                        }
                    }
                    ast::LetBinding::Wildcard(_) => (Stmt::Discard(value), false),
                }
            }
            ast::Statement::Var(v) => {
                let value = self.binding_value(&v.value, v.ty.as_ref());
                match self.tcx.cx.names.binders.get(&v.name.span).copied() {
                    Some(local) => {
                        let ty = binding_type(&value.ty, v.ty.as_ref().map(|t| self.annotation(t)));
                        self.set_local_ty(local, ty);
                        (Stmt::Let { local, value }, false)
                    }
                    None => (Stmt::Discard(value), false),
                }
            }
            ast::Statement::Assign(a) => {
                match self.tcx.cx.names.assigns.get(&a.target.span).cloned() {
                    Some(Res::Local(local)) => {
                        let t = self.local_ty(local);
                        let value = self.check(&a.value, &t);
                        (Stmt::Assign { local, value }, false)
                    }
                    _ => {
                        // The target failed (`unresolved_name`,
                        // `assign_to_immutable`): the right-hand side, a
                        // check-mode position (S56), accepts every type (S62).
                        let value = self.accept_any(&a.value);
                        (Stmt::Discard(value), false)
                    }
                }
            }
            ast::Statement::Return(r) => {
                // `return` belongs to a function body; elsewhere (a test, a
                // property, a contract) the position is the error (3.11, 5.1).
                let in_body = self.owner == Owner::Fn && !self.contract;
                let value = match &r.value {
                    Some(v) if in_body => {
                        let ret = self.ret.clone();
                        Some(self.check(v, &ret))
                    }
                    // `return_outside_function` or `statement_in_contract`:
                    // the operand has no return type to check against and
                    // accepts every type (S62).
                    Some(v) => Some(self.accept_any(v)),
                    None => {
                        if in_body {
                            // 3.11: the missing operand stands for the
                            // constant expression `Unit`, checked against the
                            // declared return type; a refinement of `Unit` is
                            // proved by 3.5, as for a missing tail (3.10).
                            let ret = self.ret.clone();
                            let operand = self.unit_operand(
                                r.span,
                                &ret,
                                "a `return` without a value returns `Unit`",
                            );
                            if operand == UnitOperand::Mismatch {
                                let expected = self.render(&ret);
                                self.error(
                                    diag(
                                        codes::RETURN_VALUE_MISSING,
                                        Some(r.span),
                                        format!("this function returns `{expected}`; `return` needs a value"),
                                    )
                                    .with_data("expected", json!(expected)),
                                );
                            }
                        }
                        None
                    }
                };
                (Stmt::Return(value), true)
            }
            ast::Statement::Assert(a) => {
                let expr = self.check(&a.condition, &Ty::Bool);
                let text = self.text(&a.condition);
                (Stmt::Assert { expr, text }, false)
            }
            ast::Statement::While(w) => {
                let cond = self.check(&w.condition, &Ty::Bool);
                let mut invariants = Vec::new();
                let mut decreases = None;
                for clause in &w.clauses {
                    match clause {
                        ast::WhileClause::Invariant(i) => {
                            let expr = self.clause_bool(&i.expr, i.span);
                            invariants.push(Clause {
                                expr,
                                text: self.text(&i.expr),
                                span: i.span,
                            });
                        }
                        ast::WhileClause::Decreases(d) => {
                            let expr = self.clause_measure(&d.expr, d.span);
                            decreases = Some(Clause {
                                expr,
                                text: self.text(&d.expr),
                                span: d.span,
                            });
                        }
                    }
                }
                self.whiles
                    .push(invariants.iter().map(|c| c.text.clone()).collect());
                let unit = self.unit();
                let body = self.block(&w.body, Some(&unit)).block;
                self.whiles.pop();
                (
                    Stmt::While {
                        cond,
                        invariants,
                        decreases,
                        body,
                    },
                    false,
                )
            }
            // `context app = e`: the call stack's state machine, from a value of a program type
            ast::Statement::Context(c) => {
                let value = self.infer(&c.value);
                let handle = match self.zonk(&value.ty) {
                    Ty::Adt(d, a) if a.is_empty() && self.program.is_program_state(d) => Ty::Context(d),
                    Ty::Error => Ty::Error,
                    other => {
                        let found = self.render(&other);
                        self.error(
                            diag(
                                codes::CONTEXT_NOT_PROGRAM,
                                Some(c.value.span),
                                format!("a `context` starts from a value of a `program` type, found `{found}`"),
                            )
                            .with_data("type", json!(found)),
                        );
                        Ty::Error
                    }
                };
                match self.tcx.cx.names.binders.get(&c.name.span).copied() {
                    Some(local) => {
                        self.set_local_ty(local, handle);
                        (Stmt::Context { local, value }, false)
                    }
                    None => (Stmt::Discard(value), false),
                }
            }
            ast::Statement::Concurrent(c) => {
                let mut tasks = Vec::new();
                for task in &c.body.statements {
                    if !matches!(task, ast::Statement::Let(_) | ast::Statement::Expr(_)) {
                        self.error(diag(
                            codes::CONCURRENT_TASK_INVALID,
                            Some(task.span()),
                            "a `concurrent` task is a `let` or an expression statement; move other statements into a helper function",
                        ));
                    }
                    self.push_frame();
                    let (stmt, _) = self.stmt(task);
                    self.pop_frame();
                    tasks.push(cpl_hir::Task {
                        stmt,
                        span: task.span(),
                    });
                }
                (Stmt::Concurrent(tasks), false)
            }
            ast::Statement::Expr(x) => {
                let unit = self.unit();
                if is_branching(&x.expr) {
                    // S68: `Unit` flows into every branch and arm.
                    let out = self.expr(&x.expr, Some(&unit));
                    (Stmt::Expr(out.e), out.div)
                } else {
                    let e = self.check_how(&x.expr, &unit, How::Unused);
                    (Stmt::Expr(e), false)
                }
            }
        }
    }

    /// `unreachable_statement` (S24) at the first unreachable statement of
    /// a block (S63 (5)).
    fn unreachable(&mut self, span: cpl_diag::Span) {
        self.error(diag(
            codes::UNREACHABLE_STATEMENT,
            Some(span),
            "this statement is never executed: the statement before it always exits",
        ));
    }

    /// The initializer of a `let`/`var`: checked against the annotation,
    /// or inferred (an infer-only position, S56).
    fn binding_value(&mut self, value: &ast::Expr, ty: Option<&ast::Type>) -> cpl_hir::Expr {
        match ty {
            Some(t) => {
                let ann = self.annotation(t);
                self.check(value, &ann)
            }
            None => self.infer(value),
        }
    }

    /// The resolved type of an annotation (`Ty::Error` after a diagnostic).
    fn annotation(&self, t: &ast::Type) -> Ty {
        self.tcx
            .cx
            .names
            .types
            .get(&t.span)
            .cloned()
            .unwrap_or(Ty::Error)
    }
}

/// The type a `let`/`var` binds (3.7): the annotation, or the
/// initializer's type; a handle or generator initializer (S27, S14) binds
/// the error type, since its diagnostic has been (or will be) reported.
fn binding_type(value: &Ty, annotation: Option<Ty>) -> Ty {
    if let Some(a) = annotation {
        return a;
    }
    match value {
        Ty::Capability(_) | Ty::Context(_) | Ty::Gen(_) => Ty::Error,
        other => other.clone(),
    }
}

/// An `if` or `match` statement, through parentheses (S68).
fn is_branching(e: &ast::Expr) -> bool {
    match &e.kind {
        ast::ExprKind::Paren(inner) => is_branching(inner),
        ast::ExprKind::If(_) | ast::ExprKind::Match(_) => true,
        _ => false,
    }
}
