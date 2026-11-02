//! cpl-sema: the checker: the module graph, name resolution, types,
//! effects, contract well-formedness, holes, and the typed HIR
//! (`spec/toolchain.md` 1.6 and 3; the rules are `spec/semantics.md`).
//!
//! * [`project`]: finding and loading a project (`cpl.toml`, `cpl.lock`,
//!   the source set, std), Sections 1.6 and 11. It is the `cpl-project`
//!   crate, re-exported (`pub use cpl_project as project;`) so that every
//!   `cpl_sema::project` path names the `cpl-project` item.
//! * [`mod@check`]: checking a loaded project into a [`hir::Program`]
//!   ([`fn@check`], [`check_with`]), hole reports, verification statuses, and
//!   selector resolution ([`resolve_selector`]).
//! * [`hir`]: the typed HIR of Section 3.3 (the `cpl-hir` crate).
//! * [`codes`]: every diagnostic code the checker emits.
//! * [`interop`] (Milestone 2, T74, T80): the plan of the program's C
//!   surface ([`interop::InteropPlan`], [`interop::plan`],
//!   [`interop::type_text`]), which [`CheckResult::interop`] carries when
//!   `"c"` is in `[interop] targets`.
//!
//! # Internal layout
//!
//! | Module | Stage (toolchain 3.1) | Specification |
//! | - | - | - |
//! | `checker` | the state threaded through the stages | 3.1 |
//! | `collect` | collect: modules, definitions (T15), identities (4.3-4.5), annotations (1.8), names (1.4, S3, S6), the member table (stdlib S37), the universe | semantics 1.2-1.8, toolchain 3.6-3.7 |
//! | `imports` | resolve: `use` declarations, `unused_import` | semantics 1.6, S5, S62 |
//! | `scope` | module, file, universe, and local scopes | semantics 2.1 |
//! | `resolve` | type paths, chains, constructors; the side tables of bodies | semantics 2.2-2.5 |
//! | `refine` | refinement bases, `cyclic_type_definition`, identity classes | semantics 3.3, S55, S66 |
//! | `sig` | every signature type, the 1.8 rules for signatures, exportability, effect bounds | semantics 1.5, 1.8, 4.1 |
//! | `wf` | the containment graph, `infinite_type` | semantics S7 |
//! | `names` | the resolution walk of bodies, clauses, and predicates | semantics 2.1-2.5, S8-S11, S67 |
//! | `members` | the member table | semantics 3.9 |
//! | `selector` | `.work` selector resolution | toolchain 1.6, T42, T43 |
//! | `types` | type: body typing, the typed HIR, hole reports (its submodules: `infer`, `typer`, `expr`, `stmt`, `pattern`, `holes`) | semantics 3.4-3.14, 8.1-8.3, S47, S48, S64 |
//! | `consteval` | the compile-time proof rule's evaluator, value JSON | semantics 3.5, S16, S40 |
//! | `exhaust` | match exhaustiveness and reachability | semantics 3.13, S25 |
//! | `bodies` | position-aware walks of typed bodies for the later stages | semantics 4.2-4.5, 5.1 |
//! | `ast_walk` | clause-aware walks of declarations' syntax for the later stages | semantics 4.4, 5.1, 5.4 |
//! | `effects` | effect: bounds, forwarding, handle positions, `unused_capability`, `EffectBound::inferred` | semantics 4.1-4.5, S26-S30 |
//! | `contracts` | contract: the rules of contract expressions, `decreases`, verification statuses | semantics 5.1, 5.4, 5.5 |
//! | `policy` | release_policy: `verification_unsatisfied` | semantics 8.2, S36, S47 |
//! | `cbind` | project (Milestone 2): the semantic half of `project_cbind_invalid`; a stub until track F2 | cabi.md 6.3 |

pub use cpl_hir as hir;
pub use cpl_project as project;

pub mod check;
pub mod codes;
pub mod interop;

mod ast_walk;
mod bodies;
mod cbind;
mod checker;
mod collect;
mod consteval;
mod contracts;
mod effects;
mod exhaust;
mod imports;
mod machines;
mod members;
mod names;
mod policy;
mod refine;
mod resolve;
mod scope;
mod selector;
mod sig;
mod types;
mod wf;

pub use check::{
    check, check_with, resolve_selector, CheckConfig, CheckResult, HoleBinding, HoleCapability,
    HoleOperation, HoleReport, ProofStatus, Selected, SelectorScope, VerificationStatus,
};
pub use project::{
    find_project_root, load_project, load_project_from_tree, parse_lock, parse_manifest,
    render_lock, LoadError, LoadOptions, Lock, Manifest, ParsedFile, Project, SourceEntry,
    StdSource, VirtualTree,
};

#[cfg(test)]
mod tests;
