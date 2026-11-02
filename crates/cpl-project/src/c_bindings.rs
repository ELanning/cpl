//! The C target's checks that need no name resolution (`spec/toolchain.md`
//! 11.1, T74, T80): [`check_c_bindings`], which the check driver of
//! `cpl-sema` runs after [`crate::check_bindings`], and the digests of the
//! backend-`c` bindings' build inputs ([`c_input_digests`], Section 11.2),
//! which the release lock check compares with `cpl.lock` and the build
//! records in it.
//!
//! [`crate::check_bindings`] (T48) is unchanged and still accepts
//! `backend = "c"` with any `adapter`: the backend-`c` rules live here
//! (T80). Every diagnostic is a span-less error of category `project`.

use crate::cbind::{self, CBindError, CBindSpec};
use crate::codes;
use crate::manifest::{is_relative_path, AdapterForm, CBindingSection, Manifest};
use crate::stdlib::digest_bytes;
use crate::util::join_key;
use cpl_diag::json::Digest;
use cpl_diag::{DiagCode, Diagnostic};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::Path;

/// What [`check_c_bindings`] found: its diagnostics, and the declarative
/// bindings whose `.cbind.toml` files it read and parsed, by binding name
/// (for `cpl_sema::interop::plan`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CBindingCheck {
    pub diagnostics: Vec<Diagnostic>,
    pub cbind: BTreeMap<String, CBindSpec>,
}

/// The rules of the C-target table of toolchain 11.1 that need no name
/// resolution, in this order:
///
/// 1. `project_c_target_not_enabled` (`key` `"interop.c"`, `targets`) when
///    `[interop.c]` is present while none of `"c"`, `"python"`, and `"node"`
///    (whose packages contain the C package) is in `[interop] targets`.
/// 2. Per binding, in name order: for a backend-`c` binding,
///    `project_binding_name_invalid` (`binding`) when its name does not
///    match `^[a-z][a-z0-9_]*$` (from Milestone 3 either `isolation` is
///    legal, so `project_binding_isolation_unsupported` is no longer raised,
///    `spec/python.md` 6.6);
///    `project_binding_adapter_kind` (`binding`, `found`) when `adapter`
///    ends neither in `.c` nor in `.cbind.toml`, else
///    `project_binding_adapter_missing` (`binding`, `path`) when it is
///    absolute or has a `..` segment or, with a root, names no regular file
///    under it (a link to a file counts as that file, T54), else, for a
///    `.cbind.toml` read from the root, its syntax (`project_cbind_invalid`
///    with `binding`, `key`, `line` when known, `reason`; the file is
///    parsed by [`cbind::parse_cbind`]); then, with a root,
///    `project_binding_path_missing` (`binding`, the dotted `key`, `path`)
///    for each `[bindings.<name>.c]` `sources` entry that is no file and
///    each `include_dirs` and `library_dirs` entry that is no directory.
///    For a binding of another backend, `project_binding_c_options_misplaced`
///    (`binding`) when it has a `[bindings.<name>.c]` subtable.
/// 3. `project_binding_duplicate_capability` (`capability`, `bindings` in
///    name order) once per capability that two or more backend-`c` bindings
///    bind, in capability order (the capability as written: these checks
///    have no name resolution, and `check_bindings` already requires a
///    qualified path).
///
/// With `root` None (a `VirtualTree`) the file checks are skipped and
/// `cbind` is empty; the path rules that need no file still apply.
pub fn check_c_bindings(manifest: &Manifest, root: Option<&Path>) -> CBindingCheck {
    let mut out = CBindingCheck::default();
    // the Python and Node packages contain the C package (spec/python.md 1)
    let c_enabled = manifest
        .interop
        .targets
        .iter()
        .any(|t| t == "c" || t == "python" || t == "node");
    if manifest.interop_c.is_some() && !c_enabled {
        out.diagnostics.push(
            error(
                codes::PROJECT_C_TARGET_NOT_ENABLED,
                "`[interop.c]` is present but none of \"c\", \"python\", and \"node\" is in `[interop] targets`".to_string(),
            )
            .with_data("key", json!("interop.c"))
            .with_data("targets", json!(manifest.interop.targets)),
        );
    }
    let mut by_capability: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (name, binding) in &manifest.bindings {
        if binding.backend != "c" {
            if manifest.binding_c.contains_key(name) {
                out.diagnostics.push(
                    error(
                        codes::PROJECT_BINDING_C_OPTIONS_MISPLACED,
                        format!(
                            "binding `{name}`: `[bindings.{name}.c]` applies to backend \"c\" only, but the backend is {:?}",
                            binding.backend
                        ),
                    )
                    .with_data("binding", json!(name)),
                );
            }
            continue;
        }
        by_capability
            .entry(binding.capability.as_str())
            .or_default()
            .push(name.as_str());
        if !is_binding_name(name) {
            out.diagnostics.push(
                error(
                    codes::PROJECT_BINDING_NAME_INVALID,
                    format!(
                        "binding `{name}`: a backend-c binding's name must match ^[a-z][a-z0-9_]*$, so that its grant `{}_bind_{name}` is a C identifier",
                        manifest.c_prefix()
                    ),
                )
                .with_data("binding", json!(name)),
            );
        }
        check_adapter(&mut out, name, &binding.adapter, manifest.c_prefix(), root);
        if let (Some(root), Some(section)) = (root, manifest.binding_c.get(name)) {
            check_paths(&mut out.diagnostics, name, section, root);
        }
    }
    for (capability, bindings) in by_capability {
        if bindings.len() > 1 {
            out.diagnostics.push(
                error(
                    codes::PROJECT_BINDING_DUPLICATE_CAPABILITY,
                    format!(
                        "capability `{capability}` has {} backend-c bindings ({}); at most one may bind it",
                        bindings.len(),
                        bindings.join(", ")
                    ),
                )
                .with_data("capability", json!(capability))
                .with_data("bindings", json!(bindings)),
            );
        }
    }
    out
}

/// The `adapter` key of a backend-`c` binding: its kind, its file, and a
/// declarative binding's syntax.
fn check_adapter(
    out: &mut CBindingCheck,
    name: &str,
    adapter: &str,
    prefix: &str,
    root: Option<&Path>,
) {
    let Some(form) = AdapterForm::of_adapter(adapter) else {
        out.diagnostics.push(
            error(
                codes::PROJECT_BINDING_ADAPTER_KIND,
                format!(
                    "binding `{name}`: a backend-c adapter is a `.c` reviewed wrapper or a `.cbind.toml` declarative binding, found {adapter:?}"
                ),
            )
            .with_data("binding", json!(name))
            .with_data("found", json!(adapter)),
        );
        return;
    };
    let missing = |why: &str| {
        error(
            codes::PROJECT_BINDING_ADAPTER_MISSING,
            format!("binding `{name}`: adapter {adapter:?} {why}"),
        )
        .with_data("binding", json!(name))
        .with_data("path", json!(adapter))
    };
    if !is_relative_path(adapter) {
        out.diagnostics.push(missing(
            "is not a project-relative path (it is absolute or has a `..` segment)",
        ));
        return;
    }
    let Some(root) = root else {
        return;
    };
    let path = root.join(adapter);
    if !names_project_file(root, adapter) {
        out.diagnostics
            .push(missing("names no regular file under the project root"));
        return;
    }
    if form == AdapterForm::Declarative {
        let parsed = match std::fs::read(&path) {
            Err(e) => Err(CBindError {
                key: String::new(),
                line: None,
                reason: format!("the file cannot be read: {e}"),
            }),
            Ok(bytes) => match String::from_utf8(bytes) {
                Err(e) => Err(CBindError {
                    key: String::new(),
                    line: None,
                    reason: format!(
                        "the file is not valid UTF-8 (at byte {})",
                        e.utf8_error().valid_up_to()
                    ),
                }),
                Ok(text) => cbind::parse_cbind(&text),
            },
        };
        match parsed.and_then(|spec| shadowed_header(&spec, prefix, &text_of(&path)).map(|()| spec))
        {
            Ok(spec) => {
                out.cbind.insert(name.to_string(), spec);
            }
            Err(e) => out.diagnostics.push(cbind_invalid(name, &e)),
        }
    }
}

fn text_of(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// A `[library] headers` entry that no include_dirs can provide, because the generated wrapper's own directory or the
/// output's `include` directory, searched first, holds a file of that name: the runtime's and the API's headers
/// (`cpl.h`, `cpl_*.h`), the package header `<prefix>.h`, and the adapter headers `<prefix>_adapter_*.h` (cabi.md 1.5,
/// 6.3: the headers are "found through the binding's include_dirs").
fn shadowed_header(spec: &cbind::CBindSpec, prefix: &str, text: &str) -> Result<(), CBindError> {
    for h in &spec.headers {
        let generated = h == "cpl.h"
            || (h.starts_with("cpl_") && h.ends_with(".h"))
            || *h == format!("{prefix}.h")
            || (h.starts_with(&format!("{prefix}_adapter_")) && h.ends_with(".h"));
        if generated {
            let line = text
                .lines()
                .position(|l| l.trim_start().starts_with("headers"))
                .map(|i| i as u32 + 1);
            return Err(CBindError {
                key: "library.headers".to_string(),
                line,
                reason: format!(
                    "`{h}` is the name of a header the build generates, which the include_dirs cannot provide"
                ),
            });
        }
    }
    Ok(())
}

/// `project_cbind_invalid` for a [`CBindError`] of binding `binding`:
/// `binding`, `key`, `line` (omitted when unknown, as `project_manifest_invalid`'s),
/// and `reason`.
pub fn cbind_invalid(binding: &str, e: &CBindError) -> Diagnostic {
    let mut d = error(
        codes::PROJECT_CBIND_INVALID,
        format!(
            "binding `{binding}`: the declarative binding is outside the schema of spec/cabi.md 6.3{}: {}",
            if e.key.is_empty() {
                String::new()
            } else {
                format!(" at `{}`", e.key)
            },
            e.reason
        ),
    )
    .with_data("binding", json!(binding))
    .with_data("key", json!(e.key));
    if let Some(line) = e.line {
        d = d.with_data("line", json!(line));
    }
    d.with_data("reason", json!(e.reason))
}

/// `project_binding_path_missing` for the `[bindings.<name>.c]` paths.
fn check_paths(out: &mut Vec<Diagnostic>, name: &str, section: &CBindingSection, root: &Path) {
    let table = join_key(&join_key("bindings", name), "c");
    let groups: [(&str, &[String], bool); 3] = [
        ("sources", &section.sources, true),
        ("include_dirs", &section.include_dirs, false),
        ("library_dirs", &section.library_dirs, false),
    ];
    for (field, paths, file) in groups {
        let key = join_key(&table, field);
        for path in paths {
            let target = root.join(path);
            let exists = if file {
                names_project_file(root, path)
            } else {
                target.is_dir()
            };
            if !exists {
                out.push(
                    error(
                        codes::PROJECT_BINDING_PATH_MISSING,
                        format!(
                            "binding `{name}`: `{key}` names {path:?}, which is no {} under the project root",
                            if file { "file" } else { "directory" }
                        ),
                    )
                    .with_data("binding", json!(name))
                    .with_data("key", json!(key))
                    .with_data("path", json!(path)),
                );
            }
        }
    }
}

/// `^[a-z][a-z0-9_]*$`: the name of a backend-`c` binding, so that its grant
/// `<prefix>_bind_<name>` is a C identifier.
fn is_binding_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_lowercase())
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn error(code: DiagCode, message: String) -> Diagnostic {
    Diagnostic::error(code, codes::CATEGORY, message)
}

// ---------------------------------------------------------------------------
// Build-input digests (Section 11.2)
// ---------------------------------------------------------------------------

/// The digests of the build inputs of every backend-`c` binding, by binding
/// name and project-relative path: `adapters` its adapter file, `sources`
/// its `[bindings.<name>.c]` sources; what `adapters.<b>.files` and
/// `foreign.c.<b>.sources` of `cpl.lock` record.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CInputDigests {
    pub adapters: BTreeMap<String, BTreeMap<String, Digest>>,
    pub sources: BTreeMap<String, BTreeMap<String, Digest>>,
}

/// The digests of the build inputs of every backend-`c` binding, read from
/// `root` (Section 11.2). Every backend-`c` binding has an entry in both
/// maps; a file that cannot be read, or whose path is not project-relative,
/// has no entry in its binding's map ([`check_c_bindings`] reports it).
/// The paths are the manifest's, as written.
pub fn c_input_digests(manifest: &Manifest, root: &Path) -> CInputDigests {
    let read = |path: &str| -> Option<Digest> {
        if !is_relative_path(path) {
            return None;
        }
        let full = root.join(path);
        if !names_project_file(root, path) {
            return None;
        }
        std::fs::read(full).ok().map(|bytes| digest_bytes(&bytes))
    };
    let mut out = CInputDigests::default();
    for (name, binding) in &manifest.bindings {
        if binding.backend != "c" {
            continue;
        }
        let mut adapter = BTreeMap::new();
        if let Some(digest) = read(&binding.adapter) {
            adapter.insert(binding.adapter.clone(), digest);
        }
        out.adapters.insert(name.clone(), adapter);
        let mut sources = BTreeMap::new();
        for source in manifest
            .binding_c
            .get(name)
            .map(|c| c.sources.as_slice())
            .unwrap_or_default()
        {
            if let Some(digest) = read(source) {
                sources.insert(source.clone(), digest);
            }
        }
        out.sources.insert(name.clone(), sources);
    }
    out
}

/// Whether the project-relative `rel` names a regular file of the project (toolchain 11.1, T54): a link to a file
/// counts as that file, but a link to a directory is never followed, so a path that passes through one names no file.
fn names_project_file(root: &Path, rel: &str) -> bool {
    let mut at = root.to_path_buf();
    let parts: Vec<&str> = rel
        .split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .collect();
    for (i, part) in parts.iter().enumerate() {
        at.push(part);
        if i + 1 < parts.len() {
            match std::fs::symlink_metadata(&at) {
                Ok(m) if m.file_type().is_symlink() => return false,
                Ok(m) if m.is_dir() => {}
                _ => return false,
            }
        }
    }
    at.is_file()
}
