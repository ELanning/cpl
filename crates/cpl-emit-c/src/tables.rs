//! The program tables (`spec/cabi.md` 3.2, Appendix B.1): every fact a fault or a rendering reports, computed at emit
//! time by the Rust functions the evaluator uses for it (`cpl_eval::types`, `cpl_eval::builtins`,
//! `cpl_eval::interp::scope_at`, `Program::display_path`, `Program::display_ty_in`, the source map), and written into
//! `<prefix>_tables.c` as the `cplrt_program` of the package.

use crate::print::{c_index, c_opt_string, c_string, comment_text};
use cpl_diag::{DeclName, Span};
use cpl_eval::types::{bare_name, instantiate, operation_identity, strip, type_name};
use cpl_hir::{BindingKind, DefId, DefKind, FnBody, LocalDef, LocalId, Program, Ty};
use cpl_sema::interop::type_text;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

pub(crate) const CK_REQUIRES: u32 = 1;
pub(crate) const CK_ENSURES: u32 = 2;
pub(crate) const CK_INVARIANT: u32 = 3;
pub(crate) const CK_DECREASES: u32 = 4;
pub(crate) const CK_WHERE: u32 = 5;

/// One clause row (`cplrt_clause`).
#[derive(Clone, Debug)]
pub(crate) struct ClauseRow {
    pub text: String,
    pub kind: u32,
    pub site: u32,
    pub owner: u32,
    pub op: Option<u32>,
    pub owner_id: String,
    pub owner_qualified: String,
    pub bsite: Option<u32>,
}

/// One visible local of a binding site (`cplrt_binding`).
#[derive(Clone, Debug)]
pub(crate) struct BindingRow {
    #[allow(dead_code)]
    pub local: LocalId,
    pub name: String,
    pub ty: String,
    pub kind: &'static str,
    pub mutable: bool,
    pub capability: Option<u32>,
}

/// One binding site (`cplrt_bsite`).
#[derive(Clone, Debug)]
pub(crate) struct BsiteRow {
    pub text: Option<String>,
    pub site: Option<u32>,
    /// innermost first: the order of a `visible` argument
    pub bindings: Vec<BindingRow>,
    /// a loop's `actual.bindings` object: indices into `bindings`, the innermost per name, in name order
    pub object: Vec<u32>,
}

/// One hole (`cplrt_hole`), by `HoleId`.
#[derive(Clone, Debug)]
pub(crate) struct HoleRow {
    pub name: String,
    pub expected_type: String,
    pub site: u32,
    pub bsite: u32,
}

/// One refinement (`cplrt_refinement`), by CPL_R.
#[derive(Clone, Debug)]
pub(crate) struct RefRow {
    pub def: DefId,
    pub type_name: String,
    pub declared_name: String,
    pub predicate: String,
    pub clause: u32,
    pub base: Ty,
    pub chain: Vec<u32>,
}

/// The forms of the type table (`CPLRT_TY_*`).
pub(crate) const TY_INT: u32 = 1;
pub(crate) const TY_BOOL: u32 = 2;
pub(crate) const TY_TEXT: u32 = 3;
pub(crate) const TY_BYTES: u32 = 4;
pub(crate) const TY_LIST: u32 = 5;
pub(crate) const TY_STRUCT: u32 = 6;
pub(crate) const TY_ENUM: u32 = 7;
pub(crate) const TY_WRAPPER: u32 = 8;
pub(crate) const TY_REFINED: u32 = 9;
pub(crate) const TY_CAPABILITY: u32 = 10;
pub(crate) const TY_PARAM: u32 = 11;

/// A program-unit constant (`CPL_K<n>`): a `Text` literal or an `Int` outside the small range.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ConstKey {
    Text(String),
    Int(num_bigint::BigInt),
}

/// Everything the tables unit and the program unit share: the rows, and the index of each fact.
pub(crate) struct Tables<'p> {
    pub p: &'p Program,
    pub with_tests: bool,
    pub optimize: bool,
    pub sites: Vec<Span>,
    site_ix: HashMap<(u32, u32, u32), u32>,
    pub clauses: Vec<ClauseRow>,
    pub bsites: Vec<BsiteRow>,
    pub holes: BTreeMap<u32, HoleRow>,
    pub refs: Vec<RefRow>,
    pub ref_ix: HashMap<DefId, u32>,
    pub consts: Vec<ConstKey>,
    const_ix: HashMap<ConstKey, u32>,
    /// the requires clauses of every function, builtin stubs included (CPL_CL indices)
    pub fn_requires: HashMap<DefId, Vec<u32>>,
    /// the types to place in the type table (finalized by `type_ids`)
    type_set: BTreeMap<String, Ty>,
    /// the payload-less variants of every enum: their statics
    pub ops_ret: Vec<(DefId, u32, Ty, bool)>,
    /// with EmitConfig::coverage, the HIR variant of every coverage counter `k` (cabi.md 1.4)
    pub cov: Option<Vec<&'static str>>,
}

/// The finalized type table: the rows in id order and the id of each type text.
pub(crate) struct TypeTable {
    pub rows: Vec<TypeRow>,
    pub ids: HashMap<String, u32>,
}

#[derive(Clone, Debug)]
pub(crate) struct TypeRow {
    pub text: String,
    pub display: String,
    pub form: u32,
    pub def: u32,
    pub elem: u32,
    pub members: Vec<u32>,
    pub payloads: Vec<u32>,
}

impl<'p> Tables<'p> {
    pub fn new(p: &'p Program, with_tests: bool) -> Self {
        let mut t = Tables {
            p,
            with_tests,
            optimize: false,
            sites: Vec::new(),
            site_ix: HashMap::new(),
            clauses: Vec::new(),
            bsites: Vec::new(),
            holes: BTreeMap::new(),
            refs: Vec::new(),
            ref_ix: HashMap::new(),
            consts: Vec::new(),
            const_ix: HashMap::new(),
            fn_requires: HashMap::new(),
            type_set: BTreeMap::new(),
            ops_ret: Vec::new(),
            cov: None,
        };
        // CPL_R: every refinement declaration, in DefId order, with its predicate clause.
        for d in &p.defs {
            if let Some(rd) = d.kind.checked_predicate() {
                let ix = t.refs.len() as u32;
                t.ref_ix.insert(d.id, ix);
                t.refs.push(RefRow {
                    def: d.id,
                    type_name: cpl_eval::builtins::constraint_type_name(p, d.id),
                    declared_name: d.name.display(),
                    predicate: rd.predicate.text.clone(),
                    clause: 0,
                    base: strip(p, &rd.base),
                    chain: Vec::new(),
                });
            }
        }
        for i in 0..t.refs.len() {
            let def = t.refs[i].def;
            let rd = p
                .def(def)
                .kind
                .checked_predicate()
                .expect("a checked predicate");
            let clause = t.clause(
                &rd.predicate.text,
                CK_WHERE,
                rd.predicate.span,
                def,
                None,
                None,
            );
            let chain: Vec<u32> = cpl_eval::builtins::check_order(p, def)
                .into_iter()
                .filter_map(|r| t.ref_ix.get(&r).copied())
                .collect();
            let base = t.refs[i].base.clone();
            t.refs[i].clause = clause;
            t.refs[i].chain = chain;
            t.add_type(&base);
        }
        // the requires clauses of every function (a builtin stub's are the facts of requires_spec), and the sites of
        // its parameters
        for d in &p.defs {
            if let DefKind::Fn(fd) = &d.kind {
                for param in &fd.params {
                    t.site(param.span);
                }
                let mut ix = Vec::new();
                for c in &fd.requires {
                    ix.push(t.clause(&c.text, CK_REQUIRES, c.span, d.id, None, None));
                }
                t.fn_requires.insert(d.id, ix);
            }
        }
        // the parameter and return types of every capability operation (6.4), and the well-known types
        for d in &p.defs {
            if let DefKind::Capability(cd) = &d.kind {
                for (i, op) in cd.ops.iter().enumerate() {
                    for param in &op.params {
                        t.add_type(&param.ty);
                    }
                    t.add_type(&op.ret);
                    let foreign = cpl_eval::boundary::is_foreign_error_return(p, &op.ret);
                    t.ops_ret.push((d.id, i as u32, op.ret.clone(), foreign));
                }
            }
        }
        if with_tests {
            for d in &p.defs {
                if let DefKind::Fn(fd) = &d.kind {
                    if fd.generics.is_empty() {
                        for param in &fd.params {
                            t.add_type(&param.ty);
                        }
                    }
                }
            }
        }
        t
    }

    /// The site of `span`, which must be registered.
    pub fn site_of(&self, span: Span) -> u32 {
        self.site_ix
            .get(&(span.file.0, span.start, span.end))
            .copied()
            .unwrap_or(u32::MAX)
    }

    /// The site of `span` (`CPL_S`), registered on first use.
    pub fn site(&mut self, span: Span) -> u32 {
        let key = (span.file.0, span.start, span.end);
        if let Some(&ix) = self.site_ix.get(&key) {
            return ix;
        }
        let ix = self.sites.len() as u32;
        self.sites.push(span);
        self.site_ix.insert(key, ix);
        ix
    }

    /// A new clause row (`CPL_CL`). `op` is an operation clause's operation index; the owner identity is the
    /// operation's identity then.
    pub fn clause(
        &mut self,
        text: &str,
        kind: u32,
        span: Span,
        owner: DefId,
        op: Option<u32>,
        bsite: Option<u32>,
    ) -> u32 {
        let p = self.p;
        let site = self.site(span);
        let (owner_id, owner_qualified) = match op {
            Some(i) => {
                let name = p
                    .def(owner)
                    .kind
                    .as_capability()
                    .and_then(|c| c.ops.get(i as usize))
                    .map(|o| o.name.clone())
                    .unwrap_or_default();
                let id = operation_identity(p, owner, &name).as_str().to_string();
                (id.clone(), id)
            }
            None => {
                let d = p.def(owner);
                let id = d.stable_id.as_str().to_string();
                let q = match p.by_stable_id(&d.stable_id) {
                    Some(def) => p.display_path(def),
                    None => id.clone(),
                };
                (id, q)
            }
        };
        let ix = self.clauses.len() as u32;
        self.clauses.push(ClauseRow {
            text: text.to_string(),
            kind,
            site,
            owner: owner.0,
            op,
            owner_id,
            owner_qualified,
            bsite,
        });
        ix
    }

    /// A new binding site (`CPL_A`) of `def` with the locals in scope `scope` (outermost first, as `scope_at` lists
    /// them).
    pub fn bsite(
        &mut self,
        def: DefId,
        scope: &[LocalId],
        text: Option<&str>,
        site: Option<Span>,
    ) -> u32 {
        let p = self.p;
        let d = p.def(def);
        let locals = d.kind.locals();
        let generics = d.kind.generics();
        let using: HashMap<LocalId, DefId> = match &d.kind {
            DefKind::Fn(fd) => fd
                .using
                .iter()
                .filter_map(|u| match &u.ty {
                    Ty::Capability(c) => Some((u.local, *c)),
                    _ => None,
                })
                .collect(),
            _ => HashMap::new(),
        };
        let mut bindings = Vec::new();
        for id in scope.iter().rev() {
            let Some(ld) = locals.get(id.0 as usize) else {
                continue;
            };
            bindings.push(binding_row(p, ld, generics, using.get(id).map(|c| c.0)));
        }
        // the actual.bindings object: one key per name, the innermost binding of each, in name order
        let mut by_name: BTreeMap<String, u32> = BTreeMap::new();
        for (i, b) in bindings.iter().enumerate().rev() {
            by_name.insert(b.name.clone(), i as u32);
        }
        let object = by_name.values().copied().collect();
        let site = site.map(|s| self.site(s));
        let ix = self.bsites.len() as u32;
        self.bsites.push(BsiteRow {
            text: text.map(str::to_string),
            site,
            bindings,
            object,
        });
        ix
    }

    /// A program-unit constant (`CPL_K`).
    pub fn constant(&mut self, key: ConstKey) -> u32 {
        if let Some(&ix) = self.const_ix.get(&key) {
            return ix;
        }
        let ix = self.consts.len() as u32;
        self.consts.push(key.clone());
        self.const_ix.insert(key, ix);
        ix
    }

    /// Registers `ty` and every type nested in it for the type table.
    pub fn add_type(&mut self, ty: &Ty) {
        let p = self.p;
        let mut work = vec![ty.clone()];
        while let Some(t) = work.pop() {
            let text = type_text(p, &t);
            if self.type_set.contains_key(&text) {
                continue;
            }
            self.type_set.insert(text, t.clone());
            match &t {
                Ty::List(e) => work.push((**e).clone()),
                Ty::Adt(def, args) => {
                    for a in args {
                        work.push(a.clone());
                    }
                    match &p.def(*def).kind {
                        DefKind::Struct(s) => {
                            for f in &s.fields {
                                work.push(instantiate(&f.ty, args));
                            }
                        }
                        DefKind::Enum(e) => {
                            for v in &e.variants {
                                for t in &v.payload {
                                    work.push(instantiate(t, args));
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Ty::Wrapper(def) => {
                    if let Some(w) = p.def(*def).kind.as_wrapper() {
                        work.push(w.inner.clone());
                    }
                }
                Ty::Refined(_, _) => work.push(strip(p, &t)),
                _ => {}
            }
        }
    }

    /// The well-known types of `cpl.h` (ids 1-11) and the declarations they name.
    fn well_known(&self) -> Vec<Option<Ty>> {
        let p = self.p;
        let std = &p.std_defs;
        // a declaration as the interop plan lists it: Ty::Adt(def, []), generic ones included
        let decl = |d: DefId| -> Option<Ty> {
            p.defs.get(d.0 as usize)?;
            Some(Ty::Adt(d, Vec::new()))
        };
        let (fek, fdetail) = foreign_parts(p);
        vec![
            Some(Ty::Int),
            Some(Ty::Bool),
            Some(Ty::Text),
            Some(Ty::Bytes),
            decl(std.unit),
            decl(std.option),
            decl(std.result),
            decl(std.constraint_error),
            decl(std.foreign_error),
            fek.and_then(decl),
            fdetail.and_then(decl),
        ]
    }

    /// The finalized type table: [0] unused, [1..11] the well-known types, then every registered type in byte
    /// order of its qualified type text (5.3).
    pub fn type_table(&mut self) -> TypeTable {
        let p = self.p;
        let wk = self.well_known();
        for t in wk.iter().flatten() {
            self.add_type(t);
        }
        let mut order: Vec<(String, Ty)> = Vec::new();
        let mut ids: HashMap<String, u32> = HashMap::new();
        order.push((String::new(), Ty::Error));
        for (i, t) in wk.iter().enumerate() {
            let ty = t.clone().unwrap_or(Ty::Error);
            let text = if t.is_some() {
                type_text(p, &ty)
            } else {
                format!("?{}", i + 1)
            };
            ids.insert(text.clone(), (i + 1) as u32);
            order.push((text, ty));
        }
        for (text, ty) in &self.type_set {
            if ids.contains_key(text) {
                continue;
            }
            ids.insert(text.clone(), order.len() as u32);
            order.push((text.clone(), ty.clone()));
        }
        let id_of = |ty: &Ty| -> u32 { ids.get(&type_text(p, ty)).copied().unwrap_or(0) };
        let mut rows = Vec::with_capacity(order.len());
        for (i, (text, ty)) in order.iter().enumerate() {
            if i == 0 {
                rows.push(TypeRow {
                    text: String::new(),
                    display: String::new(),
                    form: 0,
                    def: 0,
                    elem: 0,
                    members: Vec::new(),
                    payloads: Vec::new(),
                });
                continue;
            }
            let display = p.display_ty(ty);
            let mut row = TypeRow {
                text: text.clone(),
                display,
                form: 0,
                def: 0,
                elem: 0,
                members: Vec::new(),
                payloads: Vec::new(),
            };
            match ty {
                Ty::Int => row.form = TY_INT,
                Ty::Bool => row.form = TY_BOOL,
                Ty::Text => row.form = TY_TEXT,
                Ty::Bytes => row.form = TY_BYTES,
                Ty::List(e) => {
                    row.form = TY_LIST;
                    row.elem = id_of(e);
                }
                Ty::Adt(def, args) => {
                    row.def = def.0;
                    match &p.def(*def).kind {
                        DefKind::Struct(s) => {
                            row.form = TY_STRUCT;
                            row.elem = self.ref_ix.get(def).map_or(0, |i| i + 1);
                            row.members = s
                                .fields
                                .iter()
                                .map(|f| id_of(&instantiate(&f.ty, args)))
                                .collect();
                        }
                        DefKind::Enum(e) => {
                            row.form = TY_ENUM;
                            let mut off = 0u32;
                            for v in &e.variants {
                                row.members.push(off);
                                for t in &v.payload {
                                    row.payloads.push(id_of(&instantiate(t, args)));
                                    off += 1;
                                }
                            }
                            row.members.push(off);
                        }
                        _ => {}
                    }
                }
                Ty::Wrapper(def) => {
                    row.form = TY_WRAPPER;
                    row.def = def.0;
                    if let Some(w) = p.def(*def).kind.as_wrapper() {
                        row.elem = id_of(&w.inner);
                    }
                }
                Ty::Refined(_, def) => {
                    row.form = TY_REFINED;
                    row.def = self.ref_ix.get(def).copied().unwrap_or(u32::MAX);
                    row.elem = id_of(&strip(p, ty));
                }
                Ty::Capability(def) => {
                    row.form = TY_CAPABILITY;
                    row.def = def.0;
                }
                Ty::Param(_) => row.form = TY_PARAM,
                Ty::Gen(_) | Ty::Error => {}
            }
            rows.push(row);
        }
        TypeTable { rows, ids }
    }
}

/// The DefIds of `std.foreign.ForeignErrorKind` and `std.foreign.ForeignDetail`, read from the fields of
/// `std.foreign.ForeignError` as `cpl_eval::boundary::foreign_error` reads them.
pub(crate) fn foreign_parts(p: &Program) -> (Option<DefId>, Option<DefId>) {
    let Some(fe) = p.defs.get(p.std_defs.foreign_error.0 as usize) else {
        return (None, None);
    };
    let Some(s) = fe.kind.as_struct() else {
        return (None, None);
    };
    let mut kind = None;
    let mut detail = None;
    for f in &s.fields {
        match (&*f.name, &f.ty) {
            ("kind", Ty::Adt(d, _)) => kind = Some(*d),
            ("details", Ty::List(inner)) => {
                if let Ty::Adt(d, _) = &**inner {
                    detail = Some(*d);
                }
            }
            _ => {}
        }
    }
    (kind, detail)
}

fn binding_row(
    p: &Program,
    ld: &LocalDef,
    generics: &[String],
    capability: Option<u32>,
) -> BindingRow {
    BindingRow {
        local: ld.id,
        name: ld.name.clone(),
        ty: p.display_ty_in(&ld.ty, generics),
        kind: ld.kind.as_str(),
        mutable: ld.mutable,
        capability,
    }
}

/// The `members` of a definition row: fields, variants, positional parameters (a member stub's receiver `self`), or
/// operations.
pub(crate) fn members_of(p: &Program, def: DefId) -> Vec<String> {
    let d = p.def(def);
    match &d.kind {
        DefKind::Struct(s) => s.fields.iter().map(|f| f.name.clone()).collect(),
        DefKind::Enum(e) => e.variants.iter().map(|v| v.name.clone()).collect(),
        DefKind::Fn(fd) => fd
            .params
            .iter()
            .enumerate()
            .map(|(i, param)| {
                if i == 0 && matches!(d.name, DeclName::Member { .. }) {
                    "self".to_string()
                } else {
                    param.name.clone()
                }
            })
            .collect(),
        DefKind::Capability(c) => c.ops.iter().map(|o| o.name.clone()).collect(),
        _ => Vec::new(),
    }
}

fn def_kind_code(kind: &DefKind) -> &'static str {
    match kind {
        DefKind::Fn(_) => "CPLRT_DEF_FN",
        DefKind::Wrapper(_) => "CPLRT_DEF_WRAPPER",
        DefKind::Refinement(_) => "CPLRT_DEF_REFINEMENT",
        DefKind::Struct(_) => "CPLRT_DEF_STRUCT",
        DefKind::Enum(_) => "CPLRT_DEF_ENUM",
        DefKind::Capability(_) => "CPLRT_DEF_CAPABILITY",
        DefKind::Test(_) => "CPLRT_DEF_TEST",
        DefKind::Property(_) => "CPLRT_DEF_PROPERTY",
        DefKind::BuiltinType(_) => "CPLRT_DEF_BUILTIN_TYPE",
    }
}

fn sorted_indices(names: &[String]) -> Vec<u32> {
    let mut ix: Vec<u32> = (0..names.len() as u32).collect();
    ix.sort_by(|&a, &b| {
        names[a as usize]
            .as_bytes()
            .cmp(names[b as usize].as_bytes())
            .then(a.cmp(&b))
    });
    ix
}

fn u32_array(out: &mut String, name: &str, items: &[u32]) {
    let body: Vec<String> = items.iter().map(|n| format!("{n}u")).collect();
    let _ = writeln!(
        out,
        "static const uint32_t {name}[{}] = {{ {} }};",
        items.len().max(1),
        if body.is_empty() {
            "0u".to_string()
        } else {
            body.join(", ")
        }
    );
}

fn str_array(out: &mut String, name: &str, items: &[String]) {
    let body: Vec<String> = items.iter().map(|s| c_string(s)).collect();
    let _ = writeln!(
        out,
        "static const char *const {name}[{}] = {{ {} }};",
        items.len().max(1),
        if body.is_empty() {
            "NULL".to_string()
        } else {
            body.join(", ")
        }
    );
}

/// What the tables unit records about the package besides the rows.
pub(crate) struct UnitInfo<'a> {
    pub prefix: &'a str,
    pub package: &'a str,
    pub edition: &'a str,
    pub abi: u32,
    pub release: bool,
    pub holes: u32,
    pub runtime_digest: &'a str,
    pub program_digest: &'a str,
    pub exports_digest: &'a str,
    pub target: &'a str,
    pub compiler: &'a str,
    pub profile: &'a str,
    pub max_call_depth: u32,
    /// the export table entries, already rendered (`cplrt_export` initializers) with their arrays
    pub exports_c: &'a str,
    pub nexports: usize,
    /// the grant table entries, already rendered
    pub grants_c: &'a str,
    pub ngrants: usize,
    /// Milestone 3: the Python worker's source and its digest (spec/python.md 6.1, 6.5).
    pub python_worker: Option<(&'a str, &'a str)>,
    /// With coverage, the HIR variant of every counter (the tables are defined even when there is none).
    pub coverage: Option<&'a [&'static str]>,
}

impl Tables<'_> {
    /// The text of `<prefix>_tables.c`.
    pub fn render(&self, tt: &TypeTable, info: &UnitInfo<'_>) -> String {
        let p = self.p;
        let sm = p.source_map();
        let mut out = String::with_capacity(64 * 1024);
        let _ = writeln!(
            out,
            "/* {}_tables.c: the program tables of package \"{}\" (spec/cabi.md 3.2, Appendix B.1). Generated by cpl; do not edit. */",
            info.prefix,
            comment_text(info.package)
        );
        out.push_str(&crate::print::prologue(info.prefix));
        out.push_str("#include \"cpl_rt.h\"\n\n");

        // ---- files
        let files: Vec<String> = sm.files().map(|f| f.path.clone()).collect();
        str_array(&mut out, "cpl_files", &files);

        // ---- the statics of payload-less variants
        out.push_str("\n/* the immortal statics of payload-less variants */\n");
        let mut statics: Vec<(DefId, Vec<Option<String>>)> = Vec::new();
        for d in &p.defs {
            if let DefKind::Enum(e) = &d.kind {
                let mut names = Vec::new();
                for (i, v) in e.variants.iter().enumerate() {
                    if v.payload.is_empty() {
                        let name = format!("cpl_s{}_{}", d.id.0, i);
                        if i <= 0xffff {
                            let _ = writeln!(
                                out,
                                "static cplrt_obj {name} = {{ CPLRT_ENUM, 0u, {i}u, 0u, {{ .live = {{ CPLRT_RC_IMMORTAL, {}u }} }} }};",
                                d.id.0
                            );
                            names.push(Some(name));
                        } else {
                            names.push(None);
                        }
                    } else {
                        names.push(None);
                    }
                }
                statics.push((d.id, names));
            }
        }
        let mut statics_name: HashMap<DefId, String> = HashMap::new();
        for (def, names) in &statics {
            let name = format!("cpl_st{}", def.0);
            let items: Vec<String> = names
                .iter()
                .map(|n| match n {
                    Some(n) => format!("&{n}"),
                    None => "NULL".to_string(),
                })
                .collect();
            let _ = writeln!(
                out,
                "static cplrt_obj *const {name}[{}] = {{ {} }};",
                items.len().max(1),
                if items.is_empty() {
                    "NULL".to_string()
                } else {
                    items.join(", ")
                }
            );
            statics_name.insert(*def, name);
        }

        // ---- definitions
        out.push_str("\n/* definitions (CPL_D) */\n");
        let mut def_rows = Vec::with_capacity(p.defs.len());
        for d in &p.defs {
            let n = d.id.0;
            let members = members_of(p, d.id);
            let (m_name, o_name) = if members.is_empty() {
                ("NULL".to_string(), "NULL".to_string())
            } else {
                let m = format!("cpl_m{n}");
                let o = format!("cpl_o{n}");
                str_array(&mut out, &m, &members);
                u32_array(&mut out, &o, &sorted_indices(&members));
                (m, o)
            };
            let ops_name = if let DefKind::Capability(cd) = &d.kind {
                let mut ops = Vec::new();
                for (i, op) in cd.ops.iter().enumerate() {
                    let pn: Vec<String> = op.params.iter().map(|x| x.name.clone()).collect();
                    let pa = format!("cpl_op{n}_{i}_p");
                    let ps = format!("cpl_op{n}_{i}_s");
                    str_array(&mut out, &pa, &pn);
                    u32_array(&mut out, &ps, &sorted_indices(&pn));
                    let foreign = cpl_eval::boundary::is_foreign_error_return(p, &op.ret);
                    ops.push(format!(
                        "{{ {}, {}, {}u, {pa}, {ps}, {}u, {}u }}",
                        c_string(&op.name),
                        c_string(operation_identity(p, d.id, &op.name).as_str()),
                        op.params.len(),
                        tt.ids.get(&type_text(p, &op.ret)).copied().unwrap_or(0),
                        u32::from(foreign)
                    ));
                }
                let name = format!("cpl_ops{n}");
                let _ = writeln!(
                    out,
                    "static const cplrt_op {name}[{}] = {{ {} }};",
                    ops.len().max(1),
                    if ops.is_empty() {
                        "{ NULL, NULL, 0u, NULL, NULL, 0u, 0u }".to_string()
                    } else {
                        ops.join(", ")
                    }
                );
                name
            } else {
                "NULL".to_string()
            };
            let (nreq, req_name) = match self.fn_requires.get(&d.id) {
                Some(r) if !r.is_empty() => {
                    let name = format!("cpl_r{n}");
                    u32_array(&mut out, &name, r);
                    (r.len(), name)
                }
                _ => (0, "NULL".to_string()),
            };
            let ptypes = match &d.kind {
                DefKind::Fn(fd)
                    if self.with_tests && fd.generics.is_empty() && !fd.params.is_empty() =>
                {
                    let name = format!("cpl_pt{n}");
                    let ids: Vec<u32> = fd
                        .params
                        .iter()
                        .map(|x| tt.ids.get(&type_text(p, &x.ty)).copied().unwrap_or(0))
                        .collect();
                    u32_array(&mut out, &name, &ids);
                    name
                }
                _ => "NULL".to_string(),
            };
            let psites = match &d.kind {
                DefKind::Fn(fd) if !fd.params.is_empty() => {
                    let name = format!("cpl_ps{n}");
                    let ids: Vec<u32> = fd.params.iter().map(|x| self.site_of(x.span)).collect();
                    u32_array(&mut out, &name, &ids);
                    name
                }
                _ => "NULL".to_string(),
            };
            let qualified = p.display_path(d.id);
            def_rows.push(format!(
                "    /* {n} */ {{ {}, {}, {}, {}, {}, {}u, {m_name}, {o_name}, {}, {ops_name}, {nreq}u, {req_name}, {ptypes}, {psites} }}",
                c_string(d.stable_id.as_str()),
                c_string(&qualified),
                c_string(&type_name(p, d.id)),
                c_string(&bare_name(p, d.id)),
                def_kind_code(&d.kind),
                members.len(),
                statics_name.get(&d.id).cloned().unwrap_or_else(|| "NULL".to_string()),
            ));
        }
        let _ = writeln!(
            out,
            "static const cplrt_def cpl_defs[{}] = {{\n{}\n}};",
            def_rows.len().max(1),
            def_rows.join(",\n")
        );

        // ---- sites
        out.push_str("\n/* sites (CPL_S): file, start, end, line, col, end_line, end_col */\n");
        let mut site_rows = Vec::with_capacity(self.sites.len());
        for (i, s) in self.sites.iter().enumerate() {
            let loc = sm.location(*s);
            site_rows.push(format!(
                "    /* {i} */ {{ {}u, {}u, {}u, {}u, {}u, {}u, {}u }}",
                s.file.0, s.start, s.end, loc.line, loc.col, loc.end_line, loc.end_col
            ));
        }
        let _ = writeln!(
            out,
            "static const cplrt_site cpl_sites[{}] = {{\n{}\n}};",
            site_rows.len().max(1),
            if site_rows.is_empty() {
                "    { 0u, 0u, 0u, 0u, 0u, 0u, 0u }".to_string()
            } else {
                site_rows.join(",\n")
            }
        );

        // ---- clauses
        out.push_str("\n/* clauses (CPL_CL) */\n");
        let mut clause_rows = Vec::new();
        for (i, c) in self.clauses.iter().enumerate() {
            let kind = match c.kind {
                CK_REQUIRES => "CPLRT_CK_REQUIRES",
                CK_ENSURES => "CPLRT_CK_ENSURES",
                CK_INVARIANT => "CPLRT_CK_INVARIANT",
                CK_DECREASES => "CPLRT_CK_DECREASES",
                CK_WHERE => "CPLRT_CK_WHERE",
                _ => "CPLRT_CK_RANGE",
            };
            clause_rows.push(format!(
                "    /* {i} */ {{ {}, {kind}, {}u, {}u, {}, {}, {}, {} }}",
                c_string(&c.text),
                c.site,
                c.owner,
                c_index(c.op),
                c_string(&c.owner_id),
                c_string(&c.owner_qualified),
                c_index(c.bsite)
            ));
        }
        let _ = writeln!(
            out,
            "static const cplrt_clause cpl_clauses[{}] = {{\n{}\n}};",
            clause_rows.len().max(1),
            if clause_rows.is_empty() {
                "    { \"\", 0u, 0u, 0u, CPLRT_NONE, \"\", \"\", CPLRT_NONE }".to_string()
            } else {
                clause_rows.join(",\n")
            }
        );

        // ---- binding sites
        out.push_str("\n/* binding sites (CPL_A) */\n");
        let mut bsite_rows = Vec::new();
        for (i, b) in self.bsites.iter().enumerate() {
            let bname = if b.bindings.is_empty() {
                "NULL".to_string()
            } else {
                let rows: Vec<String> = b
                    .bindings
                    .iter()
                    .map(|x| {
                        format!(
                            "{{ {}, {}, {}, {}u, {} }}",
                            c_string(&x.name),
                            c_string(&x.ty),
                            c_string(x.kind),
                            u32::from(x.mutable),
                            c_index(x.capability)
                        )
                    })
                    .collect();
                let name = format!("cpl_ab{i}");
                let _ = writeln!(
                    out,
                    "static const cplrt_binding {name}[{}] = {{ {} }};",
                    rows.len(),
                    rows.join(", ")
                );
                name
            };
            let oname = if b.object.is_empty() {
                "NULL".to_string()
            } else {
                let name = format!("cpl_ao{i}");
                u32_array(&mut out, &name, &b.object);
                name
            };
            bsite_rows.push(format!(
                "    /* {i} */ {{ {}, {}, {}u, {bname}, {}u, {oname} }}",
                c_opt_string(b.text.as_deref()),
                c_index(b.site),
                b.bindings.len(),
                b.object.len()
            ));
        }
        let _ = writeln!(
            out,
            "static const cplrt_bsite cpl_bsites[{}] = {{\n{}\n}};",
            bsite_rows.len().max(1),
            if bsite_rows.is_empty() {
                "    { NULL, CPLRT_NONE, 0u, NULL, 0u, NULL }".to_string()
            } else {
                bsite_rows.join(",\n")
            }
        );

        // ---- holes, by HoleId (a gap is an empty row)
        out.push_str("\n/* holes (CPL_H) */\n");
        let nholes = self.holes.keys().next_back().map_or(0, |k| k + 1);
        let mut hole_rows = Vec::new();
        for h in 0..nholes {
            match self.holes.get(&h) {
                Some(r) => hole_rows.push(format!(
                    "    /* {h} */ {{ {}, {}, {}u, {}u }}",
                    c_string(&r.name),
                    c_string(&r.expected_type),
                    r.site,
                    r.bsite
                )),
                None => hole_rows.push(format!(
                    "    /* {h} */ {{ \"\", \"\", CPLRT_NONE, CPLRT_NONE }}"
                )),
            }
        }
        // the type name cplrt_hole is also the macro of a runtime function (cpl_rt.h), which this unit never calls
        out.push_str("#undef cplrt_hole\n");
        let _ = writeln!(
            out,
            "static const cplrt_hole cpl_holes[{}] = {{\n{}\n}};",
            hole_rows.len().max(1),
            if hole_rows.is_empty() {
                "    { \"\", \"\", CPLRT_NONE, CPLRT_NONE }".to_string()
            } else {
                hole_rows.join(",\n")
            }
        );

        // ---- refinements
        out.push_str("\n/* refinements (CPL_R) */\n");
        let mut ref_rows = Vec::new();
        for (i, r) in self.refs.iter().enumerate() {
            let cname = format!("cpl_rc{i}");
            u32_array(&mut out, &cname, &r.chain);
            ref_rows.push(format!(
                "    /* {i} */ {{ {}u, {}, {}, {}, {}u, {}u, {}u, {cname} }}",
                r.def.0,
                c_string(&r.type_name),
                c_string(&r.declared_name),
                c_string(&r.predicate),
                r.clause,
                tt.ids.get(&type_text(p, &r.base)).copied().unwrap_or(0),
                r.chain.len()
            ));
        }
        let _ = writeln!(
            out,
            "static const cplrt_refinement cpl_refinements[{}] = {{\n{}\n}};",
            ref_rows.len().max(1),
            if ref_rows.is_empty() {
                "    { 0u, \"\", \"\", \"\", 0u, 0u, 0u, NULL }".to_string()
            } else {
                ref_rows.join(",\n")
            }
        );

        // ---- types
        out.push_str("\n/* types (CPL_X): the package's cpl_type ids */\n");
        let mut type_rows = Vec::new();
        for (i, t) in tt.rows.iter().enumerate() {
            let mname = if t.members.is_empty() {
                "NULL".to_string()
            } else {
                let name = format!("cpl_xm{i}");
                u32_array(&mut out, &name, &t.members);
                name
            };
            let pname = if t.payloads.is_empty() {
                "NULL".to_string()
            } else {
                let name = format!("cpl_xp{i}");
                u32_array(&mut out, &name, &t.payloads);
                name
            };
            let nmembers = if t.form == TY_ENUM {
                t.members.len().saturating_sub(1)
            } else {
                t.members.len()
            };
            type_rows.push(format!(
                "    /* {i} */ {{ {}, {}, {}u, {}u, {}u, {}u, {mname}, {pname} }}",
                c_string(&t.text),
                c_string(&t.display),
                t.form,
                t.def,
                t.elem,
                nmembers
            ));
        }
        let _ = writeln!(
            out,
            "static const cplrt_type cpl_types[{}] = {{\n{}\n}};",
            type_rows.len(),
            type_rows.join(",\n")
        );

        // ---- exports and grants
        out.push_str(info.exports_c);
        out.push_str(info.grants_c);

        // ---- coverage
        if let Some(coverage) = info.coverage {
            out.push_str("\n#if defined(CPLRT_COVERAGE) && CPLRT_COVERAGE\n");
            let n = coverage.len();
            let _ = writeln!(out, "_Atomic uint64_t CPLPKG(cov)[{}];", n.max(1));
            let mut kinds: Vec<String> = coverage.iter().map(|k| c_string(k)).collect();
            if kinds.is_empty() {
                kinds.push("NULL".to_string());
            }
            let _ = writeln!(
                out,
                "const char *const CPLPKG(cov_kinds)[{}] = {{ {} }};",
                kinds.len(),
                kinds.join(", ")
            );
            let _ = writeln!(out, "const uint32_t CPLPKG(ncov) = {n}u;");
            out.push_str("#endif\n");
        }

        // ---- the program
        let (fek, fdetail) = foreign_parts(p);
        let std = &p.std_defs;
        let opt = |d: DefId| -> String {
            if (d.0 as usize) < p.defs.len() {
                format!("{}u", d.0)
            } else {
                "CPLRT_NONE".to_string()
            }
        };
        out.push_str("\nconst cplrt_program CPLPKG(program) = {\n");
        let _ = writeln!(
            out,
            "    {{ sizeof(cpl_library_info), CPL_API_VERSION, {}u, {}u, {}u, 0u, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {} }},",
            info.abi,
            u32::from(info.release),
            info.holes,
            c_string(info.package),
            c_string(info.prefix),
            c_string(info.edition),
            c_string(cpl_diag::version::TOOLCHAIN),
            c_string(cpl_rt::RUNTIME_VERSION),
            c_string(info.runtime_digest),
            c_string(info.program_digest),
            c_string(info.exports_digest),
            c_string(info.target),
            c_string(info.compiler),
            c_string(info.profile)
        );
        out.push_str("    &CPLPKG(code),\n");
        let _ = writeln!(out, "    {}u, cpl_defs,", p.defs.len());
        let _ = writeln!(out, "    {}u, cpl_files,", files.len());
        let _ = writeln!(out, "    {}u, cpl_sites,", self.sites.len());
        let _ = writeln!(out, "    {}u, cpl_clauses,", self.clauses.len());
        let _ = writeln!(out, "    {}u, cpl_bsites,", self.bsites.len());
        let _ = writeln!(out, "    {}u, cpl_holes,", nholes);
        let _ = writeln!(out, "    {}u, cpl_refinements,", self.refs.len());
        let _ = writeln!(out, "    {}u, cpl_types,", tt.rows.len());
        let _ = writeln!(
            out,
            "    {}u, {},",
            info.nexports,
            if info.nexports > 0 {
                "cpl_exports"
            } else {
                "NULL"
            }
        );
        let _ = writeln!(
            out,
            "    {}u, {},",
            info.ngrants,
            if info.ngrants > 0 {
                "cpl_grants"
            } else {
                "NULL"
            }
        );
        let _ = writeln!(
            out,
            "    {{ {}, {}, {}, {}, {}, {}, {} }},",
            opt(std.unit),
            opt(std.option),
            opt(std.result),
            opt(std.constraint_error),
            opt(std.foreign_error),
            fek.map_or("CPLRT_NONE".to_string(), opt),
            fdetail.map_or("CPLRT_NONE".to_string(), opt)
        );
        let _ = writeln!(out, "    {}u,", info.max_call_depth);
        match info.python_worker {
            None => out.push_str("    NULL, 0u, NULL,\n"),
            Some((source, digest)) => {
                // one literal per source line, concatenated by the compiler
                out.push_str("    ");
                for line in source.split_inclusive('\n') {
                    out.push_str(&c_string(line));
                    out.push_str("\n    ");
                }
                let _ = writeln!(out, ",\n    {}u, {},", source.len(), c_string(digest));
            }
        }
        out.push_str("};\n");
        out
    }
}

/// Whether `def` is a function with a block body.
pub(crate) fn has_block_body(p: &Program, def: DefId) -> bool {
    matches!(
        p.def(def).kind.as_fn().map(|f| &f.body),
        Some(FnBody::Block(_))
    )
}

/// The C name of a `BindingKind` (for comments).
#[allow(dead_code)]
pub(crate) fn binding_kind_name(k: BindingKind) -> &'static str {
    k.as_str()
}
