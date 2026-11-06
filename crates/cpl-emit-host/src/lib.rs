//! cpl-emit-host: CPL compiled to the source of a host language (`spec/hosts.md`).
//!
//! The JVM, Go, and Rust reach CPL the way Kotlin reaches Java: the generated package is source
//! of the host language, so a host program calls CPL functions as its own functions, uses CPL
//! structs and enums as its own types, and implements capabilities as its own traits and
//! interfaces. No call crosses a foreign-function boundary, nothing is marshalled, and the host's
//! compiler can inline across the boundary.
//!
//! [`emit`] lowers the checked program once ([`lower`], into the IR of [`lir`]) and prints it
//! for one [`Target`] together with that target's runtime: values (`Int` keeps an `i64` until a
//! result does not fit), faults, the calling context with the call-depth limit, the standard
//! library's builtins, value text, and value JSON. Contracts are checked as in checked mode.
//!
//! | Module | Contents |
//! | - | - |
//! | [`lir`] | the lowered IR: statements, simple expressions, patterns, faults |
//! | [`lower`] | the HIR to the IR, with evaluation order and contract checks explicit |
//! | [`rust`] | the Rust printer (a crate) |
//! | [`go`] | the Go printer (a module) |
//! | [`java`] | the Java printer (sources for the JVM; Kotlin uses them through its Java interop) |

pub mod go;
pub mod java;
pub mod lir;
pub mod lower;
mod moves;
pub mod rust;

use cpl_diag::FileOrigin;
use cpl_hir::{DefId, DefKind, DefStatus, Program, Ty};
use std::collections::{BTreeMap, BTreeSet};

/// A host language.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Rust,
    Go,
    Java,
}

impl Target {
    pub fn name(self) -> &'static str {
        match self {
            Target::Rust => "rust",
            Target::Go => "go",
            Target::Java => "java",
        }
    }
    pub fn parse(s: &str) -> Option<Target> {
        match s {
            "rust" => Some(Target::Rust),
            "go" => Some(Target::Go),
            "java" | "jvm" | "kotlin" => Some(Target::Java),
            _ => None,
        }
    }
}

/// How to name and shape the generated package.
#[derive(Clone, Debug)]
pub struct Options {
    /// The crate name (Rust), module path (Go), or base package (Java).
    pub name: String,
    /// Also generate the JSON driver that the differential tests run: one request per line,
    /// `{"function": "<qualified name>", "args": [<value JSON>...]}`, answered with
    /// `{"value": ...}`, `{"fault": {...}}`, or `{"outside": ...}`.
    pub driver: bool,
}

/// The generated files, by path relative to the package root.
#[derive(Clone, Debug, Default)]
pub struct Package {
    pub files: BTreeMap<String, String>,
    /// The qualified names of the functions a host can call.
    pub exports: Vec<String>,
}

impl Package {
    /// Write the package under `dir`: a file whose content is unchanged keeps its time stamp
    /// (the host's incremental build), and a file an earlier `write` generated there that this
    /// package no longer has is removed (the list is `dir/.cpl-generated`). Nothing else in
    /// `dir` is touched.
    pub fn write(&self, dir: &std::path::Path) -> std::io::Result<()> {
        let index = dir.join(".cpl-generated");
        let previous = std::fs::read_to_string(&index).unwrap_or_default();
        for old in previous.lines() {
            let path = std::path::Path::new(old);
            let inside = !old.is_empty()
                && path
                    .components()
                    .all(|c| matches!(c, std::path::Component::Normal(_)));
            if inside && !self.files.contains_key(old) {
                let _ = std::fs::remove_file(dir.join(path));
            }
        }
        for (path, text) in &self.files {
            let full = dir.join(path);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent)?;
            }
            if std::fs::read(&full).ok().as_deref() != Some(text.as_bytes()) {
                std::fs::write(&full, text)?;
            }
        }
        let mut list: String = self.files.keys().map(|k| format!("{k}\n")).collect();
        if list.is_empty() {
            list.push('\n');
        }
        std::fs::write(index, list)
    }
}

/// Compile the checked program `p` to the source of `target`.
pub fn emit(p: &Program, target: Target, options: &Options) -> Result<Package, String> {
    if let Some(d) = p
        .defs()
        .find(|d| is_project(p, d.id) && d.status == DefStatus::HasErrors)
    {
        return Err(format!(
            "{} has errors; only a program that checks is compiled",
            p.display_path(d.id)
        ));
    }
    let mut lowered = lower::lower_program(p, &|d| emitted(p, d));
    prune_std(p, &mut lowered);
    Ok(match target {
        Target::Rust => rust::emit(p, &lowered, options),
        Target::Go => go::emit(p, &lowered, options),
        Target::Java => java::emit(p, &lowered, options),
    })
}

/// Keep the standard library's refinements the project reaches: those its `check<R>` and its
/// boundary checks evaluate, the refinements in its declared types, and their chains.
fn prune_std(p: &Program, lowered: &mut lower::Lowered) {
    fn refined_in(p: &Program, ty: &Ty, out: &mut Vec<DefId>, seen: &mut BTreeSet<String>) {
        match ty {
            Ty::Refined(_, d) => {
                out.push(*d);
                if let Some(r) = p.def(*d).kind.as_refinement() {
                    refined_in(p, &r.base, out, seen);
                }
            }
            Ty::List(t) | Ty::Gen(t) => refined_in(p, t, out, seen),
            Ty::Adt(d, args) => {
                for a in args {
                    refined_in(p, a, out, seen);
                }
                if seen.insert(p.display_path(*d)) {
                    match &p.def(*d).kind {
                        DefKind::Struct(s) => {
                            for f in &s.fields {
                                refined_in(p, &f.ty, out, seen);
                            }
                        }
                        DefKind::Enum(e) => {
                            for t in e.variants.iter().flat_map(|v| &v.payload) {
                                refined_in(p, t, out, seen);
                            }
                        }
                        _ => {}
                    }
                }
            }
            Ty::Wrapper(d) if seen.insert(p.display_path(*d)) => {
                if let Some(w) = p.def(*d).kind.as_wrapper() {
                    refined_in(p, &w.inner, out, seen);
                }
            }
            _ => {}
        }
    }
    let mut work: Vec<DefId> = Vec::new();
    let mut seen = BTreeSet::new();
    for d in p
        .defs()
        .filter(|d| lowered.defs.contains(&d.id) && is_project(p, d.id))
    {
        let mut tys: Vec<&Ty> = Vec::new();
        match &d.kind {
            DefKind::Fn(f) => {
                tys.extend(f.params.iter().chain(&f.using).map(|q| &q.ty));
                tys.push(&f.ret);
            }
            DefKind::Struct(s) => tys.extend(s.fields.iter().map(|f| &f.ty)),
            DefKind::Enum(e) => tys.extend(e.variants.iter().flat_map(|v| &v.payload)),
            DefKind::Wrapper(w) => tys.push(&w.inner),
            DefKind::Refinement(r) => tys.push(&r.base),
            DefKind::Capability(c) => {
                for op in &c.ops {
                    tys.extend(op.params.iter().map(|q| &q.ty));
                    tys.push(&op.ret);
                }
            }
            _ => {}
        }
        for t in tys {
            refined_in(p, t, &mut work, &mut seen);
        }
    }
    let mut called = |e: &lir::LExpr| {
        if let lir::LExpr::Pred { def, .. } = e {
            work.push(*def);
        }
    };
    for f in &lowered.fns {
        lir::visit_exprs(&f.requires, &mut called);
        lir::visit_exprs(&f.body, &mut called);
        lir::visit_exprs(&f.ensures, &mut called);
        if let Some((stmts, value, _)) = &f.measure {
            lir::visit_exprs(stmts, &mut called);
            lir::visit_expr(value, &mut called);
        }
    }
    for q in lowered.preds.iter().filter(|q| is_project(p, q.def)) {
        lir::visit_exprs(&q.body, &mut called);
        lir::visit_expr(&q.value, &mut called);
    }
    let bodies: BTreeMap<DefId, &lir::LPred> = lowered.preds.iter().map(|q| (q.def, q)).collect();
    let mut needed = BTreeSet::new();
    while let Some(d) = work.pop() {
        if !needed.insert(d) {
            continue;
        }
        work.extend(lower::chain(p, d));
        if let Some(q) = bodies.get(&d) {
            let mut inner = Vec::new();
            let mut called = |e: &lir::LExpr| {
                if let lir::LExpr::Pred { def, .. } = e {
                    inner.push(*def);
                }
            };
            lir::visit_exprs(&q.body, &mut called);
            lir::visit_expr(&q.value, &mut called);
            work.extend(inner);
        }
    }
    lowered
        .preds
        .retain(|q| is_project(p, q.def) || needed.contains(&q.def));
    lowered
        .defs
        .retain(|d| is_project(p, *d) || needed.contains(d));
}

/// A definition of the project's own modules.
pub fn is_project(p: &Program, d: DefId) -> bool {
    p.module(p.def(d).module).origin != FileOrigin::Std
        && p.module(p.def(d).module)
            .path
            .first()
            .is_none_or(|s| s != "std")
}

/// Whether a definition is compiled: the project's functions, types, and capabilities, and the
/// standard library's refinements (whose predicates `check<R>` and boundaries evaluate).
pub fn emitted(p: &Program, d: DefId) -> bool {
    let def = p.def(d);
    if def.status == DefStatus::HasErrors {
        return false;
    }
    match &def.kind {
        DefKind::Test(_) | DefKind::Property(_) | DefKind::BuiltinType(_) => false,
        DefKind::Refinement(_) => true,
        DefKind::Fn(f) => is_project(p, d) && matches!(f.body, cpl_hir::FnBody::Block(_)),
        _ => is_project(p, d),
    }
}

/// The standard library's data types, which every runtime defines itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StdType {
    Unit,
    Option,
    Result,
    ConstraintError,
    ForeignError,
    ForeignErrorKind,
    ForeignDetail,
}

pub fn std_type(p: &Program, d: DefId) -> Option<StdType> {
    let s = p.std_defs();
    if d == s.unit {
        return Some(StdType::Unit);
    }
    if d == s.option {
        return Some(StdType::Option);
    }
    if d == s.result {
        return Some(StdType::Result);
    }
    if d == s.constraint_error {
        return Some(StdType::ConstraintError);
    }
    if d == s.foreign_error {
        return Some(StdType::ForeignError);
    }
    let def = p.def(d);
    if p.module(def.module).path == ["std", "foreign"] {
        return match def.name.display().as_str() {
            "ForeignErrorKind" => Some(StdType::ForeignErrorKind),
            "ForeignDetail" => Some(StdType::ForeignDetail),
            _ => None,
        };
    }
    None
}

/// `ty` with every `Ty::Param(i)` replaced by `args[i]`.
pub fn instantiate(ty: &Ty, args: &[Ty]) -> Ty {
    if args.is_empty() {
        return ty.clone();
    }
    match ty {
        Ty::Param(i) => args.get(*i as usize).cloned().unwrap_or_else(|| ty.clone()),
        Ty::List(t) => Ty::List(Box::new(instantiate(t, args))),
        Ty::Gen(t) => Ty::Gen(Box::new(instantiate(t, args))),
        Ty::Adt(d, a) => Ty::Adt(*d, a.iter().map(|t| instantiate(t, args)).collect()),
        other => other.clone(),
    }
}

/// The base type of a refinement (refinements are erased).
pub fn erase(p: &Program, ty: &Ty) -> Ty {
    match ty {
        Ty::Refined(_, d) => match p.def(*d).kind.as_refinement() {
            Some(r) => erase(p, &r.base),
            None => Ty::Error,
        },
        Ty::List(t) => Ty::List(Box::new(erase(p, t))),
        Ty::Adt(d, a) => Ty::Adt(*d, a.iter().map(|t| erase(p, t)).collect()),
        other => other.clone(),
    }
}

/// Whether a value entering at `ty` must be validated: a refinement or a struct invariant is
/// reachable in it (semantics S39, S81).
pub fn needs_validation(p: &Program, ty: &Ty) -> bool {
    fn walk(p: &Program, ty: &Ty, seen: &mut BTreeSet<String>) -> bool {
        match ty {
            Ty::Refined(..) => true,
            Ty::List(t) => walk(p, t, seen),
            Ty::Adt(d, args) => {
                let key = p.display_ty(ty);
                if !seen.insert(key) {
                    return false;
                }
                match &p.def(*d).kind {
                    DefKind::Struct(s) => {
                        s.invariant.is_some()
                            || s.fields
                                .iter()
                                .any(|f| walk(p, &instantiate(&f.ty, args), seen))
                    }
                    DefKind::Enum(e) => e.variants.iter().any(|v| {
                        v.payload
                            .iter()
                            .any(|t| walk(p, &instantiate(t, args), seen))
                    }),
                    _ => false,
                }
            }
            Ty::Wrapper(d) => {
                let key = p.display_path(*d);
                if !seen.insert(key) {
                    return false;
                }
                p.def(*d)
                    .kind
                    .as_wrapper()
                    .is_some_and(|w| walk(p, &w.inner, seen))
            }
            _ => false,
        }
    }
    walk(p, ty, &mut BTreeSet::new())
}

/// Whether a value of `ty` can hold a `Text`: a host whose strings need not be Unicode scalar
/// values (Go's may be any bytes, Java's may hold unpaired surrogates) checks every one that
/// enters at its boundary.
pub fn contains_text(p: &Program, ty: &Ty) -> bool {
    fn walk(p: &Program, ty: &Ty, seen: &mut BTreeSet<String>) -> bool {
        match ty {
            Ty::Text => true,
            Ty::Refined(..) => walk(p, &erase(p, ty), seen),
            Ty::List(t) => walk(p, t, seen),
            Ty::Adt(d, args) => {
                if !seen.insert(p.display_ty(ty)) {
                    return false;
                }
                match &p.def(*d).kind {
                    DefKind::Struct(s) => s
                        .fields
                        .iter()
                        .any(|f| walk(p, &instantiate(&f.ty, args), seen)),
                    DefKind::Enum(e) => e.variants.iter().any(|v| {
                        v.payload
                            .iter()
                            .any(|t| walk(p, &instantiate(t, args), seen))
                    }),
                    _ => false,
                }
            }
            Ty::Wrapper(d) => {
                seen.insert(p.display_path(*d))
                    && p.def(*d)
                        .kind
                        .as_wrapper()
                        .is_some_and(|w| walk(p, &w.inner, seen))
            }
            _ => false,
        }
    }
    walk(p, ty, &mut BTreeSet::new())
}

/// Whether a value of `ty` contains a value of the type `target` by value, without a list (a
/// list is a reference) between them; `enums_by_value` says whether the host stores an enum
/// inline (Rust) or behind a reference (Go's interfaces).
///
/// A field, payload, or wrapped value whose type reaches the type being declared needs a
/// reference in a host whose types have a size (`Arc` in Rust, a pointer in Go).
pub fn reaches(p: &Program, ty: &Ty, target: DefId, enums_by_value: bool) -> bool {
    fn walk(p: &Program, ty: &Ty, target: DefId, enums: bool, seen: &mut BTreeSet<String>) -> bool {
        match ty {
            Ty::Refined(..) => walk(p, &erase(p, ty), target, enums, seen),
            Ty::Wrapper(d) => {
                *d == target
                    || (seen.insert(p.display_path(*d))
                        && p.def(*d)
                            .kind
                            .as_wrapper()
                            .is_some_and(|w| walk(p, &w.inner, target, enums, seen)))
            }
            Ty::Adt(d, args) => {
                if *d == target {
                    return true;
                }
                if !seen.insert(p.display_ty(ty)) {
                    return false;
                }
                match std_type(p, *d) {
                    Some(StdType::Option | StdType::Result) => {
                        args.iter().any(|a| walk(p, a, target, enums, seen))
                    }
                    Some(_) => false,
                    None => match &p.def(*d).kind {
                        DefKind::Struct(s) => s
                            .fields
                            .iter()
                            .any(|f| walk(p, &instantiate(&f.ty, args), target, enums, seen)),
                        DefKind::Enum(e) if enums => e.variants.iter().any(|v| {
                            v.payload
                                .iter()
                                .any(|t| walk(p, &instantiate(t, args), target, enums, seen))
                        }),
                        _ => false,
                    },
                }
            }
            _ => false,
        }
    }
    walk(p, ty, target, enums_by_value, &mut BTreeSet::new())
}

/// The functions a JSON driver can call: monomorphic, without capability parameters.
pub fn driver_functions(_p: &Program, lowered: &lower::Lowered) -> Vec<DefId> {
    lowered
        .fns
        .iter()
        .filter(|f| f.generics.is_empty() && f.using.is_empty())
        .map(|f| f.def)
        .collect()
}

/// An identifier made safe for a host whose reserved words are `reserved`: a reserved word
/// gets a trailing `_`.
pub fn safe(name: &str, reserved: &[&str]) -> String {
    if reserved.contains(&name) {
        format!("{name}_")
    } else {
        name.to_string()
    }
}

/// `snake_case` to `PascalCase`.
pub fn pascal(name: &str) -> String {
    let mut out = String::new();
    let mut up = true;
    for c in name.chars() {
        if c == '_' {
            up = true;
        } else if up {
            out.extend(c.to_uppercase());
            up = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// `snake_case` to `camelCase`.
pub fn camel(name: &str) -> String {
    let p = pascal(name);
    let mut chars = p.chars();
    match chars.next() {
        Some(c) => c.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}
