//! The compile-time proof rule's evaluator (`spec/semantics.md` 3.5, S16):
//! which typed expressions are *constant expressions*, their evaluation
//! with the dynamic semantics of Section 6 (6.3 arithmetic, 6.5 equality,
//! the `division_by_zero` fault), the evaluation of a refinement's `where`
//! predicate in the *compile-time evaluable subset*, and the value JSON of
//! S40 that `refinement_false` reports as `actual`.
//!
//! The evaluator works on the typed HIR (`cpl_hir::Expr`), so a constant's
//! parts that the typer already proved (`Some(2)` at `Option<Positive>`
//! carries a `Refine` around `2`) are evaluated through their proof nodes.
//! It never runs a user function: a predicate that calls one, reads a
//! field, or uses any other form outside the subset is reported as
//! [`Fail::Unsupported`] with the note S16 step 2 prescribes.

use cpl_hir::{
    Block, DefId, DefKind, Expr, ExprKind, FnBody, Lit, LocalId, Pattern, PatternKind, Program,
    Stmt,
};
use cpl_syntax::ast::{BinaryOp, UnaryOp};
use num_bigint::BigInt;
use num_traits::Zero;
use serde_json::json;
use std::collections::HashMap;

/// A compile-time value (6.1, refinements erased).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Value {
    Int(BigInt),
    Bool(bool),
    Text(String),
    List(Vec<Value>),
    /// An enum value, the prelude's `Unit`, `Option`, and `Result` included.
    Enum {
        def: DefId,
        variant: u32,
        payload: Vec<Value>,
    },
    Wrapper {
        def: DefId,
        inner: Box<Value>,
    },
    /// Fields in declaration order.
    Struct {
        def: DefId,
        fields: Vec<Value>,
    },
}

/// Why an evaluation did not produce a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Fail {
    /// A fault of Section 6.7 (`division_by_zero`): `constant_evaluation_fault`.
    Fault(&'static str),
    /// A form outside the compile-time evaluable subset (S16 step 2): the
    /// note of `cannot_prove_refinement`.
    Unsupported(String),
}

/// S16's grammar of constant expressions over the typed HIR: literals,
/// `Unit` and every constructor application with constant payloads, unary
/// `-` and `not`, the binary operators, list literals, and struct literals
/// without spread, all of constant parts (parentheses leave no HIR node).
/// A `Refine` or `Widen` the typer put around a constant part is
/// transparent. Names, calls, member accesses, `if`, `match`, `forall`,
/// holes, and `?` are not constant.
pub(crate) fn is_constant(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Lit(_) => true,
        ExprKind::Ctor { args, .. } | ExprKind::List(args) => args.iter().all(is_constant),
        ExprKind::Wrap { arg, .. } => is_constant(arg),
        ExprKind::Unary { operand, .. } => is_constant(operand),
        ExprKind::Binary { lhs, rhs, .. } => is_constant(lhs) && is_constant(rhs),
        ExprKind::StructLit {
            spread: None,
            fields,
            ..
        } => fields.iter().all(|(_, f)| is_constant(f)),
        ExprKind::Refine { expr, .. } | ExprKind::Widen { expr, .. } => is_constant(expr),
        _ => false,
    }
}

/// Evaluate a constant expression (S16 step 1).
pub(crate) fn eval_constant(program: &Program, e: &Expr) -> Result<Value, Fail> {
    let mut ev = Evaluator {
        program,
        env: HashMap::new(),
        refinement: None,
        constructor: false,
        fuel: 10_000,
        allocation_budget: 1_000_000,
    };
    ev.expr(e)
}

/// Evaluate the `where` predicate `pred` of refinement `refinement` with
/// `self` (`self_local`) bound to `value`, in the compile-time evaluable
/// subset (S16 step 2).
pub(crate) fn eval_predicate(
    program: &Program,
    refinement: DefId,
    pred: &Expr,
    self_local: LocalId,
    value: Value,
) -> Result<bool, Fail> {
    let mut ev = Evaluator {
        program,
        env: HashMap::new(),
        refinement: Some(refinement),
        constructor: false,
        fuel: 10_000,
        allocation_budget: 1_000_000,
    };
    ev.env.insert(self_local, value);
    match ev.expr(pred)? {
        Value::Bool(b) => Ok(b),
        _ => Err(Fail::Unsupported(
            "the predicate does not evaluate to a Bool".to_string(),
        )),
    }
}

/// A deliberately bounded proof attempt for a constructor. Unsupported or exhausted means a runtime check,
/// never a successful proof. No user functions or capabilities execute during this phase.
pub(crate) fn eval_constructor(
    program: &Program,
    value: &Expr,
    predicate: &Expr,
    self_local: LocalId,
) -> Result<bool, Fail> {
    let mut ev = Evaluator {
        program,
        env: HashMap::new(),
        refinement: None,
        constructor: true,
        fuel: 10_000,
        allocation_budget: 1_000_000,
    };
    let value = ev.expr(value)?;
    ev.env.insert(self_local, value);
    ev.expr_bool(predicate)
}

struct Evaluator<'p> {
    program: &'p Program,
    env: HashMap<LocalId, Value>,
    /// The refinement whose predicate is evaluated (`None` for a constant):
    /// names the notes of unsupported forms.
    refinement: Option<DefId>,
    constructor: bool,
    fuel: usize,
    allocation_budget: usize,
}

/// The compile-time members of the subset: `.len` and `.is_empty()`.
enum Member {
    Len,
    IsEmpty,
}

impl Evaluator<'_> {
    fn unsupported(&self, what: String) -> Fail {
        match self.refinement {
            Some(r) => {
                let name = self.program.def(r).name.display();
                Fail::Unsupported(format!(
                    "predicate of `{name}` {what}; use `check<{name}>(...)`"
                ))
            }
            None => Fail::Unsupported(format!("the expression {what}")),
        }
    }

    fn member(&self, callee: DefId) -> Option<Member> {
        let f = self.program.defs.get(callee.0 as usize)?.kind.as_fn()?;
        match &f.body {
            FnBody::Builtin(id) => match id.as_str() {
                "list_len" | "text_len" | "bytes_len" => Some(Member::Len),
                "list_is_empty" | "text_is_empty" | "bytes_is_empty" => Some(Member::IsEmpty),
                _ => None,
            },
            FnBody::Block(_) => None,
        }
    }

    fn int(&self, v: Value) -> Result<BigInt, Fail> {
        match v {
            Value::Int(n) => Ok(n),
            _ => Err(Fail::Unsupported("an operand is not an Int".to_string())),
        }
    }

    fn bool(&self, v: Value) -> Result<bool, Fail> {
        match v {
            Value::Bool(b) => Ok(b),
            _ => Err(Fail::Unsupported("an operand is not a Bool".to_string())),
        }
    }

    fn expr(&mut self, e: &Expr) -> Result<Value, Fail> {
        let value = self.expr_inner(e)?;
        let cost = value_cost(&value, self.allocation_budget)
            .ok_or_else(|| self.unsupported("exceeds the compile-time allocation budget".into()))?;
        self.allocation_budget -= cost;
        Ok(value)
    }

    fn expr_inner(&mut self, e: &Expr) -> Result<Value, Fail> {
        if self.fuel == 0 {
            return Err(self.unsupported("exceeds the compile-time evaluation budget".into()));
        }
        self.fuel -= 1;
        {
            match &e.kind {
                ExprKind::Lit(Lit::Int(n)) if n.bits() > 16_384 => {
                    return Err(self.unsupported("exceeds the compile-time integer budget".into()))
                }
                ExprKind::Lit(Lit::Text(s)) if s.len() > 65_536 => {
                    return Err(self.unsupported("exceeds the compile-time text budget".into()))
                }
                ExprKind::List(xs) if xs.len() > 65_536 => {
                    return Err(
                        self.unsupported("exceeds the compile-time collection budget".into())
                    )
                }
                _ => {}
            }
        }
        match &e.kind {
            ExprKind::Lit(Lit::Int(n)) => Ok(Value::Int(n.clone())),
            ExprKind::Lit(Lit::Text(t)) => Ok(Value::Text(t.clone())),
            ExprKind::Lit(Lit::Bool(b)) => Ok(Value::Bool(*b)),
            ExprKind::Local(id) => match self.env.get(id) {
                Some(v) => Ok(v.clone()),
                None => Err(self.unsupported("reads a name that is not a constant".to_string())),
            },
            ExprKind::Ctor { def, variant, args } => Ok(Value::Enum {
                def: *def,
                variant: *variant,
                payload: self.all(args)?,
            }),
            ExprKind::Wrap { def, arg } => Ok(Value::Wrapper {
                def: *def,
                inner: Box::new(self.expr(arg)?),
            }),
            ExprKind::List(items) => Ok(Value::List(self.all(items)?)),
            ExprKind::StructLit {
                def,
                spread: None,
                fields,
                ..
            } => {
                let count = self
                    .program
                    .def(*def)
                    .kind
                    .as_struct()
                    .map_or(0, |s| s.fields.len());
                let mut values: Vec<Option<Value>> = vec![None; count];
                for (index, f) in fields {
                    let v = self.expr(f)?;
                    if let Some(slot) = values.get_mut(*index as usize) {
                        *slot = Some(v);
                    }
                }
                let fields = values
                    .into_iter()
                    .collect::<Option<Vec<Value>>>()
                    .ok_or_else(|| self.unsupported("builds an incomplete struct".to_string()))?;
                Ok(Value::Struct { def: *def, fields })
            }
            ExprKind::Refine { expr, .. } | ExprKind::Widen { expr, .. } => self.expr(expr),
            ExprKind::Unary { op, operand } => {
                let v = self.expr(operand)?;
                match op {
                    UnaryOp::Neg => Ok(Value::Int(-self.int(v)?)),
                    UnaryOp::Not => Ok(Value::Bool(!self.bool(v)?)),
                }
            }
            ExprKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs),
            ExprKind::Parallel { .. } => {
                Err(self.unsupported("parallel computation requires runtime execution".into()))
            }
            ExprKind::Forall {
                binder,
                collection,
                predicate,
            } => {
                let Value::List(items) = self.expr(collection)? else {
                    return Err(self.unsupported("ranges `forall` over a non-list".to_string()));
                };
                for item in items {
                    self.env.insert(*binder, item);
                    if !self.expr_bool(predicate)? {
                        return Ok(Value::Bool(false));
                    }
                }
                Ok(Value::Bool(true))
            }
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => {
                if self.expr_bool(cond)? {
                    self.block(then_block)
                } else {
                    match else_block {
                        Some(b) => self.block(b),
                        None => Ok(self.unit()),
                    }
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                let v = self.expr(scrutinee)?;
                for arm in arms {
                    let mut bound = Vec::new();
                    if matches(&arm.pattern, &v, &mut bound) {
                        for (local, value) in bound {
                            self.env.insert(local, value);
                        }
                        return self.block(&arm.body);
                    }
                }
                Err(self.unsupported("has a `match` that no arm matches".to_string()))
            }
            ExprKind::Call { callee, args, .. } => match self.member(*callee) {
                Some(m) => {
                    let receiver = match args.first() {
                        Some(r) => self.expr(r)?,
                        None => {
                            return Err(
                                self.unsupported("calls a member without a receiver".to_string())
                            )
                        }
                    };
                    let len = match &receiver {
                        Value::List(xs) => xs.len(),
                        Value::Text(t) => t.chars().count(),
                        _ => {
                            return Err(
                                self.unsupported("reads `len` of an unsupported value".to_string())
                            )
                        }
                    };
                    Ok(match m {
                        Member::Len => Value::Int(BigInt::from(len)),
                        Member::IsEmpty => Value::Bool(len == 0),
                    })
                }
                None => {
                    let name = self.program.def(*callee).name.display();
                    Err(self.unsupported(format!("calls `{name}`")))
                }
            },
            ExprKind::Check { .. } => Err(self.unsupported("calls `check`".to_string())),
            ExprKind::Field { base, def, index } => {
                if self.constructor {
                    if let Value::Struct { fields, .. } = self.expr(base)? {
                        return fields
                            .get(*index as usize)
                            .cloned()
                            .ok_or_else(|| self.unsupported("reads an absent field".into()));
                    }
                }
                let field = self
                    .program
                    .def(*def)
                    .kind
                    .as_struct()
                    .and_then(|s| s.fields.get(*index as usize))
                    .map_or("?".to_string(), |f| f.name.clone());
                let _ = base;
                Err(self.unsupported(format!("reads the field `{field}`")))
            }
            ExprKind::CapCall { .. } => {
                Err(self.unsupported("calls a capability operation".to_string()))
            }
            ExprKind::ContextOp { .. } => {
                Err(self.unsupported("uses a state machine's context".to_string()))
            }
            ExprKind::Propagate(_) => Err(self.unsupported("uses `?`".to_string())),
            ExprKind::Hole { name, .. } => {
                Err(self.unsupported(format!("contains the hole `?{name}`")))
            }
            ExprKind::StructLit { .. } => Err(self.unsupported("uses a struct spread".to_string())),
            ExprKind::Gen(_) => Err(self.unsupported("uses a generator".to_string())),
            ExprKind::Error => Err(self.unsupported("has an erroneous part".to_string())),
        }
    }

    fn expr_bool(&mut self, e: &Expr) -> Result<bool, Fail> {
        let v = self.expr(e)?;
        self.bool(v)
    }

    fn all(&mut self, es: &[Expr]) -> Result<Vec<Value>, Fail> {
        es.iter().map(|e| self.expr(e)).collect()
    }

    fn unit(&self) -> Value {
        Value::Enum {
            def: self.program.std_defs.unit,
            variant: 0,
            payload: Vec::new(),
        }
    }

    /// A block of a contract sub-expression: only `let` statements (S32).
    fn block(&mut self, b: &Block) -> Result<Value, Fail> {
        for s in &b.stmts {
            match s {
                Stmt::Let { local, value } => {
                    let v = self.expr(value)?;
                    self.env.insert(*local, v);
                }
                Stmt::Discard(value) => {
                    self.expr(value)?;
                }
                _ => {
                    return Err(
                        self.unsupported("contains a statement other than `let`".to_string())
                    )
                }
            }
        }
        match &b.tail {
            Some(t) => self.expr(t),
            None => Ok(self.unit()),
        }
    }

    fn binary(&mut self, op: BinaryOp, lhs: &Expr, rhs: &Expr) -> Result<Value, Fail> {
        // `and` and `or` evaluate their right operand only when needed (6.2).
        match op {
            BinaryOp::And => {
                return Ok(Value::Bool(self.expr_bool(lhs)? && self.expr_bool(rhs)?));
            }
            BinaryOp::Or => {
                return Ok(Value::Bool(self.expr_bool(lhs)? || self.expr_bool(rhs)?));
            }
            _ => {}
        }
        let l = self.expr(lhs)?;
        let r = self.expr(rhs)?;
        Ok(match op {
            BinaryOp::Eq => Value::Bool(l == r),
            BinaryOp::Ne => Value::Bool(l != r),
            BinaryOp::Concat => match (l, r) {
                (Value::List(mut a), Value::List(b)) => {
                    if a.len().saturating_add(b.len()) > 65_536 {
                        return Err(
                            self.unsupported("exceeds the compile-time collection budget".into())
                        );
                    }
                    a.extend(b);
                    Value::List(a)
                }
                (Value::Text(a), Value::Text(b)) => {
                    if a.len().saturating_add(b.len()) > 65_536 {
                        return Err(self.unsupported("exceeds the compile-time text budget".into()));
                    }
                    Value::Text(a + &b)
                }
                _ => return Err(self.unsupported("concatenates unsupported values".to_string())),
            },
            _ => {
                let a = self.int(l)?;
                let b = self.int(r)?;
                if a.bits().saturating_add(b.bits()) > 16_384 {
                    return Err(self.unsupported("exceeds the compile-time integer budget".into()));
                }
                match op {
                    BinaryOp::Add => Value::Int(a + b),
                    BinaryOp::Sub => Value::Int(a - b),
                    BinaryOp::Mul => Value::Int(a * b),
                    BinaryOp::Div | BinaryOp::Rem => {
                        if b.is_zero() {
                            return Err(Fail::Fault("division_by_zero"));
                        }
                        Value::Int(div_rem(&a, &b, op == BinaryOp::Div))
                    }
                    BinaryOp::Lt => Value::Bool(a < b),
                    BinaryOp::Le => Value::Bool(a <= b),
                    BinaryOp::Gt => Value::Bool(a > b),
                    BinaryOp::Ge => Value::Bool(a >= b),
                    _ => unreachable!("handled above"),
                }
            }
        })
    }
}

fn value_cost(value: &Value, limit: usize) -> Option<usize> {
    let mut pending = vec![value];
    let mut used = 0usize;
    while let Some(value) = pending.pop() {
        let amount = match value {
            Value::Int(n) => 32usize.saturating_add((n.bits() / 8) as usize),
            Value::Text(s) => 32usize.saturating_add(s.len()),
            Value::Bool(_) => 8,
            Value::List(xs) => {
                pending.extend(xs);
                32
            }
            Value::Enum { payload, .. } => {
                pending.extend(payload);
                32
            }
            Value::Struct { fields, .. } => {
                pending.extend(fields);
                32
            }
            Value::Wrapper { inner, .. } => {
                pending.push(inner);
                32
            }
        };
        used = used.checked_add(amount)?;
        if used > limit {
            return None;
        }
    }
    Some(used)
}

/// Truncating division and the remainder with the dividend's sign (S37):
/// `a == (a / b) * b + a % b`. `b` is nonzero. The one division of CPL is
/// `cpl_rtspec::arith::div_rem` (toolchain T9 as amended by T70, stdlib
/// S47), which both engines use too, so constant folding cannot disagree
/// with evaluation.
pub(crate) fn div_rem(a: &BigInt, b: &BigInt, quotient: bool) -> BigInt {
    let (q, r) = cpl_rtspec::arith::div_rem(a, b).expect("div_rem: the divisor is nonzero");
    if quotient {
        q
    } else {
        r
    }
}

/// Match a value against a typed pattern, collecting the bindings.
fn matches(p: &Pattern, v: &Value, bound: &mut Vec<(LocalId, Value)>) -> bool {
    match (&p.kind, v) {
        (PatternKind::Wildcard, _) => true,
        (PatternKind::Bind(local), _) => {
            bound.push((*local, v.clone()));
            true
        }
        (PatternKind::Int(n), Value::Int(m)) => n == m,
        (PatternKind::Text(t), Value::Text(u)) => t == u,
        (PatternKind::Bool(b), Value::Bool(c)) => b == c,
        (
            PatternKind::Ctor { def, variant, args },
            Value::Enum {
                def: d,
                variant: w,
                payload,
            },
        ) => {
            def == d
                && variant == w
                && args.len() == payload.len()
                && args.iter().zip(payload).all(|(a, x)| matches(a, x, bound))
        }
        (PatternKind::Unwrap { def, arg }, Value::Wrapper { def: d, inner }) => {
            def == d && matches(arg, inner, bound)
        }
        _ => false,
    }
}

/// The `$type` of a value JSON object (S40): a prelude type's bare name,
/// every other declaration's qualified name.
fn type_name(program: &Program, def: DefId) -> String {
    let d = program.def(def);
    if program.module(d.module).path == ["std", "prelude"] {
        d.name.display()
    } else {
        program.display_path(def)
    }
}

/// The value JSON of S40.
pub(crate) fn value_json(program: &Program, v: &Value) -> serde_json::Value {
    match v {
        Value::Int(n) => n
            .to_string()
            .parse::<serde_json::Number>()
            .map(serde_json::Value::Number)
            .unwrap_or_else(|_| json!(n.to_string())),
        Value::Bool(b) => json!(b),
        Value::Text(t) => json!(t),
        Value::List(xs) => {
            serde_json::Value::Array(xs.iter().map(|x| value_json(program, x)).collect())
        }
        Value::Enum {
            def,
            variant,
            payload,
        } => {
            let mut m = serde_json::Map::new();
            m.insert("$type".into(), json!(type_name(program, *def)));
            if *def != program.std_defs.unit {
                let name = program
                    .def(*def)
                    .kind
                    .as_enum()
                    .and_then(|e| e.variants.get(*variant as usize))
                    .map_or("?".to_string(), |v| v.name.clone());
                m.insert("$variant".into(), json!(name));
                if !payload.is_empty() {
                    m.insert(
                        "$payload".into(),
                        serde_json::Value::Array(
                            payload.iter().map(|x| value_json(program, x)).collect(),
                        ),
                    );
                }
            }
            serde_json::Value::Object(m)
        }
        Value::Wrapper { def, inner } => json!({
            "$type": type_name(program, *def),
            "$payload": [value_json(program, inner)],
        }),
        Value::Struct { def, fields } => {
            let mut m = serde_json::Map::new();
            m.insert("$type".into(), json!(type_name(program, *def)));
            if let DefKind::Struct(s) = &program.def(*def).kind {
                for (f, v) in s.fields.iter().zip(fields) {
                    m.insert(f.name.clone(), value_json(program, v));
                }
            }
            serde_json::Value::Object(m)
        }
    }
}

/// The value text of S40 (CPL literal syntax), for messages.
pub(crate) fn value_text(program: &Program, v: &Value) -> String {
    match v {
        Value::Int(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Text(t) => cpl_diag::quote_text(t),
        Value::List(xs) => format!(
            "[{}]",
            xs.iter()
                .map(|x| value_text(program, x))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Enum {
            def,
            variant,
            payload,
        } => {
            let name = program
                .def(*def)
                .kind
                .as_enum()
                .and_then(|e| e.variants.get(*variant as usize))
                .map_or("?".to_string(), |v| v.name.clone());
            if payload.is_empty() {
                name
            } else {
                format!(
                    "{name}({})",
                    payload
                        .iter()
                        .map(|x| value_text(program, x))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
        Value::Wrapper { def, inner } => format!(
            "{}({})",
            program.def(*def).name.display(),
            value_text(program, inner)
        ),
        Value::Struct { def, fields } => {
            let names: Vec<String> = match &program.def(*def).kind {
                DefKind::Struct(s) => s.fields.iter().map(|f| f.name.clone()).collect(),
                _ => Vec::new(),
            };
            let parts: Vec<String> = names
                .iter()
                .zip(fields)
                .map(|(n, v)| format!("{n}: {}", value_text(program, v)))
                .collect();
            if parts.is_empty() {
                format!("{} {{}}", program.def(*def).name.display())
            } else {
                format!(
                    "{} {{ {} }}",
                    program.def(*def).name.display(),
                    parts.join(", ")
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpl_diag::{FileId, SourceMap, Span};
    use cpl_hir::{Arm, Ty};
    use std::sync::Arc;

    fn span() -> Span {
        Span::point(FileId(0), 0)
    }

    fn program() -> Program {
        Program::empty(Arc::new(SourceMap::new()))
    }

    fn int(n: i64) -> Expr {
        Expr::new(ExprKind::Lit(Lit::Int(BigInt::from(n))), Ty::Int, span())
    }

    fn text(t: &str) -> Expr {
        Expr::new(ExprKind::Lit(Lit::Text(t.into())), Ty::Text, span())
    }

    fn bool_lit(b: bool) -> Expr {
        Expr::new(ExprKind::Lit(Lit::Bool(b)), Ty::Bool, span())
    }

    fn bin(op: BinaryOp, l: Expr, r: Expr) -> Expr {
        let ty = match op {
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => {
                Ty::Int
            }
            BinaryOp::Concat => l.ty.clone(),
            _ => Ty::Bool,
        };
        Expr::new(
            ExprKind::Binary {
                op,
                lhs: Box::new(l),
                rhs: Box::new(r),
            },
            ty,
            span(),
        )
    }

    fn neg(e: Expr) -> Expr {
        Expr::new(
            ExprKind::Unary {
                op: UnaryOp::Neg,
                operand: Box::new(e),
            },
            Ty::Int,
            span(),
        )
    }

    fn local(id: u32, ty: Ty) -> Expr {
        Expr::new(ExprKind::Local(LocalId(id)), ty, span())
    }

    fn eval(e: &Expr) -> Result<Value, Fail> {
        eval_constant(&program(), e)
    }

    fn i(n: i64) -> Value {
        Value::Int(BigInt::from(n))
    }

    #[test]
    fn arithmetic_truncates_toward_zero() {
        // S37: 7 / -2 == -3, 7 % -2 == 1, -7 / 2 == -3, -7 % 2 == -1.
        let cases = [
            (BinaryOp::Div, 7, 2, 3),
            (BinaryOp::Div, -7, 2, -3),
            (BinaryOp::Div, 7, -2, -3),
            (BinaryOp::Div, -7, -2, 3),
            (BinaryOp::Rem, 7, 2, 1),
            (BinaryOp::Rem, -7, 2, -1),
            (BinaryOp::Rem, 7, -2, 1),
            (BinaryOp::Rem, -7, -2, -1),
            (BinaryOp::Add, 2, 1, 3),
            (BinaryOp::Sub, 2, 2, 0),
            (BinaryOp::Mul, 2, 3, 6),
        ];
        for (op, a, b, want) in cases {
            let a_expr = if a < 0 { neg(int(-a)) } else { int(a) };
            let b_expr = if b < 0 { neg(int(-b)) } else { int(b) };
            assert_eq!(
                eval(&bin(op, a_expr, b_expr)),
                Ok(i(want)),
                "{op:?} {a} {b}"
            );
        }
        let big = bin(
            BinaryOp::Mul,
            Expr::new(
                ExprKind::Lit(Lit::Int("12345678901234567890".parse().unwrap())),
                Ty::Int,
                span(),
            ),
            int(10),
        );
        assert_eq!(
            eval(&big),
            Ok(Value::Int("123456789012345678900".parse().unwrap()))
        );
    }

    #[test]
    fn division_by_zero_is_a_fault() {
        assert_eq!(
            eval(&bin(BinaryOp::Div, int(1), int(0))),
            Err(Fail::Fault("division_by_zero"))
        );
        assert_eq!(
            eval(&bin(BinaryOp::Rem, int(7), int(0))),
            Err(Fail::Fault("division_by_zero"))
        );
        // `and` and `or` short-circuit: the faulting operand is not evaluated.
        let guarded = bin(
            BinaryOp::And,
            bool_lit(false),
            bin(BinaryOp::Eq, bin(BinaryOp::Div, int(1), int(0)), int(1)),
        );
        assert_eq!(eval(&guarded), Ok(Value::Bool(false)));
    }

    #[test]
    fn concatenation_and_equality_are_structural() {
        let l = |xs: &[i64]| {
            Expr::new(
                ExprKind::List(xs.iter().map(|&x| int(x)).collect()),
                Ty::list(Ty::Int),
                span(),
            )
        };
        assert_eq!(
            eval(&bin(BinaryOp::Concat, l(&[1]), l(&[2, 3]))),
            Ok(Value::List(vec![i(1), i(2), i(3)]))
        );
        assert_eq!(
            eval(&bin(BinaryOp::Concat, text("a"), text("b"))),
            Ok(Value::Text("ab".into()))
        );
        assert_eq!(
            eval(&bin(BinaryOp::Eq, l(&[1, 2]), l(&[1, 2]))),
            Ok(Value::Bool(true))
        );
        assert_eq!(
            eval(&bin(BinaryOp::Ne, l(&[1, 2]), l(&[2, 1]))),
            Ok(Value::Bool(true))
        );
    }

    #[test]
    fn constant_expressions_follow_s16() {
        assert!(is_constant(&int(1)));
        assert!(is_constant(&neg(neg(int(3)))));
        assert!(is_constant(&bin(BinaryOp::Add, int(2), int(1))));
        let ctor = Expr::new(
            ExprKind::Ctor {
                def: DefId(0),
                variant: 0,
                args: vec![int(2)],
            },
            Ty::Error,
            span(),
        );
        assert!(is_constant(&ctor));
        // A name is not a constant, even one bound to a literal.
        assert!(!is_constant(&local(0, Ty::Int)));
        assert!(!is_constant(&bin(BinaryOp::Add, local(0, Ty::Int), int(1))));
        let call = Expr::new(
            ExprKind::Call {
                callee: DefId(0),
                generic_args: Vec::new(),
                args: vec![int(1)],
                using: Vec::new(),
            },
            Ty::Int,
            span(),
        );
        assert!(!is_constant(&call));
    }

    #[test]
    fn predicates_evaluate_with_self() {
        let p = program();
        // self > 0
        let positive = bin(BinaryOp::Gt, local(0, Ty::Int), int(0));
        let check = |pred: &Expr, v: Value| eval_predicate(&p, DefId(0), pred, LocalId(0), v);
        assert_eq!(check(&positive, i(2)), Ok(true));
        assert_eq!(check(&positive, i(0)), Ok(false));
        // 10 / self > 0 faults at 0.
        let reciprocal = bin(
            BinaryOp::Gt,
            bin(BinaryOp::Div, int(10), local(0, Ty::Int)),
            int(0),
        );
        assert_eq!(
            check(&reciprocal, i(0)),
            Err(Fail::Fault("division_by_zero"))
        );
        // forall x in self: x > 0
        let all = Expr::new(
            ExprKind::Forall {
                binder: LocalId(1),
                collection: Box::new(local(0, Ty::list(Ty::Int))),
                predicate: Box::new(bin(BinaryOp::Gt, local(1, Ty::Int), int(0))),
            },
            Ty::Bool,
            span(),
        );
        assert_eq!(check(&all, Value::List(vec![i(1), i(2)])), Ok(true));
        assert_eq!(check(&all, Value::List(vec![i(1), i(0)])), Ok(false));
        assert_eq!(check(&all, Value::List(Vec::new())), Ok(true));
        // if self % 2 == 0 { true } else { self > 0 }
        let block = |e: Expr| Block {
            stmts: Vec::new(),
            ty: e.ty.clone(),
            tail: Some(Box::new(e)),
            span: span(),
        };
        let parity = Expr::new(
            ExprKind::If {
                cond: Box::new(bin(
                    BinaryOp::Eq,
                    bin(BinaryOp::Rem, local(0, Ty::Int), int(2)),
                    int(0),
                )),
                then_block: block(bool_lit(true)),
                else_block: Some(block(bin(BinaryOp::Gt, local(0, Ty::Int), int(0)))),
            },
            Ty::Bool,
            span(),
        );
        assert_eq!(check(&parity, i(4)), Ok(true));
        assert_eq!(check(&parity, i(-3)), Ok(false));
        // match self { 1 => true  n => n > 5 }
        let arms = vec![
            Arm {
                pattern: Pattern {
                    kind: PatternKind::Int(BigInt::from(1)),
                    ty: Ty::Int,
                    span: span(),
                },
                body: block(bool_lit(true)),
                span: span(),
            },
            Arm {
                pattern: Pattern {
                    kind: PatternKind::Bind(LocalId(2)),
                    ty: Ty::Int,
                    span: span(),
                },
                body: block(bin(BinaryOp::Gt, local(2, Ty::Int), int(5))),
                span: span(),
            },
        ];
        let m = Expr::new(
            ExprKind::Match {
                scrutinee: Box::new(local(0, Ty::Int)),
                arms,
            },
            Ty::Bool,
            span(),
        );
        assert_eq!(check(&m, i(1)), Ok(true));
        assert_eq!(check(&m, i(3)), Ok(false));
        assert_eq!(check(&m, i(9)), Ok(true));
    }

    #[test]
    fn value_json_follows_s40() {
        let p = program();
        assert_eq!(value_json(&p, &i(3)), json!(3));
        assert_eq!(
            value_json(
                &p,
                &Value::List(vec![Value::Text("a".into()), Value::Bool(true)])
            ),
            json!(["a", true])
        );
        let huge = Value::Int("99999999999999999999".parse().unwrap());
        assert_eq!(value_json(&p, &huge).to_string(), "99999999999999999999");
        assert_eq!(value_text(&p, &Value::Text("a\"b".into())), "\"a\\\"b\"");
    }
}
