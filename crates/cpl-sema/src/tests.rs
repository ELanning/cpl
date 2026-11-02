//! Unit tests of the internal tables: the resolution side tables cover
//! every name of the acceptance examples and the positive corpus, the
//! member table, refinement classes and the functions of semantics 3.3,
//! constructor deferral, and hole bindings.

use crate::check::with_resolved;
use crate::checker::Checker;
use crate::members::{MemberKind, TypeHead};
use crate::project::{load_project, load_project_from_tree, LoadOptions, Project, VirtualTree};
use crate::resolve::{Ctor, Res};
use cpl_diag::FileOrigin;
use cpl_hir::{DefId, DefKind, Ty};
use cpl_syntax::ast;
use std::path::{Path, PathBuf};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn options() -> LoadOptions {
    LoadOptions {
        include_work_files: true,
        ..LoadOptions::default()
    }
}

fn load_dir(path: &Path) -> Project {
    load_project(path, &options()).expect("the project loads")
}

fn load_source(source: &str) -> Project {
    let header = source
        .lines()
        .find_map(|l| l.trim().strip_prefix("module "))
        .expect("a module header")
        .trim()
        .to_string();
    let path = format!("src/{}.cpl", header.replace('.', "/"));
    load_files(&[(&path, source)])
}

fn load_files(files: &[(&str, &str)]) -> Project {
    let mut tree = VirtualTree::default();
    tree.files.insert(
        "cpl.toml".into(),
        b"[project]\nname = \"t\"\nedition = \"0.1\"\n".to_vec(),
    );
    for (p, t) in files {
        tree.files.insert(p.to_string(), t.as_bytes().to_vec());
    }
    load_project_from_tree(&tree, FileOrigin::Virtual, &options()).expect("the tree loads")
}

fn def_named(cx: &Checker<'_>, path: &str) -> DefId {
    cx.program
        .defs()
        .find(|d| cx.program.display_path(d.id) == path)
        .unwrap_or_else(|| panic!("no definition {path}"))
        .id
}

// ---- coverage of the side tables ------------------------------------------------

/// Walks a file's AST and asserts that every name the typer looks up has
/// an entry, and that no entry is `Res::Error` (a project without
/// diagnostics has no erroneous name).
struct Coverage<'c, 'a> {
    cx: &'c Checker<'a>,
    file: String,
    missing: Vec<String>,
}

impl Coverage<'_, '_> {
    fn note(&mut self, what: &str, span: cpl_diag::Span) {
        let lc = self.cx.project.source_map.line_col(span.file, span.start);
        self.missing
            .push(format!("{}:{}:{}: {what}", self.file, lc.line, lc.col));
    }

    fn res(
        &mut self,
        table: &std::collections::HashMap<cpl_diag::Span, Res>,
        span: cpl_diag::Span,
        what: &str,
    ) {
        match table.get(&span) {
            None => self.note(&format!("no entry for {what}"), span),
            Some(Res::Error) => self.note(&format!("{what} is Res::Error"), span),
            Some(_) => {}
        }
    }

    fn binder(&mut self, n: &ast::Name) {
        if !self.cx.names.binders.contains_key(&n.span) {
            self.note(&format!("no binder for `{}`", n.text), n.span);
        }
    }

    fn ty(&mut self, t: &ast::Type) {
        match self.cx.names.types.get(&t.span) {
            None => self.note("no type entry", t.span),
            Some(Ty::Error) => self.note("type is Ty::Error", t.span),
            Some(_) => {}
        }
    }

    fn decl(&mut self, d: &ast::Declaration) {
        match &d.kind {
            ast::DeclKind::Fn(f) => {
                for p in &f.params.positional {
                    self.binder(&p.name);
                }
                for u in &f.params.using {
                    self.binder(&u.name);
                }
                for c in &f.clauses {
                    match c {
                        ast::FnClause::Contract(c) => self.expr(&c.expr),
                        ast::FnClause::Decreases(c) => self.expr(&c.expr),
                        ast::FnClause::Effects(_) => {}
                    }
                }
                if let Some(b) = &f.body {
                    self.block(b);
                }
            }
            ast::DeclKind::Type(t) => {
                if let ast::TypeDeclKind::Refinement(r) = &t.kind {
                    self.expr(&r.predicate);
                }
            }
            ast::DeclKind::Capability(c) => {
                for op in &c.operations {
                    for p in &op.params.positional {
                        self.binder(&p.name);
                    }
                    for cl in &op.clauses {
                        self.expr(&cl.expr);
                    }
                }
            }
            ast::DeclKind::Test(t) => self.block(&t.body),
            ast::DeclKind::Property(p) => {
                for c in &p.for_clauses {
                    self.binder(&c.binder);
                    self.expr(&c.range);
                }
                self.block(&p.body);
            }
            _ => {}
        }
    }

    fn block(&mut self, b: &ast::Block) {
        for s in &b.statements {
            match s {
                ast::Statement::Let(l) => {
                    self.expr(&l.value);
                    if let Some(t) = &l.ty {
                        self.ty(t);
                    }
                    if let ast::LetBinding::Name(n) = &l.binding {
                        self.binder(n);
                    }
                }
                ast::Statement::Var(v) => {
                    self.expr(&v.value);
                    if let Some(t) = &v.ty {
                        self.ty(t);
                    }
                    self.binder(&v.name);
                }
                ast::Statement::Assign(a) => {
                    self.expr(&a.value);
                    let table = &self.cx.names.assigns;
                    self.res(table, a.target.span, "an assignment target");
                }
                ast::Statement::Return(r) => {
                    if let Some(v) = &r.value {
                        self.expr(v);
                    }
                }
                ast::Statement::Assert(a) => self.expr(&a.condition),
                ast::Statement::While(w) => {
                    self.expr(&w.condition);
                    for c in &w.clauses {
                        match c {
                            ast::WhileClause::Invariant(i) => self.expr(&i.expr),
                            ast::WhileClause::Decreases(d) => self.expr(&d.expr),
                        }
                    }
                    self.block(&w.body);
                }
                ast::Statement::Expr(x) => self.expr(&x.expr),
            }
        }
        if let Some(t) = &b.tail {
            self.expr(&t.expr);
        }
    }

    fn pattern(&mut self, p: &ast::Pattern) {
        match &p.kind {
            ast::PatternKind::Binding(n) => self.binder(n),
            ast::PatternKind::Ctor(c) => {
                let table = &self.cx.names.patterns;
                self.res(table, c.path.span, "a pattern constructor");
                for a in c.args.iter().flatten() {
                    self.pattern(a);
                }
            }
            _ => {}
        }
    }

    fn expr(&mut self, e: &ast::Expr) {
        use ast::ExprKind as K;
        match &e.kind {
            K::Name(n) => {
                let table = &self.cx.names.exprs;
                self.res(table, e.span, &format!("name `{}`", n.text));
            }
            K::Member(m) => {
                if !self.cx.names.exprs.contains_key(&e.span) {
                    self.expr(&m.target);
                } else if let Some(Res::Error) = self.cx.names.exprs.get(&e.span) {
                    self.note("a chain is Res::Error", e.span);
                }
            }
            K::Call(c) => {
                self.expr(&c.callee);
                for t in c.generic_args.iter().flat_map(|g| &g.args) {
                    self.ty(t);
                }
                for a in &c.args.positional {
                    self.expr(&a.expr);
                }
                for u in &c.args.using {
                    self.expr(&u.expr);
                }
            }
            K::Forall(f) | K::Parallel(f) => {
                self.binder(&f.binder);
                self.expr(&f.collection);
                self.expr(&f.predicate);
            }
            K::Binary(b) => {
                self.expr(&b.lhs);
                self.expr(&b.rhs);
            }
            K::Unary(u) => self.expr(&u.operand),
            K::Propagate(x) | K::Paren(x) => self.expr(x),
            K::List(l) => {
                for el in &l.elements {
                    self.expr(&el.expr);
                }
            }
            K::Struct(s) => {
                let table = &self.cx.names.struct_paths;
                self.res(table, s.path.span, "a struct literal path");
                if let Some(b) = &s.entries.spread {
                    self.expr(b);
                }
                for f in &s.entries.fields {
                    self.expr(&f.value);
                }
            }
            K::If(i) => self.if_expr(i),
            K::Match(m) => {
                self.expr(&m.scrutinee);
                for arm in &m.arms {
                    self.pattern(&arm.pattern);
                    match &arm.body {
                        ast::ArmBody::Block(b) => self.block(b),
                        ast::ArmBody::Expr(x) => self.expr(x),
                    }
                }
            }
            K::Hole(_) => {
                if !self.cx.names.holes.contains_key(&e.span) {
                    self.note("no hole entry", e.span);
                }
            }
            K::Literal(_) => {}
        }
    }

    fn if_expr(&mut self, i: &ast::IfExpr) {
        self.expr(&i.condition);
        self.block(&i.then_block);
        match &i.else_branch {
            Some(ast::ElseBranch::Block(b)) => self.block(b),
            Some(ast::ElseBranch::If(n)) => self.if_expr(n),
            None => {}
        }
    }
}

/// Assert full coverage for every file of `project` (std included when
/// `with_std`).
fn assert_covered(project: &Project, with_std: bool) {
    with_resolved(project, |cx| {
        let mut missing = Vec::new();
        for file in &cx.files {
            if file.is_std && !with_std {
                continue;
            }
            let mut c = Coverage {
                cx,
                file: cx.project.source_map.file(file.id).path.clone(),
                missing: Vec::new(),
            };
            for d in &file.ast.declarations {
                c.decl(d);
            }
            missing.extend(c.missing);
        }
        assert!(
            missing.is_empty(),
            "uncovered names:\n{}",
            missing.join("\n")
        );
    });
}

#[test]
fn examples_and_std_names_are_covered() {
    assert_covered(&load_dir(&workspace().join("examples/reader")), true);
    assert_covered(&load_dir(&workspace().join("examples/holes")), false);
}

#[test]
fn positive_corpus_names_are_covered() {
    let ok = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/ok");
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&ok)
        .expect("the ok corpus")
        .map(|e| e.expect("an entry").path())
        .collect();
    entries.sort();
    for path in entries {
        let project = if path.is_dir() {
            load_dir(&path)
        } else {
            let text = std::fs::read_to_string(&path).expect("a corpus file");
            load_source(&text)
        };
        // Files whose diagnostics are deliberate warnings may hold names
        // that resolve with an error only in their marked lines; the
        // positive corpus has none.
        assert_covered(&project, false);
    }
}

// ---- specific resolutions ---------------------------------------------------------

#[test]
fn reader_resolutions() {
    let project = load_dir(&workspace().join("examples/reader"));
    with_resolved(&project, |cx| {
        let trim = def_named(cx, "std.text.trim");
        let paginate = def_named(cx, "reader.pages.paginate");
        let flatten = def_named(cx, "std.prelude.flatten");
        let gen_int = def_named(cx, "std.gen.int");
        let gen_list = def_named(cx, "std.gen.list");
        let store_error = def_named(cx, "reader.pages.StoreError");
        let fns: Vec<DefId> = cx
            .names
            .exprs
            .values()
            .filter_map(|r| match r {
                Res::Fn(d) => Some(*d),
                _ => None,
            })
            .collect();
        for d in [trim, paginate, flatten, gen_int, gen_list] {
            assert!(
                fns.contains(&d),
                "{} is never resolved",
                cx.program.display_path(d)
            );
        }
        // `Missing` has one constructor in the program: decided; `Unavailable`
        // is also a std.foreign variant: deferred to the expected type.
        let missing = Ctor::Variant {
            def: store_error,
            index: 0,
        };
        assert!(cx.names.patterns.values().any(|r| *r == Res::Ctor(missing)));
        assert!(cx.names.patterns.values().any(|r| matches!(
            r,
            Res::DeferredCtor { name, candidates } if name == "Unavailable" && candidates.len() == 1
        )));
        assert!(cx.names.exprs.values().any(|r| *r == Res::Check));
    });
}

#[test]
fn constructors_are_deferred_only_when_the_expected_type_decides() {
    let project =
        load_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/ok/variant_resolution"));
    with_resolved(&project, |cx| {
        // vr.user imports only functions: `Missing` has no lexical candidate.
        let deferred: Vec<(&String, usize)> = cx
            .names
            .exprs
            .values()
            .filter_map(|r| match r {
                Res::DeferredCtor { name, candidates } => Some((name, candidates.len())),
                _ => None,
            })
            .collect();
        assert!(
            deferred.contains(&(&"Missing".to_string(), 0)),
            "{deferred:?}"
        );
        assert!(
            deferred.contains(&(&"PageId".to_string(), 0)),
            "{deferred:?}"
        );
    });
}

#[test]
fn hole_bindings_are_innermost_and_most_recent_first() {
    let project = load_source(
        "module m.h\n\nfn f(a: Int, b: Int) -> Int {\n    let c = a\n    if c > 0 {\n        let d = b\n        ?inner\n    } else {\n        ?outer\n    }\n}\n",
    );
    with_resolved(&project, |cx| {
        let f = def_named(cx, "m.h.f");
        let locals = &cx.program.def(f).kind.as_fn().expect("fn").locals;
        let mut holes: Vec<Vec<String>> = cx
            .names
            .holes
            .iter()
            .map(|(span, ids)| (span.start, ids))
            .collect::<std::collections::BTreeMap<_, _>>()
            .into_values()
            .map(|ids| {
                ids.iter()
                    .map(|id| locals[id.0 as usize].name.clone())
                    .collect()
            })
            .collect();
        assert_eq!(holes.remove(0), ["d", "c", "b", "a"]);
        assert_eq!(holes.remove(0), ["c", "b", "a"]);
    });
}

// ---- member table and refinements -------------------------------------------------

#[test]
fn member_table() {
    let project = load_source("module m.t\n");
    with_resolved(&project, |cx| {
        let len = cx.members.lookup(TypeHead::List, "len").expect("List.len");
        assert_eq!(len.kind, MemberKind::Property);
        assert_eq!(cx.program.display_path(len.def), "std.prelude.List.len");
        assert_eq!(
            cx.members
                .lookup(TypeHead::List, "take")
                .expect("take")
                .kind,
            MemberKind::Method
        );
        assert_eq!(
            cx.members
                .lookup(TypeHead::Text, "len")
                .expect("Text.len")
                .kind,
            MemberKind::Property
        );
        assert!(cx.members.lookup(TypeHead::Int, "abs").is_some());
        assert!(cx.members.lookup(TypeHead::Option, "unwrap_or").is_some());
        assert!(cx.members.lookup(TypeHead::Result, "unwrap_or").is_some());
        assert!(cx.members.lookup(TypeHead::Option, "unwrap").is_none());
        assert_eq!(cx.members.members_of(TypeHead::List).count(), 11);
        // std.gen's generator properties are module items, not members.
        assert!(
            cx.decls[def_named(cx, "std.gen.int").0 as usize]
                .annots
                .module_property
        );
        let std = cx.program.std_defs;
        assert_eq!(
            TypeHead::of(&Ty::Adt(std.option, vec![Ty::Int]), &std),
            Some(TypeHead::Option)
        );
        assert_eq!(TypeHead::of(&Ty::list(Ty::Int), &std), Some(TypeHead::List));
        assert!(cx.program.def(std.unit).kind.as_enum().is_some());
    });
}

#[test]
fn refinement_identity_is_structural() {
    let project = load_source(
        "module m.r\n\nfn ok(n: Int) -> Bool {\n    n > 1\n}\n\npub type A = Int where (self > 0)\npub type B = Int where self > 0\npub type C = Int where 0 < self\npub type D = List<Int> where forall x in self: x > 0\npub type E = List<Int> where forall y in self: y > 0\npub type F = Int where ok(self)\npub type G = A where self < 10\npub type H = B where self < 10\n",
    );
    with_resolved(&project, |cx| {
        let class = |name: &str| {
            cx.refinements
                .get(def_named(cx, name))
                .and_then(|i| i.class)
                .expect("a class")
        };
        assert_eq!(class("m.r.A"), class("m.r.B"));
        assert_eq!(class("m.r.A"), class("std.gen.Positive"));
        assert_ne!(class("m.r.A"), class("m.r.C"));
        assert_eq!(class("m.r.D"), class("m.r.E"));
        assert_ne!(class("m.r.F"), class("m.r.A"));
        // G and H: equal bases (one class) and equal predicates.
        assert_eq!(class("m.r.G"), class("m.r.H"));
        // 3.3: strip, chain, erase.
        let g = def_named(cx, "m.r.G");
        let a = def_named(cx, "m.r.A");
        assert_eq!(cx.refinements.chain(g), vec![g, a]);
        assert_eq!(cx.refinements.strip(&cx.refinements.ty_of(g)), Ty::Int);
        let list_g = Ty::list(cx.refinements.ty_of(g));
        assert_eq!(cx.refinements.erase(&list_g), Ty::list(Ty::Int));
        assert!(matches!(cx.program.def(g).kind, DefKind::Refinement(_)));
    });
}

#[test]
fn resolve_ctor_uses_the_expected_type() {
    let project = load_source(
        "module m.c\n\npub enum Maybe {\n    None\n    Value(Int)\n}\n\nfn f() -> Maybe {\n    None\n}\n",
    );
    with_resolved(&project, |cx| {
        let maybe = def_named(cx, "m.c.Maybe");
        let f = def_named(cx, "m.c.f");
        let resolver = cx.resolver(f, &[]);
        let name = ast::Name::new("None", cpl_diag::Span::point(cpl_diag::FileId(0), 0));
        let mut out = crate::resolve::ResOut::default();
        let expected = Ty::Adt(maybe, Vec::new());
        assert_eq!(
            resolver.resolve_ctor(&name, Some(&expected), &mut out),
            Res::Ctor(Ctor::Variant {
                def: maybe,
                index: 0
            })
        );
        let option = Ty::option(&cx.program.std_defs, Ty::Int);
        assert_eq!(
            resolver.resolve_ctor(&name, Some(&option), &mut out),
            Res::Ctor(Ctor::Variant {
                def: cx.program.std_defs.option,
                index: 1
            })
        );
        assert!(out.diags.is_empty());
        assert_eq!(resolver.resolve_ctor(&name, None, &mut out), Res::Error);
        assert_eq!(out.diags[0].code, crate::codes::AMBIGUOUS_CONSTRUCTOR);
    });
}

/// Toolchain 1.3: `toml` is used by `cpl-project` alone and `blake3` by
/// `cpl-project`, `cpl-index`, and `cpl-eval`; `cpl-sema` depends on
/// neither (manifests and digests come from the loader), in any section of
/// its manifest.
#[test]
fn the_manifest_has_no_toml_or_blake3_dependency() {
    for line in include_str!("../Cargo.toml").lines() {
        let key = line.trim().split(['.', '=', ' ']).next().unwrap_or("");
        assert!(key != "toml" && key != "blake3", "Cargo.toml: {line}");
    }
}
