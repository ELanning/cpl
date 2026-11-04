//! Collection (`spec/toolchain.md` 3.1 `collect`, 3.7, 4.3-4.5;
//! `spec/semantics.md` 1.2-1.4, 1.8 annotations, S2, S3, S6; stdlib 10.2,
//! 10.3, 10.6): the module graph, the definition table with `DefId`s in
//! T15 order, declaration names and qualified names, stable identities,
//! the annotation vocabulary, the item and test namespaces, the member
//! table, the universe scope, and `StdDefs`.
//!
//! Every type in the definitions collection creates is `Ty::Error` and every
//! body a placeholder; the signature stage (`crate::sig`) fills the types
//! and the body typer the bodies.

use crate::checker::{diag, Annotations, Checker, DeclInfo, FileInfo, Stage};
use crate::codes;
use crate::members::{Member, MemberKind, TypeHead};
use crate::project::ParsedFile;
use crate::resolve::CtorIndex;
use crate::scope::{FileScope, ModuleScope, MEMBER_HEADS, PRELUDE_NAMES, RESERVED_TYPE_NAMES};
use cpl_diag::{DeclName, Diagnostic, FileOrigin, Span, StableId, SymbolPath};
use cpl_hir::{
    Block, BuiltinTypeDef, CapabilityDef, Clause, Def, DefId, DefKind, DefStatus, EffectBound,
    EnumDef, Expr, ExprKind, FieldDef, FnBody, FnDef, LocalId, ModuleDef, ModuleId, OpDef,
    PropertyDef, RefinementDef, StdDefs, StructDef, TestDef, Ty, VariantDef, VerificationPolicy,
    WrapperDef,
};
use cpl_syntax::ast::{self, DeclKind};
use cpl_syntax::{format_fragment, FileKind, Fragment, FragmentMode};
use serde_json::json;
use std::collections::BTreeMap;

/// Run collection.
pub(crate) fn collect(cx: &mut Checker<'_>) {
    collect_files(cx);
    collect_defs(cx);
    enter_names(cx);
    build_universe(cx);
    find_std_defs(cx);
    cx.ctors = CtorIndex::build(&cx.program);
}

/// The `.cpl` files (std and project) and their modules (3.7 rules 1, 2,
/// 4). A project file whose module path is reserved (`std.*`, `gen.*`) has
/// its `reserved_module_path` diagnostic from loading and is not collected:
/// merging it into the standard library, or checking it under a path no
/// reference can reach, would only produce cascades.
fn collect_files(cx: &mut Checker<'_>) {
    let project = cx.project;
    let mut by_path: BTreeMap<(bool, Vec<String>), ModuleId> = BTreeMap::new();
    for (entry, parsed) in project.files.iter().zip(&project.parsed) {
        if entry.kind != FileKind::Cpl {
            continue;
        }
        let ParsedFile::Module(ast) = parsed else {
            continue;
        };
        cx.program.asts.push((entry.file, ast.clone()));
        let is_std = entry.origin == FileOrigin::Std;
        let path: Vec<String> = ast
            .header
            .path
            .segments
            .iter()
            .map(|s| s.text.clone())
            .collect();
        if !is_std && matches!(path.first().map(String::as_str), Some("std") | Some("gen")) {
            continue;
        }
        let module = *by_path.entry((is_std, path.clone())).or_insert_with(|| {
            let id = ModuleId(cx.modules.len() as u32);
            cx.modules.push(ModuleScope {
                id,
                path: path.clone(),
                is_std,
                items: BTreeMap::new(),
                tests: BTreeMap::new(),
            });
            cx.program.modules.push(ModuleDef {
                id,
                path: path.clone(),
                origin: entry.origin.clone(),
                files: Vec::new(),
                defs: Vec::new(),
                docs: None,
            });
            id
        });
        let m = &mut cx.program.modules[module.0 as usize];
        m.files.push(entry.file);
        if let Some(docs) = &ast.docs {
            let text = docs.text();
            m.docs = Some(match m.docs.take() {
                Some(before) => format!("{before}\n\n{text}"),
                None => text,
            });
        }
        cx.files.push(FileInfo {
            id: entry.file,
            ast: ast.clone(),
            module,
            is_std,
            scope: FileScope::default(),
        });
    }
}

/// One `Def` per declaration, in (file, declaration) order (T15).
fn collect_defs(cx: &mut Checker<'_>) {
    let mut ids: BTreeMap<String, Span> = BTreeMap::new();
    let lint_id_missing = cx.config.lints.contains("id_missing");
    for file_index in 0..cx.files.len() {
        let ast = cx.files[file_index].ast.clone();
        let is_std = cx.files[file_index].is_std;
        let module = cx.files[file_index].module;
        let file = cx.files[file_index].id;
        let module_path = cx.modules[module.0 as usize].dotted();
        for (decl_index, decl) in ast.declarations.iter().enumerate() {
            let id = DefId(cx.program.defs.len() as u32);
            let mut diags: Vec<Diagnostic> = Vec::new();
            let annots = annotations(decl, is_std, &mut diags);
            let name_span = name_span(decl);
            let mut failed = diags.iter().any(Diagnostic::is_error);

            // missing_body (1.8, stdlib 10.2).
            let builtin_ok = is_std && annots.builtin.is_some();
            match &decl.kind {
                DeclKind::Fn(f) if f.is_bodiless() && !builtin_ok => {
                    diags.push(
                        diag(
                            codes::MISSING_BODY,
                            Some(name_span),
                            format!("function `{}` has no body: function without a body requires @builtin, which only the standard library may use", f.name.text),
                        )
                        .with_data("name", json!(f.name.text)),
                    );
                    failed = true;
                }
                DeclKind::Type(t)
                    if matches!(t.kind, ast::TypeDeclKind::Opaque(_)) && !builtin_ok =>
                {
                    diags.push(
                        diag(
                            codes::MISSING_BODY,
                            Some(name_span),
                            format!("type `{}` has no definition: type without a definition requires @builtin, which only the standard library may use", t.name.text),
                        )
                        .with_data("name", json!(t.name.text)),
                    );
                    failed = true;
                }
                _ => {}
            }
            if let Some((builtin, span)) = &annots.builtin {
                if codes::BUILTIN_IDS.binary_search(&builtin.as_str()).is_err() {
                    diags.push(
                        diag(
                            codes::STD_BUILTIN_MISSING,
                            Some(*span),
                            format!("unknown builtin \"{builtin}\""),
                        )
                        .with_data("name", json!(builtin)),
                    );
                    failed = true;
                }
            }

            let name = decl_name(decl, &annots);
            let kind = skeleton(decl, &annots, is_std);
            let id_kind = kind.id_kind();

            // Identity (S2, toolchain 4.1, 4.5, T21). A repeated `@id` is one
            // `duplicate_identity` about the later declaration as a whole: it
            // spans that declaration's name token (S63 (3) lists the code
            // among the declaration-level diagnostics; (4) puts the one
            // diagnostic at the later occurrence) and its note is the earlier
            // declaration's name token (`ids` records name tokens).
            let mut stable_id = StableId::provisional(&module_path, &name);
            let mut provisional = true;
            if let Some((value, span)) = &annots.id {
                if let Some(first) = ids.get(value) {
                    diags.push(
                        diag(
                            codes::DUPLICATE_IDENTITY,
                            Some(name_span),
                            format!("identity `{value}` is already used by another declaration"),
                        )
                        .with_symbol(
                            StableId::from_annotation(value).unwrap_or_else(|_| stable_id.clone()),
                            SymbolPath::new(
                                cx.modules[module.0 as usize].path.clone(),
                                name.clone(),
                            )
                            .display(),
                            Some(cpl_diag::Facet::Identity),
                        )
                        .with_note("the identity is first used here", Some(*first)),
                    );
                    failed = true;
                } else {
                    ids.insert(value.clone(), name_span);
                    if let Ok(sid) = StableId::from_annotation(value) {
                        stable_id = sid;
                        provisional = false;
                    }
                }
                if is_generated_format(value) {
                    let prefix = value.split('_').next().unwrap_or("");
                    if prefix != id_kind {
                        diags.push(
                            diag(
                                codes::ID_KIND_MISMATCH,
                                Some(*span),
                                format!("the generated identity `{value}` has the kind prefix `{prefix}`, but the declaration is a `{id_kind}`"),
                            )
                            .with_data("expected", json!(id_kind))
                            .with_data("found", json!(prefix)),
                        );
                    }
                }
            } else if lint_id_missing && !is_std && decl.is_pub() {
                diags.push(
                    diag(
                        codes::ID_MISSING,
                        Some(name_span),
                        format!(
                            "public declaration `{}` has no @id; run `cpl id assign`",
                            decl.display_name()
                        ),
                    )
                    .with_data("name", json!(decl.display_name())),
                );
            }

            let def = Def {
                id,
                stable_id,
                provisional,
                module,
                name,
                is_pub: decl.is_pub(),
                file,
                decl_index,
                span: decl.span,
                docs: decl.docs.as_ref().map(ast::Docs::text),
                policy: VerificationPolicy {
                    prove: annots.prove,
                },
                status: if failed {
                    DefStatus::HasErrors
                } else {
                    DefStatus::Ok
                },
                kind,
            };
            cx.program.defs.push(def);
            cx.program.modules[module.0 as usize].defs.push(id);
            cx.decls.push(DeclInfo {
                file: file_index,
                decl_index,
                annots,
                name_span,
            });
            cx.emit_all(Stage::Resolve, diags);
        }
    }
}

/// The item namespace, the test namespace, and the member table of every
/// module (S3, S6, stdlib S37), with `duplicate_declaration`,
/// `duplicate_test_name`, `empty_test_name`, and `prelude_name_redefined`.
fn enter_names(cx: &mut Checker<'_>) {
    for index in 0..cx.program.defs.len() {
        let id = DefId(index as u32);
        let module = cx.program.defs[index].module;
        let is_std = cx.modules[module.0 as usize].is_std;
        let name = cx.program.defs[index].name.clone();
        let span = cx.decls[index].name_span;
        match &name {
            DeclName::Ident(n) => {
                if !is_std && PRELUDE_NAMES.contains(&n.as_str()) {
                    let d = diag(
                        codes::PRELUDE_NAME_REDEFINED,
                        Some(span),
                        format!("`{n}` is a prelude name and cannot be redefined at module level"),
                    )
                    .with_data("name", json!(n));
                    let d = with_prelude_note(cx, d, n);
                    cx.emit(Stage::Resolve, d);
                    cx.fail(id);
                    continue;
                }
                if let Some(&first) = cx.modules[module.0 as usize].items.get(n) {
                    let d = diag(
                        codes::DUPLICATE_DECLARATION,
                        Some(span),
                        format!(
                            "`{n}` is declared twice in module `{}`",
                            cx.modules[module.0 as usize].dotted()
                        ),
                    )
                    .with_data("name", json!(n))
                    .with_note(
                        "the first declaration is here",
                        Some(cx.decls[first.0 as usize].name_span),
                    );
                    cx.emit(Stage::Resolve, d);
                    cx.fail(id);
                    continue;
                }
                cx.modules[module.0 as usize].items.insert(n.clone(), id);
            }
            DeclName::Text(n) => {
                if n.is_empty() {
                    let d = diag(
                        codes::EMPTY_TEST_NAME,
                        Some(span),
                        "a test or property needs a non-empty name",
                    )
                    .with_data("name", json!(n));
                    cx.emit(Stage::Resolve, d);
                    cx.fail(id);
                    continue;
                }
                if let Some(&first) = cx.modules[module.0 as usize].tests.get(n) {
                    let d = diag(
                        codes::DUPLICATE_TEST_NAME,
                        Some(span),
                        format!(
                            "a test or property named {} already exists in module `{}`",
                            cpl_diag::quote_text(n),
                            cx.modules[module.0 as usize].dotted()
                        ),
                    )
                    .with_data("name", json!(n))
                    .with_note(
                        "the first one is here",
                        Some(cx.decls[first.0 as usize].name_span),
                    );
                    cx.emit(Stage::Resolve, d);
                    cx.fail(id);
                    continue;
                }
                cx.modules[module.0 as usize].tests.insert(n.clone(), id);
            }
            DeclName::Member { receiver, name } => {
                let Some(head) = TypeHead::from_name(receiver) else {
                    continue;
                };
                let kind = cx.decls[index].annots.member.unwrap_or(MemberKind::Method);
                if !cx.members.insert(head, name, Member { def: id, kind }) {
                    let d = diag(
                        codes::DUPLICATE_DECLARATION,
                        Some(span),
                        format!("`{receiver}.{name}` is declared twice"),
                    )
                    .with_data("name", json!(format!("{receiver}.{name}")));
                    cx.emit(Stage::Resolve, d);
                    cx.fail(id);
                }
            }
        }
    }
}

/// Add the note of a `prelude_name_redefined` (9.4: `name`, `notes`): the
/// prelude's own declaration of the name, or what the name is reserved for.
pub(crate) fn with_prelude_note(cx: &Checker<'_>, d: Diagnostic, name: &str) -> Diagnostic {
    if RESERVED_TYPE_NAMES.contains(&name) {
        return d.with_note("reserved for a later edition", None);
    }
    match name {
        "check" => {
            return d.with_note(
                "`check<R>(value)` is the prelude's intrinsic (stdlib 1.4)",
                None,
            )
        }
        "gen" => return d.with_note("`gen` is the prelude's alias of the module std.gen", None),
        "Some" | "None" => {
            return d.with_note(
                format!("`{name}` is a constructor of the prelude's Option"),
                None,
            )
        }
        "Ok" | "Err" => {
            return d.with_note(
                format!("`{name}` is a constructor of the prelude's Result"),
                None,
            )
        }
        _ => {}
    }
    let module = if name == "Gen" {
        ["std", "gen"]
    } else {
        ["std", "prelude"]
    };
    let prelude = cx
        .modules
        .iter()
        .find(|m| m.is_std && m.path == module)
        .and_then(|m| m.items.get(name).copied());
    match prelude {
        Some(def) => d.with_note(
            format!("the prelude declares `{name}` here"),
            Some(cx.decls[def.0 as usize].name_span),
        ),
        None => d,
    }
}

/// Scope 1 and the module-path index (2.1, stdlib S1, S54).
fn build_universe(cx: &mut Checker<'_>) {
    let u = &mut cx.universe;
    for m in &cx.modules {
        u.module_paths.insert(m.path.clone(), m.id);
        for k in 1..m.path.len() {
            u.prefixes.insert(m.path[..k].to_vec());
        }
        if let Some(first) = m.path.first() {
            u.roots.insert(first.clone());
        }
        if m.is_std && m.path == ["std", "prelude"] {
            u.prelude_module = Some(m.id);
        }
        if m.is_std && m.path == ["std", "gen"] {
            u.gen_module = Some(m.id);
        }
    }
    u.roots.insert("std".to_string());
    if let Some(prelude) = u.prelude_module {
        for (name, &def) in &cx.modules[prelude.0 as usize].items {
            if cx.program.def(def).is_pub {
                u.prelude.insert(name.clone(), def);
            }
        }
    }
}

/// `Program::std_defs` (Section 3.3).
fn find_std_defs(cx: &mut Checker<'_>) {
    let find = |module: &[&str], name: &str| -> DefId {
        let path: Vec<String> = module.iter().map(|s| s.to_string()).collect();
        cx.universe
            .module(&path)
            .and_then(|m| cx.modules[m.0 as usize].items.get(name).copied())
            .unwrap_or(DefId(u32::MAX))
    };
    cx.program.std_defs = StdDefs {
        unit: find(&["std", "prelude"], "Unit"),
        option: find(&["std", "prelude"], "Option"),
        result: find(&["std", "prelude"], "Result"),
        constraint_error: find(&["std", "prelude"], "ConstraintError"),
        foreign_error: find(&["std", "foreign"], "ForeignError"),
        positive: find(&["std", "gen"], "Positive"),
    };
}

// ---- annotations ----------------------------------------------------------------

/// Validate a declaration's annotations (1.8, grammar 3.3, stdlib 10.2).
fn annotations(decl: &ast::Declaration, is_std: bool, out: &mut Vec<Diagnostic>) -> Annotations {
    let mut a = Annotations::default();
    let mut seen: BTreeMap<&str, Span> = BTreeMap::new();
    let mut method: Option<Span> = None;
    let mut property: Option<Span> = None;
    let is_fn = matches!(decl.kind, DeclKind::Fn(_));
    for ann in &decl.annotations {
        let name = ann.name.text.as_str();
        let std_only = matches!(name, "builtin" | "method" | "property");
        if !(matches!(
            name,
            "id" | "prove"
                | "intent"
                | "preserve"
                | "protocol"
                | "transition"
                | "message"
                | "schema"
                | "migration"
                | "read_only"
        ) || (std_only && is_std))
        {
            let message = if std_only {
                format!("`@{name}`: builtin forms are permitted only in the standard library")
            } else {
                format!("unknown annotation `@{name}`")
            };
            out.push(
                diag(codes::UNKNOWN_ANNOTATION, Some(ann.span), message)
                    .with_data("name", json!(name)),
            );
            continue;
        }
        if let Some(first) = seen.get(name) {
            out.push(
                diag(
                    codes::DUPLICATE_ANNOTATION,
                    Some(ann.span),
                    format!("`@{name}` is given more than once"),
                )
                .with_data("name", json!(name))
                .with_note("the first one is here", Some(*first)),
            );
            continue;
        }
        seen.insert(name, ann.span);
        let invalid = |message: String| {
            diag(codes::INVALID_ANNOTATION_ARGUMENT, Some(ann.span), message)
                .with_data("name", json!(name))
        };
        let misplaced = |message: &str| {
            diag(
                codes::ANNOTATION_MISPLACED,
                Some(ann.span),
                message.to_string(),
            )
            .with_data("name", json!(name))
        };
        match name {
            "read_only" => {
                if ann.args.is_empty()
                    || ann.args.iter().any(
                        |a| !matches!(&a.kind, ast::LiteralKind::Text(t) if !t.trim().is_empty()),
                    )
                {
                    out.push(invalid(
                        "@read_only takes one or more operation names as text arguments".into(),
                    ));
                }
                if !matches!(&decl.kind, DeclKind::Capability(_)) {
                    out.push(misplaced(
                        "read_only annotations require a capability or interface",
                    ));
                }
            }
            "intent" | "preserve" | "protocol" | "transition" | "message" | "schema"
            | "migration" => {
                let count = match name {
                    "intent" | "preserve" => 1,
                    "message" | "schema" => 2,
                    _ => 3,
                };
                if ann.args.len() != count
                    || ann.args.iter().any(
                        |a| !matches!(&a.kind, ast::LiteralKind::Text(t) if !t.trim().is_empty()),
                    )
                {
                    out.push(invalid(format!(
                        "@{name} takes {count} nonempty text arguments"
                    )));
                }
                if matches!(name, "transition" | "message" | "migration") && !is_fn {
                    out.push(misplaced(
                        "transition, message, and migration annotations require a function",
                    ));
                }
                if matches!(name, "protocol" | "schema")
                    && !matches!(
                        &decl.kind,
                        DeclKind::Struct(_)
                            | DeclKind::Enum(_)
                            | DeclKind::Type(ast::TypeDecl {
                                kind: ast::TypeDeclKind::Wrapper(_),
                                ..
                            })
                    )
                {
                    out.push(misplaced(
                        "protocol and schema annotations require a nominal value type",
                    ));
                }
            }
            "id" => match single_text(ann) {
                None => out.push(invalid(
                    "@id takes exactly one text argument, such as @id(\"fn_k7w3qz2mb6ptx4ve\")"
                        .to_string(),
                )),
                Some(value) => match StableId::from_annotation(value) {
                    Ok(_) => a.id = Some((value.to_string(), ann.span)),
                    Err(e) => out.push(invalid(e.to_string())),
                },
            },
            "prove" => {
                if !ann.args.is_empty() {
                    out.push(invalid("@prove takes no argument".to_string()));
                } else if !is_fn {
                    out.push(misplaced("@prove is permitted on fn only"));
                } else {
                    a.prove = true;
                }
            }
            "builtin" => match single_text(ann) {
                None => out.push(invalid(
                    "@builtin takes exactly one text argument: the registry id".to_string(),
                )),
                Some(id) => {
                    let placement = match &decl.kind {
                        DeclKind::Fn(f) => {
                            if f.is_bodiless() {
                                None
                            } else {
                                Some("a builtin declaration has no body")
                            }
                        }
                        DeclKind::Type(t) => {
                            if matches!(t.kind, ast::TypeDeclKind::Opaque(_)) {
                                None
                            } else {
                                Some("a builtin declaration has no body")
                            }
                        }
                        _ => {
                            Some("@builtin is permitted on a bodiless fn or a definition-less type")
                        }
                    };
                    match placement {
                        Some(message) => out.push(misplaced(message)),
                        None => a.builtin = Some((id.to_string(), ann.span)),
                    }
                }
            },
            _ => {
                // method, property
                if !ann.args.is_empty() {
                    out.push(invalid(format!("@{name} takes no argument")));
                } else if !is_fn {
                    out.push(misplaced(&format!("@{name} is permitted on fn only")));
                } else if name == "method" {
                    method = Some(ann.span);
                } else {
                    property = Some(ann.span);
                }
            }
        }
    }
    if let DeclKind::Fn(f) = &decl.kind {
        let receiver_ok = f
            .params
            .positional
            .first()
            .is_some_and(|p| is_member_head(&p.ty.path));
        let misplaced = |span: Span, message: &str, name: &str| {
            diag(codes::ANNOTATION_MISPLACED, Some(span), message.to_string())
                .with_data("name", json!(name))
        };
        match (method, property) {
            (Some(m), Some(p)) => {
                let (later, name) = if p.start > m.start {
                    (p, "property")
                } else {
                    (m, "method")
                };
                out.push(misplaced(later, "choose @method or @property", name));
            }
            (Some(m), None) => {
                if f.params.positional.is_empty() {
                    out.push(misplaced(
                        m,
                        "@method requires a receiver parameter",
                        "method",
                    ));
                } else if !receiver_ok {
                    out.push(misplaced(
                        m,
                        "members may be declared only on prelude types",
                        "method",
                    ));
                } else {
                    a.member = Some(MemberKind::Method);
                }
            }
            (None, Some(p)) => {
                if f.params.positional.len() > 1 {
                    out.push(misplaced(
                        p,
                        "@property takes at most a receiver",
                        "property",
                    ));
                } else if f.params.positional.is_empty() {
                    a.module_property = true;
                } else if !receiver_ok {
                    out.push(misplaced(
                        p,
                        "members may be declared only on prelude types",
                        "property",
                    ));
                } else {
                    a.member = Some(MemberKind::Property);
                }
            }
            (None, None) => {}
        }
    }
    a
}

/// The text of a one-text-argument annotation (`@id("...")`, `@builtin("...")`).
fn single_text(ann: &ast::Annotation) -> Option<&str> {
    match ann.args.as_slice() {
        [ast::Literal {
            kind: ast::LiteralKind::Text(t),
            ..
        }] if ann.parens => Some(t),
        _ => None,
    }
}

/// A receiver type naming a prelude type head (stdlib 10.2).
fn is_member_head(path: &ast::TypePath) -> bool {
    let modules: Vec<&str> = path.modules.iter().map(|m| m.text.as_str()).collect();
    (modules.is_empty() || modules == ["std", "prelude"])
        && MEMBER_HEADS.contains(&path.name.text.as_str())
}

/// `^(fn|ty|cap|test|prop)_[a-z2-7]{16}$` (toolchain 4.1): the generated
/// format `id_kind_mismatch` tests (T21). (`cpl_index::is_generated_format`
/// is the same test; `cpl-sema` cannot depend on `cpl-index`.)
pub(crate) fn is_generated_format(value: &str) -> bool {
    let Some((kind, rest)) = value.split_once('_') else {
        return false;
    };
    matches!(kind, "fn" | "ty" | "cap" | "test" | "prop")
        && rest.len() == 16
        && rest
            .bytes()
            .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
}

// ---- names and skeletons --------------------------------------------------------

/// The declaration's name token, or the `TEXT` name of a test or property
/// (semantics S63 (3)).
pub(crate) fn name_span(decl: &ast::Declaration) -> Span {
    match &decl.kind {
        DeclKind::Test(t) => t.name.span,
        DeclKind::Property(p) => p.name.span,
        _ => decl.name().map_or(decl.span, |n| n.span),
    }
}

/// `DeclName` (toolchain 4.4): a std member stub is `Member { receiver, name }`.
fn decl_name(decl: &ast::Declaration, annots: &Annotations) -> DeclName {
    match &decl.kind {
        DeclKind::Test(t) => DeclName::Text(t.name.value.clone()),
        DeclKind::Property(p) => DeclName::Text(p.name.value.clone()),
        DeclKind::Fn(f) if annots.member.is_some() => DeclName::Member {
            receiver: f.params.positional[0].ty.path.name.text.clone(),
            name: f.name.text.clone(),
        },
        _ => DeclName::Ident(decl.name().map(|n| n.text.clone()).unwrap_or_default()),
    }
}

/// The names of a generic parameter list.
pub(crate) fn generic_names(g: &Option<ast::GenericParams>) -> Vec<String> {
    g.as_ref()
        .map(|g| g.params.iter().map(|p| p.text.clone()).collect())
        .unwrap_or_default()
}

/// The canonical clause text (toolchain 5.2, semantics 9.2 `clause`): the
/// formatter's canonical rendering, on one line. 9.2 defines `clause` as
/// "the formatter's rendering on one line with canonical spacing, source
/// line breaks removed" (a stable key) and toolchain 5.2 says every clause
/// is one line, but the formatter prints the blocks of an `if` or a
/// `match` on several lines (2.4 rule 8, kept by `FragmentMode::Canonical`),
/// and no one-line form of a block is defined. Removing those line breaks
/// is the reading taken here: each line break of the canonical rendering,
/// with the indentation after it, becomes one space (`if n > 0 { let m =
/// n m < 10 } else { true }`). Canonical text has no comments and escapes
/// every line break inside a text literal, so every line break it has is
/// layout. This is also the HIR's `Clause::text` (toolchain 3.3), which the
/// hole report's `requires`/`ensures`/`invariants` (8.3) and the
/// evaluator's `clause` fields reuse.
pub(crate) fn clause_text(e: &ast::Expr) -> String {
    one_line(&format_fragment(Fragment::Expr(e), FragmentMode::Canonical))
}

/// `text` with every line break and the indentation after it replaced by
/// one space.
fn one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(if i > 0 { line.trim_start() } else { line });
    }
    out
}

/// A clause whose expression the body typer has not typed yet.
pub(crate) fn placeholder_clause(e: &ast::Expr, span: Span) -> Clause {
    Clause {
        expr: Expr::new(ExprKind::Error, Ty::Error, e.span),
        text: clause_text(e),
        span,
    }
}

/// A block the body typer has not typed yet.
pub(crate) fn placeholder_block(span: Span) -> Block {
    Block {
        stmts: Vec::new(),
        tail: None,
        ty: Ty::Error,
        span,
    }
}

/// The `DefKind` of a declaration with every type `Ty::Error` and every
/// body a placeholder: names, generics, members, and spans are final.
fn skeleton(decl: &ast::Declaration, annots: &Annotations, is_std: bool) -> DefKind {
    match &decl.kind {
        DeclKind::Type(t) => match &t.kind {
            ast::TypeDeclKind::Wrapper(_) => DefKind::Wrapper(WrapperDef { inner: Ty::Error }),
            ast::TypeDeclKind::Refinement(r) => DefKind::Refinement(RefinementDef {
                base: Ty::Error,
                self_local: LocalId(0),
                predicate: placeholder_clause(&r.predicate, r.span),
                locals: Vec::new(),
            }),
            ast::TypeDeclKind::Opaque(o) => DefKind::BuiltinType(BuiltinTypeDef {
                generics: generic_names(&o.generic_params),
                builtin: annots
                    .builtin
                    .as_ref()
                    .filter(|_| is_std)
                    .map(|(id, _)| id.clone())
                    .unwrap_or_default(),
            }),
        },
        DeclKind::Struct(s) => DefKind::Struct(StructDef {
            generics: generic_names(&s.generic_params),
            invariant: s.invariant.as_ref().map(|inv| RefinementDef {
                base: Ty::Error,
                self_local: LocalId(0),
                predicate: placeholder_clause(&inv.expr, inv.span),
                locals: Vec::new(),
            }),
            fields: s
                .fields
                .iter()
                .map(|f| FieldDef {
                    name: f.name.text.clone(),
                    ty: Ty::Error,
                    docs: f.docs.as_ref().map(ast::Docs::text),
                    span: f.span,
                })
                .collect(),
        }),
        DeclKind::Enum(e) => DefKind::Enum(EnumDef {
            generics: generic_names(&e.generic_params),
            variants: e
                .variants
                .iter()
                .map(|v| VariantDef {
                    name: v.name.text.clone(),
                    payload: vec![Ty::Error; v.payload.len()],
                    docs: v.docs.as_ref().map(ast::Docs::text),
                    span: v.span,
                })
                .collect(),
        }),
        DeclKind::Capability(c) => DefKind::Capability(CapabilityDef {
            ops: c
                .operations
                .iter()
                .map(|op| OpDef {
                    name: op.name.text.clone(),
                    params: Vec::new(),
                    ret: Ty::Error,
                    requires: Vec::new(),
                    ensures: Vec::new(),
                    result_local: None,
                    locals: Vec::new(),
                    docs: op.docs.as_ref().map(ast::Docs::text),
                    span: op.span,
                })
                .collect(),
        }),
        DeclKind::Fn(f) => DefKind::Fn(FnDef {
            generics: generic_names(&f.generic_params),
            params: Vec::new(),
            using: Vec::new(),
            ret: Ty::Error,
            effects: EffectBound::default(),
            requires: Vec::new(),
            ensures: Vec::new(),
            result_local: None,
            decreases: None,
            locals: Vec::new(),
            body: match (&annots.builtin, is_std) {
                (Some((id, _)), true) => FnBody::Builtin(id.clone()),
                _ => FnBody::Block(placeholder_block(
                    f.body.as_ref().map_or(f.span, |b| b.span),
                )),
            },
        }),
        DeclKind::Test(t) => DefKind::Test(TestDef {
            locals: Vec::new(),
            body: placeholder_block(t.body.span),
        }),
        DeclKind::Property(p) => DefKind::Property(PropertyDef {
            binders: Vec::new(),
            locals: Vec::new(),
            body: placeholder_block(p.body.span),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_format() {
        // Toolchain 4.1: the example id is of the generated format; the
        // earlier example, with the digit `9`, is accepted but is not.
        assert!(is_generated_format("fn_k7w3qz2mb6ptx4ve"));
        assert!(!is_generated_format("fn_k7w3qz2mb9ptx4ve"));
        assert!(is_generated_format("fn_k7w3qz2mbaptx4ve"));
        assert!(!is_generated_format("pgn_7c2d"));
        assert!(!is_generated_format("fn_one2mb9ptx4vek7w3"));
        assert!(is_generated_format("ty_abcdefghijklmnop"));
        assert!(!is_generated_format("ty_abcdefghijklmno"));
    }
}
