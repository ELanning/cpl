//! Type helpers shared by the encodings, the builtins, and the interpreter
//! (semantics 3.3; `spec/toolchain.md` T30; stdlib S44). They are pure
//! functions of the program, and `cpl-emit-c` computes the names the native
//! side reports with them (`spec/cabi.md` 3.2, toolchain 1.7.4).

use cpl_diag::{DeclName, StableId};
use cpl_hir::{DefId, Program, Ty};

/// The module path of the prelude, `std.prelude`.
pub const PRELUDE: [&str; 2] = ["std", "prelude"];

/// Refinement chains deeper than this are treated as malformed (the checker
/// rejects cyclic ones, `cyclic_type_definition`), so no helper here loops
/// forever on a broken program.
pub const MAX_CHAIN: usize = 256;

/// True when `def` is declared in `std.prelude`.
pub fn is_prelude(p: &Program, def: DefId) -> bool {
    p.defs
        .get(def.0 as usize)
        .and_then(|d| p.modules.get(d.module.0 as usize))
        .is_some_and(|m| m.path.iter().map(String::as_str).eq(PRELUDE))
}

/// The one naming rule of `$type` (toolchain T30, semantics S40) and of
/// `ConstraintError.type_name` (stdlib S44): a prelude declaration renders
/// as its bare name (`Option`, `Result`, `Unit`, `ConstraintError`), every
/// other declaration as its qualified name (`reader.pages.PageId`,
/// `std.foreign.ForeignError`, `std.gen.Positive`). An id the program does
/// not have renders as `{def N}`.
pub fn type_name(p: &Program, def: DefId) -> String {
    match p.defs.get(def.0 as usize) {
        None => format!("{{def {}}}", def.0),
        Some(d) if is_prelude(p, def) => d.name.display(),
        Some(_) => p.display_path(def),
    }
}

/// The qualified identity of operation `op` of `capability`
/// (`reader.pages.PageRead.get`, semantics 1.4): the `symbol` of an
/// operation's clause faults and of the faults and details of semantics S77
/// and S80 (`operation`), which the native tables carry as computed here
/// (`spec/toolchain.md` 1.7.4, T77; `spec/cabi.md` 3.2). Its `as_str` is the
/// operation's qualified name. Panics when `capability` is not a definition
/// of `p`.
pub fn operation_identity(p: &Program, capability: DefId, op: &str) -> StableId {
    let def = p.def(capability);
    StableId::provisional(
        &p.module(def.module).path.join("."),
        &DeclName::Member {
            receiver: def.name.display(),
            name: op.to_string(),
        },
    )
}

/// The bare declared name of `def` (`Page`, `PageId`, `Option`), as value
/// text writes struct and wrapper names (semantics S40).
pub fn bare_name(p: &Program, def: DefId) -> String {
    match p.defs.get(def.0 as usize) {
        Some(d) => d.name.display(),
        None => format!("{{def {}}}", def.0),
    }
}

/// `base(R)` (semantics 3.3): the declared base type of the refinement
/// declaration `def`; `None` when `def` is not a refinement.
pub fn refinement_base(p: &Program, def: DefId) -> Option<&Ty> {
    p.defs
        .get(def.0 as usize)
        .and_then(|d| d.kind.as_refinement())
        .map(|r| &r.base)
}

/// `strip(T)` (semantics 3.3): `T` itself when it is not a refinement, else
/// the first non-refinement of its declared base chain (the root base).
/// `Ty::Error` for a chain that does not end (a malformed program).
pub fn strip(p: &Program, ty: &Ty) -> Ty {
    let mut current = ty;
    for _ in 0..MAX_CHAIN {
        match current {
            Ty::Refined(_, def) => match refinement_base(p, *def) {
                Some(base) => current = base,
                None => return Ty::Error,
            },
            other => return other.clone(),
        }
    }
    Ty::Error
}

/// `chain(R)` (semantics 3.3) for the refinement declaration `refined`:
/// `[R, base(R), ...]` down to, but excluding, the root base, each element
/// the declaration *as named* at the previous level (the `DefId` of
/// `Ty::Refined`, S55). Empty when `refined` is not a refinement.
pub fn chain(p: &Program, refined: DefId) -> Vec<DefId> {
    let mut out = Vec::new();
    let mut current = Some(refined);
    while let Some(def) = current {
        if out.len() >= MAX_CHAIN || out.contains(&def) {
            break;
        }
        let Some(base) = refinement_base(p, def) else {
            break;
        };
        out.push(def);
        current = match base {
            Ty::Refined(_, next) => Some(*next),
            _ => None,
        };
    }
    out
}

/// Substitute generic arguments: every `Ty::Param(i)` of `ty` becomes
/// `args[i]` (a parameter without an argument is kept). Used for the field
/// types of a generic struct and the payload types of a generic enum.
pub fn instantiate(ty: &Ty, args: &[Ty]) -> Ty {
    if args.is_empty() {
        return ty.clone();
    }
    match ty {
        Ty::Param(i) => args.get(*i as usize).cloned().unwrap_or(Ty::Param(*i)),
        Ty::List(t) => Ty::List(Box::new(instantiate(t, args))),
        Ty::Gen(t) => Ty::Gen(Box::new(instantiate(t, args))),
        Ty::Adt(def, targs) => Ty::Adt(*def, targs.iter().map(|t| instantiate(t, args)).collect()),
        other => other.clone(),
    }
}
