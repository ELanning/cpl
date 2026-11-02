//! Name resolution machinery (`spec/semantics.md` 2.1-2.5): type paths in
//! type positions, the scope-2/scope-1 lookup of value names, qualified
//! chains (2.4), and constructor resolution (2.5, S10), plus the side tables
//! ([`Resolutions`]) in which the resolution walk of bodies
//! (`crate::names`) records what every name of a body, clause, or
//! predicate resolved to.
//!
//! # For the body typer
//!
//! A [`Resolver`] is the scope of one file (its imports and module) with a
//! declaration's type parameters; get one with `Checker::resolver(def,
//! generics)`. It never sees local bindings: the resolution walk resolves
//! locals with `scope::LocalScopes` before it calls
//! [`Resolver::resolve_global_chain`] or [`Resolver::lookup_value`].
//!
//! The typer reads [`Resolutions`] instead of resolving again:
//!
//! | AST node | Table | Entry |
//! | - | - | - |
//! | `ExprKind::Name` (lower or upper) | `exprs[expr.span]` | a [`Res`] |
//! | an `ExprKind::Member` chain resolved through a module or type root (`std.text.trim`, `StoreError.Missing`, `gen.int`) | `exprs[outermost member expr.span]` | a [`Res`]; the inner members and the root name have no entry |
//! | any other `ExprKind::Member` | none | a value member, resolved by type (3.8, 3.9) |
//! | `PatternKind::Ctor` | `patterns[ctor_path.span]` | a [`Res`] |
//! | `StructLiteral::path` | `struct_paths[path.span]` | `Res::Struct` or `Res::Error` |
//! | `AssignStmt::target` | `assigns[target.span]` | `Res::Local` or `Res::Error` |
//! | let/var names, pattern bindings, `forall`/`for` binders, parameters | `binders[name.span]` | the `LocalId` in the declaration's local table |
//! | let/var annotations, call and constructor `generic_args` | `types[ast::Type::span]` | the resolved `Ty` (`Ty::Error` after a diagnostic) |
//! | `ExprKind::Hole` | `holes[expr.span]` | the visible bindings, innermost first |
//!
//! A [`Res::DeferredCtor`] is an unqualified constructor whose resolution
//! depends on the expected type (S10 step 1): the typer resolves it with
//! [`Resolver::resolve_ctor`], which reports `ambiguous_constructor`,
//! `unresolved_name`, and `not_a_constructor` itself. Every other entry is
//! final: its diagnostics have been reported, and a [`Res::Error`] gives
//! the expression the error type (S62).

use crate::checker::{diag, Checker, FileInfo};
use crate::codes;
use crate::members::MemberTable;
use crate::refine::Refinements;
use crate::scope::{Binding, ModuleScope, Universe, RESERVED_TYPE_NAMES};
use cpl_diag::{Diagnostic, Span};
use cpl_hir::{DefId, DefKind, LocalId, ModuleId, Program, Ty};
use cpl_syntax::ast;
use serde_json::json;
use std::collections::{BTreeMap, HashMap};

/// A constructor (semantics 3.1): an enum variant, the prelude's `Unit`,
/// `Some`, `None`, `Ok`, `Err` included, or a nominal wrapper.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Ctor {
    Variant { def: DefId, index: u32 },
    Wrapper(DefId),
}

impl Ctor {
    /// `reader.pages.StoreError.Missing`, `reader.pages.PageId`.
    pub fn qualified_name(self, program: &Program) -> String {
        match self {
            Ctor::Variant { def, index } => {
                let variant = program
                    .def(def)
                    .kind
                    .as_enum()
                    .and_then(|e| e.variants.get(index as usize))
                    .map_or("?", |v| v.name.as_str())
                    .to_string();
                format!("{}.{variant}", program.display_path(def))
            }
            Ctor::Wrapper(def) => program.display_path(def),
        }
    }

    /// The declaration the constructor belongs to.
    #[allow(dead_code)] // the body typer's `Ctor`/`Wrap` HIR nodes
    pub fn def(self) -> DefId {
        match self {
            Ctor::Variant { def, .. } | Ctor::Wrapper(def) => def,
        }
    }
}

/// What a name or chain of a body resolved to (see the module docs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Res {
    /// A binding of the enclosing declaration's local table (parameters,
    /// `using` parameters, `result`, `self`, and every local).
    Local(LocalId),
    /// A function: an ordinary `fn`, a `@builtin` stub, or a module-level
    /// property (`std.gen.int`, read without parentheses, stdlib S40).
    Fn(DefId),
    /// The prelude intrinsic `check` (stdlib 1.4).
    Check,
    /// A constructor.
    Ctor(Ctor),
    /// An unqualified constructor that the expected type decides (S10 step
    /// 1); `candidates` is the lexical set of step 2.
    #[allow(dead_code)]
    // the typer resolves again from the AST name (`Resolver::resolve_ctor`)
    DeferredCtor { name: String, candidates: Vec<Ctor> },
    /// A module path in callee position (`std.text()`): the typer reports
    /// `not_callable` (3.8). In any other position it was `not_a_value`.
    Module(Vec<String>),
    /// The struct a struct literal names.
    Struct(DefId),
    /// Diagnosed: the expression has the error type (S62).
    Error,
}

/// The resolution side tables of every body, clause, and predicate (see
/// the module docs for the key of each table).
#[derive(Clone, Debug, Default)]
pub(crate) struct Resolutions {
    pub exprs: HashMap<Span, Res>,
    pub patterns: HashMap<Span, Res>,
    pub struct_paths: HashMap<Span, Res>,
    pub assigns: HashMap<Span, Res>,
    pub binders: HashMap<Span, LocalId>,
    pub types: HashMap<Span, Ty>,
    pub holes: HashMap<Span, Vec<LocalId>>,
    /// A local callee that hides a module or universe item: the item, for
    /// the note of `not_callable` (S8, 3.8).
    pub hidden: HashMap<Span, DefId>,
}

/// Every constructor of the program by name (std included). A name no
/// declaration of the program uses for a constructor cannot be resolved by
/// any expected type, which is what lets the resolution walk report
/// `unresolved_name` and `not_a_constructor` before typing (S10).
#[derive(Clone, Debug, Default)]
pub(crate) struct CtorIndex {
    by_name: BTreeMap<String, Vec<Ctor>>,
}

impl CtorIndex {
    pub fn build(program: &Program) -> CtorIndex {
        let mut by_name: BTreeMap<String, Vec<Ctor>> = BTreeMap::new();
        for def in program.defs() {
            match &def.kind {
                DefKind::Enum(e) => {
                    for (index, v) in e.variants.iter().enumerate() {
                        by_name
                            .entry(v.name.clone())
                            .or_default()
                            .push(Ctor::Variant {
                                def: def.id,
                                index: index as u32,
                            });
                    }
                }
                DefKind::Wrapper(_) => {
                    by_name
                        .entry(def.name.display())
                        .or_default()
                        .push(Ctor::Wrapper(def.id));
                }
                _ => {}
            }
        }
        CtorIndex { by_name }
    }

    /// Every constructor named `name`.
    pub fn named(&self, name: &str) -> &[Ctor] {
        self.by_name.get(name).map_or(&[], Vec::as_slice)
    }
}

/// Diagnostics and type uses produced by one resolution.
#[derive(Debug, Default)]
pub(crate) struct ResOut {
    pub diags: Vec<Diagnostic>,
    /// Every type path resolved to a definition, with the path's span:
    /// `TypeUse` references (toolchain 6.5) and the exportability check of
    /// semantics 1.5.
    pub uses: Vec<(Span, DefId)>,
}

impl ResOut {
    pub fn push(&mut self, d: Diagnostic) {
        self.diags.push(d);
    }
}

/// What a type path names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TypeTarget {
    /// A type parameter of the enclosing declaration (scope 3).
    Param(u32),
    Def(DefId),
    /// Diagnosed (or an erroneous import, S62).
    Error,
}

/// What step 3 of S10 finds for an unqualified constructor name that no
/// constructor matched ([`Resolver::ctor_step3`]).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum CtorStep3 {
    /// A type that is not a constructor (a type parameter, a refinement, a
    /// capability, `Int`, a struct or a bare enum name): `not_a_constructor`.
    Type,
    /// A diagnosed `use` item (S62): nothing more is reported.
    Erroneous,
    /// Nothing of that name: `unresolved_name`.
    Nothing,
}

/// What a lower-case name of an expression names at scope 2 or 1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ValueTarget {
    Fn(DefId),
    Check,
    /// A module, submodule alias, `gen`, or root module name: the start of
    /// a module chain, with the module path it denotes.
    Module(Vec<String>),
    /// An erroneous import (S62).
    Error,
    NotFound,
}

/// What a qualified chain (2.4) names when its root is not a local.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PathOutcome {
    Fn(DefId),
    Check,
    Ctor(Ctor),
    /// A type used as a chain's end: not a constructor unless a wrapper.
    Type(DefId),
    TypeParam,
    /// The chain ends inside a module path.
    Module(Vec<String>),
    /// Diagnosed.
    Error,
}

/// The scope of one file and declaration (see the module docs).
pub(crate) struct Resolver<'c> {
    pub program: &'c Program,
    pub modules: &'c [ModuleScope],
    pub universe: &'c Universe,
    pub refinements: &'c Refinements,
    pub ctors: &'c CtorIndex,
    pub members: &'c MemberTable,
    pub file: &'c FileInfo,
    /// Scope 3: the type parameters of the declaration (`Ty::Param(i)` is
    /// the first `generics[i]` of that name).
    pub generics: &'c [String],
}

impl<'c> Resolver<'c> {
    pub fn new(cx: &'c Checker<'_>, file: &'c FileInfo, generics: &'c [String]) -> Resolver<'c> {
        Resolver {
            program: &cx.program,
            modules: &cx.modules,
            universe: &cx.universe,
            refinements: &cx.refinements,
            ctors: &cx.ctors,
            members: &cx.members,
            file,
            generics,
        }
    }

    fn scope2(&self) -> &ModuleScope {
        &self.modules[self.file.module.0 as usize]
    }

    fn is_pub(&self, def: DefId) -> bool {
        self.program.def(def).is_pub
    }

    fn is_type(&self, def: DefId) -> bool {
        self.program.def(def).kind.is_type()
    }

    fn is_fn(&self, def: DefId) -> bool {
        matches!(self.program.def(def).kind, DefKind::Fn(_))
    }

    // ---- types ---------------------------------------------------------------

    /// Resolve a type expression (2.2 `type_path` row, S12): unresolved
    /// paths, private items, reserved names, and wrong type-argument counts
    /// are reported and give `Ty::Error` (S62). Value-type rules (S13) are
    /// the caller's: [`value_type_errors`].
    pub fn resolve_type(&self, t: &ast::Type, out: &mut ResOut) -> Ty {
        let args: Vec<&ast::Type> = t
            .generic_args
            .as_ref()
            .map(|g| g.args.iter().collect())
            .unwrap_or_default();
        match self.type_path(&t.path, out) {
            TypeTarget::Param(i) => {
                if args.is_empty() {
                    Ty::Param(i)
                } else {
                    for a in &args {
                        self.resolve_type(a, out);
                    }
                    out.push(count_mismatch(t, &t.path.name.text, 0, args.len()));
                    Ty::Error
                }
            }
            TypeTarget::Error => {
                for a in &args {
                    self.resolve_type(a, out);
                }
                Ty::Error
            }
            TypeTarget::Def(def) => {
                out.uses.push((t.path.span, def));
                let resolved: Vec<Ty> = args.iter().map(|a| self.resolve_type(a, out)).collect();
                self.instantiate(def, t, resolved, out)
            }
        }
    }

    /// The type a definition denotes with `args`.
    fn instantiate(&self, def: DefId, t: &ast::Type, args: Vec<Ty>, out: &mut ResOut) -> Ty {
        let name = &t.path.name.text;
        let arity = |n: usize, out: &mut ResOut| -> bool {
            if args.len() == n {
                true
            } else {
                out.push(count_mismatch(t, name, n, args.len()));
                false
            }
        };
        match &self.program.def(def).kind {
            DefKind::BuiltinType(b) => match b.builtin.as_str() {
                "int" if arity(0, out) => Ty::Int,
                "bool" if arity(0, out) => Ty::Bool,
                "text" if arity(0, out) => Ty::Text,
                "bytes" if arity(0, out) => Ty::Bytes,
                "list" if arity(1, out) => Ty::List(Box::new(args[0].clone())),
                "gen" if !self.file.is_std => {
                    out.push(
                        diag(
                            codes::UNRESOLVED_TYPE,
                            Some(t.path.span),
                            "`Gen` is not nameable in edition 0.1: generators appear only in `for` ranges",
                        )
                        .with_data("name", json!("Gen"))
                        .with_note("Gen<T> is the hermetic generator type (semantics S14)", None),
                    );
                    Ty::Error
                }
                "gen" if arity(1, out) => Ty::Gen(Box::new(args[0].clone())),
                _ => Ty::Error,
            },
            DefKind::Struct(_) | DefKind::Enum(_) => {
                let n = self.program.def(def).kind.generics().len();
                if arity(n, out) {
                    Ty::Adt(def, args)
                } else {
                    Ty::Error
                }
            }
            DefKind::Wrapper(_) if arity(0, out) => Ty::Wrapper(def),
            DefKind::Refinement(_) if arity(0, out) => self.refinements.ty_of(def),
            DefKind::Capability(_) if arity(0, out) => Ty::Capability(def),
            _ => Ty::Error,
        }
    }

    /// Resolve a type path: scope 3, then the file's imports and the
    /// module's items (scope 2), then the prelude (scope 1); a qualified
    /// path through its module prefix (2.4). Marks the imports it uses.
    pub fn type_path(&self, path: &ast::TypePath, out: &mut ResOut) -> TypeTarget {
        let name = path.name.text.as_str();
        if path.modules.is_empty() {
            if let Some(i) = self.generics.iter().position(|g| g == name) {
                return TypeTarget::Param(i as u32);
            }
            if let Some(binding) = self.file.scope.use_import(name) {
                return match binding {
                    Binding::Def(d) if self.is_type(*d) => TypeTarget::Def(*d),
                    Binding::Error => TypeTarget::Error,
                    _ => {
                        out.push(not_a_type(path, name));
                        TypeTarget::Error
                    }
                };
            }
            if let Some(&d) = self.scope2().items.get(name) {
                if self.is_type(d) {
                    return TypeTarget::Def(d);
                }
                out.push(not_a_type(path, name));
                return TypeTarget::Error;
            }
            if let Some(&d) = self.universe.prelude.get(name) {
                if self.is_type(d) {
                    return TypeTarget::Def(d);
                }
            }
            out.push(unresolved_type(path, name));
            return TypeTarget::Error;
        }
        let Some(module) = self.type_module(&path.modules, out) else {
            return TypeTarget::Error;
        };
        let scope = &self.modules[module.0 as usize];
        let Some(&item) = scope.items.get(name) else {
            out.push(
                diag(
                    codes::UNRESOLVED_TYPE,
                    Some(path.span),
                    format!("module `{}` has no type `{name}`", scope.dotted()),
                )
                .with_data("name", json!(name))
                .with_data("module", json!(scope.dotted())),
            );
            return TypeTarget::Error;
        };
        if !self.is_pub(item) && module != self.file.module {
            out.push(self.private_item(item, path.span));
            return TypeTarget::Error;
        }
        if !self.is_type(item) {
            out.push(not_a_type(path, name));
            return TypeTarget::Error;
        }
        TypeTarget::Def(item)
    }

    /// The module of a type path's qualification: every segment must be
    /// part of the module path (a submodule alias, `gen`, or a root module
    /// name first).
    fn type_module(&self, segs: &[ast::Name], out: &mut ResOut) -> Option<ModuleId> {
        let first = &segs[0];
        let mut path: Vec<String> = match self.file.scope.use_import(&first.text) {
            Some(Binding::Module(m)) => self.modules[m.0 as usize].path.clone(),
            Some(Binding::Namespace(p)) => p.clone(),
            Some(Binding::Error) => return None,
            Some(Binding::Def(_)) | None => {
                if first.text == "gen" {
                    self.gen_path()?
                } else if self.universe.roots.contains(&first.text) {
                    vec![first.text.clone()]
                } else {
                    out.push(unresolved_module_at(
                        segs.iter()
                            .map(|s| s.text.as_str())
                            .collect::<Vec<_>>()
                            .join("."),
                        first.span.join(segs[segs.len() - 1].span),
                    ));
                    return None;
                }
            }
        };
        path.extend(segs[1..].iter().map(|s| s.text.clone()));
        match self.universe.module(&path) {
            Some(m) => Some(m),
            None => {
                out.push(unresolved_module_at(
                    path.join("."),
                    first.span.join(segs[segs.len() - 1].span),
                ));
                None
            }
        }
    }

    fn gen_path(&self) -> Option<Vec<String>> {
        self.universe
            .gen_module
            .map(|m| self.modules[m.0 as usize].path.clone())
    }

    /// A type-path lookup with no diagnostics and without marking imports:
    /// what a type name would resolve to (the step-3 test of S10, the
    /// dependency scan of refinement bases).
    pub fn peek_type(&self, path: &ast::TypePath) -> TypeTarget {
        let name = path.name.text.as_str();
        if path.modules.is_empty() {
            if let Some(i) = self.generics.iter().position(|g| g == name) {
                return TypeTarget::Param(i as u32);
            }
            if let Some(b) = self.file.scope.peek(name) {
                return match b {
                    Binding::Def(d) if self.is_type(*d) => TypeTarget::Def(*d),
                    _ => TypeTarget::Error,
                };
            }
            if let Some(&d) = self.scope2().items.get(name) {
                return if self.is_type(d) {
                    TypeTarget::Def(d)
                } else {
                    TypeTarget::Error
                };
            }
            return match self.universe.prelude.get(name) {
                Some(&d) if self.is_type(d) => TypeTarget::Def(d),
                _ => TypeTarget::Error,
            };
        }
        let mut quiet = ResOut::default();
        let used: Vec<bool> = self
            .file
            .scope
            .imports
            .values()
            .map(|i| i.used.get())
            .collect();
        let target = self.type_path(path, &mut quiet);
        for (import, was) in self.file.scope.imports.values().zip(used) {
            import.used.set(was);
        }
        target
    }

    // ---- values --------------------------------------------------------------

    /// A lower-case name at scope 2, then scope 1 (2.2 `name` row): an
    /// imported or module function or submodule alias, the prelude's
    /// `flatten` and `check`, `gen`, or a root module name. Marks the
    /// import it uses.
    pub fn lookup_value(&self, name: &str) -> ValueTarget {
        if let Some(binding) = self.file.scope.use_import(name) {
            return match binding {
                Binding::Def(d) if self.is_fn(*d) => ValueTarget::Fn(*d),
                Binding::Def(_) | Binding::Error => ValueTarget::Error,
                Binding::Module(m) => ValueTarget::Module(self.modules[m.0 as usize].path.clone()),
                Binding::Namespace(p) => ValueTarget::Module(p.clone()),
            };
        }
        if let Some(&d) = self.scope2().items.get(name) {
            if self.is_fn(d) {
                return ValueTarget::Fn(d);
            }
        }
        if let Some(&d) = self.universe.prelude.get(name) {
            if self.is_fn(d) {
                return ValueTarget::Fn(d);
            }
        }
        if name == "check" {
            return ValueTarget::Check;
        }
        if name == "gen" {
            if let Some(p) = self.gen_path() {
                return ValueTarget::Module(p);
            }
        }
        if self.universe.roots.contains(name) {
            return ValueTarget::Module(vec![name.to_string()]);
        }
        ValueTarget::NotFound
    }

    /// The note of an `unresolved_name` for a member called as a free
    /// function (stdlib S37: `take is a method of List; write list.take(n)`).
    pub fn member_hint(&self, name: &str) -> Option<String> {
        use crate::members::{MemberKind, TypeHead};
        let heads = [
            TypeHead::Int,
            TypeHead::Bool,
            TypeHead::Text,
            TypeHead::Bytes,
            TypeHead::List,
            TypeHead::Option,
            TypeHead::Result,
        ];
        let found: Vec<(TypeHead, MemberKind)> = heads
            .iter()
            .filter_map(|h| self.members.lookup(*h, name).map(|m| (*h, m.kind)))
            .collect();
        let (_, kind) = found.first()?;
        let receivers: Vec<&str> = found.iter().map(|(h, _)| h.name()).collect();
        let (what, call) = match kind {
            MemberKind::Property => ("property", ""),
            MemberKind::Method => ("method", "(...)"),
        };
        Some(format!(
            "`{name}` is a {what} of {}, not a free function; write `x.{name}{call}`",
            receivers.join(", ")
        ))
    }

    /// The module-level item a lower-case name would name, without marking
    /// imports: the item a local binding hides (S8, `not_callable`'s note).
    pub fn peek_value_item(&self, name: &str) -> Option<DefId> {
        if let Some(b) = self.file.scope.peek(name) {
            return match b {
                Binding::Def(d) => Some(*d),
                _ => None,
            };
        }
        if let Some(&d) = self.scope2().items.get(name) {
            return Some(d);
        }
        self.universe.prelude.get(name).copied()
    }

    /// Resolve a qualified chain (2.4) whose root is not a local binding:
    /// a module chain (submodule alias, `gen`, root module name, or a
    /// function root), or a type chain (`StoreError.Missing`). Reports
    /// `unresolved_name`, `unresolved_module`, and `private_item`.
    pub fn resolve_global_chain(&self, segs: &[&ast::Name], out: &mut ResOut) -> PathOutcome {
        let root = segs[0];
        if root.is_upper() {
            let path = ast::TypePath {
                span: root.span,
                modules: Vec::new(),
                name: root.clone(),
            };
            return match self.type_path_quiet_unresolved(&path, out) {
                TypeTarget::Def(d) => self.continue_on_type(d, &segs[1..], out),
                TypeTarget::Param(_) => {
                    if segs.len() == 1 {
                        PathOutcome::TypeParam
                    } else {
                        out.push(not_a_member(segs[1], &root.text, "type parameter"));
                        PathOutcome::Error
                    }
                }
                TypeTarget::Error => PathOutcome::Error,
            };
        }
        match self.lookup_value(&root.text) {
            ValueTarget::Fn(d) => {
                if segs.len() == 1 {
                    PathOutcome::Fn(d)
                } else {
                    out.push(not_a_member(segs[1], &root.text, "function"));
                    PathOutcome::Error
                }
            }
            ValueTarget::Check => {
                if segs.len() == 1 {
                    PathOutcome::Check
                } else {
                    out.push(not_a_member(segs[1], "check", "function"));
                    PathOutcome::Error
                }
            }
            ValueTarget::Module(path) => self.continue_on_module(path, &segs[1..], out),
            ValueTarget::Error => PathOutcome::Error,
            ValueTarget::NotFound => {
                out.push(unresolved_name(root.span, &root.text, None));
                PathOutcome::Error
            }
        }
    }

    /// A type root of a chain: like [`Resolver::type_path`], but a name
    /// that is not a type is `unresolved_name` (an expression position).
    fn type_path_quiet_unresolved(&self, path: &ast::TypePath, out: &mut ResOut) -> TypeTarget {
        let mut inner = ResOut::default();
        let target = self.type_path(path, &mut inner);
        for d in inner.diags {
            if d.code == codes::UNRESOLVED_TYPE {
                out.push(unresolved_name(path.span, &path.name.text, None));
            } else {
                out.push(d);
            }
        }
        out.uses.extend(inner.uses);
        target
    }

    /// The rest of a chain after a module path `path` (2.4 step 1, module
    /// branch, and step 2).
    pub fn continue_on_module(
        &self,
        mut path: Vec<String>,
        rest: &[&ast::Name],
        out: &mut ResOut,
    ) -> PathOutcome {
        let mut i = 0;
        while i < rest.len() && !rest[i].is_upper() {
            let mut longer = path.clone();
            longer.push(rest[i].text.clone());
            if !self.universe.is_module_or_prefix(&longer) {
                break;
            }
            path = longer;
            i += 1;
        }
        if i == rest.len() {
            return PathOutcome::Module(path);
        }
        let next = rest[i];
        let Some(module) = self.universe.module(&path) else {
            let mut missing = path.clone();
            if !next.is_upper() {
                missing.push(next.text.clone());
            }
            out.push(unresolved_module_at(missing.join("."), next.span));
            return PathOutcome::Error;
        };
        let scope = &self.modules[module.0 as usize];
        let Some(&item) = scope.items.get(&next.text) else {
            out.push(unresolved_name(
                next.span,
                &next.text,
                Some(&scope.dotted()),
            ));
            return PathOutcome::Error;
        };
        if !self.is_pub(item) && module != self.file.module {
            out.push(self.private_item(item, next.span));
            return PathOutcome::Error;
        }
        if self.is_fn(item) {
            return if i + 1 == rest.len() {
                PathOutcome::Fn(item)
            } else {
                out.push(not_a_member(rest[i + 1], &next.text, "function"));
                PathOutcome::Error
            };
        }
        self.continue_on_type(item, &rest[i + 1..], out)
    }

    /// The rest of a chain after a type (2.4 step 2): an enum's variant, or
    /// nothing.
    fn continue_on_type(&self, def: DefId, rest: &[&ast::Name], out: &mut ResOut) -> PathOutcome {
        let Some(seg) = rest.first() else {
            return PathOutcome::Type(def);
        };
        let type_name = self.program.def(def).name.display();
        if let DefKind::Enum(e) = &self.program.def(def).kind {
            if seg.is_upper() {
                let Some(index) = e.variants.iter().position(|v| v.name == seg.text) else {
                    out.push(
                        diag(
                            codes::UNRESOLVED_NAME,
                            Some(seg.span),
                            format!("enum `{type_name}` has no variant `{}`", seg.text),
                        )
                        .with_data("name", json!(seg.text)),
                    );
                    return PathOutcome::Error;
                };
                if let Some(after) = rest.get(1) {
                    out.push(not_a_member(after, &seg.text, "variant"));
                    return PathOutcome::Error;
                }
                return PathOutcome::Ctor(Ctor::Variant {
                    def,
                    index: index as u32,
                });
            }
        }
        out.push(not_a_member(seg, &type_name, "type"));
        PathOutcome::Error
    }

    /// `private_item` for `item`, carrying its symbol (1.5).
    pub fn private_item(&self, item: DefId, span: Span) -> Diagnostic {
        let def = self.program.def(item);
        let qualified = self.program.display_path(item);
        diag(
            codes::PRIVATE_ITEM,
            Some(span),
            format!(
                "`{qualified}` is private to module `{}`",
                self.modules[def.module.0 as usize].dotted()
            ),
        )
        .with_symbol(def.stable_id.clone(), qualified, None)
    }

    // ---- constructors --------------------------------------------------------

    /// The lexical candidates of S10 step 2 for `name`: every variant of
    /// that name of an enum declared in the module or imported into the
    /// file, every wrapper of that name declared or imported, and the
    /// prelude's `Unit`, `Some`, `None`, `Ok`, `Err`. Marks the imports a
    /// candidate came through as used.
    pub fn lexical_ctors(&self, name: &str) -> Vec<Ctor> {
        let mut out: Vec<Ctor> = Vec::new();
        let push = |c: Ctor, out: &mut Vec<Ctor>| {
            if !out.contains(&c) {
                out.push(c);
            }
        };
        let consider = |def: DefId, item_name: &str, out: &mut Vec<Ctor>| -> bool {
            match &self.program.def(def).kind {
                DefKind::Enum(e) => {
                    let mut found = false;
                    for (index, v) in e.variants.iter().enumerate() {
                        if v.name == name {
                            push(
                                Ctor::Variant {
                                    def,
                                    index: index as u32,
                                },
                                out,
                            );
                            found = true;
                        }
                    }
                    found
                }
                DefKind::Wrapper(_) if item_name == name => {
                    push(Ctor::Wrapper(def), out);
                    true
                }
                _ => false,
            }
        };
        for (item_name, &def) in &self.scope2().items {
            consider(def, item_name, &mut out);
        }
        for (item_name, import) in &self.file.scope.imports {
            if let Binding::Def(def) = import.binding {
                if consider(def, item_name, &mut out) {
                    import.used.set(true);
                }
            }
        }
        let std = self.program.std_defs;
        for def in [std.unit, std.option, std.result] {
            if (def.0 as usize) < self.program.defs.len() {
                consider(def, "", &mut out);
            }
        }
        out
    }

    /// Steps 2 and 3 of S10 with no expected type: the unique lexical
    /// candidate, or `ambiguous_constructor`, `not_a_constructor` (a type
    /// that is not a constructor), or `unresolved_name`.
    pub fn lexical_ctor(&self, name: &ast::Name, out: &mut ResOut) -> Res {
        let candidates = self.lexical_ctors(&name.text);
        match candidates.len() {
            1 => Res::Ctor(candidates[0]),
            0 => {
                out.diags.extend(self.no_ctor(name));
                Res::Error
            }
            _ => {
                let mut names: Vec<String> = candidates
                    .iter()
                    .map(|c| c.qualified_name(self.program))
                    .collect();
                names.sort();
                out.push(
                    diag(
                        codes::AMBIGUOUS_CONSTRUCTOR,
                        Some(name.span),
                        format!(
                            "`{}` is ambiguous: it may be {}; write the qualified form",
                            name.text,
                            names
                                .iter()
                                .map(|n| format!("`{n}`"))
                                .collect::<Vec<_>>()
                                .join(" or ")
                        ),
                    )
                    .with_data("name", json!(name.text))
                    .with_data("candidates", json!(names)),
                );
                Res::Error
            }
        }
    }

    /// Step 3 of S10 for an unqualified constructor name: what the name
    /// denotes as a type (scopes 3 to 1, the lookup of [`Resolver::peek_type`]).
    /// Like every other lookup it goes through the file's imports and marks
    /// the import it finds used: the name resolves through it, and without
    /// it the diagnostic would be `unresolved_name` instead of
    /// `not_a_constructor` (1.6; the same holds for a function or a module
    /// alias in value position, which is `not_a_value` and uses its import).
    pub fn ctor_step3(&self, name: &str) -> CtorStep3 {
        let ty = |d: DefId| {
            if self.is_type(d) {
                CtorStep3::Type
            } else {
                CtorStep3::Nothing
            }
        };
        if self.generics.iter().any(|g| g == name) {
            return CtorStep3::Type;
        }
        if let Some(binding) = self.file.scope.use_import(name) {
            return match binding {
                Binding::Def(d) => ty(*d),
                Binding::Error => CtorStep3::Erroneous,
                Binding::Module(_) | Binding::Namespace(_) => CtorStep3::Nothing,
            };
        }
        if let Some(&d) = self.scope2().items.get(name) {
            return ty(d);
        }
        match self.universe.prelude.get(name) {
            Some(&d) => ty(d),
            None => CtorStep3::Nothing,
        }
    }

    /// `not_a_constructor` when `name` names a type (S10 step 3), else
    /// `unresolved_name`; nothing when it names a diagnosed import (S62:
    /// such a name is "not reported again where it is used").
    pub fn no_ctor(&self, name: &ast::Name) -> Option<Diagnostic> {
        match self.ctor_step3(&name.text) {
            CtorStep3::Type => Some(not_a_constructor(name.span, &name.text)),
            CtorStep3::Erroneous => None,
            CtorStep3::Nothing => Some(unresolved_name(name.span, &name.text, None)),
        }
    }

    /// Full constructor resolution (S10) for an unqualified constructor
    /// with the expected type `expected` (`None` when no expected type is
    /// known): step 1 against `strip(expected)`, then [`Resolver::lexical_ctor`].
    /// This is what the body typer calls for a [`Res::DeferredCtor`]; for a
    /// pattern, pass the scrutinee (or payload) type.
    pub fn resolve_ctor(&self, name: &ast::Name, expected: Option<&Ty>, out: &mut ResOut) -> Res {
        if let Some(e) = expected {
            match self.refinements.strip(e) {
                Ty::Adt(def, _) => {
                    if let Some(en) = self.program.def(def).kind.as_enum() {
                        if let Some(index) = en.variants.iter().position(|v| v.name == name.text) {
                            return Res::Ctor(Ctor::Variant {
                                def,
                                index: index as u32,
                            });
                        }
                    }
                }
                Ty::Wrapper(def) if self.program.def(def).name.display() == name.text => {
                    return Res::Ctor(Ctor::Wrapper(def));
                }
                _ => {}
            }
        }
        self.lexical_ctor(name, out)
    }
}

// ---- diagnostics --------------------------------------------------------------

pub(crate) fn unresolved_name(span: Span, name: &str, module: Option<&str>) -> Diagnostic {
    let message = match module {
        Some(m) => format!("module `{m}` has no item `{name}`"),
        None => format!("cannot find `{name}` in this scope"),
    };
    let mut d = diag(codes::UNRESOLVED_NAME, Some(span), message).with_data("name", json!(name));
    if let Some(m) = module {
        d = d.with_data("module", json!(m));
    }
    d
}

pub(crate) fn unresolved_module_at(module: String, span: Span) -> Diagnostic {
    diag(
        codes::UNRESOLVED_MODULE,
        Some(span),
        format!("no module `{module}` exists in the program"),
    )
    .with_data("module", json!(module))
}

fn unresolved_type(path: &ast::TypePath, name: &str) -> Diagnostic {
    let mut d = diag(
        codes::UNRESOLVED_TYPE,
        Some(path.span),
        format!("cannot find type `{name}` in this scope"),
    )
    .with_data("name", json!(name));
    if RESERVED_TYPE_NAMES.contains(&name) {
        d.message = format!("`{name}` is a reserved type name");
        d = d.with_note("reserved for a later edition", None);
    } else if name == "Gen" {
        d.message = "`Gen` is not nameable in edition 0.1".to_string();
        d = d.with_note(
            "generators appear only in `for` ranges and `gen` arguments (semantics S14)",
            None,
        );
    }
    d
}

fn not_a_type(path: &ast::TypePath, name: &str) -> Diagnostic {
    diag(
        codes::UNRESOLVED_TYPE,
        Some(path.span),
        format!("`{name}` is not a type"),
    )
    .with_data("name", json!(name))
}

fn not_a_member(seg: &ast::Name, of: &str, what: &str) -> Diagnostic {
    diag(
        codes::UNRESOLVED_NAME,
        Some(seg.span),
        format!("`{}` is not a member of {what} `{of}`", seg.text),
    )
    .with_data("name", json!(seg.text))
}

pub(crate) fn not_a_constructor(span: Span, name: &str) -> Diagnostic {
    diag(
        codes::NOT_A_CONSTRUCTOR,
        Some(span),
        format!("`{name}` is a type, not a constructor"),
    )
    .with_data("name", json!(name))
}

fn count_mismatch(t: &ast::Type, name: &str, expected: usize, actual: usize) -> Diagnostic {
    diag(
        codes::TYPE_ARGUMENT_COUNT_MISMATCH,
        Some(t.span),
        format!(
            "`{name}` takes {expected} type argument{}, found {actual}",
            if expected == 1 { "" } else { "s" }
        ),
    )
    .with_data("expected", json!(expected))
    .with_data("actual", json!(actual))
}

/// The value-type rule (S13) for a resolved type at a value position:
/// `capability_in_value_position` for a capability at the top level or as
/// a type argument (S27), `not_a_value_type` for `Gen` outside the standard
/// library (unreachable in 0.1, S14). `ast` and `ty` are walked in
/// parallel; a type argument is always a value position.
pub(crate) fn value_type_errors(
    t: &ast::Type,
    ty: &Ty,
    program: &Program,
    in_std: bool,
    out: &mut Vec<Diagnostic>,
) {
    match ty {
        Ty::Capability(def) => out.push(
            diag(
                codes::CAPABILITY_IN_VALUE_POSITION,
                Some(t.span),
                format!(
                    "capability `{}` cannot be used as a value type; a handle is received only through a `using` parameter",
                    program.def(*def).name.display()
                ),
            )
            .with_data("name", json!(program.def(*def).name.display()))
            .with_data("type", json!(program.display_ty(ty))),
        ),
        Ty::Gen(_) if !in_std => out.push(
            diag(
                codes::NOT_A_VALUE_TYPE,
                Some(t.span),
                "a generator is not a value type",
            )
            .with_data("type", json!(program.display_ty(ty))),
        ),
        _ => {
            let args: Vec<&Ty> = match ty {
                Ty::List(a) | Ty::Gen(a) => vec![a.as_ref()],
                Ty::Adt(_, args) => args.iter().collect(),
                _ => Vec::new(),
            };
            if let Some(g) = &t.generic_args {
                for (a, aty) in g.args.iter().zip(args) {
                    value_type_errors(a, aty, program, in_std, out);
                }
            }
        }
    }
}
