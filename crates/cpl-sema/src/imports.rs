//! `use` declarations (`spec/semantics.md` 1.6, S5, S6, S62; stdlib S3,
//! S28; toolchain T20, 6.5): every file's imports are resolved into its
//! [`FileScope`], recorded in `Program::imports`, and, after every
//! resolution of the file has run, reported `unused_import` when nothing
//! went through them.
//!
//! A diagnosed `use` item (`unresolved_module`, `unresolved_name`,
//! `private_item`, `generator_import`, `prelude_name_redefined`,
//! `import_conflicts_with_declaration`) binds its name to
//! [`Binding::Error`], so that uses of the name are error-typed and not
//! reported again, and is never `unused_import` (S62); a `duplicate_import`
//! keeps the first import's binding.

use crate::checker::{diag, Checker, Stage};
use crate::codes;
use crate::scope::{Binding, FileScope, ImportBinding, Universe};
use cpl_diag::{Diagnostic, Span};
use cpl_hir::{DefKind, Import, ImportTarget, ModuleId};
use cpl_syntax::ast;
use serde_json::json;
use std::cell::Cell;

/// Resolve the imports of every collected file.
pub(crate) fn resolve_imports(cx: &mut Checker<'_>) {
    for index in 0..cx.files.len() {
        let ast = cx.files[index].ast.clone();
        let module = cx.files[index].module;
        let file = cx.files[index].id;
        let mut scope = FileScope::default();
        let mut diags = Vec::new();
        let mut imports = Vec::new();
        for use_decl in &ast.uses {
            resolve_use(
                cx,
                module,
                use_decl,
                &mut scope,
                &mut diags,
                |target, span| imports.push(Import { file, target, span }),
            );
        }
        cx.files[index].scope = scope;
        cx.program.imports.extend(imports);
        cx.emit_all(Stage::Resolve, diags);
    }
}

/// What a `use` path denotes.
enum UsePath {
    Module(ModuleId),
    /// A proper prefix of module paths (no module has exactly this path).
    Prefix,
    Missing,
}

fn resolve_use(
    cx: &Checker<'_>,
    module: ModuleId,
    use_decl: &ast::UseDecl,
    scope: &mut FileScope,
    diags: &mut Vec<Diagnostic>,
    mut record: impl FnMut(ImportTarget, Span),
) {
    let path: Vec<String> = use_decl
        .path
        .segments
        .iter()
        .map(|s| s.text.clone())
        .collect();
    let dotted = path.join(".");
    let kind = match cx.universe.module(&path) {
        Some(m) => UsePath::Module(m),
        None if cx.universe.prefixes.contains(&path) => UsePath::Prefix,
        None => UsePath::Missing,
    };
    if let UsePath::Missing = kind {
        diags.push(
            diag(
                codes::UNRESOLVED_MODULE,
                Some(use_decl.path.span),
                format!("no module `{dotted}` exists in the program"),
            )
            .with_data("module", json!(dotted)),
        );
        for item in &use_decl.items {
            bind_error(scope, item);
        }
        return;
    }
    for item in &use_decl.items {
        let name = item.name.text.as_str();
        let mut sub = path.clone();
        sub.push(name.to_string());
        let resolved: Result<Binding, Diagnostic> = match kind {
            UsePath::Module(m) => {
                let scope_m = &cx.modules[m.0 as usize];
                if let Some(&def) = scope_m.items.get(name) {
                    let d = cx.program.def(def);
                    if !d.is_pub && m != module {
                        Err(diag(
                            codes::PRIVATE_ITEM,
                            Some(item.span),
                            format!("`{}` is private to module `{dotted}`", cx.program.display_path(def)),
                        )
                        .with_symbol(d.stable_id.clone(), cx.program.display_path(def), None))
                    } else if Some(m) == cx.universe.gen_module {
                        Err(generator_import(item, name))
                    } else {
                        Ok(Binding::Def(def))
                    }
                } else if let Some(b) = submodule(&cx.universe, &sub) {
                    Ok(b)
                } else if Some(m) == cx.universe.gen_module && name == "Gen" {
                    Err(generator_import(item, name))
                } else {
                    Err(diag(
                        codes::UNRESOLVED_NAME,
                        Some(item.span),
                        format!("module `{dotted}` has no item `{name}`{}", member_hint(cx, name)),
                    )
                    .with_data("name", json!(name))
                    .with_data("module", json!(dotted)))
                }
            }
            UsePath::Prefix => match submodule(&cx.universe, &sub) {
                Some(b) => Ok(b),
                None => Err(diag(
                    codes::UNRESOLVED_MODULE,
                    Some(item.span),
                    format!("no module `{dotted}` exists in the program (it is only a prefix of module paths), so `{name}` cannot be imported from it"),
                )
                .with_data("module", json!(dotted))),
            },
            UsePath::Missing => unreachable!("handled above"),
        };
        let binding = match resolved {
            Ok(b) => b,
            Err(d) => {
                diags.push(d);
                bind_error(scope, item);
                continue;
            }
        };
        if let Some(target) = target_of(&binding) {
            record(target, item.span);
        }
        // The name rules of 1.6 and S6.
        if Universe::is_prelude_name(name)
            && cx.universe.prelude_binding(name) != Some(binding.clone())
        {
            let d = diag(
                codes::PRELUDE_NAME_REDEFINED,
                Some(item.span),
                format!("`{name}` is a prelude name and cannot be imported under that name"),
            )
            .with_data("name", json!(name));
            diags.push(crate::collect::with_prelude_note(cx, d, name));
            bind_error(scope, item);
            continue;
        }
        if let Some(&decl) = cx.modules[module.0 as usize].items.get(name) {
            diags.push(
                diag(
                    codes::IMPORT_CONFLICTS_WITH_DECLARATION,
                    Some(item.span),
                    format!(
                        "the import `{name}` conflicts with the declaration `{name}` of module `{}`",
                        cx.modules[module.0 as usize].dotted()
                    ),
                )
                .with_data("name", json!(name))
                .with_note("the declaration is here", Some(cx.decls[decl.0 as usize].name_span)),
            );
            bind_error(scope, item);
            continue;
        }
        if let Some(first) = scope.imports.get(name) {
            diags.push(
                diag(
                    codes::DUPLICATE_IMPORT,
                    Some(item.span),
                    format!("`{name}` is imported twice in this file"),
                )
                .with_data("name", json!(name))
                .with_note("the first import is here", Some(first.span)),
            );
            continue;
        }
        scope.imports.insert(
            name.to_string(),
            ImportBinding {
                binding,
                span: item.span,
                used: Cell::new(false),
            },
        );
    }
}

fn generator_import(item: &ast::UseItem, name: &str) -> Diagnostic {
    let message = if name == "Gen" {
        "`Gen` is not nameable in edition 0.1".to_string()
    } else {
        format!("std.gen is in scope as gen; write gen.{name}")
    };
    diag(codes::GENERATOR_IMPORT, Some(item.span), message).with_data("name", json!(name))
}

/// A note for an import of a member name (`len`, `take`): members are not
/// free names (stdlib S37).
fn member_hint(cx: &Checker<'_>, name: &str) -> String {
    let is_member = cx.program.defs().any(|d| {
        matches!(&d.name, cpl_diag::DeclName::Member { name: n, .. } if n == name)
            && matches!(d.kind, DefKind::Fn(_))
    });
    if is_member {
        format!(" (`{name}` is a member of a prelude type, reached through its receiver)")
    } else {
        String::new()
    }
}

/// The submodule alias `sub` names, if it is a module or a module prefix.
fn submodule(universe: &Universe, sub: &[String]) -> Option<Binding> {
    match universe.module(sub) {
        Some(m) => Some(Binding::Module(m)),
        None if universe.prefixes.contains(sub) => Some(Binding::Namespace(sub.to_vec())),
        None => None,
    }
}

fn target_of(binding: &Binding) -> Option<ImportTarget> {
    match binding {
        Binding::Def(d) => Some(ImportTarget::Def(*d)),
        Binding::Module(m) => Some(ImportTarget::Module(*m)),
        Binding::Namespace(p) => Some(ImportTarget::Namespace(p.clone())),
        Binding::Error => None,
    }
}

/// Bind a diagnosed item's name to the erroneous binding (S62), unless an
/// earlier import of the name stands.
fn bind_error(scope: &mut FileScope, item: &ast::UseItem) {
    scope
        .imports
        .entry(item.name.text.clone())
        .or_insert(ImportBinding {
            binding: Binding::Error,
            span: item.span,
            used: Cell::new(true),
        });
}

/// `unused_import` (1.6) for every import nothing resolved through. Runs
/// after every resolution of the program (signatures, bodies, clauses, and
/// the constructor resolutions of S10 that the body typer completes once it
/// knows the expected type).
pub(crate) fn report_unused(cx: &mut Checker<'_>) {
    let mut diags = Vec::new();
    for file in &cx.files {
        let mut unused: Vec<(&String, &ImportBinding)> = file
            .scope
            .imports
            .iter()
            .filter(|(_, i)| i.binding != Binding::Error && !i.used.get())
            .collect();
        unused.sort_by_key(|(_, i)| i.span);
        for (name, import) in unused {
            diags.push(
                diag(
                    codes::UNUSED_IMPORT,
                    Some(import.span),
                    format!("`{name}` is imported but never used in this file"),
                )
                .with_data("name", json!(name)),
            );
        }
    }
    cx.emit_all(Stage::Resolve, diags);
}
