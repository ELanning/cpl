//! The evaluator proper: expressions, statements, patterns, calls, and the
//! contract checks of `spec/toolchain.md` 1.7.1 (semantics 5.2-5.4, 6.2,
//! 6.10, 7.1, 8.1, S31, S34, S35).

use super::{HandleId, Interpreter, Table};
use crate::arith;
use crate::builtins;
use crate::fault::{AttributedTo, ClauseKind, Fault, FaultKind, Frame, Resource};
use crate::interp::EffectEvent;
use crate::json::{int_to_json, value_text_untyped, value_to_json, value_to_json_untyped};
use crate::types::{operation_identity, type_name};
use crate::value::Value;
use cpl_diag::{Facet, Span, StableId};
use cpl_hir::{
    Arm, Block, Clause, DefId, Expr, ExprKind, FnBody, FnDef, Lit, LocalId, OpDef, Pattern,
    PatternKind, Stmt, Ty,
};
use cpl_rtspec::messages::{self, render};
use cpl_syntax::ast::{BinaryOp, UnaryOp};
use num_bigint::BigInt;
use num_traits::Signed;
use serde_json::{json, Map, Value as Json};
use std::rc::Rc;

/// Why evaluation left the current expression early: a fault, or a
/// function exit (`return e`, or a `?` that propagates an `Err`, whose
/// exit value is that `Err`, semantics 6.2).
pub(crate) enum Flow {
    Fault(Box<Fault>),
    Return(Value),
}

/// The result of evaluating one HIR node.
pub(crate) type Eval<T> = Result<T, Flow>;

impl Flow {
    /// The fault this flow carries; a function exit that escaped every
    /// function boundary is an impossible state.
    pub(crate) fn into_fault(self) -> Fault {
        match self {
            Flow::Fault(fault) => *fault,
            Flow::Return(_) => Fault::internal(
                "return_outside_function",
                render(&messages::DETAIL_RETURN_OUTSIDE_FUNCTION, &[]),
            ),
        }
    }
}

impl<'p> Interpreter<'p> {
    // =======================================================================
    // Expressions (semantics 6.2: strict, left to right)
    // =======================================================================

    pub(super) fn eval_expr(&mut self, e: &'p Expr) -> Eval<Value> {
        match &e.kind {
            ExprKind::Lit(lit) => Ok(match lit {
                Lit::Int(n) => Value::Int(n.clone()),
                Lit::Text(s) => Value::text(s),
                Lit::Bool(b) => Value::Bool(*b),
            }),
            ExprKind::Local(id) => self.read_local(*id, e.span),
            ExprKind::Ctor { def, variant, args } => {
                let payload = self.eval_all(args)?;
                Ok(Value::new_enum(*def, *variant, payload))
            }
            ExprKind::Wrap { def, arg } => {
                let inner = self.eval_expr(arg)?;
                Ok(Value::wrap(*def, inner))
            }
            ExprKind::Field { base, index, .. } => self.eval_field(base, *index, e.span),
            ExprKind::Call {
                callee,
                args,
                using,
                ..
            } => self.eval_call(*callee, args, using, e.span),
            ExprKind::CapCall {
                handle,
                capability,
                op,
                args,
            } => self.eval_cap_call(handle, *capability, *op, args, e.span),
            ExprKind::Check { refined, arg } => self.eval_check(*refined, arg, e.span),
            ExprKind::Propagate(inner) => self.eval_propagate(inner, e.span),
            ExprKind::Unary { op, operand } => self.eval_unary(*op, operand, e.span),
            ExprKind::Binary { op, lhs, rhs } => self.eval_binary(*op, lhs, rhs, e.span),
            ExprKind::Parallel {
                binder,
                collection,
                predicate,
            } => self.eval_parallel(*binder, collection, predicate),
            ExprKind::Forall {
                binder,
                collection,
                predicate,
            } => self.eval_forall(*binder, collection, predicate),
            ExprKind::Hole { name, .. } => {
                self.probe_site(e.span);
                self.raise_hole(name, &e.ty, e.span)
            }
            ExprKind::List(items) => Ok(Value::list(self.eval_all(items)?)),
            ExprKind::StructLit {
                def,
                invariant_proven,
                spread,
                fields,
            } => {
                let value = self.eval_struct(*def, spread.as_deref(), fields, e.span)?;
                if !invariant_proven {
                    if let Some(inv) = self.program.def(*def).kind.checked_predicate() {
                        if !self.eval_predicate(*def, &value, Some(e.span))? {
                            let fault = Fault::contract_violation(
                                ClauseKind::Invariant,
                                AttributedTo::Body,
                                &inv.predicate.text,
                            )
                            .with_symbol(self.program.def(*def).stable_id.clone())
                            .with_span(inv.predicate.span)
                            .with_data("actual", value_to_json_untyped(self.program, &value));
                            return Err(Flow::Fault(Box::new(fault)));
                        }
                    }
                }
                Ok(value)
            }
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => self.eval_if(cond, then_block, else_block.as_ref()),
            ExprKind::Match { scrutinee, arms } => self.eval_match(scrutinee, arms, e.span),
            // Refinements are erased (semantics 6.1): widening is the identity.
            ExprKind::Widen { expr, .. } => self.eval_expr(expr),
            ExprKind::Refine { expr, refined, .. } => self.eval_refine(expr, *refined, e.span),
            ExprKind::Gen(_) => self.internal(
                "generator_not_a_value",
                render(&messages::DETAIL_GENERATOR_NOT_A_VALUE_EXPR, &[]),
                e.span,
            ),
            ExprKind::Error => self.internal(
                "error_node",
                render(&messages::DETAIL_ERROR_NODE, &[]),
                e.span,
            ),
        }
    }

    /// Evaluate `es` left to right.
    fn eval_all(&mut self, es: &'p [Expr]) -> Eval<Vec<Value>> {
        let mut out = Vec::with_capacity(es.len());
        for e in es {
            out.push(self.eval_expr(e)?);
        }
        Ok(out)
    }

    fn eval_bool(&mut self, e: &'p Expr) -> Eval<bool> {
        match self.eval_expr(e)? {
            Value::Bool(b) => Ok(b),
            other => self.confusion("a Bool", &other, e.span),
        }
    }

    /// A value of the wrong form for its typed position.
    pub(super) fn confusion<T>(&self, expected: &str, found: &Value, span: Span) -> Eval<T> {
        self.internal(
            "type_confusion",
            render(
                &messages::DETAIL_TYPE_CONFUSION,
                &[("found", found.kind_name()), ("expected", expected)],
            ),
            span,
        )
    }

    fn eval_field(&mut self, base: &'p Expr, index: u32, span: Span) -> Eval<Value> {
        match &self.eval_expr(base)? {
            Value::Struct { fields, .. } => match fields.get(index as usize) {
                Some(v) => Ok(v.clone()),
                None => self.internal(
                    "missing_field",
                    render(
                        &messages::DETAIL_MISSING_FIELD,
                        &[("index", &index.to_string())],
                    ),
                    span,
                ),
            },
            other => self.confusion("a struct", other, span),
        }
    }

    /// A struct literal: the spread base first, then the field
    /// initializers in source order (semantics 6.2); the base is unchanged.
    fn eval_struct(
        &mut self,
        def: DefId,
        spread: Option<&'p Expr>,
        fields: &'p [(u32, Expr)],
        span: Span,
    ) -> Eval<Value> {
        let p = self.program;
        let Some(count) = p
            .defs
            .get(def.0 as usize)
            .and_then(|d| d.kind.as_struct())
            .map(|s| s.fields.len())
        else {
            return self.internal(
                "not_a_struct",
                render(
                    &messages::DETAIL_NOT_A_STRUCT,
                    &[("def", &def.0.to_string())],
                ),
                span,
            );
        };
        let mut slots: Vec<Option<Value>> = match spread {
            Some(base) => match &self.eval_expr(base)? {
                Value::Struct {
                    fields: base_fields,
                    ..
                } if base_fields.len() == count => base_fields.iter().cloned().map(Some).collect(),
                other => return self.confusion("a struct of the literal's type", other, span),
            },
            None => vec![None; count],
        };
        for (index, e) in fields {
            let v = self.eval_expr(e)?;
            match slots.get_mut(*index as usize) {
                Some(slot) => *slot = Some(v),
                None => {
                    return self.internal(
                        "missing_field",
                        render(
                            &messages::DETAIL_MISSING_FIELD_LITERAL,
                            &[
                                ("struct", &p.display_path(def)),
                                ("index", &index.to_string()),
                            ],
                        ),
                        span,
                    )
                }
            }
        }
        let mut out = Vec::with_capacity(count);
        for (i, slot) in slots.into_iter().enumerate() {
            match slot {
                Some(v) => out.push(v),
                None => {
                    return self.internal(
                        "missing_field",
                        render(
                            &messages::DETAIL_MISSING_FIELD_UNSET,
                            &[("index", &i.to_string()), ("struct", &p.display_path(def))],
                        ),
                        span,
                    )
                }
            }
        }
        Ok(Value::new_struct(def, out))
    }

    fn eval_if(
        &mut self,
        cond: &'p Expr,
        then_block: &'p Block,
        else_block: Option<&'p Block>,
    ) -> Eval<Value> {
        if self.eval_bool(cond)? {
            self.eval_block(then_block)
        } else {
            match else_block {
                Some(b) => self.eval_block(b),
                None => Ok(Value::unit(self.program)),
            }
        }
    }

    /// `match`: the scrutinee, then the arms' patterns in source order;
    /// only the first matching arm's body runs (semantics 6.2). No matching
    /// arm is an impossible state after checking (3.13).
    fn eval_match(&mut self, scrutinee: &'p Expr, arms: &'p [Arm], span: Span) -> Eval<Value> {
        let value = self.eval_expr(scrutinee)?;
        for arm in arms {
            let mut binds = Vec::new();
            if matches(&arm.pattern, &value, &mut binds) {
                let mark = self.scope_mark();
                for (local, v) in binds {
                    self.bind(local, v);
                }
                let outcome = self.eval_block(&arm.body);
                self.restore_scope(mark);
                return outcome;
            }
        }
        self.internal(
            "no_matching_arm",
            render(
                &messages::DETAIL_NO_MATCHING_ARM,
                &[("value", &value_text_untyped(self.program, &value))],
            ),
            span,
        )
    }

    /// `e?` (semantics 6.2): `Ok(v)` continues with `v`; `Err(x)` exits the
    /// function with the `Err(x)` value as its exit value.
    fn eval_propagate(&mut self, inner: &'p Expr, span: Span) -> Eval<Value> {
        let value = self.eval_expr(inner)?;
        let ok = match value.as_result(self.program) {
            Some(Ok(v)) => Some(v.clone()),
            Some(Err(_)) => None,
            None => return self.confusion("a Result", &value, span),
        };
        match ok {
            Some(v) => Ok(v),
            None => Err(Flow::Return(value)),
        }
    }

    fn eval_unary(&mut self, op: UnaryOp, operand: &'p Expr, span: Span) -> Eval<Value> {
        match op {
            UnaryOp::Not => Ok(Value::Bool(!self.eval_bool(operand)?)),
            UnaryOp::Neg => match &self.eval_expr(operand)? {
                Value::Int(n) => Ok(Value::Int(arith::negate(n))),
                other => self.confusion("an Int", other, span),
            },
        }
    }

    /// Binary operators (semantics 6.2, 6.3, 6.5): `and`/`or` evaluate the
    /// right operand only when needed; every other operator evaluates both
    /// operands left to right. Division by zero is blamed on the enclosing
    /// declaration at the operator expression (stdlib S8).
    fn eval_binary(
        &mut self,
        op: BinaryOp,
        lhs: &'p Expr,
        rhs: &'p Expr,
        span: Span,
    ) -> Eval<Value> {
        match op {
            BinaryOp::And => return Ok(Value::Bool(self.eval_bool(lhs)? && self.eval_bool(rhs)?)),
            BinaryOp::Or => return Ok(Value::Bool(self.eval_bool(lhs)? || self.eval_bool(rhs)?)),
            _ => {}
        }
        let a = self.eval_expr(lhs)?;
        let b = self.eval_expr(rhs)?;
        match op {
            BinaryOp::Eq => Ok(Value::Bool(a.structurally_equal(&b))),
            BinaryOp::Ne => Ok(Value::Bool(!a.structurally_equal(&b))),
            BinaryOp::Concat => match a.concat(&b) {
                Some(v) => Ok(v),
                None => self.internal(
                    "type_confusion",
                    render(
                        &messages::DETAIL_TYPE_CONFUSION_CONCAT,
                        &[("lhs", a.kind_name()), ("rhs", b.kind_name())],
                    ),
                    span,
                ),
            },
            _ => match (&a, &b) {
                (Value::Int(x), Value::Int(y)) => match arith::binary(op, x, y) {
                    Ok(v) => Ok(v),
                    Err(fault) => self.raise(fault, Some(span)),
                },
                _ => self.internal(
                    "type_confusion",
                    render(
                        &messages::DETAIL_TYPE_CONFUSION_OPERATOR,
                        &[
                            ("operator", op.symbol()),
                            ("lhs", a.kind_name()),
                            ("rhs", b.kind_name()),
                        ],
                    ),
                    span,
                ),
            },
        }
    }

    /// `forall x in c: p` (semantics 6.2): `c`, then `p` for each element in
    /// order, stopping at the first `false`; `true` for an empty list.
    fn eval_forall(
        &mut self,
        binder: LocalId,
        collection: &'p Expr,
        predicate: &'p Expr,
    ) -> Eval<Value> {
        let items = match &self.eval_expr(collection)? {
            Value::List(items) => Rc::clone(items),
            other => return self.confusion("a List", other, collection.span),
        };
        for item in items.iter() {
            let mark = self.scope_mark();
            self.bind(binder, item.clone());
            let holds = self.eval_bool(predicate);
            self.restore_scope(mark);
            if !holds? {
                return Ok(Value::Bool(false));
            }
        }
        Ok(Value::Bool(true))
    }

    /// `?name` (semantics 6.7, 8.1): the fault `unresolved_hole` with the
    /// expected type rendered as written and every visible binding with its
    /// value, innermost first.
    fn raise_hole(&self, name: &str, ty: &Ty, span: Span) -> Eval<Value> {
        let generics = self.generics_of(self.env().table);
        let expected = self.program.display_ty_in(ty, generics);
        let fault = Fault::unresolved_hole(name, &expected, self.visible_bindings());
        self.raise(fault, Some(span))
    }

    /// `check<R>(x)` (semantics 6.10; stdlib 1.4): the predicates of
    /// `chain(R)` from the root base upward with `self` = `x`; the first
    /// `false` gives `Err(ConstraintError)` (an ordinary value) and stops.
    fn eval_check(&mut self, refined: DefId, arg: &'p Expr, span: Span) -> Eval<Value> {
        let value = self.eval_expr(arg)?;
        let p = self.program;
        for r in builtins::check_order(p, refined) {
            if !self.eval_predicate(r, &value, Some(span))? {
                return Ok(builtins::check_failure(p, r, &value));
            }
        }
        Ok(Value::ok(p, value))
    }

    /// A statically proved refinement (T39), re-checked when contracts are
    /// checked: a false predicate means the proof rule was unsound.
    fn eval_refine(&mut self, expr: &'p Expr, refined: DefId, span: Span) -> Eval<Value> {
        let value = self.eval_expr(expr)?;
        if self.config.check_contracts {
            let p = self.program;
            for r in builtins::check_order(p, refined) {
                if !self.eval_predicate(r, &value, Some(span))? {
                    let text = p
                        .def(r)
                        .kind
                        .as_refinement()
                        .map_or("", |rd| rd.predicate.text.as_str());
                    return self.internal(
                        "refinement_proof_unsound",
                        render(
                            &messages::DETAIL_REFINEMENT_PROOF_UNSOUND,
                            &[
                                ("value", &value_text_untyped(p, &value)),
                                ("refinement", &type_name(p, r)),
                                ("predicate", text),
                            ],
                        ),
                        span,
                    );
                }
            }
        }
        Ok(value)
    }

    /// The `where` predicate of the refinement declaration `refinement` with
    /// `self` = `value`, in an activation of the refinement (entered through
    /// `span`). A fault inside it is attributed to the predicate (S34).
    pub(crate) fn eval_predicate(
        &mut self,
        refinement: DefId,
        value: &Value,
        span: Option<Span>,
    ) -> Eval<bool> {
        let p = self.program;
        let Some(def) = p.defs.get(refinement.0 as usize) else {
            return self.internal(
                "not_a_refinement",
                render(
                    &messages::DETAIL_NOT_A_REFINEMENT_DEF,
                    &[("def", &refinement.0.to_string())],
                ),
                span,
            );
        };
        let Some(rd) = def.kind.checked_predicate() else {
            return self.internal(
                "not_a_refinement",
                render(
                    &messages::DETAIL_NOT_A_REFINEMENT,
                    &[("name", &p.display_path(refinement))],
                ),
                span,
            );
        };
        self.enter_activation(refinement, span)?;
        self.bind(rd.self_local, value.clone());
        let outcome = self.clause_holds(&rd.predicate, Some(&def.stable_id));
        self.leave_activation();
        outcome.map_err(|flow| leaving(flow, refinement, span))
    }

    // =======================================================================
    // Blocks and statements
    // =======================================================================

    /// A block: statements in order, then the tail (`Unit` without one);
    /// its bindings go out of scope when it ends.
    pub(super) fn eval_block(&mut self, b: &'p Block) -> Eval<Value> {
        let mark = self.scope_mark();
        let outcome = self.eval_block_inner(b);
        self.restore_scope(mark);
        outcome
    }

    fn eval_block_inner(&mut self, b: &'p Block) -> Eval<Value> {
        for s in &b.stmts {
            self.exec_stmt(s)?;
        }
        match &b.tail {
            Some(t) => self.eval_expr(t),
            None => Ok(Value::unit(self.program)),
        }
    }

    fn exec_stmt(&mut self, s: &'p Stmt) -> Eval<()> {
        match s {
            Stmt::Let { local, value } => {
                let v = self.eval_expr(value)?;
                self.bind(*local, v);
                Ok(())
            }
            Stmt::Discard(e) | Stmt::Expr(e) => self.eval_expr(e).map(drop),
            Stmt::Assign { local, value } => {
                if let ExprKind::Call {
                    callee,
                    args,
                    using,
                    ..
                } = &value.kind
                {
                    let push = self.program.def(*callee).kind.as_fn().is_some_and(
                        |f| matches!(&f.body, FnBody::Builtin(id) if id == "list_push"),
                    );
                    let mut blocks = false;
                    if let Some(arg) = args.get(1) {
                        arg.walk(&mut |e| {
                            blocks |= matches!(e.kind, ExprKind::If { .. } | ExprKind::Match { .. })
                        });
                    }
                    if push
                        && using.is_empty()
                        && args.len() == 2
                        && !blocks
                        && matches!(args[0].kind, ExprKind::Local(id) if id == *local)
                    {
                        let item = self.eval_expr(&args[1])?;
                        if let Some(Some(Value::List(xs))) =
                            self.env_mut().values.get_mut(local.0 as usize)
                        {
                            Rc::make_mut(xs).push(item);
                            return Ok(());
                        }
                    }
                }
                let v = self.eval_expr(value)?;
                self.assign(*local, v, value.span)
            }
            Stmt::Return(e) => {
                let v = match e {
                    Some(e) => self.eval_expr(e)?,
                    None => Value::unit(self.program),
                };
                Err(Flow::Return(v))
            }
            Stmt::Assert { expr, text } => {
                self.probe_site(expr.span);
                if self.eval_bool(expr)? {
                    Ok(())
                } else {
                    let fault = Fault::assertion_failed(text, self.visible_bindings());
                    self.raise(fault, Some(expr.span))
                }
            }
            Stmt::While {
                cond,
                invariants,
                decreases,
                body,
            } => self.exec_while(cond, invariants, decreases.as_ref(), body),
        }
    }

    /// `while` with its clauses, exactly semantics 5.3: the invariants
    /// before the first test of the condition (iteration 0); then, each
    /// time around, the condition, the measure (only after the condition
    /// held: `>= 0` and strictly below the previous iteration's), the body,
    /// and the invariants again (iteration = completed iterations). The
    /// measure is never evaluated after the loop exits.
    fn exec_while(
        &mut self,
        cond: &'p Expr,
        invariants: &'p [Clause],
        decreases: Option<&'p Clause>,
        body: &'p Block,
    ) -> Eval<()> {
        self.probe_site(cond.span);
        let check = self.config.check_contracts;
        let owner = self.owner_identity();
        if check {
            self.check_invariants(invariants, 0, owner.as_ref())?;
        }
        let mut previous: Option<BigInt> = None;
        let mut iteration: u64 = 0;
        loop {
            self.check_time(Some(cond.span))?;
            if !self.eval_bool(cond)? {
                return Ok(());
            }
            if let (true, Some(clause)) = (check, decreases) {
                let measure = self.eval_measure(clause, owner.as_ref())?;
                let negative = measure.is_negative();
                if negative || previous.as_ref().is_some_and(|prev| &measure >= prev) {
                    let mut extra = Map::new();
                    if let (false, Some(prev)) = (negative, &previous) {
                        extra.insert("previous".to_string(), int_to_json(prev));
                    }
                    extra.insert("measure".to_string(), int_to_json(&measure));
                    return Err(self.loop_violation(
                        ClauseKind::Decreases,
                        clause,
                        iteration,
                        extra,
                    ));
                }
                previous = Some(measure);
            }
            self.eval_block(body)?;
            iteration += 1;
            if check {
                self.check_invariants(invariants, iteration, owner.as_ref())?;
            }
        }
    }

    fn check_invariants(
        &mut self,
        invariants: &'p [Clause],
        iteration: u64,
        owner: Option<&StableId>,
    ) -> Eval<()> {
        for clause in invariants {
            if !self.clause_holds(clause, owner)? {
                return Err(self.loop_violation(
                    ClauseKind::Invariant,
                    clause,
                    iteration,
                    Map::new(),
                ));
            }
        }
        Ok(())
    }

    /// An `invariant` or loop `decreases` violation (semantics 5.3):
    /// blamed on the enclosing declaration, `attributed_to: "body"`,
    /// `iteration`, `input` (the enclosing function's arguments; none in a
    /// test or property body), and `actual = {bindings, measure?,
    /// previous?}` with every binding visible at the loop.
    fn loop_violation(
        &mut self,
        kind: ClauseKind,
        clause: &Clause,
        iteration: u64,
        extra: Map<String, Json>,
    ) -> Flow {
        let p = self.program;
        let mut actual = Map::new();
        actual.insert("bindings".to_string(), Json::Object(self.bindings_object()));
        actual.extend(extra);
        let mut fault = Fault::contract_violation(kind, AttributedTo::Body, &clause.text)
            .with_span(clause.span)
            .with_data("iteration", json!(iteration))
            .with_data("actual", Json::Object(actual));
        if let Some(def) = self.current_def() {
            fault = fault.blamed_on(p, def);
            if let Some(input) = self.input_object(def) {
                fault = fault.with_data("input", input);
            }
            self.violation = Some((def, fault.clone()));
        }
        Flow::Fault(Box::new(fault))
    }

    /// The stable identity of the enclosing declaration: the owner of a
    /// loop clause for S34.
    fn owner_identity(&self) -> Option<StableId> {
        let p = self.program;
        self.current_def()
            .and_then(|d| p.defs.get(d.0 as usize))
            .map(|def| def.stable_id.clone())
    }

    // =======================================================================
    // Contract expressions (semantics 5.1, S34)
    // =======================================================================

    /// Evaluate a contract clause owned by the declaration `owner` (S34,
    /// [`Interpreter::eval_contract_expr`]).
    fn eval_clause(&mut self, clause: &'p Clause, owner: Option<&StableId>) -> Eval<Value> {
        self.eval_contract_expr(&clause.expr, &clause.text, owner)
    }

    /// Evaluate `e`, (part of) a contract expression owned by the
    /// declaration `owner` whose canonical text is `text` (semantics 5.1).
    /// A fault escaping it is reported as that fault with `facet:
    /// "contract"`, `clause` = `text`, and `symbol` = `owner` (S34), unless
    /// a clause evaluated inside it already claimed the fault (the innermost
    /// clause a fault escapes is the one reported).
    ///
    /// An `internal_error` is never claimed: it has no symbol, hence no
    /// facet (semantics 6.7, toolchain 1.7.3), and an impossible state of
    /// the typed IR (a host's ill-typed value meeting a clause) is no
    /// clause's specification bug, which is what S34 reports; it keeps no
    /// `clause` either, a field of contract and assertion diagnostics
    /// (9.2) that 9.4 does not list for it. Read with 6.7 taking precedence
    /// over S34, S70, and S71, which do not mention `internal_error`.
    pub(super) fn eval_contract_expr(
        &mut self,
        e: &'p Expr,
        text: &str,
        owner: Option<&StableId>,
    ) -> Eval<Value> {
        match self.eval_expr(e) {
            Ok(v) => Ok(v),
            Err(Flow::Fault(mut fault)) => {
                if fault.kind != FaultKind::Internal
                    && !std::mem::replace(&mut self.attributed, true)
                {
                    if let Some(owner) = owner {
                        fault.symbol = Some(owner.clone());
                    }
                    fault.facet = Some(Facet::Contract);
                    fault
                        .data
                        .insert("clause".to_string(), Json::String(text.to_string()));
                }
                Err(Flow::Fault(fault))
            }
            Err(Flow::Return(_)) => self.internal(
                "control_flow_in_contract",
                render(&messages::DETAIL_CONTROL_FLOW_IN_CONTRACT, &[]),
                e.span,
            ),
        }
    }

    fn clause_holds(&mut self, clause: &'p Clause, owner: Option<&StableId>) -> Eval<bool> {
        match self.eval_clause(clause, owner)? {
            Value::Bool(b) => Ok(b),
            other => self.confusion("a Bool", &other, clause.span),
        }
    }

    fn eval_measure(&mut self, clause: &'p Clause, owner: Option<&StableId>) -> Eval<BigInt> {
        match self.eval_clause(clause, owner)?.into_int() {
            Ok(n) => Ok(n),
            Err(other) => self.confusion("an Int", &other, clause.span),
        }
    }

    // =======================================================================
    // Calls (semantics 5.2, 5.4, 6.10, S31, S58)
    // =======================================================================

    fn eval_call(
        &mut self,
        callee: DefId,
        args: &'p [Expr],
        using: &'p [Expr],
        span: Span,
    ) -> Eval<Value> {
        let values = self.eval_all(args)?;
        let mut handles = Vec::with_capacity(using.len());
        for u in using {
            match self.eval_expr(u)? {
                Value::Capability(h) => handles.push(h),
                other => return self.confusion("a capability handle", &other, u.span),
            }
        }
        self.call_fn(callee, values, handles, Some(span))
    }

    /// Call `f` through the call at `span` (`None` at the top level).
    pub(super) fn call_fn(
        &mut self,
        f: DefId,
        args: Vec<Value>,
        using: Vec<HandleId>,
        span: Option<Span>,
    ) -> Eval<Value> {
        let p = self.program;
        let Some(fd) = p.defs.get(f.0 as usize).and_then(|d| d.kind.as_fn()) else {
            return self.internal(
                "not_a_function",
                render(
                    &messages::DETAIL_NOT_A_FUNCTION,
                    &[("def", &f.0.to_string())],
                ),
                span,
            );
        };
        if let FnBody::Builtin(id) = &fd.body {
            return self.call_builtin(f, id, args, span);
        }
        if args.len() != fd.params.len() || using.len() != fd.using.len() {
            return self.internal(
                "call_mismatch",
                render(
                    &messages::DETAIL_CALL_MISMATCH,
                    &[
                        ("function", &p.display_path(f)),
                        ("params", &fd.params.len().to_string()),
                        ("using", &fd.using.len().to_string()),
                        ("args", &args.len().to_string()),
                        ("handles", &using.len().to_string()),
                    ],
                ),
                span,
            );
        }
        // S38: the call depth counts function activations; exceeding the
        // limit is refused at the call, blamed on the caller.
        let limit = self.config.max_call_depth;
        if self.calls >= limit {
            return self.raise(
                Fault::resource_exhaustion(Resource::Stack, u64::from(limit)),
                span,
            );
        }
        self.enter_activation(f, span)?;
        self.calls += 1;
        for (param, v) in fd.params.iter().zip(args) {
            self.bind(param.local, v);
        }
        for (param, h) in fd.using.iter().zip(using) {
            self.bind(param.local, Value::Capability(h));
        }
        let outcome = self.run_fn(f, fd);
        self.calls -= 1;
        self.leave_activation();
        outcome.map_err(|flow| leaving(flow, f, span))
    }

    /// The activation of a CPL function, with its parameters bound: 5.2
    /// steps 3-6 and S35.
    fn run_fn(&mut self, f: DefId, fd: &'p FnDef) -> Eval<Value> {
        let p = self.program;
        let owner = &p.def(f).stable_id;
        let check = self.config.check_contracts;
        if check {
            for clause in &fd.requires {
                if !self.clause_holds(clause, Some(owner))? {
                    let mut fault = Fault::contract_violation(
                        ClauseKind::Requires,
                        AttributedTo::Caller,
                        &clause.text,
                    )
                    .blamed_on(p, f)
                    .with_span(clause.span);
                    if let Some(input) = self.input_object(f) {
                        fault = fault.with_data("input", input);
                    }
                    // The frame below this activation is the caller; the
                    // command line has none (S39 (c)).
                    if let Some(caller) =
                        self.frames.len().checked_sub(2).map(|i| self.frames[i].def)
                    {
                        fault = fault.with_data("caller", json!(p.display_path(caller)));
                    }
                    return Err(Flow::Fault(Box::new(fault)));
                }
            }
            if let Some(clause) = &fd.decreases {
                self.enter_measure(f, clause, owner)?;
            }
        }
        let FnBody::Block(body) = &fd.body else {
            return self.internal(
                "builtin_body",
                render(&messages::DETAIL_BUILTIN_BODY, &[]),
                None,
            );
        };
        let exit = match self.eval_block(body) {
            Ok(v) | Err(Flow::Return(v)) => v,
            Err(fault) => return Err(fault),
        };
        if check && !fd.ensures.is_empty() {
            let mark = self.scope_mark();
            if let Some(result) = fd.result_local {
                self.bind(result, exit.clone());
            }
            let outcome = self.check_ensures(f, fd, owner, &exit);
            self.restore_scope(mark);
            outcome?;
        }
        Ok(exit)
    }

    /// S35: on entry, after the `requires` clauses held, the header measure
    /// must be `>= 0` and strictly below the measure recorded by the
    /// nearest enclosing activation of the same function; it is recorded
    /// on this activation.
    fn enter_measure(&mut self, f: DefId, clause: &'p Clause, owner: &StableId) -> Eval<()> {
        let measure = self.eval_measure(clause, Some(owner))?;
        let n = self.frames.len();
        let previous = (0..n.saturating_sub(1))
            .rev()
            .find(|&i| self.frames[i].def == f)
            .and_then(|i| self.measures[i].clone());
        let negative = measure.is_negative();
        if negative || previous.as_ref().is_some_and(|prev| &measure >= prev) {
            let p = self.program;
            let mut actual = Map::new();
            if let (false, Some(prev)) = (negative, &previous) {
                actual.insert("previous".to_string(), int_to_json(prev));
            }
            actual.insert("measure".to_string(), int_to_json(&measure));
            let mut fault =
                Fault::contract_violation(ClauseKind::Decreases, AttributedTo::Body, &clause.text)
                    .blamed_on(p, f)
                    .with_span(clause.span)
                    .with_data("actual", Json::Object(actual));
            if let Some(input) = self.input_object(f) {
                fault = fault.with_data("input", input);
            }
            self.violation = Some((f, fault.clone()));
            return Err(Flow::Fault(Box::new(fault)));
        }
        if let Some(slot) = self.measures.last_mut() {
            *slot = Some(measure);
        }
        Ok(())
    }

    /// 5.2 step 5: the `ensures` clauses in order with `result` bound.
    fn check_ensures(
        &mut self,
        f: DefId,
        fd: &'p FnDef,
        owner: &StableId,
        exit: &Value,
    ) -> Eval<()> {
        for clause in &fd.ensures {
            if !self.clause_holds(clause, Some(owner))? {
                let p = self.program;
                let mut fault = Fault::contract_violation(
                    ClauseKind::Ensures,
                    AttributedTo::Body,
                    &clause.text,
                )
                .blamed_on(p, f)
                .with_span(clause.span);
                if let Some(input) = self.input_object(f) {
                    fault = fault.with_data("input", input);
                }
                fault = fault.with_data("actual", value_to_json(p, exit, &fd.ret));
                self.violation = Some((f, fault.clone()));
                return Err(Flow::Fault(Box::new(fault)));
            }
        }
        Ok(())
    }

    /// A `@builtin` call (S58): the Rust implementation checks its own
    /// `requires` and its `ensures` are trusted. No frame is pushed while it
    /// runs, so a precondition fault names the calling declaration as
    /// `caller`; the builtin joins the backtrace as the fault leaves it.
    fn call_builtin(
        &mut self,
        stub: DefId,
        id: &str,
        args: Vec<Value>,
        span: Option<Span>,
    ) -> Eval<Value> {
        let Some(imp) = builtins::lookup(id) else {
            return self.internal(
                "unknown_builtin",
                render(&messages::DETAIL_UNKNOWN_BUILTIN, &[("id", id)]),
                span,
            );
        };
        imp(self, &args).map_err(|mut fault| {
            if fault.span.is_none() {
                fault.span = span;
            }
            fault.push_frame(Frame { def: stub, span });
            Flow::Fault(Box::new(fault))
        })
    }

    fn eval_cap_call(
        &mut self,
        handle: &'p Expr,
        capability: DefId,
        op: u32,
        args: &'p [Expr],
        span: Span,
    ) -> Eval<Value> {
        let h = match self.eval_expr(handle)? {
            Value::Capability(h) => h,
            other => return self.confusion("a capability handle", &other, handle.span),
        };
        let values = self.eval_all(args)?;
        self.call_operation(capability, op, h, values, span)
    }

    /// A capability operation call (S31, toolchain 1.7.1): the operation's
    /// `requires`, evaluated by the caller before the handle is invoked;
    /// the effect event; the host call; for a handle bound with
    /// `validate_results`, the validation of semantics S77; then its
    /// `ensures` with `result` bound to the handle's (validated) result
    /// (`attributed_to: "binding"`).
    fn call_operation(
        &mut self,
        capability: DefId,
        op: u32,
        handle: HandleId,
        args: Vec<Value>,
        span: Span,
    ) -> Eval<Value> {
        let p = self.program;
        let Some(opdef) = p
            .defs
            .get(capability.0 as usize)
            .and_then(|d| d.kind.as_capability())
            .and_then(|c| c.ops.get(op as usize))
        else {
            return self.internal(
                "unknown_operation",
                render(
                    &messages::DETAIL_UNKNOWN_OPERATION,
                    &[("def", &capability.0.to_string()), ("op", &op.to_string())],
                ),
                span,
            );
        };
        let identity = operation_identity(p, capability, &opdef.name);
        let check = self.config.check_contracts;
        let table = Table::Op { capability, op };
        if check && !opdef.requires.is_empty() {
            self.push_env(table);
            for (param, v) in opdef.params.iter().zip(&args) {
                self.bind(param.local, v.clone());
            }
            let outcome = self.check_op_requires(opdef, &identity);
            self.envs.pop();
            outcome?;
        }
        if self.config.trace_effects {
            let def = self.current_def().unwrap_or(capability);
            self.effects.push(EffectEvent {
                handle,
                op: opdef.name.clone(),
                def,
                span,
            });
        }
        let saved = (check && !opdef.ensures.is_empty()).then(|| args.clone());
        let binding = self.binding_of(handle);
        let result = match self.call_handle(handle, &opdef.name, args) {
            Ok(v) => v,
            Err(mut fault) => {
                if fault.span.is_none() {
                    fault.span = Some(span);
                }
                // Semantics S76: the memory fault an adapter host reports is
                // raised at the operation call and blamed, as every
                // exhaustion is (toolchain 1.7.3), on the innermost active
                // declaration (none at the top level, S72). The host cannot
                // name it (`HostContext` carries only the program).
                if fault.kind == FaultKind::ResourceExhaustion(Resource::Memory)
                    && fault.symbol.is_none()
                {
                    if let Some(def) = self.current_def() {
                        fault = fault.blamed_on(p, def);
                    }
                }
                return Err(Flow::Fault(Box::new(fault)));
            }
        };
        // Semantics S77: a validating binding's result is validated after the
        // host returned and before the `ensures` clauses run, in every
        // contract mode; the clauses see the value that replaced it.
        let result = match binding {
            Some((label, true)) => {
                match crate::boundary::validate_result(self, capability, op, &label, span, result) {
                    Ok(v) => v,
                    Err(fault) => return Err(Flow::Fault(Box::new(fault))),
                }
            }
            _ => result,
        };
        if let Some(args) = saved {
            self.push_env(table);
            for (param, v) in opdef.params.iter().zip(args) {
                self.bind(param.local, v);
            }
            if let Some(r) = opdef.result_local {
                self.bind(r, result.clone());
            }
            let outcome = self.check_op_ensures(opdef, &identity, &result);
            self.envs.pop();
            outcome?;
        }
        Ok(result)
    }

    fn check_op_requires(&mut self, opdef: &'p OpDef, identity: &StableId) -> Eval<()> {
        for clause in &opdef.requires {
            if !self.clause_holds(clause, Some(identity))? {
                let p = self.program;
                let mut fault = Fault::contract_violation(
                    ClauseKind::Requires,
                    AttributedTo::Caller,
                    &clause.text,
                )
                .with_symbol(identity.clone())
                .with_span(clause.span)
                .with_data("input", self.op_input(opdef));
                if let Some(caller) = self.current_def() {
                    fault = fault.with_data("caller", json!(p.display_path(caller)));
                }
                return Err(Flow::Fault(Box::new(fault)));
            }
        }
        Ok(())
    }

    fn check_op_ensures(
        &mut self,
        opdef: &'p OpDef,
        identity: &StableId,
        result: &Value,
    ) -> Eval<()> {
        for clause in &opdef.ensures {
            if !self.clause_holds(clause, Some(identity))? {
                let p = self.program;
                let fault = Fault::contract_violation(
                    ClauseKind::Ensures,
                    AttributedTo::Binding,
                    &clause.text,
                )
                .with_symbol(identity.clone())
                .with_span(clause.span)
                .with_data("input", self.op_input(opdef))
                .with_data("actual", value_to_json(p, result, &opdef.ret));
                return Err(Flow::Fault(Box::new(fault)));
            }
        }
        Ok(())
    }

    /// The operation's arguments by parameter name, from its environment.
    fn op_input(&self, opdef: &OpDef) -> Json {
        let env = self.env();
        let mut out = Map::new();
        for param in &opdef.params {
            if let Some(Some(v)) = env.values.get(param.local.0 as usize) {
                out.insert(
                    param.name.clone(),
                    value_to_json(self.program, v, &param.ty),
                );
            }
        }
        Json::Object(out)
    }
}

/// Add the activation of `def` (entered through `span`) to a fault's
/// backtrace as the fault leaves it.
fn leaving(flow: Flow, def: DefId, span: Option<Span>) -> Flow {
    match flow {
        Flow::Fault(mut fault) => {
            fault.push_frame(Frame { def, span });
            Flow::Fault(fault)
        }
        other => other,
    }
}

/// Match `value` against `pattern`, collecting the bindings it makes; a
/// failed match leaves no bindings that matter (the caller discards them).
fn matches(pattern: &Pattern, value: &Value, binds: &mut Vec<(LocalId, Value)>) -> bool {
    match (&pattern.kind, value) {
        (PatternKind::Wildcard, _) => true,
        (PatternKind::Bind(local), v) => {
            binds.push((*local, v.clone()));
            true
        }
        (PatternKind::Int(n), Value::Int(v)) => n == v,
        (PatternKind::Text(t), Value::Text(v)) => t.as_str() == &**v,
        (PatternKind::Bool(b), Value::Bool(v)) => b == v,
        (
            PatternKind::Ctor { def, variant, args },
            Value::Enum {
                def: vd,
                variant: vv,
                payload,
            },
        ) => {
            def == vd
                && variant == vv
                && args.len() == payload.len()
                && args
                    .iter()
                    .zip(payload.iter())
                    .all(|(p, v)| matches(p, v, binds))
        }
        (PatternKind::Unwrap { def, arg }, Value::Wrapper { def: vd, inner }) => {
            def == vd && matches(arg, inner, binds)
        }
        _ => false,
    }
}
