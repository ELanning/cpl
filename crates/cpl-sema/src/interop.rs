//! `cpl_sema::interop` (Milestone 2, `spec/toolchain.md` 1.6, T74, T80):
//! the plan of a checked program's C surface, [`InteropPlan`], which
//! `cpl-emit-c` reads, and [`plan`], which computes it with the
//! `project_export_*` diagnostics, `project_type_name_unknown`, and the
//! semantic half of `project_cbind_invalid` (toolchain 11.1, Appendix B).
//!
//! The C names, mappings, and header layout the plan decides are
//! `spec/cabi.md` Section 5's: the export set and the export order (5.1),
//! the symbols and generated identifiers and their collisions (5.2), and
//! the listed types and capabilities of the package header (5.3), whose
//! qualified type texts [`type_text`] renders, the one renderer of that
//! text. The check driver (`crate::check`) runs the plan, when `"c"` is in
//! `[interop] targets` and every file parsed, after `check_bindings` and
//! `cpl_project::check_c_bindings`, and reports its diagnostics in the
//! project stage; every one of them is span-less and of category `project`.
//!
//! Readings of the specification this module takes (each also cited where
//! it is applied):
//!
//! * The listed types (cabi.md 5.3) are the declarations reachable from
//!   the signatures of the exports and the listed capabilities (through
//!   struct fields, enum payloads, wrapper inner types, and refinement
//!   bases), the listed capabilities themselves, and every instantiation
//!   that occurs in one of those signatures, nested ones included
//!   (`List<Int>` inside `List<List<Int>>`), but not one that occurs only
//!   inside a declaration's fields; the top-level `Result` of an export's
//!   return type is split and not listed, its payloads are. On the corpus
//!   this is exactly Appendix C.1's list.
//! * A generic declaration that is listed is listed as itself (`a.Box`,
//!   `Ty::Adt(def, [])`), like the well-known `Option` and `Result`.
//! * `project_export_not_function`'s `kind` is the id kind of the
//!   declaration (`ty`, `cap`, `test`, `prop`, as `work_prove_not_function`
//!   and a symbol block spell it) or `generator` for a generator.
//! * `project_export_int_unused` fires for a mapping other than `handle`
//!   (an explicit `int = "handle"` cannot be told from the default), and
//!   the top-level `Int`s are the positional parameters, the return type,
//!   and the `Ok` payload of a top-level `Result`, each an `Int` or a
//!   refinement whose root base is `Int` (cabi.md 5.4).
//! * The C identifier of `project_export_symbol_invalid` and
//!   `project_export_symbol_collision` is the payload key `c_symbol`
//!   ([`SYMBOL_KEY`]), not Appendix B's `symbol`, a reserved key of the
//!   diagnostic object (T5).
//! * Every export symbol, an explicit entry's default included, is checked
//!   against its pattern and the reserved families; a symbol that fails
//!   takes part in no collision check.
//! * The default set's `skipped` lists generic `pub fn`s that are not
//!   generators (a generator is never exportable, so a wrapper would not
//!   help it).

use crate::hir::{DefId, DefKind, DefStatus, FnDef, Program, Ty};
use crate::project::cbind::CBindSpec;
use crate::project::{codes, is_c_prefix, AdapterForm, IntMapping, Manifest};
use cpl_diag::{DeclName, DiagCode, Diagnostic, FileOrigin, Severity, StableId, SymbolPath};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

/// The C surface of a checked program. A definition already marked
/// `HasErrors` when the plan runs is left out without a diagnostic
/// (semantics S62). Every list has the order stated, so the plan is a
/// deterministic function of the program and the manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InteropPlan {
    /// `Manifest::c_prefix()`: `[interop.c] prefix`, else `[project] name`.
    pub prefix: String,
    /// `[interop.c] abi`, default 0.
    pub abi: u16,
    /// The export set of T74 in export order: the `exports` entries in
    /// manifest order, else qualified-name order.
    pub exports: Vec<ExportPlan>,
    /// The `pub fn`s the default set leaves out because they are generic,
    /// in qualified-name order (`project_export_skipped_generic`, reported
    /// by `cpl build` only: [`InteropPlan::skipped_diagnostics`]).
    pub skipped: Vec<DefId>,
    /// The listed types of `spec/cabi.md` 5.3, which the package header
    /// declares, in the byte order of their texts.
    pub types: Vec<TypePlan>,
    /// The listed capabilities of cabi.md 5.3 (those of the exports' `using`
    /// parameters and those the backend-`c` bindings bind), in
    /// qualified-name order.
    pub capabilities: Vec<DefId>,
    /// The backend-`c` bindings, in binding-name order.
    pub bindings: Vec<BindingPlan>,
    /// Milestone 3: the backend-`python` bindings, in binding-name order (`spec/python.md` 5.3, 6): their grants
    /// follow the backend-`c` ones in the grant table.
    pub python_bindings: Vec<PythonBindingPlan>,
}

/// One backend-`python` binding (Milestone 3, `spec/python.md` 5.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PythonBindingPlan {
    /// The `[bindings.<name>]` table's name.
    pub name: String,
    pub capability: DefId,
    /// The adapter module's dotted path.
    pub module: String,
    /// `isolation` absent or `"worker"`: a worker in a Python host too (everywhere else it always runs in a worker).
    pub worker: bool,
    /// `[bindings.<name>] timeout_ms`.
    pub timeout_ms: Option<u64>,
}

/// One entry of the export set: one C entry point.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportPlan {
    pub function: DefId,
    /// The C symbol: the entry's `symbol`, else `<prefix>_<function name>`.
    pub symbol: String,
    pub int: IntMapping,
    /// The index of its `[interop.c] exports` entry, `None` for the default set.
    pub entry: Option<usize>,
}

/// One listed type of the package header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypePlan {
    /// The type: an instantiation, or a declaration (`Ty::Adt(def, [])` for
    /// a struct or enum, generic ones included; `Ty::Wrapper`,
    /// `Ty::Refined`, `Ty::Capability`).
    pub ty: Ty,
    /// `type_text(program, &ty)`.
    pub text: String,
    /// The `<name>` of `<prefix>_type_<name>`: the `[interop.c]
    /// type_names` alias of `text`, else the generated name of cabi.md 5.2.
    pub c_name: String,
}

/// One backend-`c` binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindingPlan {
    /// The `[bindings.<name>]` table's name.
    pub name: String,
    pub capability: DefId,
    pub form: AdapterForm,
    /// The project-relative adapter file.
    pub adapter: String,
    /// The parsed `.cbind.toml` of a declarative binding (when
    /// `check_c_bindings` read it: never without a project root).
    pub cbind: Option<CBindSpec>,
    /// Milestone 3 (`spec/python.md` 6.6): `isolation` absent or `"worker"`, so the adapter runs in the C adapter
    /// host.
    pub worker: bool,
    /// `[bindings.<name>] timeout_ms`.
    pub timeout_ms: Option<u64>,
}

impl InteropPlan {
    /// Nothing exported or bound: the plan of a driver for a project
    /// without the C target, `name` being the project's effective prefix
    /// (`Manifest::c_prefix()`); its `prefix` is the driver prefix that
    /// `spec/cabi.md` 1.6 derives from `name`: `name` when it is in the
    /// prefix domain of toolchain 11.1, else `driver`.
    pub fn empty(name: &str) -> InteropPlan {
        InteropPlan {
            prefix: if is_c_prefix(name) {
                name.to_string()
            } else {
                "driver".to_string()
            },
            abi: 0,
            exports: Vec::new(),
            skipped: Vec::new(),
            types: Vec::new(),
            capabilities: Vec::new(),
            bindings: Vec::new(),
            python_bindings: Vec::new(),
        }
    }

    /// The backend-`python` binding of a capability (`spec/python.md` 7).
    pub fn python_binding_for(&self, capability: DefId) -> Option<&PythonBindingPlan> {
        self.python_bindings
            .iter()
            .find(|b| b.capability == capability)
    }

    /// The number of grants of the program tables: the backend-`c` bindings, then the backend-`python` ones.
    pub fn ngrants(&self) -> usize {
        self.bindings.len() + self.python_bindings.len()
    }

    /// The backend-`c` binding of a capability (at most one, T74; after a
    /// `project_binding_duplicate_capability`, the first in name order).
    pub fn binding_for(&self, capability: DefId) -> Option<&BindingPlan> {
        self.bindings.iter().find(|b| b.capability == capability)
    }

    /// The infos `project_export_skipped_generic` (`function`, the qualified
    /// name), one per [`InteropPlan::skipped`] function in its order, which
    /// `cpl build` alone reports (toolchain 7.2, T74), so that `cpl check`'s
    /// results on the corpus do not change.
    pub fn skipped_diagnostics(&self, program: &Program) -> Vec<Diagnostic> {
        self.skipped
            .iter()
            .map(|&def| {
                let name = program.display_path(def);
                Diagnostic::new(
                    codes::PROJECT_EXPORT_SKIPPED_GENERIC,
                    codes::CATEGORY,
                    Severity::Info,
                    format!(
                        "{name} is generic and is not exported: the default export set leaves out generic functions, which need a monomorphic wrapper"
                    ),
                )
                .with_data("function", json!(name))
            })
            .collect()
    }
}

/// The qualified type text of `spec/cabi.md` 5.3: CPL type syntax with every
/// declaration named as `$type` names it (T30: the bare name of a prelude
/// declaration, the qualified name of every other): `List<Int>`,
/// `Result<reader.pages.Page, reader.pages.StoreError>`,
/// `reader.pages.Positive`. A refinement is named by the declaration written
/// at the use site (S55); a type parameter renders as `$<index>` and the
/// error type as `?`, as `Program::display_ty` renders them.
pub fn type_text(program: &Program, ty: &Ty) -> String {
    let mut out = String::new();
    write_type_text(program, ty, &mut out);
    out
}

fn write_type_text(program: &Program, ty: &Ty, out: &mut String) {
    match ty {
        Ty::Int => out.push_str("Int"),
        Ty::Bool => out.push_str("Bool"),
        Ty::Text => out.push_str("Text"),
        Ty::Bytes => out.push_str("Bytes"),
        Ty::List(t) => {
            out.push_str("List<");
            write_type_text(program, t, out);
            out.push('>');
        }
        Ty::Gen(t) => {
            out.push_str("Gen<");
            write_type_text(program, t, out);
            out.push('>');
        }
        Ty::Adt(def, args) => {
            out.push_str(&type_name(program, *def));
            if !args.is_empty() {
                out.push('<');
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    write_type_text(program, a, out);
                }
                out.push('>');
            }
        }
        Ty::Wrapper(def) | Ty::Refined(_, def) | Ty::Capability(def) => {
            out.push_str(&type_name(program, *def))
        }
        Ty::Context(def) => {
            out.push_str("Context<");
            out.push_str(&type_name(program, *def));
            out.push('>');
        }
        Ty::Param(i) => {
            out.push('$');
            out.push_str(&i.to_string());
        }
        Ty::Error => out.push('?'),
    }
}

/// T30's naming of a declaration: the bare name in `std.prelude`, else the
/// qualified name (`cpl_eval::types::type_name`, which
/// `crates/cpl-emit-c/tests/tables.rs` compares with this).
fn type_name(program: &Program, def: DefId) -> String {
    match program.defs.get(def.0 as usize) {
        None => format!("{{def {}}}", def.0),
        Some(d) if is_prelude(program, def) => d.name.display(),
        Some(_) => program.display_path(def),
    }
}

fn is_prelude(program: &Program, def: DefId) -> bool {
    program
        .defs
        .get(def.0 as usize)
        .and_then(|d| program.modules.get(d.module.0 as usize))
        .is_some_and(|m| m.path.iter().map(String::as_str).eq(["std", "prelude"]))
}

fn is_std(program: &Program, def: DefId) -> bool {
    program
        .defs
        .get(def.0 as usize)
        .and_then(|d| program.modules.get(d.module.0 as usize))
        .is_some_and(|m| m.origin == FileOrigin::Std)
}

/// The snake case of a PascalCase name (cabi.md 5.2): `_` before every
/// upper-case letter that follows a lower-case letter or a digit, and
/// before an upper-case letter that is followed by a lower-case letter and
/// follows an upper-case letter; then every letter lowered (`PageId` gives
/// `page_id`, `IOError` gives `io_error`).
pub fn snake_case(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::with_capacity(name.len() + 4);
    for (i, &c) in chars.iter().enumerate() {
        if c.is_uppercase() && i > 0 {
            let prev = chars[i - 1];
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            if prev.is_lowercase() || prev.is_ascii_digit() || (prev.is_uppercase() && next_lower) {
                out.push('_');
            }
        }
        out.extend(c.to_lowercase());
    }
    out
}

/// The generated `<name>` of a type's accessor (cabi.md 5.2): the snake case
/// of a declaration's bare name; for an instantiation, the snake cases of
/// its bare names in pre-order joined by `_` (`list_list_int`,
/// `result_page_store_error`).
fn generated_c_name(program: &Program, ty: &Ty) -> String {
    let mut parts = Vec::new();
    name_parts(program, ty, &mut parts);
    parts.join("_")
}

fn name_parts(program: &Program, ty: &Ty, out: &mut Vec<String>) {
    match ty {
        Ty::Int => out.push("int".into()),
        Ty::Bool => out.push("bool".into()),
        Ty::Text => out.push("text".into()),
        Ty::Bytes => out.push("bytes".into()),
        Ty::List(t) => {
            out.push("list".into());
            name_parts(program, t, out);
        }
        Ty::Gen(t) => {
            out.push("gen".into());
            name_parts(program, t, out);
        }
        Ty::Adt(def, args) => {
            out.push(snake_case(&bare_name(program, *def)));
            for a in args {
                name_parts(program, a, out);
            }
        }
        Ty::Wrapper(def) | Ty::Refined(_, def) | Ty::Capability(def) => {
            out.push(snake_case(&bare_name(program, *def)))
        }
        Ty::Context(def) => {
            out.push("context".into());
            out.push(snake_case(&bare_name(program, *def)));
        }
        Ty::Param(i) => out.push(format!("t{i}")),
        Ty::Error => out.push("error".into()),
    }
}

fn bare_name(program: &Program, def: DefId) -> String {
    program
        .defs
        .get(def.0 as usize)
        .map_or_else(|| format!("def{}", def.0), |d| d.name.display())
}

// ---------------------------------------------------------------------------
// The plan
// ---------------------------------------------------------------------------

/// The rules of toolchain 11.1 that need the checked program: the
/// `project_export_*` codes (but `project_export_skipped_generic`, which
/// `cpl build` reports from [`InteropPlan::skipped`]),
/// `project_type_name_unknown`, and the semantic half of
/// `project_cbind_invalid`. An `exports` target resolves by the rows of
/// toolchain 6.1 over `program` (T59), with the `project_export_*` codes in
/// place of `index_unknown_symbol` and `index_ambiguous_symbol`. `cbind`:
/// the declarative bindings whose files `check_c_bindings` read and parsed.
///
/// The diagnostics come in this order: the export entries' in entry order
/// (the default set's in qualified-name order), the bindings' in name
/// order, `project_type_name_unknown` in key order, then the collisions in
/// symbol order.
pub fn plan(
    program: &Program,
    manifest: &Manifest,
    cbind: &BTreeMap<String, CBindSpec>,
) -> (InteropPlan, Vec<Diagnostic>) {
    let prefix = manifest.c_prefix().to_string();
    let interop_c = manifest.interop_c.clone().unwrap_or_default();
    let mut out = Planner {
        program,
        prefix: prefix.clone(),
        diagnostics: Vec::new(),
    };
    let (exports, skipped) = match &interop_c.exports {
        Some(entries) => (out.explicit_exports(entries), Vec::new()),
        None => out.default_exports(),
    };
    let bindings = out.bindings(manifest, cbind);

    // The listed capabilities: the exports' `using` parameters and the
    // backend-c bindings' capabilities, in qualified-name order.
    let mut capability_set = BTreeSet::new();
    for export in &exports {
        if let Some(f) = program.def(export.function).kind.as_fn() {
            for param in &f.using {
                if let Ty::Capability(def) = &param.ty {
                    capability_set.insert(*def);
                }
            }
        }
    }
    capability_set.extend(bindings.iter().map(|b| b.capability));
    let mut capabilities: Vec<DefId> = capability_set.into_iter().collect();
    sort_by_path(program, &mut capabilities);

    let types = out.listed_types(&exports, &capabilities, &interop_c.type_names);
    for key in interop_c.type_names.keys() {
        if !types.iter().any(|t| &t.text == key) {
            out.diagnostics.push(
                error(
                    codes::PROJECT_TYPE_NAME_UNKNOWN,
                    format!(
                        "`[interop.c] type_names` names {key:?}, which is no type the package header lists (the types reachable from the exports)"
                    ),
                )
                .with_data("key", json!(key)),
            );
        }
    }
    let python_bindings = out.python_bindings(manifest);
    let plan = InteropPlan {
        prefix,
        abi: interop_c.abi,
        exports,
        skipped,
        types,
        capabilities,
        bindings,
        python_bindings,
    };
    out.collisions(&plan);
    (plan, out.diagnostics)
}

struct Planner<'p> {
    program: &'p Program,
    prefix: String,
    diagnostics: Vec<Diagnostic>,
}

/// What an export target names (toolchain 6.1).
enum Resolved {
    One(DefId),
    None,
    Several(Vec<DefId>),
}

impl Planner<'_> {
    /// `[interop.c] exports` present: exactly its entries (T74).
    fn explicit_exports(&mut self, entries: &[crate::project::ExportEntry]) -> Vec<ExportPlan> {
        let program = self.program;
        let mut exports = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            let target = entry.function.as_str();
            let def = match resolve_target(program, target) {
                Resolved::One(def) => def,
                Resolved::None => {
                    self.diagnostics.push(
                        error(
                            codes::PROJECT_EXPORT_UNKNOWN,
                            format!(
                                "`[interop.c] exports[{index}]` names `{target}`, which is no declaration of the project or the standard library"
                            ),
                        )
                        .with_data("entry", json!(target))
                        .with_data("index", json!(index)),
                    );
                    continue;
                }
                Resolved::Several(candidates) => {
                    let paths: Vec<String> = candidates
                        .iter()
                        .map(|&d| program.display_path(d))
                        .collect();
                    self.diagnostics.push(
                        error(
                            codes::PROJECT_EXPORT_AMBIGUOUS,
                            format!(
                                "`[interop.c] exports[{index}]` names `{target}`, which names {} declarations: {}; name one by its qualified name",
                                paths.len(),
                                paths.join(", ")
                            ),
                        )
                        .with_data("entry", json!(target))
                        .with_data("candidates", json!(paths)),
                    );
                    continue;
                }
            };
            let d = program.def(def);
            if d.status == DefStatus::HasErrors {
                continue;
            }
            let name = program.display_path(def);
            let Some(f) = d.kind.as_fn() else {
                self.diagnostics.push(
                    error(
                        codes::PROJECT_EXPORT_NOT_FUNCTION,
                        format!("`{target}` names {name}, which is no function; only functions are exported"),
                    )
                    .with_data("entry", json!(target))
                    .with_data("kind", json!(kind_of(&d.kind))),
                );
                continue;
            };
            if is_generator(f) {
                self.diagnostics.push(
                    error(
                        codes::PROJECT_EXPORT_NOT_FUNCTION,
                        format!("`{target}` names {name}, which is a generator; a generator has a value only in a property's for clause and is never exported"),
                    )
                    .with_data("entry", json!(target))
                    .with_data("kind", json!("generator")),
                );
                continue;
            }
            if f.takes_context() {
                self.diagnostics.push(
                    error(
                        codes::PROJECT_EXPORT_NOT_FUNCTION,
                        format!("`{target}` names {name}, which takes a context handle; only CPL code passes one, so export the function whose `context` statement starts the state machine"),
                    )
                    .with_data("entry", json!(target))
                    .with_data("kind", json!("context")),
                );
                continue;
            }
            if is_std(program, def) {
                self.diagnostics.push(
                    error(
                        codes::PROJECT_EXPORT_NOT_PROJECT,
                        format!("`{target}` names {name}, a standard-library declaration; only functions declared in the project are exported"),
                    )
                    .with_data("entry", json!(target)),
                );
                continue;
            }
            if !d.is_pub {
                self.diagnostics.push(
                    error(
                        codes::PROJECT_EXPORT_NOT_PUBLIC,
                        format!("`{target}` names {name}, which is private; only `pub fn`s are exported"),
                    )
                    .with_data("entry", json!(target)),
                );
                continue;
            }
            if !f.generics.is_empty() {
                let callers = monomorphic_callers(program, def);
                let note = if callers.is_empty() {
                    "a type parameter has no argument to instantiate it from; write a monomorphic wrapper that calls it with concrete types and export that".to_string()
                } else {
                    format!(
                        "a type parameter has no argument to instantiate it from; export a monomorphic wrapper such as {}",
                        callers.join(", ")
                    )
                };
                self.diagnostics.push(
                    error(
                        codes::PROJECT_EXPORT_GENERIC,
                        format!("`{target}` names {name}, which is generic; generic functions need concrete export wrappers"),
                    )
                    .with_data("entry", json!(target))
                    .with_note(note, None),
                );
                continue;
            }
            let symbol = entry
                .symbol
                .clone()
                .unwrap_or_else(|| self.default_symbol(def));
            if !self.check_symbol(&symbol) {
                continue;
            }
            if entry.int != IntMapping::Handle && !has_top_level_int(program, f) {
                self.diagnostics.push(
                    Diagnostic::new(
                        codes::PROJECT_EXPORT_INT_UNUSED,
                        codes::CATEGORY,
                        Severity::Warning,
                        format!(
                            "`[interop.c] exports[{index}]` maps Int as {} but {name} has no Int at the top level of its signature",
                            entry.int.as_str()
                        ),
                    )
                    .with_data("entry", json!(target)),
                );
            }
            exports.push(ExportPlan {
                function: def,
                symbol,
                int: entry.int,
                entry: Some(index),
            });
        }
        exports
    }

    /// `[interop.c] exports` absent: every `pub fn` declared in the project
    /// that is neither generic nor a generator, in qualified-name order, in
    /// its handle form with its default symbol; the generic ones are
    /// `skipped` (T74).
    fn default_exports(&mut self) -> (Vec<ExportPlan>, Vec<DefId>) {
        let program = self.program;
        let mut candidates: Vec<DefId> = program
            .defs()
            .filter(|d| d.is_pub && d.status == DefStatus::Ok && !is_std(program, d.id))
            .filter(|d| d.kind.as_fn().is_some_and(|f| !is_generator(f)))
            .map(|d| d.id)
            .collect();
        sort_by_path(program, &mut candidates);
        candidates.dedup_by_key(|d| program.display_path(*d));
        let mut exports = Vec::new();
        let mut skipped = Vec::new();
        for def in candidates {
            let f = program
                .def(def)
                .kind
                .as_fn()
                .expect("filtered to functions");
            // a context handle is passed only by CPL code: the function starting the context is the export, and one
            // taking a handle is left out without a report (it is not generic)
            if f.takes_context() {
                continue;
            }
            if !f.generics.is_empty() {
                skipped.push(def);
                continue;
            }
            let symbol = self.default_symbol(def);
            if self.check_symbol(&symbol) {
                exports.push(ExportPlan {
                    function: def,
                    symbol,
                    int: IntMapping::Handle,
                    entry: None,
                });
            }
        }
        (exports, skipped)
    }

    /// `<prefix>_<function name>` (cabi.md 5.2).
    fn default_symbol(&self, def: DefId) -> String {
        format!("{}_{}", self.prefix, self.program.def(def).name.display())
    }

    /// `^<prefix>_[a-z0-9_]{1,56}$` and outside the reserved families
    /// (toolchain 11.1, cabi.md 5.2); `project_export_symbol_invalid` and
    /// false otherwise.
    fn check_symbol(&mut self, symbol: &str) -> bool {
        let prefix = &self.prefix;
        let rest = symbol
            .strip_prefix(prefix.as_str())
            .and_then(|r| r.strip_prefix('_'));
        let reason = match rest {
            Some(rest)
                if (1..=56).contains(&rest.len())
                    && rest
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_') =>
            {
                RESERVED_FAMILIES
                    .iter()
                    .find(|(family, _)| rest.starts_with(family))
                    .map(|(family, what)| {
                        format!("{symbol} is in the reserved family {prefix}_{family}* ({what})")
                    })
            }
            _ => Some(format!(
                "{symbol} does not match ^{prefix}_[a-z0-9_]{{1,56}}$"
            )),
        };
        match reason {
            None => true,
            Some(reason) => {
                self.diagnostics.push(
                    error(
                        codes::PROJECT_EXPORT_SYMBOL_INVALID,
                        format!("the export symbol {symbol:?} is invalid: {reason}; set the entry's `symbol` in `[interop.c] exports`"),
                    )
                    .with_data(SYMBOL_KEY, json!(symbol))
                    .with_data("reason", json!(reason)),
                );
                false
            }
        }
    }

    /// The backend-`c` bindings whose capability resolves to a `pub
    /// capability` and whose adapter has a form (the others are
    /// `check_bindings`' and `check_c_bindings`' errors), in name order.
    fn bindings(
        &mut self,
        manifest: &Manifest,
        cbind: &BTreeMap<String, CBindSpec>,
    ) -> Vec<BindingPlan> {
        let program = self.program;
        let mut out = Vec::new();
        for (name, binding) in &manifest.bindings {
            if binding.backend != "c" {
                continue;
            }
            let Some(capability) = SymbolPath::parse(&binding.capability)
                .ok()
                .and_then(|p| program.lookup(&p))
                .filter(|&d| {
                    let d = program.def(d);
                    d.is_pub && matches!(d.kind, DefKind::Capability(_))
                })
            else {
                continue;
            };
            let Some(form) = AdapterForm::of_adapter(&binding.adapter) else {
                continue;
            };
            let spec = cbind.get(name).cloned();
            if let (AdapterForm::Declarative, Some(spec)) = (form, &spec) {
                let diagnostics = crate::cbind::check_binding(program, capability, name, spec);
                self.diagnostics.extend(diagnostics);
            }
            out.push(BindingPlan {
                name: name.clone(),
                capability,
                form,
                adapter: binding.adapter.clone(),
                cbind: spec,
                worker: binding.is_worker(),
                timeout_ms: manifest
                    .binding_timeouts
                    .get(name)
                    .filter(|t| **t > 0)
                    .map(|t| *t as u64),
            });
        }
        out
    }

    /// Milestone 3: the backend-`python` bindings whose capability resolves to a `pub capability` and whose module
    /// path is valid (the others are `check_bindings`' and `check_python`'s errors), in name order.
    fn python_bindings(&mut self, manifest: &Manifest) -> Vec<PythonBindingPlan> {
        let program = self.program;
        let mut out = Vec::new();
        for (name, binding) in &manifest.bindings {
            if binding.backend != "python"
                || !crate::project::is_python_module_path(&binding.adapter)
            {
                continue;
            }
            let Some(capability) = SymbolPath::parse(&binding.capability)
                .ok()
                .and_then(|p| program.lookup(&p))
                .filter(|&d| {
                    let d = program.def(d);
                    d.is_pub && matches!(d.kind, DefKind::Capability(_))
                })
            else {
                continue;
            };
            out.push(PythonBindingPlan {
                name: name.clone(),
                capability,
                module: binding.adapter.clone(),
                worker: binding.is_worker(),
                timeout_ms: manifest
                    .binding_timeouts
                    .get(name)
                    .filter(|t| **t > 0)
                    .map(|t| *t as u64),
            });
        }
        out
    }

    /// The listed types of cabi.md 5.3, in the byte order of their texts
    /// (see the module documentation for the reading).
    fn listed_types(
        &self,
        exports: &[ExportPlan],
        capabilities: &[DefId],
        type_names: &BTreeMap<String, String>,
    ) -> Vec<TypePlan> {
        let program = self.program;
        let mut walk = TypeWalk {
            program,
            well_known: well_known(program),
            listed: BTreeMap::new(),
            reached: BTreeSet::new(),
        };
        let mut seen = BTreeSet::new();
        for export in exports {
            if !seen.insert(export.function) {
                continue;
            }
            let Some(f) = program.def(export.function).kind.as_fn() else {
                continue;
            };
            for param in f.params.iter().chain(&f.using) {
                walk.signature(&param.ty);
            }
            match &f.ret {
                Ty::Adt(def, args) if *def == program.std_defs().result && args.len() == 2 => {
                    walk.signature(&args[0]);
                    walk.signature(&args[1]);
                }
                ret => walk.signature(ret),
            }
        }
        for &capability in capabilities {
            walk.declaration(&Ty::Capability(capability));
            if let DefKind::Capability(c) = &program.def(capability).kind {
                for op in &c.ops {
                    for param in &op.params {
                        walk.signature(&param.ty);
                    }
                    walk.signature(&op.ret);
                }
            }
        }
        walk.listed
            .into_iter()
            .map(|(text, ty)| {
                let c_name = type_names
                    .get(&text)
                    .cloned()
                    .unwrap_or_else(|| generated_c_name(program, &ty));
                TypePlan { ty, text, c_name }
            })
            .collect()
    }

    /// `project_export_symbol_collision` for every generated identifier of
    /// cabi.md 5.2 that two items share, in symbol order.
    fn collisions(&mut self, plan: &InteropPlan) {
        let program = self.program;
        let prefix = &plan.prefix;
        let upper = prefix.to_uppercase();
        let mut ids: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut add = |id: String, item: String| ids.entry(id).or_default().push(item);
        add(
            format!("{prefix}_runtime_open"),
            "package function runtime_open".into(),
        );
        add(
            format!("{prefix}_library_info"),
            "package function library_info".into(),
        );
        add(format!("{upper}_H"), format!("include guard of {prefix}.h"));
        add(format!("{upper}_ABI"), format!("ABI macro of {prefix}.h"));
        for t in &plan.types {
            add(
                format!("{prefix}_type_{}", t.c_name),
                format!("type accessor of {}", t.text),
            );
            let name = t.c_name.to_uppercase();
            match &t.ty {
                Ty::Adt(def, args) if args.is_empty() => match &program.def(*def).kind {
                    DefKind::Struct(s) => {
                        for field in &s.fields {
                            add(
                                format!("{upper}_{name}_F_{}", field.name.to_uppercase()),
                                format!("field {} of {}", field.name, t.text),
                            );
                        }
                    }
                    DefKind::Enum(e) => {
                        for variant in &e.variants {
                            add(
                                format!(
                                    "{upper}_{name}_V_{}",
                                    snake_case(&variant.name).to_uppercase()
                                ),
                                format!("variant {} of {}", variant.name, t.text),
                            );
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        for &capability in &plan.capabilities {
            let name = snake_case(&bare_name(program, capability));
            let qualified = program.display_path(capability);
            add(
                format!("{prefix}_{name}_ops"),
                format!("operation struct of capability {qualified}"),
            );
            add(
                format!("{prefix}_{name}_new"),
                format!("constructor of capability {qualified}"),
            );
        }
        for binding in &plan.bindings {
            let b = &binding.name;
            add(
                format!("{prefix}_bind_{b}"),
                format!("grant of binding {b}"),
            );
            if binding.form == AdapterForm::Wrapper {
                add(
                    format!("{upper}_ADAPTER_{}_H", b.to_uppercase()),
                    format!("include guard of {prefix}_adapter_{b}.h"),
                );
            }
            add(
                format!("{prefix}_adapter_{b}_init"),
                format!("adapter init of binding {b}"),
            );
            add(
                format!("{prefix}_adapter_{b}_fini"),
                format!("adapter fini of binding {b}"),
            );
            if let DefKind::Capability(c) = &program.def(binding.capability).kind {
                let mut taken: Vec<String> = vec!["init".into(), "fini".into()];
                for op in &c.ops {
                    // cabi.md 5.2: `_` appended while the name is `init`,
                    // `fini`, or one an earlier operation received.
                    let mut name = op.name.clone();
                    while taken.contains(&name) {
                        name.push('_');
                    }
                    taken.push(name.clone());
                    add(
                        format!("{prefix}_adapter_{b}_{name}"),
                        format!("adapter operation {} of binding {b}", op.name),
                    );
                }
            }
        }
        for export in &plan.exports {
            let name = program.display_path(export.function);
            let item = match export.entry {
                Some(i) => format!("export {name} (exports[{i}])"),
                None => format!("export {name}"),
            };
            add(export.symbol.clone(), item);
        }
        for (symbol, items) in ids {
            if items.len() > 1 {
                self.diagnostics.push(
                    error(
                        codes::PROJECT_EXPORT_SYMBOL_COLLISION,
                        format!(
                            "the generated identifier {symbol} would name {}; give an export a `symbol`, add an `[interop.c] type_names` entry, or rename a binding",
                            items.join(" and ")
                        ),
                    )
                    .with_data(SYMBOL_KEY, json!(symbol))
                    .with_data("items", json!(items)),
                );
            }
        }
    }
}

/// The payload key of the C identifier in `project_export_symbol_invalid`
/// and `project_export_symbol_collision`. Toolchain Appendix B names it
/// `symbol`, which is one of the reserved keys of the diagnostic object
/// (T5, `cpl_diag::RESERVED_KEYS`: the blamed declaration's stable id), so
/// `with_data` rejects it and `to_json` would drop it; the C identifier is
/// therefore carried as `c_symbol` (a spec deviation, reported for the
/// spec editor; `build_abi_changed`'s `symbol` has the same conflict).
pub const SYMBOL_KEY: &str = "c_symbol";

/// The reserved families of export symbols (toolchain 11.1, cabi.md 5.2),
/// after `<prefix>_`, with what they hold.
const RESERVED_FAMILIES: [(&str, &str); 5] = [
    ("runtime_", "package functions"),
    ("library_", "package functions"),
    ("type_", "type accessors"),
    ("bind_", "grants"),
    ("adapter_", "adapter functions"),
];

/// The listing walk of cabi.md 5.3.
struct TypeWalk<'p> {
    program: &'p Program,
    /// The declarations of the well-known types of `cpl.h` (ids 1-11).
    well_known: BTreeSet<DefId>,
    /// By qualified type text.
    listed: BTreeMap<String, Ty>,
    /// Declarations whose fields have been walked.
    reached: BTreeSet<DefId>,
}

impl TypeWalk<'_> {
    /// A type of a signature: listed when it is an instantiation, its
    /// nested instantiations too, and every declaration in it reached.
    fn signature(&mut self, ty: &Ty) {
        match ty {
            Ty::List(elem) => {
                self.list(ty);
                self.signature(elem);
            }
            Ty::Adt(_, args) if !args.is_empty() => {
                self.list(ty);
                self.declaration(ty);
                for a in args {
                    self.signature(a);
                }
            }
            Ty::Adt(..) | Ty::Wrapper(_) | Ty::Refined(..) | Ty::Capability(_) => {
                self.declaration(ty)
            }
            Ty::Gen(elem) => self.signature(elem),
            // (never exported: see `FnDef::takes_context`)
            Ty::Context(_) | Ty::Int | Ty::Bool | Ty::Text | Ty::Bytes | Ty::Param(_) | Ty::Error => {}
        }
    }

    fn list(&mut self, ty: &Ty) {
        if contains_error_or_param(ty) {
            return;
        }
        let text = type_text(self.program, ty);
        self.listed.entry(text).or_insert_with(|| ty.clone());
    }

    /// The declaration a type names, listed unless it is well known, and
    /// the declarations its fields, payloads, inner type, or base reach.
    fn declaration(&mut self, ty: &Ty) {
        let program = self.program;
        let (def, as_decl) = match ty {
            Ty::Adt(def, _) => (*def, Ty::Adt(*def, Vec::new())),
            Ty::Wrapper(def) => (*def, ty.clone()),
            Ty::Refined(_, def) => (*def, ty.clone()),
            Ty::Capability(def) => (*def, ty.clone()),
            _ => return,
        };
        if program.defs.get(def.0 as usize).is_none() || self.well_known.contains(&def) {
            return;
        }
        let text = type_text(program, &as_decl);
        self.listed.entry(text).or_insert(as_decl);
        if !self.reached.insert(def) {
            return;
        }
        let mut inner: Vec<Ty> = Vec::new();
        match &program.def(def).kind {
            DefKind::Struct(s) => inner.extend(s.fields.iter().map(|f| f.ty.clone())),
            DefKind::Enum(e) => {
                for v in &e.variants {
                    inner.extend(v.payload.iter().cloned());
                }
            }
            DefKind::Wrapper(w) => inner.push(w.inner.clone()),
            DefKind::Refinement(r) => inner.push(r.base.clone()),
            _ => {}
        }
        for t in &inner {
            self.reach(t);
        }
    }

    /// Every declaration in a type of a declaration's members (its
    /// instantiations are not listed).
    fn reach(&mut self, ty: &Ty) {
        match ty {
            Ty::List(elem) | Ty::Gen(elem) => self.reach(elem),
            Ty::Adt(_, args) => {
                self.declaration(ty);
                for a in args {
                    self.reach(a);
                }
            }
            Ty::Wrapper(_) | Ty::Refined(..) | Ty::Capability(_) => self.declaration(ty),
            Ty::Context(_) | Ty::Int | Ty::Bool | Ty::Text | Ty::Bytes | Ty::Param(_) | Ty::Error => {}
        }
    }
}

fn contains_error_or_param(ty: &Ty) -> bool {
    match ty {
        Ty::Error | Ty::Param(_) => true,
        Ty::List(t) | Ty::Gen(t) => contains_error_or_param(t),
        Ty::Adt(_, args) => args.iter().any(contains_error_or_param),
        _ => false,
    }
}

/// The declarations of the well-known types of `cpl.h` (`CPL_T_UNIT` to
/// `CPL_T_FOREIGN_DETAIL`): `Unit`, `Option`, `Result`, `ConstraintError`,
/// `std.foreign.ForeignError`, `ForeignErrorKind`, and `ForeignDetail`
/// (`Int`, `Bool`, `Text`, and `Bytes` are no declarations).
fn well_known(program: &Program) -> BTreeSet<DefId> {
    let std = program.std_defs();
    let mut out = BTreeSet::from([
        std.unit,
        std.option,
        std.result,
        std.constraint_error,
        std.foreign_error,
    ]);
    for name in ["ForeignErrorKind", "ForeignDetail"] {
        let path = SymbolPath::new(
            vec!["std".to_string(), "foreign".to_string()],
            DeclName::Ident(name.to_string()),
        );
        if let Some(def) = program.lookup(&path) {
            out.insert(def);
        }
    }
    out
}

/// A generator: a function whose declared return type is a `Gen<T>`.
fn is_generator(f: &FnDef) -> bool {
    matches!(f.ret, Ty::Gen(_))
}

/// Whether a function has an `Int` at the top level of its signature
/// (cabi.md 5.4): a positional parameter, the return type, or the `Ok`
/// payload of a top-level `Result`, each `Int` or a refinement of it.
fn has_top_level_int(program: &Program, f: &FnDef) -> bool {
    let int_based = |ty: &Ty| strip(program, ty) == Ty::Int;
    if f.params.iter().any(|p| int_based(&p.ty)) || int_based(&f.ret) {
        return true;
    }
    match &f.ret {
        Ty::Adt(def, args) if *def == program.std_defs().result && args.len() == 2 => {
            int_based(&args[0])
        }
        _ => false,
    }
}

/// The root base of a refinement chain (semantics 3.3), `Ty::Error` for one
/// that does not end.
fn strip(program: &Program, ty: &Ty) -> Ty {
    let mut current = ty.clone();
    for _ in 0..256 {
        match &current {
            Ty::Refined(_, def) => match program.defs.get(def.0 as usize).map(|d| &d.kind) {
                Some(DefKind::Refinement(r)) => current = r.base.clone(),
                _ => return Ty::Error,
            },
            _ => return current,
        }
    }
    Ty::Error
}

/// The id kind of a declaration that is no function (`ty`, `cap`, `test`, `prop`).
fn kind_of(kind: &DefKind) -> &'static str {
    match kind {
        DefKind::Fn(_) => "fn",
        DefKind::Wrapper(_)
        | DefKind::Refinement(_)
        | DefKind::Struct(_)
        | DefKind::Enum(_)
        | DefKind::BuiltinType(_) => "ty",
        DefKind::Capability(_) => "cap",
        DefKind::Test(_) => "test",
        DefKind::Property(_) => "prop",
    }
}

/// The project's monomorphic callers of `def` (non-generic functions that
/// are not generators, other than `def`), by qualified name, as the note of
/// `cannot_run_generic` names them.
fn monomorphic_callers(program: &Program, def: DefId) -> Vec<String> {
    let mut out: Vec<String> = program
        .references_to(def)
        .into_iter()
        .map(|r| r.from)
        .filter(|&caller| caller != def && !is_std(program, caller))
        .filter(|&caller| {
            program
                .def(caller)
                .kind
                .as_fn()
                .is_some_and(|f| f.generics.is_empty() && !is_generator(f))
        })
        .map(|caller| program.display_path(caller))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// What an export target names, by the rows of toolchain 6.1 over the
/// program (`cpl_index::query::Target::parse` and `Index::resolve`, which
/// `cpl-sema` cannot call, T69): an accepted `@id` spelling with an entry,
/// a qualified path (then the provisional id it spells), or a bare name
/// that names exactly one definition of the project or std. Definitions
/// sharing one qualified name count once.
fn resolve_target(program: &Program, target: &str) -> Resolved {
    if let Ok(id) = StableId::from_annotation(target) {
        if let Some(def) = program.by_stable_id(&id) {
            return Resolved::One(def);
        }
        return resolve_name(program, &DeclName::Ident(target.to_string()));
    }
    if let Ok(path) = SymbolPath::parse(target) {
        return match program.lookup(&path).or_else(|| {
            program.by_stable_id(&StableId::provisional(&path.module_path(), &path.name))
        }) {
            Some(def) => Resolved::One(def),
            None => Resolved::None,
        };
    }
    if let Ok(path) = SymbolPath::parse(&format!("m.{target}")) {
        if path.module.len() == 1 {
            return resolve_name(program, &path.name);
        }
    }
    resolve_name(program, &DeclName::Text(target.to_string()))
}

fn resolve_name(program: &Program, name: &DeclName) -> Resolved {
    let mut seen = BTreeSet::new();
    let mut candidates: Vec<DefId> = program
        .defs()
        .filter(|d| bare_name_matches(&d.name, name))
        .filter(|d| seen.insert(program.display_path(d.id)))
        .map(|d| d.id)
        .collect();
    sort_by_path(program, &mut candidates);
    match candidates.len() {
        0 => Resolved::None,
        1 => Resolved::One(candidates[0]),
        _ => Resolved::Several(candidates),
    }
}

/// Toolchain 6.1 row 4: an identifier names declarations, std member stubs,
/// and tests and properties of that name; a `Receiver.member` pair names
/// that member; a quoted text names tests and properties.
fn bare_name_matches(def: &DeclName, name: &DeclName) -> bool {
    match (def, name) {
        (DeclName::Ident(a), DeclName::Ident(b)) | (DeclName::Text(a), DeclName::Ident(b)) => {
            a == b
        }
        (DeclName::Member { name: a, .. }, DeclName::Ident(b)) => a == b,
        (DeclName::Text(a), DeclName::Text(b)) => a == b,
        (
            DeclName::Member { receiver, name },
            DeclName::Member {
                receiver: r,
                name: n,
            },
        ) => receiver == r && name == n,
        _ => false,
    }
}

/// Display-path order, then identity, then id (as `cpl-index` sorts).
fn sort_by_path(program: &Program, defs: &mut [DefId]) {
    defs.sort_by(|&a, &b| {
        program
            .display_path(a)
            .cmp(&program.display_path(b))
            .then_with(|| program.def(a).stable_id.cmp(&program.def(b).stable_id))
            .then(a.cmp(&b))
    });
}

fn error(code: DiagCode, message: String) -> Diagnostic {
    Diagnostic::error(code, codes::CATEGORY, message)
}
