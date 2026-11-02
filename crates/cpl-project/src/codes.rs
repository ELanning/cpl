//! The diagnostic codes of project files and project loading: every
//! `project`-category code of `spec/toolchain.md` Appendix B, and the three
//! `resolve` codes of `spec/semantics.md` 1.1-1.2 that loading raises
//! (toolchain 3.4). [`CATEGORY`] is the category of the `project` codes;
//! [`UNSUPPORTED_EDITION`], [`RESERVED_MODULE_PATH`], and
//! [`MODULE_FILE_MISPLACED`] have category `resolve`.
//!
//! `project_manifest_invalid` and `project_lock_invalid` are fatal: loading
//! returns them as [`crate::LoadError::Manifest`] and
//! [`crate::LoadError::Lock`], and the command that loads reports them with
//! these codes (exit 2). `project_lock_missing` and `project_lock_stale` are
//! raised by `cpl check --release` (Section 11.2), and the
//! `project_binding_*` codes by [`crate::check_bindings`]; they are listed
//! here so that every producer spells them from one place.
//!
//! From Milestone 2 (T76) [`PROJECT`] also holds every `project_*` code of
//! the C target, whichever crate raises it, so that
//! `cpl_sema::codes::category` and the corpus harness (toolchain 12.2) know
//! it: [`crate::check_c_bindings`] raises [`PROJECT_C_TARGET_NOT_ENABLED`],
//! the seven backend-`c` binding codes, and the syntax half of
//! [`PROJECT_CBIND_INVALID`]; `cpl_sema::interop::plan` raises the
//! `project_export_*` codes (all but [`PROJECT_EXPORT_SKIPPED_GENERIC`],
//! which `cpl build` alone reports from the plan), [`PROJECT_TYPE_NAME_UNKNOWN`],
//! and the semantic half of [`PROJECT_CBIND_INVALID`]. Every one of them is
//! span-less; [`PROJECT_EXPORT_INT_UNUSED`] is a warning and
//! [`PROJECT_EXPORT_SKIPPED_GENERIC`] an info ([`SEVERITIES`]), every other
//! an error.

use cpl_diag::{Category, DiagCode, Severity};

/// The category of the `project_*` codes.
pub const CATEGORY: Category = Category::Project;

/// `cpl.toml` does not parse, or a required key is missing or ill-typed (fatal, exit 2).
pub const PROJECT_MANIFEST_INVALID: DiagCode = DiagCode("project_manifest_invalid");
/// An unknown table or key in `cpl.toml`; a diagnostic, exit 1 (T35).
pub const PROJECT_MANIFEST_UNKNOWN_KEY: DiagCode = DiagCode("project_manifest_unknown_key");
/// A `[build] sources` directory does not exist.
pub const PROJECT_SOURCE_DIR_MISSING: DiagCode = DiagCode("project_source_dir_missing");
/// No `.cpl` files found (warning).
pub const PROJECT_NO_SOURCES: DiagCode = DiagCode("project_no_sources");
/// Overlapping `[build] sources` directories make a file reachable twice.
pub const PROJECT_DUPLICATE_FILE_MODULE: DiagCode = DiagCode("project_duplicate_file_module");
/// `cpl.lock` does not parse (fatal, exit 2).
pub const PROJECT_LOCK_INVALID: DiagCode = DiagCode("project_lock_invalid");
/// Under `--release`: a present lock differs from the tree or the manifest (T37), or,
/// when the lock has a C entry, from the digests of the C build inputs (T75).
pub const PROJECT_LOCK_STALE: DiagCode = DiagCode("project_lock_stale");
/// Under `--release`: no `cpl.lock` (warning, T37).
pub const PROJECT_LOCK_MISSING: DiagCode = DiagCode("project_lock_missing");
/// The lock was written by another toolchain version or edition (warning).
pub const PROJECT_LOCK_TOOLCHAIN_MISMATCH: DiagCode = DiagCode("project_lock_toolchain_mismatch");
/// `[bindings.*] capability` does not resolve to a `pub capability`.
pub const PROJECT_BINDING_UNKNOWN_CAPABILITY: DiagCode =
    DiagCode("project_binding_unknown_capability");
/// A binding's backend is not in `[interop] targets`.
pub const PROJECT_BINDING_TARGET_NOT_ENABLED: DiagCode =
    DiagCode("project_binding_target_not_enabled");
/// A binding's backend is not one of `python`, `node`, `c`.
pub const PROJECT_BINDING_BAD_BACKEND: DiagCode = DiagCode("project_binding_bad_backend");
/// A binding's isolation is not `worker` or `in_process`.
pub const PROJECT_BINDING_BAD_ISOLATION: DiagCode = DiagCode("project_binding_bad_isolation");

// ---- Milestone 2: the C target (toolchain 11.1, T74, T76) --------------------

/// `[interop.c]` present while none of `"c"`, `"python"`, `"node"` is in `[interop] targets`; `key`, `targets`.
pub const PROJECT_C_TARGET_NOT_ENABLED: DiagCode = DiagCode("project_c_target_not_enabled");
/// A backend-`c` binding whose name does not match `^[a-z][a-z0-9_]*$`; `binding`.
pub const PROJECT_BINDING_NAME_INVALID: DiagCode = DiagCode("project_binding_name_invalid");
/// Milestone 2 only: a backend-`c` binding whose `isolation` is absent or not
/// `in_process`. Not raised from Milestone 3, where a C adapter may run in a
/// worker (`spec/python.md` 6.6); kept so that the code table stays stable.
pub const PROJECT_BINDING_ISOLATION_UNSUPPORTED: DiagCode =
    DiagCode("project_binding_isolation_unsupported");
/// A backend-`c` `adapter` that ends neither in `.c` nor in `.cbind.toml`; `binding`, `found`.
pub const PROJECT_BINDING_ADAPTER_KIND: DiagCode = DiagCode("project_binding_adapter_kind");
/// A backend-`c` `adapter` that is no project-relative path of a regular file; `binding`, `path`.
pub const PROJECT_BINDING_ADAPTER_MISSING: DiagCode = DiagCode("project_binding_adapter_missing");
/// A `[bindings.<name>.c]` source or directory that does not exist; `binding`, `key`, `path`.
pub const PROJECT_BINDING_PATH_MISSING: DiagCode = DiagCode("project_binding_path_missing");
/// A `[bindings.<name>.c]` subtable on a binding whose backend is not `c`; `binding`.
pub const PROJECT_BINDING_C_OPTIONS_MISPLACED: DiagCode =
    DiagCode("project_binding_c_options_misplaced");
/// Two backend-`c` bindings of one capability; `capability`, `bindings`.
pub const PROJECT_BINDING_DUPLICATE_CAPABILITY: DiagCode =
    DiagCode("project_binding_duplicate_capability");
/// A `.cbind.toml` outside the closed schema of `spec/cabi.md` Section 6; `binding`, `key`, `line`, `reason`.
pub const PROJECT_CBIND_INVALID: DiagCode = DiagCode("project_cbind_invalid");
/// An `[interop.c] exports` target that names nothing; `entry`, `index`.
pub const PROJECT_EXPORT_UNKNOWN: DiagCode = DiagCode("project_export_unknown");
/// An `[interop.c] exports` target that names several definitions; `entry`, `candidates`.
pub const PROJECT_EXPORT_AMBIGUOUS: DiagCode = DiagCode("project_export_ambiguous");
/// An export target that is no function, or a generator; `entry`, `kind`.
pub const PROJECT_EXPORT_NOT_FUNCTION: DiagCode = DiagCode("project_export_not_function");
/// An export target that is private; `entry`.
pub const PROJECT_EXPORT_NOT_PUBLIC: DiagCode = DiagCode("project_export_not_public");
/// An export target that is a standard-library declaration; `entry`.
pub const PROJECT_EXPORT_NOT_PROJECT: DiagCode = DiagCode("project_export_not_project");
/// A generic export target; `entry`, with a note naming the project's monomorphic callers.
pub const PROJECT_EXPORT_GENERIC: DiagCode = DiagCode("project_export_generic");
/// An export symbol outside its pattern or in a reserved family; `symbol`, `reason`.
/// The C symbol travels as the payload key `c_symbol`: `symbol` is a reserved
/// key of the diagnostic object (T5; see `cpl_sema::interop::SYMBOL_KEY`).
pub const PROJECT_EXPORT_SYMBOL_INVALID: DiagCode = DiagCode("project_export_symbol_invalid");
/// Two generated identifiers coincide (`spec/cabi.md` 5.2); `symbol` (as the
/// payload key `c_symbol`, T5), `items`.
pub const PROJECT_EXPORT_SYMBOL_COLLISION: DiagCode = DiagCode("project_export_symbol_collision");
/// `int` given on an entry whose function has no top-level `Int` (warning); `entry`.
pub const PROJECT_EXPORT_INT_UNUSED: DiagCode = DiagCode("project_export_int_unused");
/// A `type_names` key that names no type reachable from the exports; `key`.
pub const PROJECT_TYPE_NAME_UNKNOWN: DiagCode = DiagCode("project_type_name_unknown");
/// The default export set left out a generic `pub fn` (info, `cpl build` only); `function`.
pub const PROJECT_EXPORT_SKIPPED_GENERIC: DiagCode = DiagCode("project_export_skipped_generic");

// ---- Milestone 3: the Python target and fixtures (spec/python.md, toolchain 11.1) ----

/// `[interop.python]` present while `"python"` is not in `[interop] targets`; `key`, `targets`.
pub const PROJECT_PYTHON_TARGET_NOT_ENABLED: DiagCode =
    DiagCode("project_python_target_not_enabled");
/// `[interop.python] package` or `version` outside its domain; `key`, `found`, `reason`.
pub const PROJECT_PYTHON_OPTION_INVALID: DiagCode = DiagCode("project_python_option_invalid");
/// A backend-`python` `adapter` that is not a dotted path of Python identifiers; `binding`, `found`.
pub const PROJECT_BINDING_ADAPTER_INVALID: DiagCode = DiagCode("project_binding_adapter_invalid");
/// A non-positive `timeout_ms`, or one on a backend-`c` binding in process; `binding`, `found`.
pub const PROJECT_BINDING_TIMEOUT_INVALID: DiagCode = DiagCode("project_binding_timeout_invalid");
/// A `[test.fixtures]` value that names no binding; `capability`, `binding`.
pub const PROJECT_FIXTURE_BINDING_UNKNOWN: DiagCode = DiagCode("project_fixture_binding_unknown");
/// A `[test.fixtures]` binding whose capability is not the key; `capability`, `binding`, `found`.
pub const PROJECT_FIXTURE_CAPABILITY_MISMATCH: DiagCode =
    DiagCode("project_fixture_capability_mismatch");

/// `[project] edition` other than `"0.1"` (resolve, semantics 1.1).
pub const UNSUPPORTED_EDITION: DiagCode = DiagCode("unsupported_edition");
/// A project module whose path begins with `std` or `gen` (resolve, semantics 1.1).
pub const RESERVED_MODULE_PATH: DiagCode = DiagCode("reserved_module_path");
/// A module file outside its conventional location (resolve, warning, semantics 1.2).
pub const MODULE_FILE_MISPLACED: DiagCode = DiagCode("module_file_misplaced");

/// The `project` codes of Appendix B, in its order: the 13 of Milestone 1,
/// then the 20 of the C target (T76), then the 6 of the Python target and
/// fixtures (Milestone 3).
pub const PROJECT: &[DiagCode] = &[
    PROJECT_MANIFEST_INVALID,
    PROJECT_MANIFEST_UNKNOWN_KEY,
    PROJECT_SOURCE_DIR_MISSING,
    PROJECT_NO_SOURCES,
    PROJECT_DUPLICATE_FILE_MODULE,
    PROJECT_LOCK_INVALID,
    PROJECT_LOCK_STALE,
    PROJECT_LOCK_MISSING,
    PROJECT_LOCK_TOOLCHAIN_MISMATCH,
    PROJECT_BINDING_UNKNOWN_CAPABILITY,
    PROJECT_BINDING_TARGET_NOT_ENABLED,
    PROJECT_BINDING_BAD_BACKEND,
    PROJECT_BINDING_BAD_ISOLATION,
    PROJECT_C_TARGET_NOT_ENABLED,
    PROJECT_BINDING_NAME_INVALID,
    PROJECT_BINDING_ISOLATION_UNSUPPORTED,
    PROJECT_BINDING_ADAPTER_KIND,
    PROJECT_BINDING_ADAPTER_MISSING,
    PROJECT_BINDING_PATH_MISSING,
    PROJECT_BINDING_C_OPTIONS_MISPLACED,
    PROJECT_BINDING_DUPLICATE_CAPABILITY,
    PROJECT_CBIND_INVALID,
    PROJECT_EXPORT_UNKNOWN,
    PROJECT_EXPORT_AMBIGUOUS,
    PROJECT_EXPORT_NOT_FUNCTION,
    PROJECT_EXPORT_NOT_PUBLIC,
    PROJECT_EXPORT_NOT_PROJECT,
    PROJECT_EXPORT_GENERIC,
    PROJECT_EXPORT_SYMBOL_INVALID,
    PROJECT_EXPORT_SYMBOL_COLLISION,
    PROJECT_EXPORT_INT_UNUSED,
    PROJECT_TYPE_NAME_UNKNOWN,
    PROJECT_EXPORT_SKIPPED_GENERIC,
    PROJECT_PYTHON_TARGET_NOT_ENABLED,
    PROJECT_PYTHON_OPTION_INVALID,
    PROJECT_BINDING_ADAPTER_INVALID,
    PROJECT_BINDING_TIMEOUT_INVALID,
    PROJECT_FIXTURE_BINDING_UNKNOWN,
    PROJECT_FIXTURE_CAPABILITY_MISMATCH,
];

/// The `project` codes whose severity is not error (Appendix B): the
/// warnings and the info. Every other code of [`PROJECT`] is an error.
pub const SEVERITIES: &[(DiagCode, Severity)] = &[
    (PROJECT_NO_SOURCES, Severity::Warning),
    (PROJECT_LOCK_MISSING, Severity::Warning),
    (PROJECT_LOCK_TOOLCHAIN_MISMATCH, Severity::Warning),
    (PROJECT_EXPORT_INT_UNUSED, Severity::Warning),
    (PROJECT_EXPORT_SKIPPED_GENERIC, Severity::Info),
];

/// The severity of a `project` code (Appendix B): [`SEVERITIES`], else error.
pub fn severity(code: DiagCode) -> Severity {
    SEVERITIES
        .iter()
        .find(|(c, _)| *c == code)
        .map_or(Severity::Error, |(_, s)| *s)
}
