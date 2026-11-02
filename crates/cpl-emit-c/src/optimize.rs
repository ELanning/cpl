//! Bounded, fault-preserving scalar optimizations. No calls, contracts, or faulting operations are erased.
use cpl_hir::{Expr, ExprKind, Lit};
use cpl_syntax::ast::{BinaryOp as B, UnaryOp as U};
use num_bigint::BigInt;

pub(crate) fn literal(e: &Expr, fuel: &mut usize) -> Option<Lit> {
    if *fuel == 0 {
        return None;
    }
    *fuel -= 1;
    match &e.kind {
        ExprKind::Lit(Lit::Int(n)) if n.bits() > 4096 => None,
        ExprKind::Lit(Lit::Text(t)) if t.len() > 65536 => None,
        ExprKind::Lit(l) => Some(l.clone()),
        ExprKind::Unary { op, operand } => match (op, literal(operand, fuel)?) {
            (U::Neg, Lit::Int(n)) if n.bits() <= 4096 => Some(Lit::Int(-n)),
            (U::Not, Lit::Bool(b)) => Some(Lit::Bool(!b)),
            _ => None,
        },
        ExprKind::Binary { op, lhs, rhs } => {
            let a = literal(lhs, fuel)?;
            if matches!(
                (op, &a),
                (B::And, Lit::Bool(false)) | (B::Or, Lit::Bool(true))
            ) {
                return Some(a);
            }
            match (a, literal(rhs, fuel)?) {
                (Lit::Int(a), Lit::Int(b)) if a.bits() + b.bits() <= 4096 => Some(match op {
                    B::Add => Lit::Int(a + b),
                    B::Sub => Lit::Int(a - b),
                    B::Mul => Lit::Int(a * b),
                    B::Div if b != BigInt::from(0) => Lit::Int(a / b),
                    B::Rem if b != BigInt::from(0) => Lit::Int(a % b),
                    B::Eq => Lit::Bool(a == b),
                    B::Ne => Lit::Bool(a != b),
                    B::Lt => Lit::Bool(a < b),
                    B::Le => Lit::Bool(a <= b),
                    B::Gt => Lit::Bool(a > b),
                    B::Ge => Lit::Bool(a >= b),
                    _ => return None,
                }),
                (Lit::Bool(a), Lit::Bool(b)) => Some(Lit::Bool(match op {
                    B::And => a && b,
                    B::Or => a || b,
                    B::Eq => a == b,
                    B::Ne => a != b,
                    _ => return None,
                })),
                (Lit::Text(a), Lit::Text(b)) if a.len() + b.len() <= 65536 => Some(match op {
                    B::Concat => Lit::Text(a + &b),
                    B::Eq => Lit::Bool(a == b),
                    B::Ne => Lit::Bool(a != b),
                    _ => return None,
                }),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Select an identity operand only when the other operand is a literal with no evaluation or fault.
pub(crate) fn identity(e: &Expr) -> Option<&Expr> {
    let ExprKind::Binary { op, lhs, rhs } = &e.kind else {
        return None;
    };
    let zero = |e: &Expr| matches!(&e.kind,ExprKind::Lit(Lit::Int(n)) if *n==BigInt::from(0));
    let one = |e: &Expr| matches!(&e.kind,ExprKind::Lit(Lit::Int(n)) if *n==BigInt::from(1));
    let empty = |e: &Expr| {
        matches!(&e.kind,ExprKind::List(xs) if xs.is_empty())
            || matches!(&e.kind,ExprKind::Lit(Lit::Text(t)) if t.is_empty())
    };
    if (matches!(op, B::Add | B::Sub) && zero(rhs))
        || (matches!(op, B::Mul | B::Div) && one(rhs))
        || (*op == B::And && matches!(rhs.kind, ExprKind::Lit(Lit::Bool(true))))
        || (*op == B::Or && matches!(rhs.kind, ExprKind::Lit(Lit::Bool(false))))
        || (*op == B::Concat && empty(rhs))
    {
        return Some(lhs);
    }
    if (*op == B::Add && zero(lhs))
        || (*op == B::Mul && one(lhs))
        || (*op == B::Concat && empty(lhs))
    {
        return Some(rhs);
    }
    None
}
