//! Selector resolution (`spec/toolchain.md` 1.6, 10.3, T42, T43;
//! `spec/grammar.md` 4.4, D37, D38, D69): a `.work` or `.patch` selector
//! resolved against a [`SelectorScope`]: the `use` imports of the `.work`
//! file, a module, or fully qualified only. Selectors are resolved by
//! declaration, never as filesystem globs; tests are reached only through
//! namespace wildcards (D69).
//!
//! | Selector | Result |
//! | - | - |
//! | `m.*` | one entry per declaration (tests and properties included) of module `m` and of every module below it (T43), with `FacetSelector::Definition`; empty when no module exists there (an `add` namespace may be new) |
//! | `x`, `a.b.x`, `Cap`, `std.prelude.List.take` | the one declaration, with `Definition` (D38) |
//! | `x.body`, `a.b.x.contract` | the one declaration, with that facet (D37) |
//!
//! The first segment is looked up in the scope's imports (an imported item,
//! or a submodule alias that continues the path), then among the scope
//! module's declarations, then as the start of a fully qualified path. A
//! selector naming no declaration is `Err(unresolved_name)` (or
//! `unresolved_module`); callers map it to their own code (`cpl-work`'s
//! `work_selector_empty`, after trying the file selectors of T42).

use crate::check::{Selected, SelectorScope};
use crate::checker::diag;
use crate::codes;
use cpl_diag::{DeclName, Diagnostic, FacetSelector, SymbolPath};
use cpl_hir::{DefId, ModuleId, Program};
use cpl_syntax::ast;
use serde_json::json;

pub(crate) fn resolve_selector(
    program: &Program,
    sel: &ast::Selector,
    scope: &SelectorScope<'_>,
) -> Result<Vec<Selected>, Diagnostic> {
    let segs: Vec<String> = sel.path.segments.iter().map(|s| s.text.clone()).collect();
    match &sel.kind {
        ast::SelectorKind::Namespace => {
            let path = expand_alias(program, &segs, scope);
            let mut out = Vec::new();
            for m in program.modules() {
                if m.path.len() >= path.len() && m.path[..path.len()] == path[..] {
                    out.extend(m.defs.iter().map(|&def| Selected {
                        def,
                        facet: FacetSelector::Definition,
                    }));
                }
            }
            Ok(out)
        }
        ast::SelectorKind::Declaration => {
            let def = declaration(program, &segs, scope)
                .ok_or_else(|| unresolved(program, sel, &segs))?;
            Ok(vec![Selected {
                def,
                facet: FacetSelector::Definition,
            }])
        }
        ast::SelectorKind::Facet(f) => {
            let facet = FacetSelector::parse(f.kind.as_str()).unwrap_or(FacetSelector::Definition);
            let def = declaration(program, &segs, scope)
                .ok_or_else(|| unresolved(program, sel, &segs))?;
            Ok(vec![Selected { def, facet }])
        }
    }
}

/// The module path of a use item `name` (`use a.b.{name}`).
fn use_target(uses: &[ast::UseDecl], name: &str) -> Option<Vec<String>> {
    uses.iter().find_map(|u| {
        u.items.iter().find(|i| i.name.text == name).map(|i| {
            let mut p: Vec<String> = u.path.segments.iter().map(|s| s.text.clone()).collect();
            p.push(i.name.text.clone());
            p
        })
    })
}

/// A namespace path with a leading submodule alias of the scope expanded.
fn expand_alias(program: &Program, segs: &[String], scope: &SelectorScope<'_>) -> Vec<String> {
    if let Some(first) = segs.first() {
        if let Some(target) = use_target(scope.uses, first) {
            let is_module_path = program
                .modules()
                .any(|m| m.path.len() >= target.len() && m.path[..target.len()] == target[..]);
            if is_module_path {
                let mut path = target;
                path.extend(segs[1..].iter().cloned());
                return path;
            }
        }
    }
    segs.to_vec()
}

/// The declaration a selector path names.
fn declaration(program: &Program, segs: &[String], scope: &SelectorScope<'_>) -> Option<DefId> {
    let first = segs.first()?;
    if let Some(target) = use_target(scope.uses, first) {
        // An imported item, or a submodule alias followed by the rest.
        let mut path = target;
        path.extend(segs[1..].iter().cloned());
        if let Some(d) = qualified(program, &path) {
            return Some(d);
        }
    }
    if let Some(m) = scope.module {
        if let Some(d) = in_module(program, m, segs) {
            return Some(d);
        }
    }
    qualified(program, segs)
}

/// `segs` as `<module path>.<name>` or `<module path>.<Receiver>.<name>`.
fn qualified(program: &Program, segs: &[String]) -> Option<DefId> {
    for split in (1..segs.len()).rev() {
        let module = &segs[..split];
        let Some(m) = program.module_by_path(module) else {
            continue;
        };
        if let Some(d) = in_module(program, m, &segs[split..]) {
            return Some(d);
        }
    }
    None
}

/// A declaration of module `m` named by one segment (an item) or two (a
/// std member stub, `List.take`).
fn in_module(program: &Program, m: ModuleId, rest: &[String]) -> Option<DefId> {
    let name = match rest {
        [n] => DeclName::Ident(n.clone()),
        [receiver, n] => DeclName::Member {
            receiver: receiver.clone(),
            name: n.clone(),
        },
        _ => return None,
    };
    program.lookup(&SymbolPath::new(program.module(m).path.clone(), name))
}

fn unresolved(program: &Program, sel: &ast::Selector, segs: &[String]) -> Diagnostic {
    let dotted = segs.join(".");
    let module = (1..segs.len())
        .rev()
        .find(|&k| program.module_by_path(&segs[..k]).is_some())
        .map(|k| segs[..k].join("."));
    let message = match &module {
        Some(m) => format!(
            "module `{m}` has no declaration `{}`",
            segs[m.split('.').count()..].join(".")
        ),
        None => format!("`{dotted}` names no declaration"),
    };
    let mut d =
        diag(codes::UNRESOLVED_NAME, Some(sel.path.span), message).with_data("name", json!(dotted));
    if let Some(m) = module {
        d = d.with_data("module", json!(m));
    }
    d
}
