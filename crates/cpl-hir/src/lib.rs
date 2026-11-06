//! cpl-hir: the typed, resolved intermediate representation of
//! `spec/toolchain.md` 3.3, produced by `cpl-sema` and consumed by
//! `cpl-eval`, `cpl-index`, and `cpl-work`. `cpl-sema` re-exports this crate
//! as `cpl_sema::hir`.
//!
//! The HIR is a boxed tree like the AST (T16): every expression carries its
//! type, every name is resolved to a [`DefId`] or a [`LocalId`], `else if`
//! chains are nested [`ExprKind::If`]s, members of intrinsic types are
//! [`ExprKind::Call`]s of their std stubs (T19), there are no type aliases
//! (grammar D64), and `gen.*` expressions are [`ExprKind::Gen`] (T18).
//!
//! AST references use the `cpl_syntax::ast` types: a definition's
//! declaration is an [`ast::Declaration`] at [`Def::decl_index`] of its
//! file's [`ast::Module::declarations`].
//!
//! [`Program`]'s accessors (Section 1.6) are in this crate (T50); selector
//! resolution, which needs name resolution, is `cpl_sema::resolve_selector`.

mod program;
mod walk;
pub mod scalar;

#[cfg(test)]
mod tests;

use cpl_diag::{DeclName, FileId, FileOrigin, SourceMap, Span, StableId};
use cpl_syntax::ast;
use num_bigint::BigInt;
use serde::Serialize;
use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

pub use cpl_diag::json::BindingKind;
pub use cpl_syntax::ast::FunctionMode;

/// The checked shape of a deterministic machine step. A command-bearing result is an immutable record
/// with exactly `state: S` and `commands: List<C>`; `C` is an enum. A command-free step may return `S`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineSignature {
    pub state: Ty,
    pub event: Ty,
    pub command: Option<Ty>,
    pub state_field: Option<u32>,
    pub commands_field: Option<u32>,
}

// ---------------------------------------------------------------------------
// Identifiers
// ---------------------------------------------------------------------------

/// A definition of the program: the index into [`Program::defs`].
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize)]
pub struct DefId(pub u32);

/// A module of the program: the index into [`Program::modules`].
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize)]
pub struct ModuleId(pub u32);

/// A local binding, numbered per definition: the index into that
/// definition's `locals` table.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct LocalId(pub u32);

/// A hole, numbered per program.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct HoleId(pub u32);

/// The identity of a refinement type (semantics S55, T39): the index into
/// [`Program::refinement_classes`]; one per distinct (base, resolved predicate).
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct RefinementClass(pub u32);

// ---------------------------------------------------------------------------
// Program and modules
// ---------------------------------------------------------------------------

/// The typed program: every definition of the project and of std.
#[derive(Clone, Debug)]
pub struct Program {
    /// Index = `DefId.0`.
    pub defs: Vec<Def>,
    /// Index = `ModuleId.0`.
    pub modules: Vec<ModuleDef>,
    pub std_defs: StdDefs,
    /// Sorted by (from, span) ([`Program::sort_references`]).
    pub references: Vec<Reference>,
    /// Every `use` item of every file (Section 6.5); imports are not references.
    pub imports: Vec<Import>,
    /// Index = `RefinementClass.0`; one per distinct (base, resolved predicate), S55.
    pub refinement_classes: Vec<RefinementClassDef>,
    pub source_map: Arc<SourceMap>,
    /// The parsed files, in `FileId` order.
    pub asts: Vec<(FileId, Arc<ast::Module>)>,
    /// [`Program::by_stable_id`]'s lookup table, built on first use.
    pub stable_ids: StableIndex,
}

/// The stable identities of a program's definitions, first occurrence first, built on first
/// use and rebuilt when definitions are added. A clone starts empty.
#[derive(Default)]
pub struct StableIndex(
    std::sync::RwLock<Option<(usize, std::collections::HashMap<StableId, DefId>)>>,
);

impl Clone for StableIndex {
    fn clone(&self) -> Self {
        StableIndex::default()
    }
}

impl std::fmt::Debug for StableIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StableIndex")
    }
}

/// DefIds of the std declarations the checker and evaluator name directly
/// (prelude types and the std.gen refinement).
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct StdDefs {
    pub unit: DefId,
    pub option: DefId,
    pub result: DefId,
    pub constraint_error: DefId,
    pub foreign_error: DefId,
    pub positive: DefId,
}

impl StdDefs {
    /// The `std_defs` of a program without definitions (a file failed to
    /// parse, semantics S52): every id is `DefId(u32::MAX)` and names nothing.
    pub const UNSET: StdDefs = StdDefs {
        unit: DefId(u32::MAX),
        option: DefId(u32::MAX),
        result: DefId(u32::MAX),
        constraint_error: DefId(u32::MAX),
        foreign_error: DefId(u32::MAX),
        positive: DefId(u32::MAX),
    };
}

/// One `use` item of one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Import {
    pub file: FileId,
    pub target: ImportTarget,
    pub span: Span,
}

/// What a `use` item names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportTarget {
    Def(DefId),
    Module(ModuleId),
    /// A submodule alias that is only a prefix of module paths: no module
    /// has exactly that path (semantics 1.6).
    Namespace(Vec<String>),
}

/// One refinement class (S55).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefinementClassDef {
    pub base: Ty,
    /// The resolved HIR predicate (no parentheses, names bound to DefIds);
    /// classes compare `base` and `predicate` structurally
    /// ([`Expr::structurally_eq`]).
    pub predicate: Expr,
    /// Every refinement declaration of this class.
    pub members: Vec<DefId>,
}

/// A module: every file with the same `module` header (Section 3.7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleDef {
    pub id: ModuleId,
    /// `["reader", "pages"]`.
    pub path: Vec<String>,
    /// `Project` or `Std` (or the tree's origin when loaded from a `VirtualTree`).
    pub origin: FileOrigin,
    /// Path order.
    pub files: Vec<FileId>,
    /// (file order, declaration order).
    pub defs: Vec<DefId>,
    /// The files' module doc comments, joined with one blank line (Section 3.7).
    pub docs: Option<String>,
}

// ---------------------------------------------------------------------------
// Definitions
// ---------------------------------------------------------------------------

/// One top-level declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Def {
    pub id: DefId,
    pub stable_id: StableId,
    /// No `@id` in source (Section 4.3).
    pub provisional: bool,
    pub module: ModuleId,
    /// `Ident("paginate")`, `Text("pagination ...")` for tests and
    /// properties, `Member { .. }` for std `@method`/`@property` stubs (Section 4.4).
    pub name: DeclName,
    pub is_pub: bool,
    pub file: FileId,
    /// Index into that file's [`ast::Module::declarations`].
    pub decl_index: usize,
    /// The span of the [`ast::Declaration`].
    pub span: Span,
    /// Joined doc lines.
    pub docs: Option<String>,
    pub policy: VerificationPolicy,
    pub status: DefStatus,
    pub kind: DefKind,
}

/// Whether checking a definition succeeded; later stages skip
/// `HasErrors` definitions where continuing would only cascade.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum DefStatus {
    Ok,
    HasErrors,
}

/// The `verification_policy` facet: `@prove` present or not.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Default, Debug)]
pub struct VerificationPolicy {
    pub prove: bool,
}

/// What a definition is.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)] // the spec's shape (Section 3.3); one value per definition
pub enum DefKind {
    Fn(FnDef),
    Wrapper(WrapperDef),
    Refinement(RefinementDef),
    Struct(StructDef),
    Enum(EnumDef),
    Capability(CapabilityDef),
    Test(TestDef),
    Property(PropertyDef),
    /// A definition-less std type (`@builtin("int") pub type Int`,
    /// `pub type List<T>`, `pub type Gen<T>`; stdlib S35, grammar G2).
    BuiltinType(BuiltinTypeDef),
}

/// A function or a std `@builtin` stub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FnDef {
    /// `T`, `U`; referenced as `Ty::Param(index)`.
    pub generics: Vec<String>,
    /// Positional.
    pub params: Vec<ParamDef>,
    /// Capability parameters; `ty` is `Ty::Capability`.
    pub using: Vec<ParamDef>,
    pub ret: Ty,
    pub effects: EffectBound,
    /// Source order.
    pub requires: Vec<Clause>,
    /// Source order; `result` is `result_local`.
    pub ensures: Vec<Clause>,
    pub result_local: Option<LocalId>,
    pub decreases: Option<Clause>,
    /// Index = `LocalId.0`; parameters first.
    pub locals: Vec<LocalDef>,
    pub body: FnBody,
}

/// A function body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FnBody {
    Block(Block),
    /// The `@builtin("<id>")` registry id, e.g. `"text_trim"` (Section 3.6).
    Builtin(String),
}

/// A positional or `using` parameter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParamDef {
    pub local: LocalId,
    pub name: String,
    pub ty: Ty,
    pub span: Span,
}

/// One entry of a definition's local table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalDef {
    pub id: LocalId,
    pub name: String,
    pub ty: Ty,
    pub mutable: bool,
    pub kind: BindingKind,
    pub span: Span,
}

/// A function's effect bound.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct EffectBound {
    /// `None`: no `effects` clause.
    pub declared: Option<BTreeSet<EffectRef>>,
    /// What the body actually performs (Section 3.4).
    pub inferred: BTreeSet<EffectRef>,
}

/// `param.op`: an operation of a `using` parameter.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct EffectRef {
    pub param: LocalId,
    pub op: String,
}

/// A contract clause (or loop clause, or refinement predicate).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clause {
    pub expr: Expr,
    /// The canonical text (Section 5.2).
    pub text: String,
    pub span: Span,
}

/// `type PageId(Text)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WrapperDef {
    pub inner: Ty,
}

/// `type Positive = Int where self > 0`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefinementDef {
    pub base: Ty,
    pub self_local: LocalId,
    pub predicate: Clause,
    /// Index = `LocalId.0`: `self` and any `forall` binders of the predicate.
    pub locals: Vec<LocalDef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructDef {
    pub generics: Vec<String>,
    pub fields: Vec<FieldDef>,
    /// A constructor invariant, with `self` typed as this struct. It is checked at untrusted boundaries too.
    pub invariant: Option<RefinementDef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldDef {
    pub name: String,
    pub ty: Ty,
    pub docs: Option<String>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnumDef {
    pub generics: Vec<String>,
    pub variants: Vec<VariantDef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VariantDef {
    pub name: String,
    pub payload: Vec<Ty>,
    pub docs: Option<String>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilityDef {
    pub ops: Vec<OpDef>,
}

/// A capability operation (its own provisional identity is the qualified
/// name `<module>.<Cap>.<op>`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpDef {
    pub name: String,
    pub params: Vec<ParamDef>,
    pub ret: Ty,
    pub requires: Vec<Clause>,
    pub ensures: Vec<Clause>,
    /// The local `result` names in `ensures`.
    pub result_local: Option<LocalId>,
    /// Index = `LocalId.0`; parameters first.
    pub locals: Vec<LocalDef>,
    pub docs: Option<String>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestDef {
    pub locals: Vec<LocalDef>,
    pub body: Block,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropertyDef {
    /// The `for` clauses, in clause order.
    pub binders: Vec<Binder>,
    pub locals: Vec<LocalDef>,
    pub body: Block,
}

/// `for x in gen.int`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binder {
    pub local: LocalId,
    pub gen: GenExpr,
    pub elem_ty: Ty,
    pub span: Span,
}

/// A definition-less std type. Its uses are the intrinsic [`Ty`] forms
/// (`Ty::Int`, `Ty::List`, `Ty::Gen`, ...), never `Ty::Adt`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltinTypeDef {
    pub generics: Vec<String>,
    /// The `@builtin("<id>")` registry id: `int`, `bool`, `text`, `bytes`, `list`, `gen`.
    pub builtin: String,
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A type. `PartialEq` and `Hash` are written by hand: `Refined` compares
/// its class only (S55), so two refinement declarations of one class are
/// one type.
#[derive(Clone, Eq, Debug)]
pub enum Ty {
    Int,
    Bool,
    Text,
    Bytes,
    List(Box<Ty>),
    /// Struct or enum, including the prelude's `Unit`, `Option<T>`,
    /// `Result<T, E>`, `ConstraintError`.
    Adt(DefId, Vec<Ty>),
    /// Nominal wrapper.
    Wrapper(DefId),
    /// Refinement type (semantics S55, T39): identity is the class; the
    /// DefId names the declaration written at the use site, for
    /// diagnostics; its base is `RefinementDef::base`.
    Refined(RefinementClass, DefId),
    /// Generic parameter of the enclosing definition.
    Param(u32),
    Capability(DefId),
    /// Hermetic generator type (semantics S14): only `for` ranges and `gen` arguments.
    Gen(Box<Ty>),
    Error,
}

impl PartialEq for Ty {
    fn eq(&self, other: &Ty) -> bool {
        match (self, other) {
            (Ty::Int, Ty::Int)
            | (Ty::Bool, Ty::Bool)
            | (Ty::Text, Ty::Text)
            | (Ty::Bytes, Ty::Bytes)
            | (Ty::Error, Ty::Error) => true,
            (Ty::List(a), Ty::List(b)) | (Ty::Gen(a), Ty::Gen(b)) => a == b,
            (Ty::Adt(d, a), Ty::Adt(e, b)) => d == e && a == b,
            (Ty::Wrapper(a), Ty::Wrapper(b)) | (Ty::Capability(a), Ty::Capability(b)) => a == b,
            (Ty::Refined(a, _), Ty::Refined(b, _)) => a == b,
            (Ty::Param(a), Ty::Param(b)) => a == b,
            _ => false,
        }
    }
}

impl Hash for Ty {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Ty::Int | Ty::Bool | Ty::Text | Ty::Bytes | Ty::Error => {}
            Ty::List(t) | Ty::Gen(t) => t.hash(state),
            Ty::Adt(d, args) => {
                d.hash(state);
                args.hash(state);
            }
            Ty::Wrapper(d) | Ty::Capability(d) => d.hash(state),
            Ty::Refined(class, _) => class.hash(state),
            Ty::Param(i) => i.hash(state),
        }
    }
}

// ---------------------------------------------------------------------------
// Expressions, blocks, statements, patterns
// ---------------------------------------------------------------------------

/// A typed expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expr {
    pub kind: ExprKind,
    pub ty: Ty,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExprKind {
    Lit(Lit),
    Local(LocalId),
    /// Enum variant, including `Ok`/`Err`/`Some`/`None`/`Unit`.
    Ctor {
        def: DefId,
        variant: u32,
        args: Vec<Expr>,
    },
    /// `PageId("x")`.
    Wrap {
        def: DefId,
        arg: Box<Expr>,
    },
    /// Struct field.
    Field {
        base: Box<Expr>,
        def: DefId,
        index: u32,
    },
    /// A fn, `@builtin` fn, or `@method`/`@property` stub with the receiver first.
    Call {
        callee: DefId,
        generic_args: Vec<Ty>,
        args: Vec<Expr>,
        using: Vec<Expr>,
    },
    CapCall {
        handle: Box<Expr>,
        capability: DefId,
        op: u32,
        args: Vec<Expr>,
    },
    /// `check<R>(x)`: `Result<R, ConstraintError>`.
    Check {
        refined: DefId,
        arg: Box<Expr>,
    },
    /// `e?` (`Result` only in 0.1).
    Propagate(Box<Expr>),
    Unary {
        op: ast::UnaryOp,
        operand: Box<Expr>,
    },
    Binary {
        op: ast::BinaryOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Parallel {
        binder: LocalId,
        collection: Box<Expr>,
        predicate: Box<Expr>,
    },
    Forall {
        binder: LocalId,
        collection: Box<Expr>,
        predicate: Box<Expr>,
    },
    Hole {
        id: HoleId,
        name: String,
    },
    List(Vec<Expr>),
    /// Explicit fields by index.
    StructLit {
        def: DefId,
        /// A bounded compile-time evaluation established the constructor invariant.
        invariant_proven: bool,
        spread: Option<Box<Expr>>,
        fields: Vec<(u32, Expr)>,
    },
    If {
        cond: Box<Expr>,
        then_block: Block,
        else_block: Option<Block>,
    },
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<Arm>,
    },
    /// `Refined(from)` -> its base type.
    Widen {
        expr: Box<Expr>,
        from: DefId,
    },
    /// base -> `Refined`, statically established.
    Refine {
        expr: Box<Expr>,
        refined: DefId,
        proof: RefinementProof,
    },
    /// A `gen.*` expression; `ty` is `Ty::Gen(elem)`.
    Gen(GenExpr),
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lit {
    Int(BigInt),
    Text(String),
    Bool(bool),
}

/// How a `Refine` was established: semantics S16 only (T39).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum RefinementProof {
    Constant,
}

/// One `match` arm.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Arm {
    pub pattern: Pattern,
    pub body: Block,
    pub span: Span,
}

/// A block with an explicit tail (T16).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub tail: Option<Box<Expr>>,
    pub ty: Ty,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)] // the spec's shape (Section 3.3)
pub enum Stmt {
    Let {
        local: LocalId,
        value: Expr,
    },
    /// `let _ = e`.
    Discard(Expr),
    Assign {
        local: LocalId,
        value: Expr,
    },
    Return(Option<Expr>),
    Assert {
        expr: Expr,
        text: String,
    },
    While {
        cond: Expr,
        invariants: Vec<Clause>,
        decreases: Option<Clause>,
        body: Block,
    },
    /// `Unit`-typed, non-tail.
    Expr(Expr),
    /// `concurrent { tasks }`: independent tasks whose effect footprints commute
    /// ([`Program::footprint`]). The result is that of running them in order;
    /// a runtime may overlap tasks whose actual handles are disjoint.
    Concurrent(Vec<Task>),
}

/// One task of a `concurrent` block: a `Let`, `Discard`, or `Expr` statement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    pub stmt: Stmt,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pattern {
    pub kind: PatternKind,
    pub ty: Ty,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatternKind {
    Wildcard,
    Int(BigInt),
    Text(String),
    Bool(bool),
    Bind(LocalId),
    Ctor {
        def: DefId,
        variant: u32,
        args: Vec<Pattern>,
    },
    /// Wrapper pattern `PageId(t)`.
    Unwrap {
        def: DefId,
        arg: Box<Pattern>,
    },
}

/// Generator expressions of `for` clauses (Section 3.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GenExpr {
    Int,
    PositiveInt,
    Bool,
    Text,
    List(Box<GenExpr>),
    Option(Box<GenExpr>),
    Range(Box<Expr>, Box<Expr>),
}

// ---------------------------------------------------------------------------
// References
// ---------------------------------------------------------------------------

/// One resolved use of a definition by another (Section 6.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reference {
    pub from: DefId,
    pub to: DefId,
    pub kind: RefKind,
    pub span: Span,
    /// The reference occurs in the signature, contract, or effects (T17).
    pub interface: bool,
}

/// The kind of a [`Reference`]. A `use` import is an [`Import`], not a
/// `Reference` (Section 6.5).
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RefKind {
    Call,
    TypeUse,
    Ctor,
    FieldAccess,
    CapabilityOp,
    Check,
}
