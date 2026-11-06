//! The lowered IR the host printers read: the typed HIR with every control-flow construct made a
//! statement, every contract an explicit check, and evaluation order explicit.
//!
//! Expressions here have no control flow. Each operand is evaluated left to right, and a
//! potentially faulting operation is a call (`Div`, `Builtin`, `Call`, `CapCall`, `Pred`), which
//! every host evaluates in lexical order. An expression that would have needed statements
//! between its operands was split by the lowering into temporaries first.

use cpl_hir::{DefId, Ty};
use num_bigint::BigInt;

/// A variable of a lowered function: an index into [`LFn::vars`].
pub type Var = u32;

/// One variable: the HIR local it stands for (named after it), or a temporary.
#[derive(Clone, Debug)]
pub struct VarDef {
    pub name: String,
    pub ty: Ty,
    /// Assigned after its definition (a `var` local, a loop temporary, a result slot).
    pub mutable: bool,
}

/// A function of the program, lowered.
#[derive(Clone, Debug)]
pub struct LFn {
    pub def: DefId,
    pub generics: Vec<String>,
    pub params: Vec<Var>,
    /// Capability parameters.
    pub using: Vec<Var>,
    pub ret: Ty,
    pub vars: Vec<VarDef>,
    /// The `requires` checks, run first (blamed on the caller).
    pub requires: Vec<LStmt>,
    /// The header `decreases` measure: its statements and value, checked after `requires`.
    pub measure: Option<(Vec<LStmt>, LExpr, String)>,
    /// The body: every path ends in [`LStmt::Return`] or a fault.
    pub body: Vec<LStmt>,
    /// The `ensures` checks, with `result` (when the function names it) bound to the exit value.
    pub ensures: Vec<LStmt>,
    pub result: Option<Var>,
}

/// The predicate of a refinement declaration or a struct's invariant, lowered to a function of
/// its `self` value returning `Bool`.
#[derive(Clone, Debug)]
pub struct LPred {
    pub def: DefId,
    /// The struct's generics (a refinement has none).
    pub generics: Vec<String>,
    pub self_var: Var,
    pub self_ty: Ty,
    pub vars: Vec<VarDef>,
    pub body: Vec<LStmt>,
    pub value: LExpr,
}

#[derive(Clone, Debug)]
pub enum LStmt {
    /// Define `var` with `value`.
    Let {
        var: Var,
        value: LExpr,
    },
    /// Define `var` without a value; every path assigns it before it is read.
    Decl {
        var: Var,
    },
    Assign {
        var: Var,
        value: LExpr,
    },
    /// Evaluate for its effects or faults.
    Expr(LExpr),
    If {
        cond: LExpr,
        then: Vec<LStmt>,
        els: Vec<LStmt>,
    },
    /// The first arm whose pattern matches `scrutinee` runs. The checker made the arms
    /// exhaustive.
    Match {
        scrutinee: Var,
        arms: Vec<LArm>,
    },
    /// Run `body` until a `Break` of this label.
    Loop {
        label: u32,
        body: Vec<LStmt>,
    },
    /// Bind each element of the list to `var` in turn (a `Break` of `label` ends the loop).
    ForEach {
        label: u32,
        var: Var,
        list: LExpr,
        body: Vec<LStmt>,
    },
    Break {
        label: u32,
    },
    /// Leave the function's body with this exit value (its `ensures` then run).
    Return(LExpr),
    Fault(Fault),
    /// `list = list.push(value)`, in place where the host can.
    Push {
        list: Var,
        value: LExpr,
    },
    /// The evaluation of a contract expression of the declaration `owner` (a clause, a
    /// measure, a refinement or invariant predicate) begins: a fault that escapes it is
    /// reported as `owner`'s, unless a clause inside it claims the fault first (semantics S34).
    /// A fault ends the call, so the host reads the innermost clause still open when it
    /// arrives.
    ClauseEnter {
        owner: String,
    },
    /// The contract expression of the last `ClauseEnter` was evaluated.
    ClauseLeave,
}

#[derive(Clone, Debug)]
pub struct LArm {
    pub pattern: LPat,
    pub body: Vec<LStmt>,
}

/// A pattern, with the type of the value it matches.
#[derive(Clone, Debug)]
pub enum LPat {
    Wild,
    Bind(Var),
    Int(BigInt),
    Text(String),
    Bool(bool),
    /// A variant of an enum (`Option` and `Result` included): `ty` is the scrutinee's type.
    Ctor {
        def: DefId,
        variant: u32,
        args: Vec<LPat>,
        ty: Ty,
    },
    /// A wrapper's inner value.
    Unwrap {
        def: DefId,
        arg: Box<LPat>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Neg,
    Lt,
    Le,
    Gt,
    Ge,
    /// Structural equality at the operands' type.
    Eq,
    Ne,
    Not,
    /// Short-circuit; the right operand has no statements of its own.
    And,
    Or,
    /// `++` of two texts, lists, or byte strings (the operand type says which).
    Concat,
}

#[derive(Clone, Debug)]
pub enum LExpr {
    Var(Var),
    Int(BigInt),
    Text(String),
    Bool(bool),
    Unit,
    /// A function of the program, with the calling context.
    Call {
        def: DefId,
        generics: Vec<Ty>,
        args: Vec<LExpr>,
        using: Vec<LExpr>,
    },
    /// A `@builtin` of the standard library, by registry id; `ty` is the result type and
    /// `generics` the stub's type arguments.
    Builtin {
        id: String,
        generics: Vec<Ty>,
        args: Vec<LExpr>,
        ty: Ty,
    },
    /// An operator; `ty` is the operands' type (the result's for arithmetic).
    Op {
        op: Op,
        args: Vec<LExpr>,
        ty: Ty,
    },
    /// Truncating `/` or `%`; division by zero faults, blamed on `symbol`.
    Div {
        a: Box<LExpr>,
        b: Box<LExpr>,
        rem: bool,
        symbol: String,
    },
    /// An enum variant (`Option` and `Result` included); `ty` is the enum type.
    Ctor {
        def: DefId,
        variant: u32,
        args: Vec<LExpr>,
        ty: Ty,
    },
    /// A struct from all its fields in declaration order; `ty` is the struct type.
    Struct {
        def: DefId,
        fields: Vec<LExpr>,
        ty: Ty,
    },
    Wrap {
        def: DefId,
        arg: Box<LExpr>,
    },
    Field {
        base: Box<LExpr>,
        def: DefId,
        index: u32,
        /// The struct type of `base`.
        ty: Ty,
    },
    /// The payload `index` of an enum value known to be `variant`; `ty` is the enum type.
    Payload {
        base: Box<LExpr>,
        def: DefId,
        variant: u32,
        index: u32,
        ty: Ty,
    },
    /// Whether an enum value is `variant`; `ty` is the enum type.
    IsVariant {
        base: Box<LExpr>,
        def: DefId,
        variant: u32,
        ty: Ty,
    },
    /// A wrapper's inner value.
    Unwrap {
        base: Box<LExpr>,
        def: DefId,
    },
    List {
        elem: Ty,
        items: Vec<LExpr>,
    },
    /// An operation of a capability handle (its contracts are statements around it).
    CapCall {
        handle: Box<LExpr>,
        capability: DefId,
        op: u32,
        args: Vec<LExpr>,
        ty: Ty,
    },
    /// The predicate of refinement or struct `def` on a value (a `Bool`); `generics` are the
    /// struct's type arguments.
    Pred {
        def: DefId,
        generics: Vec<Ty>,
        arg: Box<LExpr>,
    },
    /// Whether `arg` satisfies every refinement and struct invariant reachable in `ty` (the
    /// validation of a value from the host, semantics S77).
    Valid {
        ty: Ty,
        arg: Box<LExpr>,
    },
    /// `Err(ConstraintError { type_name, message: "<value text> does not satisfy <predicate>" })`
    /// of `check<R>`; `ty` is the `Result` type and `value_ty` the value's.
    CheckFailure {
        value: Box<LExpr>,
        value_ty: Ty,
        type_name: String,
        predicate: String,
        ty: Ty,
    },
}

/// A fault the generated code raises.
#[derive(Clone, Debug)]
pub struct Fault {
    /// `contract_violation`, `assertion_failed`, `unresolved_hole`, `internal_error`.
    pub code: &'static str,
    /// For a contract violation: `requires`, `ensures`, `invariant`, or `decreases`.
    pub clause_kind: Option<&'static str>,
    /// The clause or asserted text; the hole's name.
    pub clause: String,
    /// The blamed declaration's qualified name.
    pub symbol: String,
    pub message: String,
}

impl LExpr {
    /// Reading it twice gives the same value with no effect, whatever runs in between.
    pub fn is_stable(&self, vars: &[VarDef]) -> bool {
        match self {
            LExpr::Var(v) => !vars[*v as usize].mutable,
            LExpr::Int(_) | LExpr::Text(_) | LExpr::Bool(_) | LExpr::Unit => true,
            _ => false,
        }
    }
}

/// Every expression of `stmts`, nested ones included, in no particular order.
pub fn visit_exprs(stmts: &[LStmt], f: &mut dyn FnMut(&LExpr)) {
    for s in stmts {
        match s {
            LStmt::Let { value, .. } | LStmt::Assign { value, .. } | LStmt::Push { value, .. } => {
                visit_expr(value, f)
            }
            LStmt::Expr(e) | LStmt::Return(e) => visit_expr(e, f),
            LStmt::If { cond, then, els } => {
                visit_expr(cond, f);
                visit_exprs(then, f);
                visit_exprs(els, f);
            }
            LStmt::Match { arms, .. } => {
                for a in arms {
                    visit_exprs(&a.body, f);
                }
            }
            LStmt::Loop { body, .. } => visit_exprs(body, f),
            LStmt::ForEach { list, body, .. } => {
                visit_expr(list, f);
                visit_exprs(body, f);
            }
            LStmt::Decl { .. }
            | LStmt::Break { .. }
            | LStmt::Fault(_)
            | LStmt::ClauseEnter { .. }
            | LStmt::ClauseLeave => {}
        }
    }
}

/// `e` and every expression in it.
pub fn visit_expr(e: &LExpr, f: &mut dyn FnMut(&LExpr)) {
    f(e);
    match e {
        LExpr::Call { args, using, .. } => {
            for a in args.iter().chain(using) {
                visit_expr(a, f);
            }
        }
        LExpr::Builtin { args, .. }
        | LExpr::Op { args, .. }
        | LExpr::Ctor { args, .. }
        | LExpr::CapCall { args, .. } => {
            if let LExpr::CapCall { handle, .. } = e {
                visit_expr(handle, f);
            }
            for a in args {
                visit_expr(a, f);
            }
        }
        LExpr::Struct { fields, .. } => {
            for a in fields {
                visit_expr(a, f);
            }
        }
        LExpr::List { items, .. } => {
            for a in items {
                visit_expr(a, f);
            }
        }
        LExpr::Div { a, b, .. } => {
            visit_expr(a, f);
            visit_expr(b, f);
        }
        LExpr::Wrap { arg, .. } | LExpr::Pred { arg, .. } | LExpr::Valid { arg, .. } => {
            visit_expr(arg, f)
        }
        LExpr::Field { base, .. }
        | LExpr::Payload { base, .. }
        | LExpr::IsVariant { base, .. }
        | LExpr::Unwrap { base, .. } => visit_expr(base, f),
        LExpr::CheckFailure { value, .. } => visit_expr(value, f),
        LExpr::Var(_) | LExpr::Int(_) | LExpr::Text(_) | LExpr::Bool(_) | LExpr::Unit => {}
    }
}

/// The builtins that can fault (a `requires` of the standard library); the rest cannot.
pub const FALLIBLE_BUILTINS: &[&str] = &["list_take", "list_drop", "text_split", "bytes_from_list"];

/// Whether running `stmts` can fault: they raise one, call a function, an operation, or a
/// predicate, divide, or call a builtin with a `requires`. `Int` arithmetic never faults.
pub fn can_fault(stmts: &[LStmt]) -> bool {
    stmts.iter().any(stmt_can_fault)
}

fn stmt_can_fault(s: &LStmt) -> bool {
    match s {
        LStmt::Fault(_) => true,
        LStmt::Let { value, .. }
        | LStmt::Assign { value, .. }
        | LStmt::Push { value, .. }
        | LStmt::Expr(value)
        | LStmt::Return(value) => expr_can_fault(value),
        LStmt::If { cond, then, els } => expr_can_fault(cond) || can_fault(then) || can_fault(els),
        LStmt::Match { arms, .. } => arms.iter().any(|a| can_fault(&a.body)),
        LStmt::Loop { body, .. } => can_fault(body),
        LStmt::ForEach { list, body, .. } => expr_can_fault(list) || can_fault(body),
        LStmt::Decl { .. }
        | LStmt::Break { .. }
        | LStmt::ClauseEnter { .. }
        | LStmt::ClauseLeave => false,
    }
}

fn expr_can_fault(e: &LExpr) -> bool {
    let mut found = false;
    visit_expr(e, &mut |x| {
        found |= match x {
            LExpr::Call { .. }
            | LExpr::Div { .. }
            | LExpr::CapCall { .. }
            | LExpr::Pred { .. }
            | LExpr::Valid { .. } => true,
            LExpr::Builtin { id, .. } => FALLIBLE_BUILTINS.contains(&id.as_str()),
            _ => false,
        }
    });
    found
}

/// Drop each `ClauseEnter`..`ClauseLeave` scope whose statements cannot fault, in `stmts` and
/// every list nested in them: S34 is observable only through a fault that escapes the scope,
/// so a contract of comparisons and arithmetic costs nothing more than itself.
pub fn prune_clause_scopes(stmts: &mut Vec<LStmt>) {
    for s in stmts.iter_mut() {
        match s {
            LStmt::If { then, els, .. } => {
                prune_clause_scopes(then);
                prune_clause_scopes(els);
            }
            LStmt::Match { arms, .. } => {
                for a in arms {
                    prune_clause_scopes(&mut a.body);
                }
            }
            LStmt::Loop { body, .. } | LStmt::ForEach { body, .. } => prune_clause_scopes(body),
            _ => {}
        }
    }
    let mut i = 0;
    while i < stmts.len() {
        if matches!(stmts[i], LStmt::ClauseEnter { .. }) {
            let mut depth = 0;
            let mut end = None;
            for (j, s) in stmts.iter().enumerate().skip(i + 1) {
                match s {
                    LStmt::ClauseEnter { .. } => depth += 1,
                    LStmt::ClauseLeave if depth == 0 => {
                        end = Some(j);
                        break;
                    }
                    LStmt::ClauseLeave => depth -= 1,
                    _ => {}
                }
            }
            if let Some(j) = end {
                if !can_fault(&stmts[i + 1..j]) {
                    stmts.remove(j);
                    stmts.remove(i);
                    continue;
                }
            }
        }
        i += 1;
    }
}
