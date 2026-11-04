//! Test support (doc-hidden): hand-built [`Program`]s, and the reader and
//! runner of the interpreter corpus ([`corpus`], toolchain 12.5).
//!
//! The integration tests of this crate check the programs they run with
//! `cpl-sema`, a dev-dependency only (toolchain 1.2, T50: the library never
//! depends on the checker). A hand-built program is for what no checked
//! program has (an impossible HIR form such as a `Refine` whose proof was
//! wrong, a `match` with no matching arm) and for the unit tests inside this
//! crate, which cannot use the dev-dependency; its forms are the checker's
//! (a member call is a call of the stub, a negative literal a negation, a
//! refined operand of an ordering operator is widened), and its standard
//! library is the checker's down to every `DefId`, span, and `requires`
//! clause (`tests/foundation/builder.rs` compares the two).
//!
//! [`ProgramBuilder::new`] starts from the real standard library: the five
//! `std/*.cpl` stub files are parsed with `cpl-syntax` (their spans and
//! ASTs are real, so [`Program::ast`] works for std definitions) and lowered
//! by hand into HIR definitions with the qualified names of stdlib S37/S43
//! (`std.prelude.List.take`, `std.text.split`, `std.gen.Positive`), their
//! signatures, variants, and fields in source order, and the `requires`
//! clauses of the builtins with canonical text and span. [`StdDefs`] names
//! the real `Unit`, `Option`, `Result`, `ConstraintError`,
//! `std.foreign.ForeignError`, and `std.gen.Positive`, and
//! `std.gen.Positive` is refinement class 0. Builtin `ensures` clauses are
//! not lowered: they are never evaluated (stdlib S39). No references or
//! imports are recorded.
//!
//! User declarations are added per module: [`ProgramBuilder::module`]
//! creates `src/<path>.cpl` (empty text unless
//! [`ProgramBuilder::module_with_text`] supplies one; spans clamp into it),
//! and the `add_*` methods append declarations with provisional
//! identities (their qualified names; [`ProgramBuilder::set_stable_id`]
//! gives one an `@id`). Function, test, and property bodies are built with
//! [`Hir`] ([`ProgramBuilder::hir`]) and the [`FnBuilder`],
//! [`TestBuilder`], and [`PropertyBuilder`] local tables. User definitions
//! have no AST (`Program::ast` panics for them).
//!
//! ```
//! use cpl_eval::testing::{FnBuilder, ProgramBuilder};
//! use cpl_hir::Ty;
//! use cpl_syntax::ast::BinaryOp;
//!
//! let mut b = ProgramBuilder::new();
//! let m = b.module("reader.pages");
//! let page_id = b.add_wrapper(m, "PageId", Ty::Text);
//! let positive = b.add_int_refinement(m, "Positive", BinaryOp::Gt, 0);
//! let h = b.hir(m);
//! // fn double(n: Positive) -> Int { n * 2 }
//! let mut f = FnBuilder::new("double", Ty::Int);
//! let n = f.param("n", b.refined(positive));
//! let n_int = h.widen(h.local(n, b.refined(positive)), positive, Ty::Int);
//! f.body(h.value(h.binary(BinaryOp::Mul, n_int, h.int(2))));
//! let double = b.add_fn(m, f);
//! let program = b.build();
//! assert_eq!(program.display_path(double), "reader.pages.double");
//! assert_eq!(program.display_path(page_id), "reader.pages.PageId");
//! // reader.pages.Positive and std.gen.Positive are one refinement class (S55).
//! assert_eq!(program.refinement_classes[0].members.len(), 2);
//! ```

pub mod corpus;
mod hir;
mod std_lib;
pub mod sweep;

pub use hir::Hir;

use cpl_diag::json::BindingKind;
use cpl_diag::{DeclName, FileId, FileOrigin, SourceMap, Span, StableId, SymbolPath};
use cpl_hir::{
    Binder, Block, CapabilityDef, Clause, Def, DefId, DefKind, DefStatus, EffectBound, EnumDef,
    Expr, FieldDef, FnBody, FnDef, GenExpr, LocalDef, LocalId, ModuleDef, ModuleId, OpDef,
    ParamDef, Program, PropertyDef, RefinementClass, RefinementClassDef, RefinementDef, StdDefs,
    StructDef, TestDef, Ty, VariantDef, VerificationPolicy, WrapperDef,
};
use cpl_syntax::ast;
use cpl_syntax::ast::BinaryOp;
use std::sync::Arc;

/// One capability operation for [`ProgramBuilder::add_capability`]: its
/// name, its parameters `(name, type)`, and its return type.
pub type OpSpec<'a> = (&'a str, Vec<(&'a str, Ty)>, Ty);

/// The standard library alone ([`ProgramBuilder::new`] built at once).
pub fn std_program() -> Program {
    ProgramBuilder::new().build()
}

/// Builds a [`Program`] by hand: std first, then user modules.
pub struct ProgramBuilder {
    source_map: SourceMap,
    defs: Vec<Def>,
    modules: Vec<ModuleDef>,
    classes: Vec<RefinementClassDef>,
    asts: Vec<(FileId, Arc<ast::Module>)>,
    std_defs: StdDefs,
}

impl Default for ProgramBuilder {
    fn default() -> Self {
        ProgramBuilder::new()
    }
}

impl ProgramBuilder {
    /// A builder holding the five std modules.
    pub fn new() -> ProgramBuilder {
        let mut b = ProgramBuilder {
            source_map: SourceMap::new(),
            defs: Vec::new(),
            modules: Vec::new(),
            classes: Vec::new(),
            asts: Vec::new(),
            std_defs: StdDefs::UNSET,
        };
        std_lib::load(&mut b);
        b
    }

    pub fn std_defs(&self) -> StdDefs {
        self.std_defs
    }

    pub fn def(&self, id: DefId) -> &Def {
        &self.defs[id.0 as usize]
    }

    /// The `DefId` the next `add_*` call returns, for a function whose body
    /// calls itself.
    pub fn next_def(&self) -> DefId {
        DefId(self.defs.len() as u32)
    }

    /// The definition with this qualified name (`std.prelude.List.take`,
    /// `reader.pages.Page`, `tests.pagination."name"`).
    pub fn lookup(&self, qualified: &str) -> Option<DefId> {
        let path = SymbolPath::parse(qualified).ok()?;
        let module = self.modules.iter().find(|m| m.path == path.module)?;
        module
            .defs
            .iter()
            .copied()
            .find(|&d| self.defs[d.0 as usize].name == path.name)
    }

    /// [`ProgramBuilder::lookup`] that panics when the name is unknown.
    pub fn std_def(&self, qualified: &str) -> DefId {
        self.lookup(qualified)
            .unwrap_or_else(|| panic!("no definition {qualified}"))
    }

    /// The std declaration carrying `@builtin(id)`.
    pub fn builtin(&self, id: &str) -> DefId {
        self.defs
            .iter()
            .find(|d| match &d.kind {
                DefKind::Fn(f) => matches!(&f.body, FnBody::Builtin(b) if b == id),
                DefKind::BuiltinType(t) => t.builtin == id,
                _ => false,
            })
            .map(|d| d.id)
            .unwrap_or_else(|| panic!("no std declaration carries @builtin(\"{id}\")"))
    }

    /// The user module `path` (`"reader.pages"`), created on first use with
    /// the empty file `src/reader/pages.cpl`.
    pub fn module(&mut self, path: &str) -> ModuleId {
        self.module_with_text(path, "")
    }

    /// The user module `path`; on creation its file holds `text` (spans
    /// set with [`Hir::at`] then resolve to real lines and columns).
    pub fn module_with_text(&mut self, path: &str, text: &str) -> ModuleId {
        let segments: Vec<String> = path.split('.').map(str::to_string).collect();
        if let Some(m) = self.modules.iter().find(|m| m.path == segments) {
            return m.id;
        }
        let file = self.source_map.add(
            format!("src/{}.cpl", segments.join("/")),
            FileOrigin::Project,
            text.to_string(),
        );
        let id = ModuleId(self.modules.len() as u32);
        self.modules.push(ModuleDef {
            id,
            path: segments,
            origin: FileOrigin::Project,
            files: vec![file],
            defs: Vec::new(),
            docs: None,
        });
        id
    }

    /// The node factory for module `module`'s file.
    pub fn hir(&self, module: ModuleId) -> Hir {
        Hir::new(self.file_of(module), self.std_defs)
    }

    fn file_of(&self, module: ModuleId) -> FileId {
        self.modules[module.0 as usize].files[0]
    }

    /// Append a definition at an explicit position (std loading).
    #[allow(clippy::too_many_arguments)]
    fn push_def_at(
        &mut self,
        module: ModuleId,
        file: FileId,
        decl_index: usize,
        span: Span,
        name: DeclName,
        is_pub: bool,
        docs: Option<String>,
        kind: DefKind,
    ) -> DefId {
        let id = DefId(self.defs.len() as u32);
        let m = &mut self.modules[module.0 as usize];
        let stable_id = StableId::provisional(&m.path.join("."), &name);
        m.defs.push(id);
        self.defs.push(Def {
            id,
            stable_id,
            provisional: true,
            module,
            name,
            is_pub,
            file,
            decl_index,
            span,
            docs,
            policy: VerificationPolicy::default(),
            status: DefStatus::Ok,
            kind,
        });
        id
    }

    /// Append a public user definition to `module`.
    fn push_def(&mut self, module: ModuleId, name: DeclName, kind: DefKind) -> DefId {
        let file = self.file_of(module);
        let index = self.modules[module.0 as usize].defs.len();
        self.push_def_at(
            module,
            file,
            index,
            Span::point(file, 0),
            name,
            true,
            None,
            kind,
        )
    }

    // ---- user declarations ----------------------------------------------------------

    /// `pub type <name>(<inner>)`.
    pub fn add_wrapper(&mut self, module: ModuleId, name: &str, inner: Ty) -> DefId {
        self.push_def(
            module,
            DeclName::Ident(name.to_string()),
            DefKind::Wrapper(WrapperDef { inner }),
        )
    }

    /// `pub struct <name><generics> { fields }`; a field's type refers to
    /// the struct's generics as `Ty::Param(i)`.
    pub fn add_struct(
        &mut self,
        module: ModuleId,
        name: &str,
        generics: &[&str],
        fields: &[(&str, Ty)],
    ) -> DefId {
        let span = Span::point(self.file_of(module), 0);
        let fields = fields
            .iter()
            .map(|(n, ty)| FieldDef {
                name: n.to_string(),
                ty: ty.clone(),
                docs: None,
                span,
            })
            .collect();
        self.push_def(
            module,
            DeclName::Ident(name.to_string()),
            DefKind::Struct(StructDef {
                invariant: None,
                generics: generics.iter().map(|g| g.to_string()).collect(),
                fields,
            }),
        )
    }

    /// `pub enum <name><generics> { variants }`.
    pub fn add_enum(
        &mut self,
        module: ModuleId,
        name: &str,
        generics: &[&str],
        variants: &[(&str, Vec<Ty>)],
    ) -> DefId {
        let span = Span::point(self.file_of(module), 0);
        let variants = variants
            .iter()
            .map(|(n, payload)| VariantDef {
                name: n.to_string(),
                payload: payload.clone(),
                docs: None,
                span,
            })
            .collect();
        self.push_def(
            module,
            DeclName::Ident(name.to_string()),
            DefKind::Enum(EnumDef {
                generics: generics.iter().map(|g| g.to_string()).collect(),
                variants,
            }),
        )
    }

    /// `pub type <name> = <base> where <predicate>`: `predicate` refers to
    /// `self` as `LocalId(0)` of type `base`, and `text` is its canonical
    /// text. Refinements with the same base and a structurally equal
    /// predicate share one class (semantics S55).
    pub fn add_refinement(
        &mut self,
        module: ModuleId,
        name: &str,
        base: Ty,
        predicate: Expr,
        text: &str,
    ) -> DefId {
        let span = Span::point(self.file_of(module), 0);
        let self_local = LocalDef {
            id: LocalId(0),
            name: "self".to_string(),
            ty: base.clone(),
            mutable: false,
            kind: BindingKind::SelfValue,
            span,
        };
        let def = DefId(self.defs.len() as u32);
        self.class_for(&base, &predicate, def);
        self.push_def(
            module,
            DeclName::Ident(name.to_string()),
            DefKind::Refinement(RefinementDef {
                base,
                self_local: LocalId(0),
                predicate: Clause {
                    expr: predicate,
                    text: text.to_string(),
                    span,
                },
                locals: vec![self_local],
            }),
        )
    }

    /// `pub type <name> = Int where self <op> <bound>` (text
    /// `self <op> <bound>`): with `Gt` and `0` it is one class with
    /// `std.gen.Positive`.
    pub fn add_int_refinement(
        &mut self,
        module: ModuleId,
        name: &str,
        op: BinaryOp,
        bound: i64,
    ) -> DefId {
        let h = self.hir(module);
        let predicate = h.binary(op, h.local(LocalId(0), Ty::Int), h.int(bound));
        let text = format!("self {} {bound}", op.symbol());
        self.add_refinement(module, name, Ty::Int, predicate, &text)
    }

    /// `pub capability <name> { ops }` with operations `(name, params,
    /// ret)` and no contracts.
    pub fn add_capability(&mut self, module: ModuleId, name: &str, ops: &[OpSpec<'_>]) -> DefId {
        let span = Span::point(self.file_of(module), 0);
        let ops = ops
            .iter()
            .map(|(op, params, ret)| {
                let locals: Vec<LocalDef> = params
                    .iter()
                    .enumerate()
                    .map(|(i, (n, ty))| LocalDef {
                        id: LocalId(i as u32),
                        name: n.to_string(),
                        ty: ty.clone(),
                        mutable: false,
                        kind: BindingKind::Parameter,
                        span,
                    })
                    .collect();
                OpDef {
                    name: op.to_string(),
                    params: locals
                        .iter()
                        .map(|l| ParamDef {
                            local: l.id,
                            name: l.name.clone(),
                            ty: l.ty.clone(),
                            span,
                        })
                        .collect(),
                    ret: ret.clone(),
                    requires: Vec::new(),
                    ensures: Vec::new(),
                    result_local: None,
                    locals,
                    docs: None,
                    span,
                }
            })
            .collect();
        self.push_def(
            module,
            DeclName::Ident(name.to_string()),
            DefKind::Capability(CapabilityDef { ops }),
        )
    }

    /// A function (public when [`FnBuilder::public`] was called).
    pub fn add_fn(&mut self, module: ModuleId, f: FnBuilder) -> DefId {
        let span = Span::point(self.file_of(module), 0);
        let FnBuilder {
            name,
            is_pub,
            prove,
            generics,
            params,
            using,
            ret,
            requires,
            ensures,
            result_local,
            decreases,
            locals,
            body,
        } = f;
        let body = body.unwrap_or_else(|| panic!("FnBuilder {name}: no body or builtin id"));
        let respan = |p: ParamDef| ParamDef { span, ..p };
        let id = self.push_def(
            module,
            DeclName::Ident(name),
            DefKind::Fn(FnDef {
                generics,
                params: params.into_iter().map(respan).collect(),
                using: using.into_iter().map(respan).collect(),
                ret,
                effects: EffectBound::default(),
                requires,
                ensures,
                result_local,
                decreases,
                locals: locals.into_local_defs(span),
                body,
            }),
        );
        let d = &mut self.defs[id.0 as usize];
        d.is_pub = is_pub;
        d.policy = VerificationPolicy { prove };
        id
    }

    /// A `test`.
    pub fn add_test(&mut self, module: ModuleId, t: TestBuilder) -> DefId {
        let span = Span::point(self.file_of(module), 0);
        let body = t
            .body
            .unwrap_or_else(|| panic!("TestBuilder {}: no body", t.name));
        let id = self.push_def(
            module,
            DeclName::Text(t.name),
            DefKind::Test(TestDef {
                locals: t.locals.into_local_defs(span),
                body,
            }),
        );
        self.defs[id.0 as usize].is_pub = false;
        id
    }

    /// A `property`.
    pub fn add_property(&mut self, module: ModuleId, p: PropertyBuilder) -> DefId {
        let span = Span::point(self.file_of(module), 0);
        let body = p
            .body
            .unwrap_or_else(|| panic!("PropertyBuilder {}: no body", p.name));
        let binders = p
            .binders
            .into_iter()
            .map(|b| Binder { span, ..b })
            .collect();
        let id = self.push_def(
            module,
            DeclName::Text(p.name),
            DefKind::Property(PropertyDef {
                binders,
                locals: p.locals.into_local_defs(span),
                body,
            }),
        );
        self.defs[id.0 as usize].is_pub = false;
        id
    }

    /// Give `def` the `@id` value `id` (no longer provisional).
    pub fn set_stable_id(&mut self, def: DefId, id: &str) {
        let d = &mut self.defs[def.0 as usize];
        d.stable_id =
            StableId::from_annotation(id).unwrap_or_else(|e| panic!("set_stable_id({id}): {e}"));
        d.provisional = false;
    }

    /// Mark `def` as failed checking (`DefStatus::HasErrors`).
    pub fn set_has_errors(&mut self, def: DefId) {
        self.defs[def.0 as usize].status = DefStatus::HasErrors;
    }

    // ---- types ------------------------------------------------------------------------

    /// The refinement class of the refinement declaration `def`.
    pub fn class_of(&self, def: DefId) -> Option<RefinementClass> {
        self.classes
            .iter()
            .position(|c| c.members.contains(&def))
            .map(|i| RefinementClass(i as u32))
    }

    /// `Ty::Refined(class, def)` for a refinement declaration.
    pub fn refined(&self, def: DefId) -> Ty {
        let class = self
            .class_of(def)
            .unwrap_or_else(|| panic!("DefId({}) is not a refinement", def.0));
        Ty::Refined(class, def)
    }

    /// `Ty::Adt(def, [])`: a non-generic struct or enum.
    pub fn adt(&self, def: DefId) -> Ty {
        Ty::Adt(def, Vec::new())
    }

    /// The class of a refinement with this base and predicate (S55),
    /// created when new; `member` joins it.
    fn class_for(&mut self, base: &Ty, predicate: &Expr, member: DefId) -> RefinementClass {
        for (i, c) in self.classes.iter_mut().enumerate() {
            if c.base == *base && c.predicate.structurally_eq(predicate) {
                c.members.push(member);
                return RefinementClass(i as u32);
            }
        }
        self.classes.push(RefinementClassDef {
            base: base.clone(),
            predicate: predicate.clone(),
            members: vec![member],
        });
        RefinementClass(self.classes.len() as u32 - 1)
    }

    /// The finished program.
    pub fn build(self) -> Program {
        let mut p = Program {
            defs: self.defs,
            modules: self.modules,
            std_defs: self.std_defs,
            references: Vec::new(),
            imports: Vec::new(),
            refinement_classes: self.classes,
            source_map: Arc::new(self.source_map),
            asts: self.asts,
            stable_ids: Default::default(),
        };
        p.sort_references();
        p
    }
}

// ===========================================================================
// Local tables
// ===========================================================================

/// A local table under construction (index = `LocalId.0`); spans are set
/// when the definition is added.
#[derive(Clone, Debug, Default)]
pub struct Locals {
    entries: Vec<(String, Ty, BindingKind, bool)>,
}

impl Locals {
    /// Add a local and return its id.
    pub fn add(&mut self, name: &str, ty: Ty, kind: BindingKind, mutable: bool) -> LocalId {
        self.entries.push((name.to_string(), ty, kind, mutable));
        LocalId(self.entries.len() as u32 - 1)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn into_local_defs(self, span: Span) -> Vec<LocalDef> {
        self.entries
            .into_iter()
            .enumerate()
            .map(|(i, (name, ty, kind, mutable))| LocalDef {
                id: LocalId(i as u32),
                name,
                ty,
                mutable,
                kind,
                span,
            })
            .collect()
    }
}

/// A function under construction: parameters first (positional, then
/// `using`), then any other locals.
pub struct FnBuilder {
    name: String,
    is_pub: bool,
    prove: bool,
    generics: Vec<String>,
    params: Vec<ParamDef>,
    using: Vec<ParamDef>,
    ret: Ty,
    requires: Vec<Clause>,
    ensures: Vec<Clause>,
    result_local: Option<LocalId>,
    decreases: Option<Clause>,
    locals: Locals,
    body: Option<FnBody>,
}

impl FnBuilder {
    /// A private function `name` returning `ret`.
    pub fn new(name: &str, ret: Ty) -> FnBuilder {
        FnBuilder {
            name: name.to_string(),
            is_pub: false,
            prove: false,
            generics: Vec::new(),
            params: Vec::new(),
            using: Vec::new(),
            ret,
            requires: Vec::new(),
            ensures: Vec::new(),
            result_local: None,
            decreases: None,
            locals: Locals::default(),
            body: None,
        }
    }

    pub fn public(mut self) -> FnBuilder {
        self.is_pub = true;
        self
    }

    /// `@prove`.
    pub fn prove(mut self) -> FnBuilder {
        self.prove = true;
        self
    }

    /// Set the return type (once the generics it mentions are declared).
    pub fn returns(&mut self, ret: Ty) {
        self.ret = ret;
    }

    /// Declare generic parameter `name`; returns `Ty::Param(i)`.
    pub fn generic(&mut self, name: &str) -> Ty {
        self.generics.push(name.to_string());
        Ty::Param(self.generics.len() as u32 - 1)
    }

    /// A positional parameter (before any `using` parameter or local).
    pub fn param(&mut self, name: &str, ty: Ty) -> LocalId {
        assert!(
            self.locals.len() == self.params.len(),
            "FnBuilder {}: positional parameters come first",
            self.name
        );
        let local = self
            .locals
            .add(name, ty.clone(), BindingKind::Parameter, false);
        self.params.push(ParamDef {
            local,
            name: name.to_string(),
            ty,
            span: Span::point(FileId(0), 0),
        });
        local
    }

    /// A `using` parameter of capability `capability` (after the
    /// positional parameters, before any local).
    pub fn using(&mut self, name: &str, capability: DefId) -> LocalId {
        assert!(
            self.locals.len() == self.params.len() + self.using.len(),
            "FnBuilder {}: using parameters follow the positional ones",
            self.name
        );
        let ty = Ty::Capability(capability);
        let local = self.locals.add(name, ty.clone(), BindingKind::Using, false);
        self.using.push(ParamDef {
            local,
            name: name.to_string(),
            ty,
            span: Span::point(FileId(0), 0),
        });
        local
    }

    /// Any other local (`let`, `var`, pattern, `forall` binder, ...).
    pub fn local(&mut self, name: &str, ty: Ty, kind: BindingKind, mutable: bool) -> LocalId {
        self.locals.add(name, ty, kind, mutable)
    }

    /// The `result` local of the `ensures` clauses (created once).
    pub fn result(&mut self) -> LocalId {
        if let Some(r) = self.result_local {
            return r;
        }
        let r = self
            .locals
            .add("result", self.ret.clone(), BindingKind::Result, false);
        self.result_local = Some(r);
        r
    }

    pub fn requires(&mut self, clause: Clause) {
        self.requires.push(clause);
    }

    pub fn ensures(&mut self, clause: Clause) {
        self.ensures.push(clause);
    }

    /// The header `decreases` clause.
    pub fn decreases(&mut self, clause: Clause) {
        self.decreases = Some(clause);
    }

    pub fn body(&mut self, block: Block) {
        self.body = Some(FnBody::Block(block));
    }

    /// Make the function a `@builtin(id)` stub.
    pub fn builtin(&mut self, id: &str) {
        self.body = Some(FnBody::Builtin(id.to_string()));
    }
}

/// A `test` under construction.
pub struct TestBuilder {
    name: String,
    locals: Locals,
    body: Option<Block>,
}

impl TestBuilder {
    pub fn new(name: &str) -> TestBuilder {
        TestBuilder {
            name: name.to_string(),
            locals: Locals::default(),
            body: None,
        }
    }

    pub fn local(&mut self, name: &str, ty: Ty, kind: BindingKind, mutable: bool) -> LocalId {
        self.locals.add(name, ty, kind, mutable)
    }

    pub fn body(&mut self, block: Block) {
        self.body = Some(block);
    }
}

/// A `property` under construction: binders first, then other locals.
pub struct PropertyBuilder {
    name: String,
    binders: Vec<Binder>,
    locals: Locals,
    body: Option<Block>,
}

impl PropertyBuilder {
    pub fn new(name: &str) -> PropertyBuilder {
        PropertyBuilder {
            name: name.to_string(),
            binders: Vec::new(),
            locals: Locals::default(),
            body: None,
        }
    }

    /// `for <name> in <gen>`, binding `elem_ty` (before any other local).
    pub fn binder(&mut self, name: &str, gen: GenExpr, elem_ty: Ty) -> LocalId {
        assert!(
            self.locals.len() == self.binders.len(),
            "PropertyBuilder {}: binders come first",
            self.name
        );
        let local = self
            .locals
            .add(name, elem_ty.clone(), BindingKind::For, false);
        self.binders.push(Binder {
            local,
            gen,
            elem_ty,
            span: Span::point(FileId(0), 0),
        });
        local
    }

    pub fn local(&mut self, name: &str, ty: Ty, kind: BindingKind, mutable: bool) -> LocalId {
        self.locals.add(name, ty, kind, mutable)
    }

    pub fn body(&mut self, block: Block) {
        self.body = Some(block);
    }
}
