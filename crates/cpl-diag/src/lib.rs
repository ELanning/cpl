//! cpl-diag: the source model, the diagnostic model, and every shared JSON
//! wire type of the CPL toolchain (`spec/toolchain.md` 1.4). It has no
//! CPL-specific logic beyond the facet and identity vocabulary, and it
//! depends on no other crate of the workspace.
//!
//! | Module | Contents |
//! | - | - |
//! | [`source`] | [`FileId`], [`Span`], [`FileOrigin`], [`SourceFile`], [`SourceMap`], [`LineCol`], [`Location`] |
//! | [`diag`] | [`Severity`], [`DiagCode`], [`Category`], [`Diagnostic`] with [`Note`] and [`BacktraceFrame`], [`Diagnostics`]; the human rendering of 1.4.1 |
//! | [`facet`] | [`Facet`], [`FacetSelector`] |
//! | [`id`] | [`StableId`], [`IdError`], [`SymbolPath`], [`DeclName`] (Section 4) |
//! | [`version`] | [`version::TOOLCHAIN`], [`version::EDITION`], [`version::JSON_SCHEMA`] |
//! | [`json`] | every wire type of 1.4.2 and Appendix C |
//!
//! The items of `source`, `diag`, `facet`, and `id` are re-exported at the
//! crate root (`cpl_diag::Span`, `cpl_diag::StableId`, ...); wire types are
//! addressed through [`json`].

pub mod fsutil;
pub mod diag;
pub mod facet;
pub mod id;
pub mod json;
pub mod source;

pub use diag::{
    has_errors, render_all, sort_diagnostics, BacktraceFrame, Category, DiagCode, Diagnostic,
    Diagnostics, Note, Severity, RESERVED_KEYS,
};
pub use facet::{Facet, FacetSelector};
pub use id::{quote_text, DeclName, IdError, StableId, SymbolPath};
pub use source::{FileId, FileOrigin, LineCol, Location, SourceFile, SourceMap, Span};

/// Version strings reported by every command and recorded in every receipt.
pub mod version {
    /// The workspace version, e.g. `"0.1.0"` (Section 1.1).
    pub const TOOLCHAIN: &str = env!("CARGO_PKG_VERSION");
    /// The language edition this toolchain implements.
    pub const EDITION: &str = "0.1";
    /// Bumped when any wire type changes incompatibly.
    pub const JSON_SCHEMA: u32 = 1;
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_m2;
