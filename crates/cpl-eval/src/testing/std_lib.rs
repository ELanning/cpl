//! Loading the five std stub files (`spec/stdlib.md` Section 11) into a
//! [`ProgramBuilder`]: parse with `cpl-syntax`, then lower by hand what
//! the evaluator needs (a small subset of `cpl-sema`'s work, enough for the
//! stub files and checked against them).
//!
//! Order: files in sorted name order (toolchain 3.6 and T15: std files get
//! the lowest `FileId`s), one module per file, definitions in declaration
//! order. Lowering runs in passes so every reference resolves: (1) declare
//! every definition; (2) refinements (base, predicate, class); (3) the
//! other types; (4) function signatures; (5) the `requires` clauses.

use super::ProgramBuilder;
use cpl_diag::json::BindingKind;
use cpl_diag::{DeclName, FileId, FileOrigin, Span};
use cpl_hir::{
    BuiltinTypeDef, Clause, DefId, DefKind, EffectBound, EnumDef, Expr, ExprKind, FieldDef, FnBody,
    FnDef, Lit, LocalDef, LocalId, ModuleDef, ModuleId, ParamDef, RefinementDef, StdDefs,
    StructDef, Ty, VariantDef, WrapperDef,
};
use cpl_syntax::ast;
use std::collections::HashMap;
use std::sync::Arc;

/// The stub files, embedded (the same bytes `cpl-sema/build.rs` embeds).
pub(super) const STD_FILES: [(&str, &str); 5] = [
    ("std/bytes.cpl", include_str!("../../../../std/bytes.cpl")),
    (
        "std/foreign.cpl",
        include_str!("../../../../std/foreign.cpl"),
    ),
    ("std/gen.cpl", include_str!("../../../../std/gen.cpl")),
    (
        "std/prelude.cpl",
        include_str!("../../../../std/prelude.cpl"),
    ),
    ("std/text.cpl", include_str!("../../../../std/text.cpl")),
];

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Form {
    Struct,
    Enum,
    Wrapper,
    Refinement,
    Builtin,
    Fn,
}

struct File {
    file: FileId,
    module: ModuleId,
    ast: Arc<ast::Module>,
    text: &'static str,
}

struct Decl {
    def: DefId,
    file: usize,
    index: usize,
    form: Form,
}

/// The name tables of the std unit.
struct Names {
    /// Bare type name -> declaration (std type names are unique across the
    /// five modules).
    types: HashMap<String, (DefId, Form)>,
    /// (receiver head, member name) -> `@method`/`@property` stub.
    members: HashMap<(String, String), DefId>,
}

pub(super) fn load(b: &mut ProgramBuilder) {
    // Files and modules.
    let mut files = Vec::new();
    for (path, text) in STD_FILES {
        let file = b
            .source_map
            .add(path.to_string(), FileOrigin::Std, text.to_string());
        let parsed = cpl_syntax::parse_module(text, file);
        let module_ast = parsed.value.unwrap_or_else(|| {
            panic!(
                "{path} does not parse: {:?}",
                parsed
                    .diagnostics
                    .iter()
                    .map(|d| &d.message)
                    .collect::<Vec<_>>()
            )
        });
        let module = ModuleId(b.modules.len() as u32);
        b.modules.push(ModuleDef {
            id: module,
            path: module_ast
                .header
                .path
                .segments
                .iter()
                .map(|s| s.text.clone())
                .collect(),
            origin: FileOrigin::Std,
            files: vec![file],
            defs: Vec::new(),
            docs: module_ast.docs.as_ref().map(ast::Docs::text),
        });
        let module_ast = Arc::new(module_ast);
        b.asts.push((file, Arc::clone(&module_ast)));
        files.push(File {
            file,
            module,
            ast: module_ast,
            text,
        });
    }

    // Pass 1: declare.
    let mut decls = Vec::new();
    let mut names = Names {
        types: HashMap::new(),
        members: HashMap::new(),
    };
    for (fi, f) in files.iter().enumerate() {
        for (index, decl) in f.ast.declarations.iter().enumerate() {
            let (name, form) = declared_name(decl);
            let placeholder = DefKind::Struct(StructDef {
                invariant: None,
                generics: Vec::new(),
                fields: Vec::new(),
            });
            let def = b.push_def_at(
                f.module,
                f.file,
                index,
                decl.span,
                name.clone(),
                decl.is_pub(),
                decl.docs.as_ref().map(ast::Docs::text),
                placeholder,
            );
            match &name {
                DeclName::Member { receiver, name } => {
                    names.members.insert((receiver.clone(), name.clone()), def);
                }
                DeclName::Ident(n) if form != Form::Fn => {
                    let previous = names.types.insert(n.clone(), (def, form));
                    assert!(previous.is_none(), "std type {n} is declared twice");
                }
                _ => {}
            }
            decls.push(Decl {
                def,
                file: fi,
                index,
                form,
            });
        }
    }

    // Pass 2: refinements (their classes are needed by every later type).
    for d in decls.iter().filter(|d| d.form == Form::Refinement) {
        let f = &files[d.file];
        let ast::DeclKind::Type(t) = &f.ast.declarations[d.index].kind else {
            unreachable!("a refinement is a type declaration")
        };
        let ast::TypeDeclKind::Refinement(r) = &t.kind else {
            unreachable!("classified as a refinement")
        };
        let base = lower_ty(b, &names, &r.base, &[]);
        let mut locals = vec![LocalDef {
            id: LocalId(0),
            name: "self".to_string(),
            ty: base.clone(),
            mutable: false,
            kind: BindingKind::SelfValue,
            span: r.span,
        }];
        let mut visible = vec![LocalId(0)];
        let predicate = lower_expr(b, &names, &r.predicate, &mut locals, &mut visible);
        b.class_for(&base, &predicate, d.def);
        let text = slice(f.text, r.predicate.span).to_string();
        // The clause runs from its `where` keyword, as every clause span runs
        // from its keyword (toolchain 3.3): the AST keeps no span for the
        // keyword, so it is found between the base and the predicate, as the
        // checker finds it.
        let between = &f.text[r.base.span.end as usize..r.predicate.span.start as usize];
        let span = match between.find("where") {
            Some(at) => Span::new(
                r.predicate.span.file,
                r.base.span.end + at as u32,
                r.predicate.span.end,
            ),
            None => r.predicate.span,
        };
        b.defs[d.def.0 as usize].kind = DefKind::Refinement(RefinementDef {
            base,
            self_local: LocalId(0),
            predicate: Clause {
                expr: predicate,
                text,
                span,
            },
            locals,
        });
    }

    // Pass 3: the other types.
    for d in decls.iter().filter(|d| {
        matches!(
            d.form,
            Form::Struct | Form::Enum | Form::Wrapper | Form::Builtin
        )
    }) {
        let decl = &files[d.file].ast.declarations[d.index];
        let kind = match &decl.kind {
            ast::DeclKind::Struct(s) => {
                let generics = generic_names(s.generic_params.as_ref());
                DefKind::Struct(StructDef {
                    invariant: None,
                    fields: s
                        .fields
                        .iter()
                        .map(|field| FieldDef {
                            name: field.name.text.clone(),
                            ty: lower_ty(b, &names, &field.ty, &generics),
                            docs: field.docs.as_ref().map(ast::Docs::text),
                            span: field.span,
                        })
                        .collect(),
                    generics,
                })
            }
            ast::DeclKind::Enum(e) => {
                let generics = generic_names(e.generic_params.as_ref());
                DefKind::Enum(EnumDef {
                    variants: e
                        .variants
                        .iter()
                        .map(|v| VariantDef {
                            name: v.name.text.clone(),
                            payload: v
                                .payload
                                .iter()
                                .map(|t| lower_ty(b, &names, t, &generics))
                                .collect(),
                            docs: v.docs.as_ref().map(ast::Docs::text),
                            span: v.span,
                        })
                        .collect(),
                    generics,
                })
            }
            ast::DeclKind::Type(t) => match &t.kind {
                ast::TypeDeclKind::Wrapper(w) => DefKind::Wrapper(WrapperDef {
                    inner: lower_ty(b, &names, &w.payload, &[]),
                }),
                ast::TypeDeclKind::Opaque(o) => DefKind::BuiltinType(BuiltinTypeDef {
                    generics: generic_names(o.generic_params.as_ref()),
                    builtin: builtin_id(decl).unwrap_or_else(|| {
                        panic!("the opaque std type {} has no @builtin", t.name.text)
                    }),
                }),
                ast::TypeDeclKind::Refinement(_) => unreachable!("lowered in pass 2"),
            },
            _ => unreachable!("classified as a type"),
        };
        b.defs[d.def.0 as usize].kind = kind;
    }

    // Pass 4: function signatures.
    for d in decls.iter().filter(|d| d.form == Form::Fn) {
        let decl = &files[d.file].ast.declarations[d.index];
        let ast::DeclKind::Fn(f) = &decl.kind else {
            unreachable!("classified as a function")
        };
        let generics = generic_names(f.generic_params.as_ref());
        let mut locals = Vec::new();
        let mut params = Vec::new();
        for param in &f.params.positional {
            let ty = lower_ty(b, &names, &param.ty, &generics);
            let local = LocalId(locals.len() as u32);
            locals.push(LocalDef {
                id: local,
                name: param.name.text.clone(),
                ty: ty.clone(),
                mutable: false,
                kind: BindingKind::Parameter,
                span: param.span,
            });
            params.push(ParamDef {
                local,
                name: param.name.text.clone(),
                ty,
                span: param.span,
            });
        }
        let body = match builtin_id(decl) {
            Some(id) => FnBody::Builtin(id),
            None => panic!(
                "the std function {} has no @builtin (stdlib S36)",
                f.name.text
            ),
        };
        b.defs[d.def.0 as usize].kind = DefKind::Fn(FnDef {
            generics,
            params,
            using: Vec::new(),
            ret: lower_ty(b, &names, &f.ret, &generic_names(f.generic_params.as_ref())),
            effects: EffectBound::default(),
            requires: Vec::new(),
            ensures: Vec::new(),
            result_local: None,
            decreases: None,
            locals,
            body,
        });
    }

    // Pass 5: requires clauses (ensures are never evaluated, S39).
    for d in decls.iter().filter(|d| d.form == Form::Fn) {
        let f = &files[d.file];
        let ast::DeclKind::Fn(fn_ast) = &f.ast.declarations[d.index].kind else {
            unreachable!("classified as a function")
        };
        let DefKind::Fn(fd) = &b.defs[d.def.0 as usize].kind else {
            unreachable!("lowered in pass 4")
        };
        let mut locals = fd.locals.clone();
        let mut requires = Vec::new();
        for clause in fn_ast.requires() {
            let mut visible: Vec<LocalId> = (0..locals.len() as u32).map(LocalId).collect();
            let expr = lower_expr(b, &names, &clause.expr, &mut locals, &mut visible);
            requires.push(Clause {
                expr,
                text: slice(f.text, clause.expr.span).to_string(),
                span: clause.span,
            });
        }
        if let DefKind::Fn(fd) = &mut b.defs[d.def.0 as usize].kind {
            fd.requires = requires;
            fd.locals = locals;
        }
    }

    // StdDefs.
    let ty = |name: &str| {
        names
            .types
            .get(name)
            .map(|(def, _)| *def)
            .unwrap_or_else(|| panic!("std declares no {name}"))
    };
    b.std_defs = StdDefs {
        unit: ty("Unit"),
        option: ty("Option"),
        result: ty("Result"),
        constraint_error: ty("ConstraintError"),
        foreign_error: ty("ForeignError"),
        positive: ty("Positive"),
    };
}

/// The definition's name (stdlib S37: a receiver member is
/// `Member { receiver, name }`) and form.
fn declared_name(decl: &ast::Declaration) -> (DeclName, Form) {
    match &decl.kind {
        ast::DeclKind::Fn(f) => {
            let member = decl.has_annotation("method") || decl.has_annotation("property");
            let name = match f.params.positional.first() {
                Some(receiver) if member => DeclName::Member {
                    receiver: receiver.ty.path.name.text.clone(),
                    name: f.name.text.clone(),
                },
                _ => DeclName::Ident(f.name.text.clone()),
            };
            (name, Form::Fn)
        }
        ast::DeclKind::Type(t) => {
            let form = match t.kind {
                ast::TypeDeclKind::Wrapper(_) => Form::Wrapper,
                ast::TypeDeclKind::Refinement(_) => Form::Refinement,
                ast::TypeDeclKind::Opaque(_) => Form::Builtin,
            };
            (DeclName::Ident(t.name.text.clone()), form)
        }
        ast::DeclKind::Struct(s) => (DeclName::Ident(s.name.text.clone()), Form::Struct),
        ast::DeclKind::Enum(e) => (DeclName::Ident(e.name.text.clone()), Form::Enum),
        other => panic!("std declares an unexpected item: {other:?}"),
    }
}

/// The argument of `@builtin("<id>")`.
fn builtin_id(decl: &ast::Declaration) -> Option<String> {
    match &decl.annotation("builtin")?.args.first()?.kind {
        ast::LiteralKind::Text(id) => Some(id.clone()),
        _ => None,
    }
}

fn generic_names(params: Option<&ast::GenericParams>) -> Vec<String> {
    params.map_or_else(Vec::new, |g| {
        g.params.iter().map(|n| n.text.clone()).collect()
    })
}

fn slice(text: &str, span: Span) -> &str {
    &text[span.start as usize..span.end as usize]
}

/// A std type expression (bare names only, as the stub files write them).
fn lower_ty(b: &ProgramBuilder, names: &Names, t: &ast::Type, generics: &[String]) -> Ty {
    let name = t.path.name.text.as_str();
    let args: Vec<Ty> = t.generic_args.as_ref().map_or_else(Vec::new, |g| {
        g.args
            .iter()
            .map(|a| lower_ty(b, names, a, generics))
            .collect()
    });
    if let Some(i) = generics.iter().position(|g| g == name) {
        return Ty::Param(i as u32);
    }
    let first = || -> Box<Ty> {
        Box::new(
            args.first()
                .cloned()
                .unwrap_or_else(|| panic!("{name} needs a type argument")),
        )
    };
    match name {
        "Int" => Ty::Int,
        "Bool" => Ty::Bool,
        "Text" => Ty::Text,
        "Bytes" => Ty::Bytes,
        "List" => Ty::List(first()),
        "Gen" => Ty::Gen(first()),
        _ => {
            let &(def, form) = names
                .types
                .get(name)
                .unwrap_or_else(|| panic!("std type {name} is not declared"));
            match form {
                Form::Struct | Form::Enum => Ty::Adt(def, args),
                Form::Wrapper => Ty::Wrapper(def),
                Form::Refinement => b.refined(def),
                Form::Builtin | Form::Fn => panic!("{name} is not a nominal std type"),
            }
        }
    }
}

/// The head of a receiver type, as member names spell it.
fn head(b: &ProgramBuilder, ty: &Ty) -> String {
    match ty {
        Ty::Int => "Int".to_string(),
        Ty::Bool => "Bool".to_string(),
        Ty::Text => "Text".to_string(),
        Ty::Bytes => "Bytes".to_string(),
        Ty::List(_) => "List".to_string(),
        Ty::Adt(def, _) => b.def(*def).name.display(),
        other => panic!("no member table for {other:?}"),
    }
}

/// Bind the generic parameters of `pattern` against `actual`.
fn bind(pattern: &Ty, actual: &Ty, out: &mut Vec<Option<Ty>>) {
    match (pattern, actual) {
        (Ty::Param(i), _) => {
            let i = *i as usize;
            if out.len() <= i {
                out.resize(i + 1, None);
            }
            out[i].get_or_insert_with(|| actual.clone());
        }
        (Ty::List(p), Ty::List(a)) | (Ty::Gen(p), Ty::Gen(a)) => bind(p, a, out),
        (Ty::Adt(_, ps), Ty::Adt(_, args)) => {
            for (p, a) in ps.iter().zip(args) {
                bind(p, a, out);
            }
        }
        _ => {}
    }
}

/// A std clause expression: literals, names, parentheses, `not`/`-`,
/// binary operators, method calls on a receiver, and `forall`. Anything
/// else is not used by the stubs' `requires` clauses and predicates.
fn lower_expr(
    b: &ProgramBuilder,
    names: &Names,
    e: &ast::Expr,
    locals: &mut Vec<LocalDef>,
    visible: &mut Vec<LocalId>,
) -> Expr {
    use ast::ExprKind as A;
    match &e.kind {
        A::Literal(l) => {
            let (lit, ty) = match &l.kind {
                ast::LiteralKind::Int(digits) => (
                    Lit::Int(digits.parse().expect("an INT literal is decimal digits")),
                    Ty::Int,
                ),
                ast::LiteralKind::Bool(v) => (Lit::Bool(*v), Ty::Bool),
                ast::LiteralKind::Text(s) => (Lit::Text(s.clone()), Ty::Text),
            };
            Expr::new(ExprKind::Lit(lit), ty, e.span)
        }
        A::Name(n) => {
            let id = visible
                .iter()
                .rev()
                .copied()
                .find(|id| locals[id.0 as usize].name == n.text)
                .unwrap_or_else(|| panic!("std clause names an unknown local {}", n.text));
            Expr::new(
                ExprKind::Local(id),
                locals[id.0 as usize].ty.clone(),
                e.span,
            )
        }
        A::Paren(inner) => lower_expr(b, names, inner, locals, visible),
        A::Unary(u) => {
            let operand = lower_expr(b, names, &u.operand, locals, visible);
            let ty = match u.op {
                ast::UnaryOp::Not => Ty::Bool,
                ast::UnaryOp::Neg => Ty::Int,
            };
            Expr::new(
                ExprKind::Unary {
                    op: u.op,
                    operand: Box::new(operand),
                },
                ty,
                e.span,
            )
        }
        A::Binary(bx) => {
            let lhs = lower_expr(b, names, &bx.lhs, locals, visible);
            let rhs = lower_expr(b, names, &bx.rhs, locals, visible);
            let ty = match bx.op {
                ast::BinaryOp::Add
                | ast::BinaryOp::Sub
                | ast::BinaryOp::Mul
                | ast::BinaryOp::Div
                | ast::BinaryOp::Rem => Ty::Int,
                ast::BinaryOp::Concat => lhs.ty.clone(),
                _ => Ty::Bool,
            };
            Expr::new(
                ExprKind::Binary {
                    op: bx.op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                ty,
                e.span,
            )
        }
        A::Call(c) => {
            let A::Member(m) = &c.callee.kind else {
                panic!("std clauses call only members")
            };
            let receiver = lower_expr(b, names, &m.target, locals, visible);
            let key = (head(b, &receiver.ty), m.member.text.clone());
            let stub = *names
                .members
                .get(&key)
                .unwrap_or_else(|| panic!("no std member {}.{}", key.0, key.1));
            let DefKind::Fn(fd) = &b.def(stub).kind else {
                panic!("member {}.{} is not lowered yet", key.0, key.1)
            };
            let mut bound = Vec::new();
            if let Some(first) = fd.params.first() {
                bind(&first.ty, &receiver.ty, &mut bound);
            }
            let generic_args: Vec<Ty> = (0..fd.generics.len())
                .map(|i| bound.get(i).cloned().flatten().unwrap_or(Ty::Error))
                .collect();
            let ret = crate::types::instantiate(&fd.ret, &generic_args);
            let mut args = vec![receiver];
            for a in &c.args.positional {
                args.push(lower_expr(b, names, &a.expr, locals, visible));
            }
            Expr::new(
                ExprKind::Call {
                    callee: stub,
                    generic_args,
                    args,
                    using: Vec::new(),
                },
                ret,
                e.span,
            )
        }
        A::Forall(f) => {
            let collection = lower_expr(b, names, &f.collection, locals, visible);
            let elem = match &collection.ty {
                Ty::List(t) => (**t).clone(),
                other => panic!("forall over a non-list {other:?}"),
            };
            let binder = LocalId(locals.len() as u32);
            locals.push(LocalDef {
                id: binder,
                name: f.binder.text.clone(),
                ty: elem,
                mutable: false,
                kind: BindingKind::Forall,
                span: f.binder.span,
            });
            visible.push(binder);
            let predicate = lower_expr(b, names, &f.predicate, locals, visible);
            visible.pop();
            Expr::new(
                ExprKind::Forall {
                    binder,
                    collection: Box::new(collection),
                    predicate: Box::new(predicate),
                },
                Ty::Bool,
                e.span,
            )
        }
        other => {
            panic!("a std clause uses a construct the test lowering does not support: {other:?}")
        }
    }
}
