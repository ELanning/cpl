//! cpl-project: finding and loading a CPL project (`spec/toolchain.md`
//! 1.6, 3.2, 3.6, 3.7, 11): `cpl.toml`, `cpl.lock`, the source set, and the
//! embedded standard library, parsed with `cpl-syntax` into a [`Project`].
//!
//! The checker re-exports this crate as `cpl_sema::project` (`pub use
//! cpl_project as project;`), so every `cpl_sema::project::...` path of the
//! specifications names the item defined here. Project loading is a crate of
//! its own, between `cpl-syntax` and `cpl-sema`, because the index and work
//! crates need it but must not depend on the checker's passes while those
//! are under construction (the reasoning of T50 for `cpl-hir`).
//!
//! | Item | Specification |
//! | - | - |
//! | [`find_project_root`], [`load_project`], [`load_project_from_tree`], [`LoadOptions`], [`StdSource`], [`VirtualTree`], [`Project`], [`SourceEntry`], [`ParsedFile`], [`LoadError`] | 1.6, 3.2 (`FileId` order, T15), 3.7 (conventional locations) |
//! | [`STD_FILES`], [`std_digest`], [`Project::std_digest`] | 3.6 (embedding, override, digest) |
//! | [`Manifest`] and its sections, [`parse_manifest`], [`ManifestError`], [`check_bindings`] | 11.1 |
//! | [`Lock`], [`LockToolchain`], [`parse_lock`], [`render_lock`], [`LockError`] | 11.2 |
//! | [`codes`] | the `project` codes of Appendix B and the `resolve` codes of semantics 1.1-1.2 that loading raises |
//! | Milestone 2: [`CInteropSection`], [`ExportEntry`], [`IntMapping`], [`ArtifactKind`], [`CBindingSection`], [`AdapterForm`], [`Manifest::c_prefix`], [`is_c_prefix`] | 11.1 (T74, T80): `[interop.c]` and `[bindings.<name>.c]` |
//! | Milestone 2: [`check_c_bindings`], [`CBindingCheck`], [`cbind_invalid`] | 11.1: the backend-`c` rules that need no name resolution |
//! | Milestone 2: [`cbind`] ([`cbind::CBindSpec`], [`cbind::parse_cbind`]) | `spec/cabi.md` 6.3: a stub until track F2 (declarative bindings) |
//! | Milestone 2: [`TargetC`], [`LockedExport`], [`AdapterC`], [`ForeignC`], [`LockedLibrary`], the views of [`Lock`], [`c_input_digests`], [`CInputDigests`] | 11.2 (T75, T80): the C entries of the lock |
//! | Milestone 3: [`PythonInteropSection`], [`check_python`], [`python_input_digest`], [`is_python_identifier`], [`is_python_module_path`], [`python_package_problem`], [`python_version_problem`], [`PYTHON_KEYWORDS`], [`PYTHON_STDLIB_MODULES`] | `spec/python.md` 1, 5.3, 9: `[interop.python]`, the binding rules, the adapter digest |
//! | Milestone 3: [`TargetPython`], [`AdapterPython`], [`Lock::target_python`], [`Lock::python_adapters`] and their setters | `spec/python.md` 9: the Python entries of the lock |
//!
//! What loading reports (in [`Project::diagnostics`], or as a fatal
//! [`LoadError`]):
//!
//! | Code | Severity | Trigger |
//! | - | - | - |
//! | `project_manifest_invalid` | fatal ([`LoadError::Manifest`]) | `cpl.toml` is not UTF-8 or not TOML, or a required key is missing, or a key has the wrong type or a value outside its domain |
//! | `project_manifest_unknown_key` | error | a table or key 11.1 does not define (T35) |
//! | `unsupported_edition` | error (resolve) | `[project] edition` other than `"0.1"` |
//! | `project_source_dir_missing` | error | a `[build] sources` directory that does not exist |
//! | `project_duplicate_file_module` | error | a `.cpl` file reachable from two overlapping `[build] sources` directories |
//! | `project_no_sources` | warning | no `.cpl` file under the source directories |
//! | `project_lock_invalid` | fatal ([`LoadError::Lock`]) | `cpl.lock` does not parse |
//! | `project_lock_toolchain_mismatch` | warning | `cpl.lock` was written by another toolchain version or edition |
//! | parse codes (`invalid_utf8`, `syntax_error`, ...) | error | a source file that is not UTF-8 or does not lex and parse (one diagnostic per file, D59, D70) |
//! | `reserved_module_path` | error (resolve) | a project module whose path starts with `std` or `gen` (semantics 1.1) |
//! | `module_file_misplaced` | warning (resolve) | a project module outside its conventional location (3.7, semantics 1.2) |
//!
//! `project_lock_missing` and `project_lock_stale` belong to `cpl check
//! --release` (11.2, T37), and the four `project_binding_*` codes to the
//! binding checks that run after name resolution ([`check_bindings`],
//! 11.1); loading raises none of them. Nor does it raise any code of the C
//! target: the check driver reports them, through [`check_c_bindings`] and
//! `cpl_sema::interop::plan` (11.1, T74); loading reports only a C value
//! outside its domain, as the fatal `project_manifest_invalid` (T53).

mod c_bindings;
pub mod cbind;
pub mod codes;
mod load;
mod lock;
mod manifest;
mod python;
mod stdlib;
mod util;

pub use c_bindings::{
    c_input_digests, cbind_invalid, check_c_bindings, CBindingCheck, CInputDigests,
};

pub use load::{
    find_project_root, load_project, load_project_from_tree, LoadError, LoadOptions, ParsedFile,
    Project, SourceEntry, StdSource, VirtualTree,
};
pub use lock::{
    parse_lock, render_lock, AdapterC, AdapterPython, ForeignC, Lock, LockError, LockToolchain,
    LockedExport, LockedLibrary, TargetC, TargetPython,
};
pub use manifest::{
    check_bindings, is_c_prefix, parse_manifest, AdapterForm, ArtifactKind, BindingSection,
    BuildSection, CBindingSection, CInteropSection, EvidenceLocation, ExportEntry, IntMapping,
    InteropSection, Manifest, ManifestError, ProjectSection, PythonInteropSection, RunSection,
    TestSection, VerificationSection, WorkPolicySection, WorkSection, SYSTEM_HEADERS,
};
pub use python::{
    check_python, is_python_identifier, is_python_module_path, python_input_digest,
    python_package_problem, python_version_problem, PYTHON_KEYWORDS, PYTHON_STDLIB_MODULES,
};
pub use stdlib::{std_digest, STD_FILES};
