//! cpl-emit-c: C emission from the checked, typed HIR and every native build of CPL (`spec/toolchain.md` 1.13, T69;
//! `spec/cabi.md` Sections 1, 3, 5, 6, 9).
//!
//! It turns a checked [`Program`] and its interop plan ([`InteropPlan`], toolchain 1.6) into a C package and builds
//! it: the lowering and printing of `spec/cabi.md` Section 1, the tables of reported facts computed with the
//! evaluator's own helpers (cabi.md Section 3, toolchain 1.7.4), the package and adapter headers and the
//! declarative-binding wrappers (cabi.md Sections 5 and 6), the version script, compiler discovery, compilation and
//! linking, the caches, `build.json`, the drivers of the native engine, and the lock's C entries (toolchain 11.2). It
//! never calls the checker's passes or the `Interpreter` (toolchain 1.2), and every process it runs goes through
//! `std::process::Command` with the scrubbed environment of T72.
//!
//! | Module | Contents | Owner |
//! | - | - | - |
//! | crate root | [`emit`], [`EmitConfig`], [`EmittedFile`], [`EmittedPackage`], [`TablesModel`] | emitter (track C) |
//! | `lower`, `print`, `tables`, `header`, `api`, `map`, `cbind` (internal) | A-normal form and the lowering table, the printer of the slot discipline, the program tables, the package and adapter headers, the API unit, the value mapping of exports, the declarative-binding wrappers | emitter (track C) |
//! | [`cc`], [`package`], [`codes`], `cache` and `lockgen` (internal) | compiler discovery, building packages and drivers, the `build_*` codes, the object and driver caches, the lock's C entries | build (track D) |
//! | [`pending`] | stand-ins for items other tracks have not yet added to the crates this one depends on | contracts (track 0), deleted when unused |

pub mod cc;
pub mod codes;
pub mod package;
#[doc(hidden)]
pub mod pending;
pub mod python;

mod api;
mod cache;
mod cbind;
mod emit;
mod header;
mod lockgen;
mod lower;
mod map;
mod optimize;
mod print;
mod tables;
mod wheel;

use cpl_diag::json::Digest;
use cpl_diag::Diagnostic;
use cpl_hir::Program;
use cpl_project::Project;

/// The C surface of a checked program (toolchain 1.6, T80): `cpl_sema::interop::InteropPlan`, re-exported so that
/// `cpl-cabi`, which depends on `cpl-sema` only as a dev-dependency (toolchain 1.2), can name it.
pub use cpl_sema::interop::InteropPlan;
/// Every HIR variant the coverage counters of a coverage driver name (cabi.md 1.4; the coverage gate, toolchain 12.6).
pub use lower::COVERAGE_KINDS;

/// What one emission produces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmitConfig {
    /// A driver's units (`-DCPL_WITH_TESTS=1`): test bodies, range bounds, entry trampolines, adapters.
    pub with_tests: bool,
    /// `-DCPLRT_COVERAGE=1` counters, for the coverage gate (toolchain 12.6).
    pub coverage: bool,
    /// Fault-preserving scalar, control-flow, and ownership optimizations; disabled for coverage.
    pub optimize: bool,
    /// The library information records `release = 1`.
    pub release: bool,
    pub runtime: cpl_rt::RuntimeSource,
    /// The probed compiler's target (`-dumpmachine`), recorded in the library information (cabi.md 4.8); empty when
    /// the caller has not probed a compiler.
    pub target: String,
    /// The compiler as the library information names it (`gcc 11.4.0`).
    pub compiler: String,
    /// `optimized` or `debug`.
    pub profile: String,
}

impl Default for EmitConfig {
    fn default() -> Self {
        EmitConfig {
            with_tests: false,
            coverage: false,
            optimize: false,
            release: false,
            runtime: cpl_rt::RuntimeSource::Embedded,
            target: String::new(),
            compiler: String::new(),
            profile: "debug".to_string(),
        }
    }
}

/// One emitted file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmittedFile {
    /// Relative to the output root: `"include/reader.h"`, `"src/reader_prog.c"`.
    pub path: String,
    pub text: String,
}

/// The files of one package or driver, before anything is compiled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmittedPackage {
    /// The `include/` and `src/` files of toolchain 7.2's layout, in path order.
    pub files: Vec<EmittedFile>,
    /// The program digest of `spec/cabi.md` 1.6, over the emitted program units (the declarative-binding wrappers
    /// included): `BuildOutput::program_digest`, the driver's key and banner.
    pub program_digest: Digest,
    /// Of the canonical export table (`spec/cabi.md` Section 9): `BuildOutput`, `targets.c.exports_digest`.
    pub exports_digest: Digest,
    /// The unresolved holes the program links (`cpl_library_info.holes`, `BuildOutput::holes`).
    pub holes: u32,
    /// The Rust model of the emitted tables, for the table-fidelity test (toolchain 12).
    pub tables: TablesModel,
}

/// One clause of the table model (`CPL_CL`): (text, `CPLRT_CK_*` kind, site, owner `DefId.0`, operation index, loop
/// binding site).
pub type ClauseRow = (String, u32, u32, u32, Option<u32>, Option<u32>);

/// The Rust model of the emitted program tables: per `DefId`, site, clause, hole, and stub, the facts of
/// `spec/cabi.md` Section 3.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TablesModel {
    /// The number of sites, clauses, binding sites, refinements, and types of the tables.
    pub sites: usize,
    pub clauses: usize,
    pub bsites: usize,
    pub refinements: usize,
    pub types: Vec<String>,
    /// Every site (`CPL_S`): its span.
    pub site_spans: Vec<cpl_diag::Span>,
    /// Every clause (`CPL_CL`).
    pub clause_rows: Vec<ClauseRow>,
    /// Every hole by `HoleId`: (id, name, expected type, site).
    pub hole_rows: Vec<(u32, String, String, u32)>,
    /// Every refinement (`CPL_R`): (`DefId.0`, type name, declared name).
    pub ref_rows: Vec<(u32, String, String)>,
}

/// The prefix of the emitted units: the effective C prefix when it is in the prefix domain, else `driver`
/// (`spec/cabi.md` 1.6).
pub fn unit_prefix(plan: &InteropPlan) -> String {
    if cpl_project::is_c_prefix(&plan.prefix) {
        plan.prefix.clone()
    } else {
        "driver".to_string()
    }
}

/// Emit the C package of a checked program. Err only for a program with a definition marked `HasErrors`, which no
/// command passes (T58): one `internal_error`, kind `emit_failed`. Of `project` it reads the manifest (the project
/// name of the banners, the `[interop.c]` keys, and `[run] max_call_depth`, the default limit of a native executable,
/// `spec/cabi.md` Section 8) and the root (the adapter files and vendored sources of the plan's bindings); locations
/// come from the program's source map. A test that builds HIR by hand (`cpl_eval::testing::ProgramBuilder`) passes a
/// synthetic `Project` (`root` None, the manifest of a minimal `cpl.toml`, the program's source map, no files) with
/// `InteropPlan::empty`; [`package::driver`] and `cpl_cabi::NativeSession::start` take the same pair.
pub fn emit(
    p: &Program,
    project: &Project,
    plan: &InteropPlan,
    cfg: &EmitConfig,
) -> Result<EmittedPackage, Vec<Diagnostic>> {
    emit::emit(p, project, plan, cfg)
}
