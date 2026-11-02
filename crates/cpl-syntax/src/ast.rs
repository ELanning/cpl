//! The abstract syntax of CPL 0.1: one Rust type per nonterminal of
//! `spec/grammar.md` Appendix B, with enums for alternatives and a span on
//! every node.
//!
//! # Nonterminal to type map
//!
//! | Grammar | Type |
//! | - | - |
//! | `source_file` | [`Module`] (alias [`SourceFile`]) |
//! | `work_file`, `patch_file` | [`WorkFile`], [`PatchFile`] |
//! | `module_header`, `module_path` | [`ModuleHeader`], [`ModulePath`] |
//! | `use_decl`, `use_path`, `use_item` | [`UseDecl`], [`UsePath`], [`UseItem`] |
//! | `declaration` | [`Declaration`] with [`DeclKind`] |
//! | `decl_prefix` | the `docs` and `annotations` fields of [`Declaration`] |
//! | `annotation` | [`Annotation`] |
//! | `type_decl`, `wrapper_decl`, `refinement_decl`, `opaque_decl` | [`TypeDecl`], [`WrapperDecl`], [`RefinementDecl`], [`OpaqueDecl`] |
//! | `struct_decl`, `field_decl` | [`StructDecl`], [`FieldDecl`] |
//! | `enum_decl`, `variant_decl` | [`EnumDecl`], [`VariantDecl`] |
//! | `capability_decl`, `op_decl` | [`CapabilityDecl`], [`OpDecl`] |
//! | `fn_decl` | [`FnDecl`] |
//! | `fn_tail` | the `clauses` and `body` fields of [`FnDecl`] (`body` is `None` for the bodiless form) |
//! | `fn_clause`, `effects_clause`, `effect_ref` | [`FnClause`], [`EffectsClause`], [`EffectRef`] |
//! | `contract_clause`, `decreases_clause` | [`ContractClause`], [`DecreasesClause`] |
//! | `params`, `param`, `using_param` | [`Params`], [`Param`], [`UsingParam`] |
//! | `test_decl`, `property_decl`, `for_clause` | [`TestDecl`], [`PropertyDecl`], [`ForClause`] |
//! | `type`, `type_path`, `generic_args`, `generic_params` | [`Type`], [`TypePath`], [`GenericArgs`], [`GenericParams`] |
//! | `block` | [`Block`] (`tail` is the block's value) |
//! | `statement` | [`Statement`] |
//! | `let_stmt` ... `while_stmt`, `expr_stmt` | [`LetStmt`], [`VarStmt`], [`AssignStmt`], [`ReturnStmt`], [`AssertStmt`], [`WhileStmt`], [`ExprStmt`] |
//! | `while_tail`, `while_clause` | the `clauses`/`body` fields of [`WhileStmt`], [`WhileClause`] |
//! | `expr` | [`Expr`] with [`ExprKind`] |
//! | `forall_expr` | [`ForallExpr`] |
//! | `or_expr`, `and_expr`, `cmp_expr`, `add_expr`, `mul_expr`, `cmp_op` | [`BinaryExpr`] with [`BinaryOp`] |
//! | `unary_expr` | [`UnaryExpr`] with [`UnaryOp`], or [`ExprKind::Hole`] |
//! | `postfix_expr`, `postfix` | [`MemberExpr`], [`CallExpr`], [`ExprKind::Propagate`] |
//! | `member_name`, `name` | [`Name`], [`ExprKind::Name`] |
//! | `args`, `using_arg` | [`Args`] with [`Arg`], [`UsingArg`] |
//! | `primary`, `paren_expr`, `literal` | [`ExprKind`] variants, [`ExprKind::Paren`], [`Literal`] |
//! | `list_literal` | [`ListLiteral`] |
//! | `struct_literal`, `struct_entries`, `field_init` | [`StructLiteral`], [`StructEntries`], [`FieldInit`] |
//! | `if_expr`, `match_expr`, `match_arm` | [`IfExpr`], [`MatchExpr`], [`MatchArm`] with [`ArmBody`] |
//! | `pattern`, `ctor_path` | [`Pattern`] with [`PatternKind`], [`CtorPath`] |
//! | `change_decl`, `change_item`, `git_ref` | [`ChangeDecl`], [`ChangeItem`], [`GitRef`] |
//! | `task_decl`, `after_clause`, `task_item`, `selector_set` | [`TaskDecl`], [`AfterClause`], [`TaskItem`], [`SelectorSet`] |
//! | `integrate_decl`, `require_item`, `requirement` | [`IntegrateDecl`], [`RequireItem`], [`Requirement`] |
//! | `selector`, `selector_path`, `selector_seg`, `facet` | [`Selector`], [`SelectorPath`], [`Name`], [`Facet`] |
//! | `patch_form` | [`PatchForm`] |
//!
//! # Trivia and docs
//!
//! Doc comments (`///`) are stored as [`Docs`] on the node they document
//! (module, declaration, struct field, enum variant, capability operation,
//! change, task, patch form); [`Docs::text`] is the `docs` facet value.
//!
//! Ordinary comments and blank lines are [`Trivia`]: every item, statement,
//! clause, parameter, argument, match arm, struct field, enum variant,
//! field initializer, and set element carries one. `leading` holds the
//! comments on the lines before the node, `trailing` the comment on the
//! node's last line (and any comment inside the node that no inner node
//! claimed), `blank_line_before` records whether at least one blank line
//! separated the node from what precedes it. Comments after the last element
//! of a brace body are the body's `trailing_comments`, and comments after
//! the last item of a file are the file's. Every comment of the source
//! appears exactly once in the tree, so a formatter can reproduce all of
//! them and preserve one blank line wherever the author had any.
//!
//! # Layout memory
//!
//! Three fields record layout the formatter (`spec/toolchain.md` 2.4) must
//! know but the grammar ignores: every bracketed, comma-separated group
//! carries a [`Group`] whose `multiline` flag says whether a line terminator
//! occurred between its brackets at the group's own level (rule 6); every
//! such group owner also has
//! `trailing_comments`, the comments after its last element (rule 11); and
//! [`BinaryExpr::break_before_op`] records a source line break directly
//! before the operator (rule 9). The debug dump omits all three.
//!
//! # Facets
//!
//! For a [`FnDecl`] the semantic facets are: `signature` = name, generic
//! parameters, [`FnDecl::params`], [`FnDecl::ret`]; `contract` =
//! [`FnDecl::requires`] and [`FnDecl::ensures`] in source order; `effects` =
//! [`FnDecl::effects`]; `body` = [`FnDecl::body`]; `docs` =
//! [`Declaration::docs`]; `verification_policy` = the `@prove` annotation
//! ([`Declaration::has_annotation`]); `identity` = the `@id` annotation
//! ([`Declaration::id`]).

use cpl_diag::Span;
use serde::Serialize;

/// An identifier with its span.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Name {
    pub text: String,
    pub span: Span,
}

impl Name {
    pub fn new(text: impl Into<String>, span: Span) -> Name {
        Name {
            text: text.into(),
            span,
        }
    }

    /// True for the `UPPER_IDENT` class (first character `A`..`Z`).
    pub fn is_upper(&self) -> bool {
        self.text.starts_with(|c: char| c.is_ascii_uppercase())
    }
}

/// A text literal in a position that admits only `TEXT` (test names, goals, git refs).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TextLiteral {
    pub span: Span,
    /// The decoded value.
    pub value: String,
}

/// An integer literal in a position that admits only `INT` (`workers 3`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct IntLiteral {
    pub span: Span,
    /// Decimal digits without separators.
    pub digits: String,
}

/// An ordinary `//` comment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Comment {
    pub span: Span,
    /// The source text from `//` to the end of the line.
    pub text: String,
    /// At least one blank line separated this comment from what precedes it.
    pub blank_line_before: bool,
}

/// Comments and blank lines attached to a node (see the module docs).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Trivia {
    pub leading: Vec<Comment>,
    pub trailing: Vec<Comment>,
    pub blank_line_before: bool,
}

impl Trivia {
    pub fn is_empty(&self) -> bool {
        self.leading.is_empty() && self.trailing.is_empty() && !self.blank_line_before
    }

    /// True when the node carries at least one comment (blank lines do not count).
    pub fn has_comments(&self) -> bool {
        !self.leading.is_empty() || !self.trailing.is_empty()
    }
}

/// Layout of a bracketed, comma-separated group as written
/// (`spec/toolchain.md` 2.4 rule 6): the formatter keeps a group broken
/// across lines once the author broke it.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Group {
    /// A line terminator occurred between the opening and closing bracket
    /// at the group's own level (not inside a nested bracket pair or block).
    pub multiline: bool,
}

/// One `///` line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DocLine {
    pub span: Span,
    /// The text after `///` with one leading space removed.
    pub text: String,
}

/// The doc comments attached to one declaration or member (grammar 1.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Docs {
    pub span: Span,
    pub lines: Vec<DocLine>,
}

impl Docs {
    /// The `docs` facet value: the lines joined with `\n`.
    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

/// A `.cpl` file (`source_file`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Module {
    pub span: Span,
    /// Comments before the module header.
    pub trivia: Trivia,
    pub docs: Option<Docs>,
    pub header: ModuleHeader,
    pub uses: Vec<UseDecl>,
    pub declarations: Vec<Declaration>,
    /// Comments after the last declaration.
    pub trailing_comments: Vec<Comment>,
}

/// Appendix B's name for [`Module`].
pub type SourceFile = Module;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModuleHeader {
    pub span: Span,
    pub path: ModulePath,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModulePath {
    pub span: Span,
    pub segments: Vec<Name>,
}

impl ModulePath {
    /// `a.b.c`
    pub fn dotted(&self) -> String {
        self.segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(".")
    }
}

/// `use a.b.{c, D}` or `use a.b.c`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UseDecl {
    pub span: Span,
    pub trivia: Trivia,
    pub path: UsePath,
    /// True for the list form `use a.b.{...}`; the single form has one item.
    pub braced: bool,
    pub items: Vec<UseItem>,
    /// Layout of the `{ }` list (unused when `!braced`).
    pub group: Group,
    /// Comments after the last item, before `}`.
    pub trailing_comments: Vec<Comment>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UsePath {
    pub span: Span,
    pub segments: Vec<Name>,
}

impl UsePath {
    pub fn dotted(&self) -> String {
        self.segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(".")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UseItem {
    pub span: Span,
    pub trivia: Trivia,
    pub name: Name,
}

// ---------------------------------------------------------------------------
// Declarations
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Declaration {
    pub span: Span,
    pub trivia: Trivia,
    pub docs: Option<Docs>,
    pub annotations: Vec<Annotation>,
    pub kind: DeclKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[allow(clippy::large_enum_variant)] // one value per declaration; boxing would only complicate consumers
pub enum DeclKind {
    Type(TypeDecl),
    Struct(StructDecl),
    Enum(EnumDecl),
    Capability(CapabilityDecl),
    Fn(FnDecl),
    Test(TestDecl),
    Property(PropertyDecl),
}

impl Declaration {
    /// The declared identifier; `None` for tests and properties, whose
    /// names are text literals (see [`Declaration::display_name`]).
    pub fn name(&self) -> Option<&Name> {
        match &self.kind {
            DeclKind::Type(d) => Some(&d.name),
            DeclKind::Struct(d) => Some(&d.name),
            DeclKind::Enum(d) => Some(&d.name),
            DeclKind::Capability(d) => Some(&d.name),
            DeclKind::Fn(d) => Some(&d.name),
            DeclKind::Test(_) | DeclKind::Property(_) => None,
        }
    }

    /// The identifier or the test/property name text.
    pub fn display_name(&self) -> &str {
        match &self.kind {
            DeclKind::Test(d) => &d.name.value,
            DeclKind::Property(d) => &d.name.value,
            _ => self.name().map(|n| n.text.as_str()).unwrap_or(""),
        }
    }

    pub fn is_pub(&self) -> bool {
        match &self.kind {
            DeclKind::Type(d) => d.is_pub,
            DeclKind::Struct(d) => d.is_pub,
            DeclKind::Enum(d) => d.is_pub,
            DeclKind::Capability(d) => d.is_pub,
            DeclKind::Fn(d) => d.is_pub,
            DeclKind::Test(_) | DeclKind::Property(_) => false,
        }
    }

    pub fn annotation(&self, name: &str) -> Option<&Annotation> {
        self.annotations.iter().find(|a| a.name.text == name)
    }

    /// `verification_policy` facet: true when `@prove` is present.
    pub fn has_annotation(&self, name: &str) -> bool {
        self.annotation(name).is_some()
    }

    /// `identity` facet: the text argument of `@id("...")`, if present.
    pub fn id(&self) -> Option<&str> {
        match self.annotation("id")?.args.first()?.kind {
            LiteralKind::Text(ref t) => Some(t),
            _ => None,
        }
    }
}

/// `@name`, `@name()`, or `@name(literal, ...)`, one per line. The name is
/// a `LOWER_IDENT` or any reserved keyword (`@property`, grammar D71).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Annotation {
    pub span: Span,
    /// Only `trailing` is ever populated: comments above an annotation are
    /// leading trivia of the declaration.
    pub trivia: Trivia,
    /// The spelling after `@`, a keyword included.
    pub name: Name,
    /// Empty for `@name` and for `@name()` alike; `parens` tells them apart.
    pub args: Vec<Literal>,
    pub group: Group,
    /// The annotation was written with an argument list, possibly empty
    /// (`@name()`, `@id("x")`); false for a bare `@name`. The formatter
    /// never removes a token (`spec/toolchain.md` T14), so `@name()` keeps
    /// its parentheses.
    pub parens: bool,
}

/// `pub type PageId(Text)`, `pub type Positive = Int where self > 0`, or
/// the definition-less `pub type List<T>` (grammar 3.5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TypeDecl {
    pub span: Span,
    pub is_pub: bool,
    pub name: Name,
    pub kind: TypeDeclKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum TypeDeclKind {
    Wrapper(WrapperDecl),
    Refinement(RefinementDecl),
    /// `opaque_decl`: a `type` with no definition, the standard library's
    /// `@builtin` type stubs (grammar 3.5, D19); `cpl check` reports
    /// `missing_body` for one in a project file.
    Opaque(OpaqueDecl),
}

/// `type Int` or `type List<T>` with no definition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OpaqueDecl {
    /// The generic parameters' span, or an empty span after the type name.
    pub span: Span,
    pub generic_params: Option<GenericParams>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WrapperDecl {
    pub span: Span,
    pub payload: Type,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RefinementDecl {
    pub span: Span,
    pub base: Type,
    /// The `where` predicate; `self` is the underlying value.
    pub predicate: Expr,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StructDecl {
    pub span: Span,
    pub is_pub: bool,
    /// An immutable application-state root, declared with `program`.
    pub is_program: bool,
    pub name: Name,
    pub generic_params: Option<GenericParams>,
    pub fields: Vec<FieldDecl>,
    /// Constructor invariant, written `where <predicate>` after the fields. `self` is the new value.
    pub invariant: Option<InvariantClause>,
    pub trailing_comments: Vec<Comment>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FieldDecl {
    pub span: Span,
    pub trivia: Trivia,
    pub docs: Option<Docs>,
    pub name: Name,
    pub ty: Type,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EnumDecl {
    pub span: Span,
    pub is_pub: bool,
    pub name: Name,
    pub generic_params: Option<GenericParams>,
    pub variants: Vec<VariantDecl>,
    pub trailing_comments: Vec<Comment>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct VariantDecl {
    pub span: Span,
    pub trivia: Trivia,
    pub docs: Option<Docs>,
    pub name: Name,
    /// Positional payload types; empty when the variant has no parentheses.
    pub payload: Vec<Type>,
    pub group: Group,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CapabilityDecl {
    pub span: Span,
    pub is_pub: bool,
    /// `interface` is the capability interface spelling; it adds no inheritance or methods to values.
    pub is_interface: bool,
    pub name: Name,
    pub operations: Vec<OpDecl>,
    pub trailing_comments: Vec<Comment>,
}

/// A capability operation signature with optional contract clauses.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OpDecl {
    pub span: Span,
    pub trivia: Trivia,
    pub docs: Option<Docs>,
    pub name: Name,
    /// Positional parameters only (`using` is empty): operations take no
    /// `using` parameters (grammar 3.8).
    pub params: Params,
    pub ret: Type,
    pub clauses: Vec<ContractClause>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FnDecl {
    pub span: Span,
    pub is_pub: bool,
    pub mode: FunctionMode,
    pub name: Name,
    pub generic_params: Option<GenericParams>,
    pub params: Params,
    pub ret: Type,
    /// `effects`, `requires`, `ensures`, and `decreases` clauses in source order.
    pub clauses: Vec<FnClause>,
    /// `None` for the bodiless form of `fn_tail` (grammar 3.9): a header,
    /// optionally with clauses, followed by neither a clause nor `{`. It is
    /// the standard library's `@builtin` stub form; `cpl check` reports
    /// `missing_body` for a bodiless `fn` in a project file.
    pub body: Option<Block>,
}

/// Function roles. Bare `fn` preserves the edition 0.1 effect-bound classification.
/// A machine and an initializer are pure; only a machine computes a new application-state snapshot.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FunctionMode {
    #[default]
    Inferred,
    Pure,
    Mutate,
    Machine,
    Initial,
}

impl FunctionMode {
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Inferred => "fn",
            Self::Pure => "pure fn",
            Self::Mutate => "mutate fn",
            Self::Machine => "machine",
            Self::Initial => "initial fn",
        }
    }

    pub fn is_pure(self) -> bool {
        matches!(self, Self::Pure | Self::Machine | Self::Initial)
    }
}

impl FnDecl {
    /// True for the bodiless (stub) form.
    pub fn is_bodiless(&self) -> bool {
        self.body.is_none()
    }

    pub fn effects(&self) -> Option<&EffectsClause> {
        self.clauses.iter().find_map(|c| match c {
            FnClause::Effects(e) => Some(e),
            _ => None,
        })
    }

    pub fn decreases(&self) -> Option<&DecreasesClause> {
        self.clauses.iter().find_map(|c| match c {
            FnClause::Decreases(d) => Some(d),
            _ => None,
        })
    }

    /// `requires` and `ensures` clauses in source order (the `contract` facet).
    pub fn contract(&self) -> impl Iterator<Item = &ContractClause> {
        self.clauses.iter().filter_map(|c| match c {
            FnClause::Contract(cc) => Some(cc),
            _ => None,
        })
    }

    pub fn requires(&self) -> impl Iterator<Item = &ContractClause> {
        self.contract().filter(|c| c.kind == ContractKind::Requires)
    }

    pub fn ensures(&self) -> impl Iterator<Item = &ContractClause> {
        self.contract().filter(|c| c.kind == ContractKind::Ensures)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Params {
    pub span: Span,
    pub positional: Vec<Param>,
    pub using: Vec<UsingParam>,
    pub group: Group,
    /// Comments after the last parameter, before `)`.
    pub trailing_comments: Vec<Comment>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Param {
    pub span: Span,
    pub trivia: Trivia,
    pub name: Name,
    pub ty: Type,
}

/// `using db: PageRead`
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UsingParam {
    pub span: Span,
    pub trivia: Trivia,
    pub name: Name,
    pub ty: Type,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum FnClause {
    Effects(EffectsClause),
    Contract(ContractClause),
    Decreases(DecreasesClause),
}

impl FnClause {
    pub fn span(&self) -> Span {
        match self {
            FnClause::Effects(c) => c.span,
            FnClause::Contract(c) => c.span,
            FnClause::Decreases(c) => c.span,
        }
    }

    pub fn trivia(&self) -> &Trivia {
        match self {
            FnClause::Effects(c) => &c.trivia,
            FnClause::Contract(c) => &c.trivia,
            FnClause::Decreases(c) => &c.trivia,
        }
    }
}

/// `effects { db.get, codec.dimensions }`
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EffectsClause {
    pub span: Span,
    pub trivia: Trivia,
    pub effects: Vec<EffectRef>,
    pub group: Group,
    /// Comments after the last effect, before `}`.
    pub trailing_comments: Vec<Comment>,
}

/// `param.operation`
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EffectRef {
    pub span: Span,
    pub trivia: Trivia,
    pub param: Name,
    pub operation: Name,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ContractClause {
    pub span: Span,
    pub trivia: Trivia,
    pub kind: ContractKind,
    pub expr: Expr,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
pub enum ContractKind {
    Requires,
    Ensures,
}

impl ContractKind {
    pub fn keyword(self) -> &'static str {
        match self {
            ContractKind::Requires => "requires",
            ContractKind::Ensures => "ensures",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DecreasesClause {
    pub span: Span,
    pub trivia: Trivia,
    pub expr: Expr,
}

/// `test "name" { ... }`
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TestDecl {
    pub span: Span,
    pub name: TextLiteral,
    pub body: Block,
}

/// `property "name"` with one or more `for` clauses.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PropertyDecl {
    pub span: Span,
    pub name: TextLiteral,
    pub for_clauses: Vec<ForClause>,
    pub body: Block,
}

/// `for x in gen.int`
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ForClause {
    pub span: Span,
    pub trivia: Trivia,
    pub binder: Name,
    pub range: Expr,
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Type {
    pub span: Span,
    pub path: TypePath,
    pub generic_args: Option<GenericArgs>,
}

/// `(LOWER_IDENT ".")* UPPER_IDENT`
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TypePath {
    pub span: Span,
    /// Module qualification, possibly empty.
    pub modules: Vec<Name>,
    pub name: Name,
}

impl TypePath {
    pub fn dotted(&self) -> String {
        let mut s = String::new();
        for m in &self.modules {
            s.push_str(&m.text);
            s.push('.');
        }
        s.push_str(&self.name.text);
        s
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GenericArgs {
    pub span: Span,
    pub args: Vec<Type>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GenericParams {
    pub span: Span,
    pub params: Vec<Name>,
}

// ---------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------

/// `{ lines(statement) }`. A final expression statement is stored as `tail`
/// and is the block's value; otherwise the value is `Unit`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Block {
    pub span: Span,
    pub statements: Vec<Statement>,
    pub tail: Option<ExprStmt>,
    /// Comments after the last statement, before `}`.
    pub trailing_comments: Vec<Comment>,
}

impl Block {
    pub fn is_empty(&self) -> bool {
        self.statements.is_empty() && self.tail.is_none()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum Statement {
    Let(LetStmt),
    Var(VarStmt),
    Assign(AssignStmt),
    Return(ReturnStmt),
    Assert(AssertStmt),
    While(WhileStmt),
    /// A non-tail expression statement (must have type `Unit`).
    Expr(ExprStmt),
}

impl Statement {
    pub fn span(&self) -> Span {
        match self {
            Statement::Let(s) => s.span,
            Statement::Var(s) => s.span,
            Statement::Assign(s) => s.span,
            Statement::Return(s) => s.span,
            Statement::Assert(s) => s.span,
            Statement::While(s) => s.span,
            Statement::Expr(s) => s.span,
        }
    }

    pub fn trivia(&self) -> &Trivia {
        match self {
            Statement::Let(s) => &s.trivia,
            Statement::Var(s) => &s.trivia,
            Statement::Assign(s) => &s.trivia,
            Statement::Return(s) => &s.trivia,
            Statement::Assert(s) => &s.trivia,
            Statement::While(s) => &s.trivia,
            Statement::Expr(s) => &s.trivia,
        }
    }

    pub fn trivia_mut(&mut self) -> &mut Trivia {
        match self {
            Statement::Let(s) => &mut s.trivia,
            Statement::Var(s) => &mut s.trivia,
            Statement::Assign(s) => &mut s.trivia,
            Statement::Return(s) => &mut s.trivia,
            Statement::Assert(s) => &mut s.trivia,
            Statement::While(s) => &mut s.trivia,
            Statement::Expr(s) => &mut s.trivia,
        }
    }
}

/// The binder of a `let`: a name or the discard `_` (D24).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum LetBinding {
    Name(Name),
    Wildcard(Span),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LetStmt {
    pub span: Span,
    pub trivia: Trivia,
    pub binding: LetBinding,
    pub ty: Option<Type>,
    pub value: Expr,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct VarStmt {
    pub span: Span,
    pub trivia: Trivia,
    pub name: Name,
    pub ty: Option<Type>,
    pub value: Expr,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AssignStmt {
    pub span: Span,
    pub trivia: Trivia,
    pub target: Name,
    pub value: Expr,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReturnStmt {
    pub span: Span,
    pub trivia: Trivia,
    pub value: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AssertStmt {
    pub span: Span,
    pub trivia: Trivia,
    pub condition: Expr,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WhileStmt {
    pub span: Span,
    pub trivia: Trivia,
    pub condition: Expr,
    /// `invariant` and `decreases` clauses in source order.
    pub clauses: Vec<WhileClause>,
    pub body: Block,
}

impl WhileStmt {
    pub fn invariants(&self) -> impl Iterator<Item = &InvariantClause> {
        self.clauses.iter().filter_map(|c| match c {
            WhileClause::Invariant(i) => Some(i),
            _ => None,
        })
    }

    pub fn decreases(&self) -> Option<&DecreasesClause> {
        self.clauses.iter().find_map(|c| match c {
            WhileClause::Decreases(d) => Some(d),
            _ => None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum WhileClause {
    Invariant(InvariantClause),
    Decreases(DecreasesClause),
}

impl WhileClause {
    pub fn span(&self) -> Span {
        match self {
            WhileClause::Invariant(c) => c.span,
            WhileClause::Decreases(c) => c.span,
        }
    }

    pub fn trivia(&self) -> &Trivia {
        match self {
            WhileClause::Invariant(c) => &c.trivia,
            WhileClause::Decreases(c) => &c.trivia,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InvariantClause {
    pub span: Span,
    pub trivia: Trivia,
    pub expr: Expr,
}

/// An expression statement (non-tail in [`Statement::Expr`], or the block's
/// tail in [`Block::tail`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ExprStmt {
    pub span: Span,
    pub trivia: Trivia,
    pub expr: Expr,
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Expr {
    pub span: Span,
    pub kind: ExprKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum ExprKind {
    Literal(Literal),
    /// A bare `LOWER_IDENT` or `UPPER_IDENT` (`name`).
    Name(Name),
    /// `?name` (the name without `?`).
    Hole(Name),
    Forall(Box<ForallExpr>),
    /// Structured, ordered parallel map: `parallel for x in items: expression`.
    Parallel(Box<ForallExpr>),
    Binary(Box<BinaryExpr>),
    Unary(Box<UnaryExpr>),
    Member(Box<MemberExpr>),
    Call(Box<CallExpr>),
    /// `expr?`
    Propagate(Box<Expr>),
    /// `(expr)`; kept so the formatter reproduces explicit parentheses.
    Paren(Box<Expr>),
    /// Boxed, like `Struct`, to keep `Expr` small (the parser's recursion
    /// depth budget is measured against a 1 MiB stack).
    List(Box<ListLiteral>),
    Struct(Box<StructLiteral>),
    If(Box<IfExpr>),
    Match(Box<MatchExpr>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Literal {
    pub span: Span,
    pub kind: LiteralKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum LiteralKind {
    /// Decimal digits without separators (arbitrary precision; no bigint here).
    Int(String),
    /// The decoded text.
    Text(String),
    Bool(bool),
}

/// `forall x in collection: predicate`
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ForallExpr {
    pub span: Span,
    pub binder: Name,
    pub collection: Expr,
    pub predicate: Expr,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BinaryExpr {
    pub span: Span,
    pub op: BinaryOp,
    pub op_span: Span,
    /// A line terminator directly preceded the operator in the source
    /// (possible only inside brackets); the formatter keeps the break when
    /// the expression is parenthesized (`spec/toolchain.md` 2.4 rule 9).
    pub break_before_op: bool,
    pub lhs: Expr,
    pub rhs: Expr,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum BinaryOp {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Concat,
    Mul,
    Div,
    Rem,
}

impl BinaryOp {
    pub fn symbol(self) -> &'static str {
        match self {
            BinaryOp::Or => "or",
            BinaryOp::And => "and",
            BinaryOp::Eq => "==",
            BinaryOp::Ne => "!=",
            BinaryOp::Lt => "<",
            BinaryOp::Le => "<=",
            BinaryOp::Gt => ">",
            BinaryOp::Ge => ">=",
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
            BinaryOp::Concat => "++",
            BinaryOp::Mul => "*",
            BinaryOp::Div => "/",
            BinaryOp::Rem => "%",
        }
    }

    pub fn is_comparison(self) -> bool {
        matches!(
            self,
            BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UnaryExpr {
    pub span: Span,
    pub op: UnaryOp,
    pub op_span: Span,
    pub operand: Expr,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum UnaryOp {
    Not,
    Neg,
}

impl UnaryOp {
    pub fn symbol(self) -> &'static str {
        match self {
            UnaryOp::Not => "not",
            UnaryOp::Neg => "-",
        }
    }
}

/// `target.member`
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MemberExpr {
    pub span: Span,
    pub target: Expr,
    pub member: Name,
}

/// `callee(args)` or `callee<T>(args)`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CallExpr {
    pub span: Span,
    pub callee: Expr,
    pub generic_args: Option<GenericArgs>,
    pub args: Args,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Args {
    pub span: Span,
    pub positional: Vec<Arg>,
    pub using: Vec<UsingArg>,
    pub group: Group,
    /// Comments after the last argument, before `)`.
    pub trailing_comments: Vec<Comment>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Arg {
    pub span: Span,
    pub trivia: Trivia,
    pub expr: Expr,
}

/// `using expr`
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UsingArg {
    pub span: Span,
    pub trivia: Trivia,
    pub expr: Expr,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ListLiteral {
    pub span: Span,
    pub elements: Vec<ListElem>,
    pub group: Group,
    /// Comments after the last element, before `]`.
    pub trailing_comments: Vec<Comment>,
}

/// One element of a list literal with the comments around it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ListElem {
    pub span: Span,
    pub trivia: Trivia,
    pub expr: Expr,
}

/// `Type { ..base, field: value }`
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StructLiteral {
    pub span: Span,
    pub path: TypePath,
    pub entries: StructEntries,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StructEntries {
    pub span: Span,
    /// The `..base` expression, always first when present (D29).
    pub spread: Option<Expr>,
    /// Comments around the spread entry (empty without a spread).
    pub spread_trivia: Trivia,
    pub fields: Vec<FieldInit>,
    pub group: Group,
    /// Comments after the last entry, before `}`.
    pub trailing_comments: Vec<Comment>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FieldInit {
    pub span: Span,
    pub trivia: Trivia,
    pub name: Name,
    pub value: Expr,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct IfExpr {
    pub span: Span,
    pub condition: Expr,
    pub then_block: Block,
    pub else_branch: Option<ElseBranch>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum ElseBranch {
    Block(Block),
    /// `else if ...`
    If(Box<IfExpr>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MatchExpr {
    pub span: Span,
    pub scrutinee: Expr,
    pub arms: Vec<MatchArm>,
    /// Comments after the last arm, before `}`.
    pub trailing_comments: Vec<Comment>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MatchArm {
    pub span: Span,
    pub trivia: Trivia,
    pub pattern: Pattern,
    pub body: ArmBody,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum ArmBody {
    Block(Block),
    Expr(Expr),
}

// ---------------------------------------------------------------------------
// Patterns
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Pattern {
    pub span: Span,
    pub kind: PatternKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum PatternKind {
    /// `_`
    Wildcard,
    /// `"-"? INT`
    Int {
        negative: bool,
        digits: String,
    },
    Text(String),
    Bool(bool),
    /// A bare `LOWER_IDENT`.
    Binding(Name),
    /// A constructor, optionally with nested patterns.
    Ctor(CtorPattern),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CtorPattern {
    pub path: CtorPath,
    /// `None` for a nullary constructor written without parentheses.
    pub args: Option<Vec<Pattern>>,
    pub group: Group,
}

/// `(LOWER_IDENT ".")* UPPER_IDENT ("." UPPER_IDENT)*`
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CtorPath {
    pub span: Span,
    /// Everything before the constructor name (modules, then types).
    pub qualifier: Vec<Name>,
    pub name: Name,
}

impl CtorPath {
    pub fn dotted(&self) -> String {
        let mut s = String::new();
        for q in &self.qualifier {
            s.push_str(&q.text);
            s.push('.');
        }
        s.push_str(&self.name.text);
        s
    }
}

// ---------------------------------------------------------------------------
// Work files
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WorkFile {
    pub span: Span,
    pub uses: Vec<UseDecl>,
    pub changes: Vec<ChangeDecl>,
    pub trailing_comments: Vec<Comment>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ChangeDecl {
    pub span: Span,
    pub trivia: Trivia,
    pub docs: Option<Docs>,
    pub name: Name,
    pub items: Vec<ChangeItem>,
    pub trailing_comments: Vec<Comment>,
}

impl ChangeDecl {
    pub fn tasks(&self) -> impl Iterator<Item = &TaskDecl> {
        self.items.iter().filter_map(|i| match &i.kind {
            ChangeItemKind::Task(t) => Some(t),
            _ => None,
        })
    }

    pub fn base(&self) -> Option<&GitRef> {
        self.items.iter().find_map(|i| match &i.kind {
            ChangeItemKind::Base(g) => Some(g),
            _ => None,
        })
    }

    pub fn target(&self) -> Option<&GitRef> {
        self.items.iter().find_map(|i| match &i.kind {
            ChangeItemKind::Target(g) => Some(g),
            _ => None,
        })
    }

    pub fn workers(&self) -> Option<&IntLiteral> {
        self.items.iter().find_map(|i| match &i.kind {
            ChangeItemKind::Workers(w) => Some(w),
            _ => None,
        })
    }

    pub fn integrate(&self) -> Option<&IntegrateDecl> {
        self.items.iter().find_map(|i| match &i.kind {
            ChangeItemKind::Integrate(d) => Some(d),
            _ => None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ChangeItem {
    pub span: Span,
    pub trivia: Trivia,
    pub kind: ChangeItemKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum ChangeItemKind {
    Base(GitRef),
    Target(GitRef),
    Workers(IntLiteral),
    Task(TaskDecl),
    Integrate(IntegrateDecl),
}

/// `git("ref")`
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GitRef {
    pub span: Span,
    pub reference: TextLiteral,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TaskDecl {
    pub span: Span,
    pub docs: Option<Docs>,
    pub name: Name,
    pub after: Option<AfterClause>,
    pub items: Vec<TaskItem>,
    pub trailing_comments: Vec<Comment>,
}

/// `after a, b`
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AfterClause {
    pub span: Span,
    pub tasks: Vec<Name>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TaskItem {
    pub span: Span,
    pub trivia: Trivia,
    pub kind: TaskItemKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum TaskItemKind {
    Write(SelectorSet),
    Add(SelectorSet),
    Depend(SelectorSet),
    Preserve(SelectorSet),
    Goal(TextLiteral),
}

impl TaskItemKind {
    pub fn keyword(&self) -> &'static str {
        match self {
            TaskItemKind::Write(_) => "write",
            TaskItemKind::Add(_) => "add",
            TaskItemKind::Depend(_) => "depend",
            TaskItemKind::Preserve(_) => "preserve",
            TaskItemKind::Goal(_) => "goal",
        }
    }
}

/// `{ selector, ... }` (possibly empty).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SelectorSet {
    pub span: Span,
    pub selectors: Vec<Selector>,
    pub group: Group,
    /// Comments after the last selector, before `}`.
    pub trailing_comments: Vec<Comment>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct IntegrateDecl {
    pub span: Span,
    pub requirements: Vec<RequireItem>,
    pub trailing_comments: Vec<Comment>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RequireItem {
    pub span: Span,
    pub trivia: Trivia,
    pub requirement: Requirement,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Requirement {
    pub span: Span,
    pub kind: RequirementKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum RequirementKind {
    Typecheck,
    NoHoles,
    NoUnresolvedAssumptions,
    Prove(Selector),
    Test(Selector),
    /// `review(task)`: a task name of the same change.
    Review(Name),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Selector {
    pub span: Span,
    pub trivia: Trivia,
    /// The segments before the final `*` or facet (or the whole path for a
    /// declaration selector).
    pub path: SelectorPath,
    pub kind: SelectorKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SelectorPath {
    pub span: Span,
    pub segments: Vec<Name>,
}

impl SelectorPath {
    pub fn dotted(&self) -> String {
        self.segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(".")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum SelectorKind {
    /// `path.*`: every declaration in a namespace.
    Namespace,
    /// `path.facet`
    Facet(Facet),
    /// `path`: a whole declaration (equivalent to `.definition`, D38).
    Declaration,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Facet {
    pub span: Span,
    pub kind: FacetKind,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum FacetKind {
    Identity,
    Name,
    Signature,
    Contract,
    Effects,
    Body,
    Docs,
    VerificationPolicy,
    Definition,
}

impl FacetKind {
    pub fn parse(spelling: &str) -> Option<FacetKind> {
        Some(match spelling {
            "identity" => FacetKind::Identity,
            "name" => FacetKind::Name,
            "signature" => FacetKind::Signature,
            "contract" => FacetKind::Contract,
            "effects" => FacetKind::Effects,
            "body" => FacetKind::Body,
            "docs" => FacetKind::Docs,
            "verification_policy" => FacetKind::VerificationPolicy,
            "definition" => FacetKind::Definition,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            FacetKind::Identity => "identity",
            FacetKind::Name => "name",
            FacetKind::Signature => "signature",
            FacetKind::Contract => "contract",
            FacetKind::Effects => "effects",
            FacetKind::Body => "body",
            FacetKind::Docs => "docs",
            FacetKind::VerificationPolicy => "verification_policy",
            FacetKind::Definition => "definition",
        }
    }
}

// ---------------------------------------------------------------------------
// Patch files
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PatchFile {
    pub span: Span,
    pub forms: Vec<PatchForm>,
    pub trailing_comments: Vec<Comment>,
}

/// `patch selector` / `expect base(selector)` / `{ block }`
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PatchForm {
    pub span: Span,
    pub trivia: Trivia,
    pub docs: Option<Docs>,
    pub target: Selector,
    pub expect: ExpectClause,
    pub body: Block,
}

/// `expect base(selector)`
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ExpectClause {
    pub span: Span,
    pub trivia: Trivia,
    pub base: Selector,
}
