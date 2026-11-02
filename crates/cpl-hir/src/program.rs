//! Accessors of [`Program`] (`spec/toolchain.md` 1.6) and small helpers on
//! the HIR types.

use crate::*;
use cpl_diag::json::Relation;
use cpl_diag::SymbolPath;

impl Program {
    /// A program without definitions (what `check` returns when a file
    /// failed to parse, semantics S52).
    pub fn empty(source_map: Arc<SourceMap>) -> Program {
        Program {
            defs: Vec::new(),
            modules: Vec::new(),
            std_defs: StdDefs::UNSET,
            references: Vec::new(),
            imports: Vec::new(),
            refinement_classes: Vec::new(),
            source_map,
            asts: Vec::new(),
        }
    }

    /// The definition with this id. Panics when the id is out of range.
    pub fn def(&self, id: DefId) -> &Def {
        &self.defs[id.0 as usize]
    }

    /// Every definition, in `DefId` order.
    pub fn defs(&self) -> impl Iterator<Item = &Def> {
        self.defs.iter()
    }

    /// The module with this id. Panics when the id is out of range.
    pub fn module(&self, id: ModuleId) -> &ModuleDef {
        &self.modules[id.0 as usize]
    }

    /// Every module, in `ModuleId` order.
    pub fn modules(&self) -> impl Iterator<Item = &ModuleDef> {
        self.modules.iter()
    }

    /// The module with exactly this path.
    pub fn module_by_path(&self, path: &[String]) -> Option<ModuleId> {
        self.modules.iter().find(|m| m.path == path).map(|m| m.id)
    }

    /// The definition a qualified name names: the first definition of
    /// that module (in [`ModuleDef::defs`] order) with that name.
    /// Capability operations and enum variants are members, not
    /// definitions, and are not found.
    pub fn lookup(&self, path: &SymbolPath) -> Option<DefId> {
        let module = self.module(self.module_by_path(&path.module)?);
        module
            .defs
            .iter()
            .copied()
            .find(|&d| self.def(d).name == path.name)
    }

    /// The definition with this identity (an `@id` value or a provisional
    /// identity).
    pub fn by_stable_id(&self, id: &StableId) -> Option<DefId> {
        self.defs.iter().find(|d| &d.stable_id == id).map(|d| d.id)
    }

    /// DefIds of Unit, Option, Result, ConstraintError, ForeignError, std.gen.Positive.
    pub fn std_defs(&self) -> &StdDefs {
        &self.std_defs
    }

    /// Outgoing edges of `id` (Section 6.5), in span order. Relies on
    /// [`Program::references`] being sorted by (from, span).
    pub fn references_from(&self, id: DefId) -> &[Reference] {
        let start = self.references.partition_point(|r| r.from < id);
        let end = self.references.partition_point(|r| r.from <= id);
        &self.references[start..end]
    }

    /// Incoming edges of `id` (callers, Section 6.5), in (from, span) order.
    pub fn references_to(&self, id: DefId) -> Vec<&Reference> {
        self.references.iter().filter(|r| r.to == id).collect()
    }

    /// Establish the (from, span) order of [`Program::references`] that
    /// [`Program::references_from`] relies on (a stable sort).
    pub fn sort_references(&mut self) {
        self.references.sort_by_key(|r| (r.from, r.span));
    }

    /// CPL type syntax without a generic context: a parameter renders as
    /// `$<index>`. Use [`Program::display_ty_in`] to render parameters by
    /// name.
    pub fn display_ty(&self, ty: &Ty) -> String {
        self.display_ty_in(ty, &[])
    }

    /// CPL type syntax, e.g. `"List<List<T>>"`, with `Ty::Param(i)`
    /// rendered as `generics[i]` (the enclosing definition's generic
    /// parameters). Named types render by their declared name; a
    /// refinement by the declaration written at the use site; `Ty::Error`
    /// as `?`, as in every rendering of a type (`spec/semantics.md` S69,
    /// D-247): `List<?>` for a list whose element type is erroneous.
    pub fn display_ty_in(&self, ty: &Ty, generics: &[String]) -> String {
        let mut out = String::new();
        self.write_ty(&mut out, ty, generics);
        out
    }

    fn write_ty(&self, out: &mut String, ty: &Ty, generics: &[String]) {
        match ty {
            Ty::Int => out.push_str("Int"),
            Ty::Bool => out.push_str("Bool"),
            Ty::Text => out.push_str("Text"),
            Ty::Bytes => out.push_str("Bytes"),
            Ty::List(t) => {
                out.push_str("List<");
                self.write_ty(out, t, generics);
                out.push('>');
            }
            Ty::Gen(t) => {
                out.push_str("Gen<");
                self.write_ty(out, t, generics);
                out.push('>');
            }
            Ty::Adt(def, args) => {
                out.push_str(&self.def_name(*def));
                if !args.is_empty() {
                    out.push('<');
                    for (i, a) in args.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        self.write_ty(out, a, generics);
                    }
                    out.push('>');
                }
            }
            Ty::Wrapper(def) | Ty::Refined(_, def) | Ty::Capability(def) => {
                out.push_str(&self.def_name(*def))
            }
            Ty::Param(i) => match generics.get(*i as usize) {
                Some(name) => out.push_str(name),
                None => out.push_str(&format!("${i}")),
            },
            // S69 (D-247): the error type renders as `?` wherever a type is
            // rendered (a hole's `expected_type`, a diagnostic's payload).
            Ty::Error => out.push('?'),
        }
    }

    /// The declared name of a definition (`Page`, `paginate`, `List.take`);
    /// `{def N}` for an id the program does not have.
    fn def_name(&self, id: DefId) -> String {
        match self.defs.get(id.0 as usize) {
            Some(d) => d.name.display(),
            None => format!("{{def {}}}", id.0),
        }
    }

    /// The qualified name (Section 4.4): `reader.pages.paginate`,
    /// `tests.pagination."pagination handles empty input"`, `std.prelude.List.take`.
    pub fn display_path(&self, id: DefId) -> String {
        self.symbol_path(id).display()
    }

    /// The symbol path of a definition.
    pub fn symbol_path(&self, id: DefId) -> SymbolPath {
        let def = self.def(id);
        SymbolPath {
            module: self.module(def.module).path.clone(),
            name: def.name.clone(),
        }
    }

    pub fn source_map(&self) -> &SourceMap {
        &self.source_map
    }

    /// The parsed file with this id.
    pub fn file_ast(&self, file: FileId) -> Option<&ast::Module> {
        self.asts
            .binary_search_by_key(&file, |(id, _)| *id)
            .ok()
            .map(|index| self.asts[index].1.as_ref())
    }

    /// The declaration's AST (needed by cpl-index facets). Panics when the
    /// definition's file is not in [`Program::asts`].
    pub fn ast(&self, id: DefId) -> &ast::Declaration {
        let def = self.def(id);
        let module = self.file_ast(def.file).unwrap_or_else(|| {
            panic!(
                "no AST for FileId({}) of {}",
                def.file.0,
                self.display_path(id)
            )
        });
        &module.declarations[def.decl_index]
    }

    /// The source role, when present. Hand-built HIR and edition 0.1 functions retain effect-bound inference.
    pub fn function_mode(&self, id: DefId) -> FunctionMode {
        let Some(def) = self.defs.get(id.0 as usize) else {
            return FunctionMode::Inferred;
        };
        match self
            .file_ast(def.file)
            .and_then(|m| m.declarations.get(def.decl_index))
            .map(|d| &d.kind)
        {
            Some(ast::DeclKind::Fn(f)) => f.mode,
            _ => FunctionMode::Inferred,
        }
    }

    pub fn is_mutating(&self, id: DefId) -> bool {
        match self.function_mode(id) {
            FunctionMode::Mutate => true,
            FunctionMode::Inferred => self
                .def(id)
                .kind
                .as_fn()
                .is_some_and(|f| f.effects.declared.as_ref().is_some_and(|e| !e.is_empty())),
            _ => false,
        }
    }

    pub fn is_program_state(&self, id: DefId) -> bool {
        let Some(def) = self.defs.get(id.0 as usize) else {
            return false;
        };
        matches!(self.file_ast(def.file).and_then(|m| m.declarations.get(def.decl_index)).map(|d| &d.kind),
            Some(ast::DeclKind::Struct(s)) if s.is_program)
    }

    /// The machine's type-level protocol. Invalid signatures return `None`; the checker supplies diagnostics.
    pub fn machine_signature(&self, id: DefId) -> Option<MachineSignature> {
        if self.function_mode(id) != FunctionMode::Machine {
            return None;
        }
        let f = self.def(id).kind.as_fn()?;
        if !f.generics.is_empty() || !f.using.is_empty() || f.params.len() != 2 {
            return None;
        }
        let state = &f.params[0].ty;
        let event = &f.params[1].ty;
        let Ty::Adt(state_id, _) = state else {
            return None;
        };
        if !matches!(
            self.def(*state_id).kind,
            DefKind::Struct(_) | DefKind::Enum(_)
        ) {
            return None;
        }
        let Ty::Adt(event_id, _) = event else {
            return None;
        };
        self.def(*event_id).kind.as_enum()?;
        let mut result = MachineSignature {
            state: state.clone(),
            event: event.clone(),
            command: None,
            state_field: None,
            commands_field: None,
        };
        if f.ret == *state {
            return Some(result);
        }
        let Ty::Adt(ret, args) = &f.ret else {
            return None;
        };
        let record = self.def(*ret).kind.as_struct()?;
        if record.fields.len() != 2 {
            return None;
        }
        let state_field = record.fields.iter().position(|f| f.name == "state")?;
        let commands_field = record.fields.iter().position(|f| f.name == "commands")?;
        if machine_substitute(&record.fields[state_field].ty, args) != *state {
            return None;
        }
        let Ty::List(command) = machine_substitute(&record.fields[commands_field].ty, args) else {
            return None;
        };
        let Ty::Adt(command_id, _) = command.as_ref() else {
            return None;
        };
        self.def(*command_id).kind.as_enum()?;
        result.command = Some(*command);
        result.state_field = Some(state_field as u32);
        result.commands_field = Some(commands_field as u32);
        Some(result)
    }

    /// The refinement class with this id. Panics when out of range.
    pub fn refinement_class(&self, class: RefinementClass) -> &RefinementClassDef {
        &self.refinement_classes[class.0 as usize]
    }
}

fn machine_substitute(ty: &Ty, args: &[Ty]) -> Ty {
    match ty {
        Ty::Param(i) => args.get(*i as usize).cloned().unwrap_or(Ty::Error),
        Ty::List(inner) => Ty::List(Box::new(machine_substitute(inner, args))),
        Ty::Adt(def, inner) => Ty::Adt(
            *def,
            inner.iter().map(|t| machine_substitute(t, args)).collect(),
        ),
        other => other.clone(),
    }
}

impl DefKind {
    /// A refinement predicate or a struct's constructor invariant.
    pub fn checked_predicate(&self) -> Option<&RefinementDef> {
        match self {
            DefKind::Refinement(r) => Some(r),
            DefKind::Struct(s) => s.invariant.as_ref(),
            _ => None,
        }
    }
    /// The kind prefix of generated ids and the `kind` of symbol blocks and
    /// diff statements: `"fn"`, `"ty"` (wrapper, refinement, struct, enum,
    /// builtin type), `"cap"`, `"test"`, `"prop"` (Section 4.1).
    pub fn id_kind(&self) -> &'static str {
        match self {
            DefKind::Fn(_) => "fn",
            DefKind::Wrapper(_)
            | DefKind::Refinement(_)
            | DefKind::Struct(_)
            | DefKind::Enum(_)
            | DefKind::BuiltinType(_) => "ty",
            DefKind::Capability(_) => "cap",
            DefKind::Test(_) => "test",
            DefKind::Property(_) => "prop",
        }
    }

    /// The declaring keyword: `fn`, `type`, `struct`, `enum`, `capability`,
    /// `test`, `property`.
    pub fn keyword(&self) -> &'static str {
        match self {
            DefKind::Fn(_) => "fn",
            DefKind::Wrapper(_) | DefKind::Refinement(_) | DefKind::BuiltinType(_) => "type",
            DefKind::Struct(_) => "struct",
            DefKind::Enum(_) => "enum",
            DefKind::Capability(_) => "capability",
            DefKind::Test(_) => "test",
            DefKind::Property(_) => "property",
        }
    }

    /// True for the declarations that introduce a type.
    pub fn is_type(&self) -> bool {
        matches!(
            self,
            DefKind::Wrapper(_)
                | DefKind::Refinement(_)
                | DefKind::Struct(_)
                | DefKind::Enum(_)
                | DefKind::Capability(_)
                | DefKind::BuiltinType(_)
        )
    }

    /// True for tests and properties (the test namespace, semantics S3).
    pub fn is_test(&self) -> bool {
        matches!(self, DefKind::Test(_) | DefKind::Property(_))
    }

    /// The generic parameters (`Ty::Param(i)` is `generics()[i]`); empty for
    /// kinds without them.
    pub fn generics(&self) -> &[String] {
        match self {
            DefKind::Fn(d) => &d.generics,
            DefKind::Struct(d) => &d.generics,
            DefKind::Enum(d) => &d.generics,
            DefKind::BuiltinType(d) => &d.generics,
            _ => &[],
        }
    }

    /// The local table (index = `LocalId.0`); empty for kinds without one.
    /// Capability operations have their own ([`OpDef::locals`]).
    pub fn locals(&self) -> &[LocalDef] {
        match self {
            DefKind::Fn(d) => &d.locals,
            DefKind::Refinement(d) => &d.locals,
            DefKind::Struct(d) => d
                .invariant
                .as_ref()
                .map_or(&[], |inv| inv.locals.as_slice()),
            DefKind::Test(d) => &d.locals,
            DefKind::Property(d) => &d.locals,
            _ => &[],
        }
    }

    pub fn as_fn(&self) -> Option<&FnDef> {
        match self {
            DefKind::Fn(d) => Some(d),
            _ => None,
        }
    }

    pub fn as_wrapper(&self) -> Option<&WrapperDef> {
        match self {
            DefKind::Wrapper(d) => Some(d),
            _ => None,
        }
    }

    pub fn as_refinement(&self) -> Option<&RefinementDef> {
        match self {
            DefKind::Refinement(d) => Some(d),
            _ => None,
        }
    }

    pub fn as_struct(&self) -> Option<&StructDef> {
        match self {
            DefKind::Struct(d) => Some(d),
            _ => None,
        }
    }

    pub fn as_enum(&self) -> Option<&EnumDef> {
        match self {
            DefKind::Enum(d) => Some(d),
            _ => None,
        }
    }

    pub fn as_capability(&self) -> Option<&CapabilityDef> {
        match self {
            DefKind::Capability(d) => Some(d),
            _ => None,
        }
    }

    pub fn as_test(&self) -> Option<&TestDef> {
        match self {
            DefKind::Test(d) => Some(d),
            _ => None,
        }
    }

    pub fn as_property(&self) -> Option<&PropertyDef> {
        match self {
            DefKind::Property(d) => Some(d),
            _ => None,
        }
    }

    pub fn as_builtin_type(&self) -> Option<&BuiltinTypeDef> {
        match self {
            DefKind::BuiltinType(d) => Some(d),
            _ => None,
        }
    }
}

impl Def {
    /// A local of this definition's table.
    pub fn local(&self, id: LocalId) -> Option<&LocalDef> {
        self.kind.locals().get(id.0 as usize)
    }
}

impl FnDef {
    /// `param.op` with the `using` parameter's name: `"db.get"`.
    pub fn display_effect(&self, effect: &EffectRef) -> String {
        let param = self
            .locals
            .get(effect.param.0 as usize)
            .map_or("?", |l| l.name.as_str());
        format!("{param}.{}", effect.op)
    }

    /// True when the body is a `@builtin` registry id.
    pub fn is_builtin(&self) -> bool {
        matches!(self.body, FnBody::Builtin(_))
    }
}

impl Ty {
    pub fn is_error(&self) -> bool {
        matches!(self, Ty::Error)
    }

    /// `List<elem>`.
    pub fn list(elem: Ty) -> Ty {
        Ty::List(Box::new(elem))
    }

    /// The prelude's `Unit`.
    pub fn unit(std: &StdDefs) -> Ty {
        Ty::Adt(std.unit, Vec::new())
    }

    /// The prelude's `Option<t>`.
    pub fn option(std: &StdDefs, t: Ty) -> Ty {
        Ty::Adt(std.option, vec![t])
    }

    /// The prelude's `Result<t, e>`.
    pub fn result(std: &StdDefs, t: Ty, e: Ty) -> Ty {
        Ty::Adt(std.result, vec![t, e])
    }
}

impl Expr {
    pub fn new(kind: ExprKind, ty: Ty, span: Span) -> Expr {
        Expr { kind, ty, span }
    }
}

impl RefKind {
    /// `"call"`, `"type_use"`, `"ctor"`, `"field_access"`, `"capability_op"`, `"check"`.
    pub fn as_str(self) -> &'static str {
        match self {
            RefKind::Call => "call",
            RefKind::TypeUse => "type_use",
            RefKind::Ctor => "ctor",
            RefKind::FieldAccess => "field_access",
            RefKind::CapabilityOp => "capability_op",
            RefKind::Check => "check",
        }
    }
}

impl From<RefKind> for Relation {
    fn from(kind: RefKind) -> Relation {
        match kind {
            RefKind::Call => Relation::Call,
            RefKind::TypeUse => Relation::TypeUse,
            RefKind::Ctor => Relation::Ctor,
            RefKind::FieldAccess => Relation::FieldAccess,
            RefKind::CapabilityOp => Relation::CapabilityOp,
            RefKind::Check => Relation::Check,
        }
    }
}
