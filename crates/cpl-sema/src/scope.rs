//! The scopes of `spec/semantics.md` 2.1: the universe (scope 1), module
//! scopes (scope 2: the module's item namespace plus the file's imports,
//! S5/T20), and the local scopes of a body (scopes 4-10), with the names the
//! prelude reserves (1.7, S6, stdlib S11).
//!
//! Module and universe scopes are built once by collection and import
//! resolution and are read-only afterwards; [`LocalScopes`] is the stack a
//! walker over one declaration maintains (the resolution walk of
//! `crate::names`, and the body typer if it walks scopes itself).

use cpl_diag::Span;
use cpl_hir::{BindingKind, DefId, LocalDef, LocalId, ModuleId, Ty};
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};

/// The prelude names of semantics 1.7 that no module-level declaration or
/// import may redefine (S6, stdlib S3 and S11): the `pub` declarations of
/// `std.prelude` that are not members, the prelude constructors, the
/// intrinsic `check`, the module alias `gen`, the type `Gen`, and the
/// reserved type names.
pub(crate) const PRELUDE_NAMES: &[&str] = &[
    "Int",
    "Bool",
    "Text",
    "Bytes",
    "Unit",
    "List",
    "Option",
    "Result",
    "Some",
    "None",
    "Ok",
    "Err",
    "ConstraintError",
    "check",
    "flatten",
    "gen",
    "Gen",
    "Int8",
    "Int16",
    "Int32",
    "Int64",
    "UInt8",
    "UInt16",
    "UInt32",
    "UInt64",
    "Float32",
    "Float64",
];

/// The reserved type names of stdlib S11: in a type position they are
/// `unresolved_type` with the note `reserved for a later edition`.
pub(crate) const RESERVED_TYPE_NAMES: &[&str] = &[
    "Int8", "Int16", "Int32", "Int64", "UInt8", "UInt16", "UInt32", "UInt64", "Float32", "Float64",
];

/// The prelude type heads that may carry `@method`/`@property` members
/// (stdlib 10.2, S37).
pub(crate) const MEMBER_HEADS: &[&str] =
    &["Int", "Bool", "Text", "Bytes", "List", "Option", "Result"];

/// One module's scope-2 declarations: its item namespace and its test
/// namespace (S3), over all of its files. A declaration that is reported as
/// a duplicate or as a prelude redefinition is not entered, so the earlier
/// declaration (or the prelude meaning) is what every use resolves to.
#[derive(Clone, Debug)]
pub(crate) struct ModuleScope {
    pub id: ModuleId,
    pub path: Vec<String>,
    pub is_std: bool,
    /// Types, structs, enums, capabilities, and functions by name (std
    /// `@method`/`@property` members excluded: they are in the member table).
    pub items: BTreeMap<String, DefId>,
    /// Tests and properties by their `TEXT` name.
    pub tests: BTreeMap<String, DefId>,
}

impl ModuleScope {
    pub fn dotted(&self) -> String {
        self.path.join(".")
    }
}

/// Scope 1 (semantics 2.1, stdlib S1): the prelude, the alias `gen`, and
/// the root segment of every module path; plus the module-path index that
/// qualified chains (2.4) and `use` paths (1.6) consult.
#[derive(Clone, Debug, Default)]
pub(crate) struct Universe {
    /// The `pub` non-member declarations of `std.prelude` by name.
    pub prelude: BTreeMap<String, DefId>,
    pub prelude_module: Option<ModuleId>,
    /// `std.gen`, which `gen` names (S54).
    pub gen_module: Option<ModuleId>,
    /// Every module of the program by path.
    pub module_paths: BTreeMap<Vec<String>, ModuleId>,
    /// Every proper prefix of a module path (`["reader"]` for `reader.pages`).
    pub prefixes: BTreeSet<Vec<String>>,
    /// The first segment of every module path, and `std`.
    pub roots: BTreeSet<String>,
}

impl Universe {
    /// The module with exactly this path.
    pub fn module(&self, path: &[String]) -> Option<ModuleId> {
        self.module_paths.get(path).copied()
    }

    /// True when some module has this path or extends it.
    pub fn is_module_or_prefix(&self, path: &[String]) -> bool {
        self.module_paths.contains_key(path) || self.prefixes.contains(path)
    }

    /// True when `name` is a prelude name of S6.
    pub fn is_prelude_name(name: &str) -> bool {
        PRELUDE_NAMES.contains(&name)
    }

    /// What the prelude itself binds `name` to, for the S6 exemption of an
    /// import that names the prelude's own declaration (`use
    /// std.prelude.{flatten}`, `use std.gen`).
    pub fn prelude_binding(&self, name: &str) -> Option<Binding> {
        if name == "gen" {
            return self.gen_module.map(Binding::Module);
        }
        self.prelude.get(name).map(|d| Binding::Def(*d))
    }
}

/// What a name of scope 2 (an import or a declaration) or scope 1 names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Binding {
    Def(DefId),
    Module(ModuleId),
    /// A submodule alias that is only a prefix of module paths (1.6).
    Namespace(Vec<String>),
    /// A diagnosed `use` item: an erroneous binding that is neither
    /// `unused_import` nor reported again where it is used (S62).
    Error,
}

/// One import of a file scope.
#[derive(Debug)]
pub(crate) struct ImportBinding {
    pub binding: Binding,
    /// The `use` item's span.
    pub span: Span,
    /// Set when a resolution goes through this import (`unused_import`, 1.6).
    pub used: Cell<bool>,
}

/// The imports of one file (S5, T20), by imported name. A diagnosed item
/// binds [`Binding::Error`]; a duplicate import keeps the first binding.
#[derive(Debug, Default)]
pub(crate) struct FileScope {
    pub imports: BTreeMap<String, ImportBinding>,
}

impl FileScope {
    /// The import of `name`, marking it used.
    pub fn use_import(&self, name: &str) -> Option<&Binding> {
        let import = self.imports.get(name)?;
        import.used.set(true);
        Some(&import.binding)
    }

    /// The import of `name` without marking it used.
    pub fn peek(&self, name: &str) -> Option<&Binding> {
        self.imports.get(name).map(|i| &i.binding)
    }
}

/// One binding of a declaration's local table while a walker is inside it.
#[derive(Clone, Debug)]
pub(crate) struct LocalInfo {
    pub name: String,
    pub kind: BindingKind,
    pub mutable: bool,
    pub span: Span,
    /// Known for parameters, `result`, and `self`; `Ty::Error` until the
    /// body typer assigns the binding's type.
    pub ty: Ty,
    /// Read at least once (S67: an assignment is a write, not a read).
    pub read: bool,
}

impl LocalInfo {
    pub fn to_def(&self, id: u32) -> LocalDef {
        LocalDef {
            id: LocalId(id),
            name: self.name.clone(),
            ty: self.ty.clone(),
            mutable: self.mutable,
            kind: self.kind,
            span: self.span,
        }
    }
}

/// The local scopes (8-10) and the parameter, `result`, `self`, and binder
/// scopes (4-7) of one declaration: a stack of frames over the
/// declaration's local table. Lookup is innermost frame first and, within a
/// frame, most recent binding first.
#[derive(Clone, Debug, Default)]
pub(crate) struct LocalScopes {
    pub locals: Vec<LocalInfo>,
    frames: Vec<Vec<u32>>,
}

impl LocalScopes {
    /// Start from an existing local table (parameters first); nothing is
    /// in scope until a frame binds it.
    pub fn new(locals: Vec<LocalInfo>) -> LocalScopes {
        LocalScopes {
            locals,
            frames: Vec::new(),
        }
    }

    pub fn push(&mut self) {
        self.frames.push(Vec::new());
    }

    /// Close the innermost frame, returning the bindings it held.
    pub fn pop(&mut self) -> Vec<u32> {
        self.frames.pop().unwrap_or_default()
    }

    /// Append a binding to the table without putting it in scope.
    pub fn allocate(&mut self, info: LocalInfo) -> LocalId {
        self.locals.push(info);
        LocalId(self.locals.len() as u32 - 1)
    }

    /// Put an allocated binding in the innermost frame.
    pub fn enter(&mut self, id: LocalId) {
        if self.frames.is_empty() {
            self.frames.push(Vec::new());
        }
        self.frames.last_mut().expect("a frame").push(id.0);
    }

    /// Allocate a binding and put it in scope.
    pub fn bind(&mut self, info: LocalInfo) -> LocalId {
        let id = self.allocate(info);
        self.enter(id);
        id
    }

    /// The innermost binding of `name` in scope.
    pub fn lookup(&self, name: &str) -> Option<LocalId> {
        self.frames
            .iter()
            .rev()
            .flat_map(|f| f.iter().rev())
            .find(|&&id| self.locals[id as usize].name == name)
            .map(|&id| LocalId(id))
    }

    /// Every binding in scope, innermost frame first and most recent first
    /// within a frame (the hole report's `bindings` order, toolchain 7.2).
    pub fn visible(&self) -> Vec<LocalId> {
        self.frames
            .iter()
            .rev()
            .flat_map(|f| f.iter().rev())
            .map(|&id| LocalId(id))
            .collect()
    }

    pub fn info(&self, id: LocalId) -> &LocalInfo {
        &self.locals[id.0 as usize]
    }

    pub fn info_mut(&mut self, id: LocalId) -> &mut LocalInfo {
        &mut self.locals[id.0 as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpl_diag::FileId;

    fn info(name: &str) -> LocalInfo {
        LocalInfo {
            name: name.into(),
            kind: BindingKind::Let,
            mutable: false,
            span: Span::point(FileId(0), 0),
            ty: Ty::Error,
            read: false,
        }
    }

    #[test]
    fn lookup_is_innermost_and_most_recent_first() {
        let mut s = LocalScopes::default();
        s.push();
        let a = s.bind(info("a"));
        let b = s.bind(info("b"));
        s.push();
        let a2 = s.bind(info("a"));
        assert_eq!(s.lookup("a"), Some(a2));
        assert_eq!(s.lookup("b"), Some(b));
        assert_eq!(s.visible(), vec![a2, b, a]);
        s.pop();
        assert_eq!(s.lookup("a"), Some(a));
        assert_eq!(s.locals.len(), 3);
    }
}
