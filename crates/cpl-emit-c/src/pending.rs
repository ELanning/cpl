//! Stand-ins for the Milestone 2 items of other crates that this crate's signatures name before the tracks that own
//! them add them (`spec/toolchain.md` 1.13). The contracts engineer's file: each stand-in names the real item, the
//! track that adds it, and the track whose `use` switches to it; an item nothing uses any more is deleted, and the
//! module with the last one. None has behaviour, and the opaque ones cannot be constructed, so no code can come to
//! depend on a stand-in by accident.

/// Stand-in for `cpl_sema::interop::InteropPlan` (toolchain 1.6, T80), which track E adds; [`crate::InteropPlan`]
/// re-exports it until track C switches the re-export to the real type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InteropPlan {
    _pending: (),
}

/// Stand-in for `cpl_project::ArtifactKind` (toolchain 11.1, T80), which track E adds; `package::BuildConfig` (track
/// D) names it. The variants are the real type's.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ArtifactKind {
    Shared,
    Static,
    Executable,
}

/// Stand-in for `cpl_diag::json::BuildOutput` (toolchain 7.2, Appendix C), which track E adds; `package::Built`
/// (track D) names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildOutput {
    _pending: (),
}
