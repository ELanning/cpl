//! Unit tests: `display_ty`, the `Refined` equality rule (S55), lookup and
//! the other accessors, traversal, and structural equality.

use crate::*;
use cpl_diag::{StableId, SymbolPath};
use cpl_syntax::parse_module;
use std::collections::HashSet;

const PAGES: &str = r#"module reader.pages

pub type PageId(Text)

pub type Positive = Int where self > 0

pub fn paginate<T>(items: List<T>, size: Positive) -> List<List<T>> {
    []
}

test "pagination handles empty input" {
    assert true
}
"#;

const PRELUDE: &str = r#"module std.prelude

pub enum Unit {
    Unit
}
"#;

fn span(file: u32, start: u32, end: u32) -> Span {
    Span::new(FileId(file), start, end)
}

fn int(n: i64, at: u32) -> Expr {
    Expr::new(
        ExprKind::Lit(Lit::Int(BigInt::from(n))),
        Ty::Int,
        span(1, at, at + 1),
    )
}

fn def(id: u32, module: u32, file: u32, item: usize, name: DeclName, kind: DefKind) -> Def {
    let module_path = if module == 0 {
        "std.prelude"
    } else {
        "reader.pages"
    };
    Def {
        id: DefId(id),
        stable_id: StableId::provisional(module_path, &name),
        provisional: true,
        module: ModuleId(module),
        name,
        is_pub: true,
        file: FileId(file),
        decl_index: item,
        span: span(file, 0, 1),
        docs: None,
        policy: VerificationPolicy::default(),
        status: DefStatus::Ok,
        kind,
    }
}

fn enum_def(generics: &[&str], variants: &[&str]) -> DefKind {
    DefKind::Enum(EnumDef {
        generics: generics.iter().map(|g| g.to_string()).collect(),
        variants: variants
            .iter()
            .map(|v| VariantDef {
                name: v.to_string(),
                payload: vec![],
                docs: None,
                span: span(0, 0, 1),
            })
            .collect(),
    })
}

fn local(id: u32, name: &str, ty: Ty, kind: BindingKind) -> LocalDef {
    LocalDef {
        id: LocalId(id),
        name: name.into(),
        ty,
        mutable: false,
        kind,
        span: span(1, 0, 1),
    }
}

/// std.prelude (file 0): Unit, Option, Result, ConstraintError, ForeignError,
/// Positive (as a stand-in for std.gen.Positive), the `List.take` stub;
/// reader.pages (file 1): PageId, Positive, paginate, a test, a capability.
fn program() -> Program {
    let mut sm = SourceMap::new();
    let std_file = sm.add("std/prelude.cpl".into(), FileOrigin::Std, PRELUDE.into());
    let pages_file = sm.add(
        "src/reader/pages.cpl".into(),
        FileOrigin::Project,
        PAGES.into(),
    );
    let prelude = Arc::new(
        parse_module(PRELUDE, std_file)
            .value
            .expect("prelude parses"),
    );
    let pages = Arc::new(parse_module(PAGES, pages_file).value.expect("pages parses"));
    let positive_class = RefinementClass(0);
    let refinement = |self_ty: Ty| {
        let self_ref = Expr::new(ExprKind::Local(LocalId(0)), self_ty.clone(), span(1, 0, 4));
        let predicate = Expr::new(
            ExprKind::Binary {
                op: ast::BinaryOp::Gt,
                lhs: Box::new(self_ref),
                rhs: Box::new(int(0, 7)),
            },
            Ty::Bool,
            span(1, 0, 8),
        );
        DefKind::Refinement(RefinementDef {
            base: Ty::Int,
            self_local: LocalId(0),
            predicate: Clause {
                expr: predicate,
                text: "self > 0".into(),
                span: span(1, 0, 8),
            },
            locals: vec![local(0, "self", self_ty, BindingKind::SelfValue)],
        })
    };
    let defs = vec![
        def(
            0,
            0,
            0,
            0,
            DeclName::Ident("Unit".into()),
            enum_def(&[], &["Unit"]),
        ),
        def(
            1,
            0,
            0,
            0,
            DeclName::Ident("Option".into()),
            enum_def(&["T"], &["None", "Some"]),
        ),
        def(
            2,
            0,
            0,
            0,
            DeclName::Ident("Result".into()),
            enum_def(&["T", "E"], &["Ok", "Err"]),
        ),
        def(
            3,
            0,
            0,
            0,
            DeclName::Ident("ConstraintError".into()),
            DefKind::Struct(StructDef {
                invariant: None,
                generics: vec![],
                fields: vec![],
            }),
        ),
        def(
            4,
            0,
            0,
            0,
            DeclName::Ident("ForeignError".into()),
            DefKind::Struct(StructDef {
                invariant: None,
                generics: vec![],
                fields: vec![],
            }),
        ),
        def(
            5,
            0,
            0,
            0,
            DeclName::Ident("Positive".into()),
            refinement(Ty::Int),
        ),
        def(
            6,
            0,
            0,
            0,
            DeclName::Member {
                receiver: "List".into(),
                name: "take".into(),
            },
            DefKind::Fn(FnDef {
                generics: vec!["T".into()],
                params: vec![],
                using: vec![],
                ret: Ty::list(Ty::Param(0)),
                effects: EffectBound::default(),
                requires: vec![],
                ensures: vec![],
                result_local: None,
                decreases: None,
                locals: vec![],
                body: FnBody::Builtin("list_take".into()),
            }),
        ),
        def(
            7,
            1,
            1,
            0,
            DeclName::Ident("PageId".into()),
            DefKind::Wrapper(WrapperDef { inner: Ty::Text }),
        ),
        def(
            8,
            1,
            1,
            1,
            DeclName::Ident("Positive".into()),
            refinement(Ty::Int),
        ),
        def(
            9,
            1,
            1,
            2,
            DeclName::Ident("paginate".into()),
            DefKind::Fn(FnDef {
                generics: vec!["T".into()],
                params: vec![],
                using: vec![],
                ret: Ty::list(Ty::list(Ty::Param(0))),
                effects: EffectBound::default(),
                requires: vec![],
                ensures: vec![],
                result_local: None,
                decreases: None,
                locals: vec![
                    local(0, "items", Ty::list(Ty::Param(0)), BindingKind::Parameter),
                    local(
                        1,
                        "size",
                        Ty::Refined(positive_class, DefId(8)),
                        BindingKind::Parameter,
                    ),
                    local(2, "db", Ty::Capability(DefId(11)), BindingKind::Using),
                ],
                body: FnBody::Block(Block {
                    stmts: vec![],
                    tail: None,
                    ty: Ty::Error,
                    span: span(1, 0, 1),
                }),
            }),
        ),
        def(
            10,
            1,
            1,
            3,
            DeclName::Text("pagination handles empty input".into()),
            DefKind::Test(TestDef {
                locals: vec![],
                body: Block {
                    stmts: vec![],
                    tail: None,
                    ty: Ty::Error,
                    span: span(1, 0, 1),
                },
            }),
        ),
        def(
            11,
            1,
            1,
            0,
            DeclName::Ident("PageRead".into()),
            DefKind::Capability(CapabilityDef { ops: vec![] }),
        ),
    ];
    let mut references = vec![
        Reference {
            from: DefId(10),
            to: DefId(9),
            kind: RefKind::Call,
            span: span(1, 50, 60),
            interface: false,
        },
        Reference {
            from: DefId(9),
            to: DefId(8),
            kind: RefKind::TypeUse,
            span: span(1, 30, 38),
            interface: true,
        },
        Reference {
            from: DefId(9),
            to: DefId(6),
            kind: RefKind::Call,
            span: span(1, 20, 25),
            interface: false,
        },
        Reference {
            from: DefId(10),
            to: DefId(8),
            kind: RefKind::TypeUse,
            span: span(1, 40, 45),
            interface: false,
        },
    ];
    references.sort_by_key(|r| (r.from, r.span));
    Program {
        defs,
        modules: vec![
            ModuleDef {
                id: ModuleId(0),
                path: vec!["std".into(), "prelude".into()],
                origin: FileOrigin::Std,
                files: vec![std_file],
                defs: (0..=6).map(DefId).collect(),
                docs: None,
            },
            ModuleDef {
                id: ModuleId(1),
                path: vec!["reader".into(), "pages".into()],
                origin: FileOrigin::Project,
                files: vec![pages_file],
                defs: (7..=11).map(DefId).collect(),
                docs: None,
            },
        ],
        std_defs: StdDefs {
            unit: DefId(0),
            option: DefId(1),
            result: DefId(2),
            constraint_error: DefId(3),
            foreign_error: DefId(4),
            positive: DefId(5),
        },
        references,
        imports: vec![],
        refinement_classes: vec![RefinementClassDef {
            base: Ty::Int,
            predicate: defs_predicate(),
            members: vec![DefId(5), DefId(8)],
        }],
        source_map: Arc::new(sm),
        asts: vec![(std_file, prelude), (pages_file, pages)],
    }
}

fn defs_predicate() -> Expr {
    let self_ref = Expr::new(ExprKind::Local(LocalId(0)), Ty::Int, span(1, 0, 4));
    Expr::new(
        ExprKind::Binary {
            op: ast::BinaryOp::Gt,
            lhs: Box::new(self_ref),
            rhs: Box::new(int(0, 7)),
        },
        Ty::Bool,
        span(1, 0, 8),
    )
}

// ---- display_ty --------------------------------------------------------------

#[test]
fn display_ty_renders_cpl_type_syntax() {
    let p = program();
    let t = vec!["T".to_string()];
    let positive = Ty::Refined(RefinementClass(0), DefId(8));
    assert_eq!(
        p.display_ty_in(&Ty::list(Ty::list(Ty::Param(0))), &t),
        "List<List<T>>"
    );
    assert_eq!(
        p.display_ty(&Ty::list(Ty::list(Ty::Param(0)))),
        "List<List<$0>>"
    );
    assert_eq!(p.display_ty(&Ty::Int), "Int");
    assert_eq!(p.display_ty(&Ty::Bool), "Bool");
    assert_eq!(p.display_ty(&Ty::Text), "Text");
    assert_eq!(p.display_ty(&Ty::Bytes), "Bytes");
    assert_eq!(p.display_ty(&Ty::unit(p.std_defs())), "Unit");
    assert_eq!(
        p.display_ty(&Ty::option(p.std_defs(), Ty::Text)),
        "Option<Text>"
    );
    assert_eq!(
        p.display_ty(&Ty::result(
            p.std_defs(),
            positive.clone(),
            Ty::Adt(DefId(3), vec![])
        )),
        "Result<Positive, ConstraintError>"
    );
    assert_eq!(p.display_ty(&Ty::Wrapper(DefId(7))), "PageId");
    assert_eq!(p.display_ty(&positive), "Positive");
    assert_eq!(p.display_ty(&Ty::Capability(DefId(11))), "PageRead");
    assert_eq!(
        p.display_ty(&Ty::Gen(Box::new(Ty::Refined(
            RefinementClass(0),
            DefId(5)
        )))),
        "Gen<Positive>"
    );
    // S69 (D-247): the error type renders as `?`, also inside a type and
    // with a generic context.
    assert_eq!(p.display_ty(&Ty::Error), "?");
    assert_eq!(p.display_ty(&Ty::list(Ty::Error)), "List<?>");
    assert_eq!(
        p.display_ty_in(
            &Ty::result(p.std_defs(), Ty::Param(0), Ty::Error),
            &["T".into()]
        ),
        "Result<T, ?>"
    );
    assert_eq!(p.display_ty(&Ty::Adt(DefId(99), vec![])), "{def 99}");
    assert_eq!(
        p.display_ty_in(
            &Ty::result(p.std_defs(), Ty::Param(0), Ty::Param(1)),
            &["A".into(), "B".into()]
        ),
        "Result<A, B>"
    );
}

// ---- the Refined rule (S55) ----------------------------------------------------

#[test]
fn refined_types_compare_and_hash_by_class_only() {
    use std::collections::hash_map::DefaultHasher;
    let hash = |t: &Ty| {
        let mut h = DefaultHasher::new();
        t.hash(&mut h);
        h.finish()
    };
    let std_positive = Ty::Refined(RefinementClass(0), DefId(5));
    let user_positive = Ty::Refined(RefinementClass(0), DefId(8));
    let other = Ty::Refined(RefinementClass(1), DefId(8));
    assert_eq!(std_positive, user_positive);
    assert_eq!(hash(&std_positive), hash(&user_positive));
    assert_ne!(user_positive, other);
    assert_ne!(user_positive, Ty::Int, "a refinement is never its base");
    assert_eq!(
        Ty::list(std_positive.clone()),
        Ty::list(user_positive.clone())
    );
    assert_eq!(
        Ty::Gen(Box::new(std_positive.clone())),
        Ty::Gen(Box::new(user_positive.clone()))
    );
    assert_ne!(Ty::list(Ty::Int), Ty::Gen(Box::new(Ty::Int)));
    assert_ne!(Ty::Wrapper(DefId(7)), Ty::Capability(DefId(7)));
    assert_eq!(
        Ty::Adt(DefId(1), vec![std_positive.clone()]),
        Ty::Adt(DefId(1), vec![user_positive.clone()])
    );
    assert_ne!(
        Ty::Adt(DefId(1), vec![Ty::Int]),
        Ty::Adt(DefId(2), vec![Ty::Int])
    );
    let set: HashSet<Ty> = [std_positive, user_positive, other, Ty::Error, Ty::Error]
        .into_iter()
        .collect();
    assert_eq!(set.len(), 3);
}

// ---- lookup and the other accessors ------------------------------------------------

#[test]
fn lookup_finds_every_decl_name_form() {
    let p = program();
    let path = |s: &str| SymbolPath::parse(s).unwrap();
    assert_eq!(p.lookup(&path("reader.pages.paginate")), Some(DefId(9)));
    assert_eq!(p.lookup(&path("reader.pages.Positive")), Some(DefId(8)));
    assert_eq!(p.lookup(&path("std.prelude.Positive")), Some(DefId(5)));
    assert_eq!(
        p.lookup(&path(r#"reader.pages."pagination handles empty input""#)),
        Some(DefId(10))
    );
    assert_eq!(p.lookup(&path("std.prelude.List.take")), Some(DefId(6)));
    assert_eq!(p.lookup(&path("reader.pages.missing")), None);
    assert_eq!(p.lookup(&path("reader.missing.paginate")), None);
    assert_eq!(p.lookup(&path(r#"reader.pages."no such test""#)), None);
    assert_eq!(
        p.lookup(&path("reader.pages.PageRead.get")),
        None,
        "operations are not definitions"
    );
}

#[test]
fn identities_paths_and_modules() {
    let p = program();
    assert_eq!(p.display_path(DefId(9)), "reader.pages.paginate");
    assert_eq!(
        p.display_path(DefId(10)),
        r#"reader.pages."pagination handles empty input""#
    );
    assert_eq!(p.display_path(DefId(6)), "std.prelude.List.take");
    let id = StableId::parse("reader.pages.paginate").unwrap();
    assert_eq!(p.by_stable_id(&id), Some(DefId(9)));
    assert_eq!(p.by_stable_id(&StableId::parse("pgn_7c2d").unwrap()), None);
    assert_eq!(
        p.module_by_path(&["reader".to_string(), "pages".to_string()]),
        Some(ModuleId(1))
    );
    assert_eq!(p.module(ModuleId(0)).path, ["std", "prelude"]);
    assert_eq!(p.modules().count(), 2);
    assert_eq!(p.defs().count(), 12);
    assert_eq!(p.def(DefId(9)).kind.id_kind(), "fn");
    assert_eq!(p.def(DefId(8)).kind.id_kind(), "ty");
    assert_eq!(p.def(DefId(11)).kind.id_kind(), "cap");
    assert_eq!(p.def(DefId(10)).kind.id_kind(), "test");
    assert_eq!(p.def(DefId(10)).kind.keyword(), "test");
    assert!(p.def(DefId(7)).kind.is_type());
    assert!(p.def(DefId(10)).kind.is_test());
    assert_eq!(p.def(DefId(9)).kind.generics(), ["T"]);
    assert_eq!(
        p.def(DefId(9)).local(LocalId(1)).map(|l| l.name.as_str()),
        Some("size")
    );
    assert!(p.def(DefId(6)).kind.as_fn().is_some_and(FnDef::is_builtin));
    let paginate = p.def(DefId(9)).kind.as_fn().unwrap();
    assert_eq!(
        paginate.display_effect(&EffectRef {
            param: LocalId(2),
            op: "get".into()
        }),
        "db.get"
    );
    assert_eq!(p.std_defs().positive, DefId(5));
    assert_eq!(
        p.refinement_class(RefinementClass(0)).members,
        [DefId(5), DefId(8)]
    );
    assert_eq!(p.source_map().len(), 2);
}

#[test]
fn references_are_indexed_by_source_and_target() {
    let p = program();
    let from: Vec<(DefId, u32)> = p
        .references_from(DefId(9))
        .iter()
        .map(|r| (r.to, r.span.start))
        .collect();
    assert_eq!(from, [(DefId(6), 20), (DefId(8), 30)]);
    assert_eq!(p.references_from(DefId(10)).len(), 2);
    assert!(p.references_from(DefId(7)).is_empty());
    let to: Vec<DefId> = p.references_to(DefId(8)).iter().map(|r| r.from).collect();
    assert_eq!(to, [DefId(9), DefId(10)]);
    let mut q = program();
    q.references.reverse();
    q.sort_references();
    assert_eq!(q.references, p.references);
    assert_eq!(
        cpl_diag::json::Relation::from(RefKind::TypeUse),
        cpl_diag::json::Relation::TypeUse
    );
    assert_eq!(RefKind::CapabilityOp.as_str(), "capability_op");
}

#[test]
fn ast_returns_the_declaration_of_a_definition() {
    let p = program();
    assert_eq!(p.ast(DefId(9)).display_name(), "paginate");
    assert_eq!(
        p.ast(DefId(10)).display_name(),
        "pagination handles empty input"
    );
    assert_eq!(p.ast(DefId(0)).display_name(), "Unit");
    assert!(p.file_ast(FileId(5)).is_none());
    let empty = Program::empty(Arc::new(SourceMap::new()));
    assert_eq!(empty.defs().count(), 0);
    assert_eq!(empty.std_defs().unit, DefId(u32::MAX));
}

// ---- traversal and structural equality ------------------------------------------------

fn sample(shift: u32, rhs: i64) -> Expr {
    let at = |n: u32| span(1, n + shift, n + shift + 1);
    let call = Expr::new(
        ExprKind::Call {
            callee: DefId(9),
            generic_args: vec![Ty::Int],
            args: vec![int(1, 2 + shift)],
            using: vec![],
        },
        Ty::list(Ty::list(Ty::Int)),
        at(1),
    );
    let block = Block {
        stmts: vec![Stmt::Let {
            local: LocalId(0),
            value: call,
        }],
        tail: Some(Box::new(Expr::new(
            ExprKind::Binary {
                op: ast::BinaryOp::Add,
                lhs: Box::new(Expr::new(ExprKind::Local(LocalId(0)), Ty::Int, at(5))),
                rhs: Box::new(int(rhs, 7 + shift)),
            },
            Ty::Int,
            at(4),
        ))),
        ty: Ty::Int,
        span: at(0),
    };
    Expr::new(
        ExprKind::If {
            cond: Box::new(Expr::new(ExprKind::Lit(Lit::Bool(true)), Ty::Bool, at(9))),
            then_block: block,
            else_block: None,
        },
        Ty::Int,
        at(0),
    )
}

#[test]
fn walk_visits_every_expression_in_body_order() {
    let mut seen = Vec::new();
    sample(0, 2).walk(&mut |e: &Expr| {
        seen.push(match &e.kind {
            ExprKind::If { .. } => "if",
            ExprKind::Lit(Lit::Bool(_)) => "bool",
            ExprKind::Call { .. } => "call",
            ExprKind::Lit(Lit::Int(_)) => "int",
            ExprKind::Binary { .. } => "binary",
            ExprKind::Local(_) => "local",
            _ => "other",
        })
    });
    assert_eq!(
        seen,
        ["if", "bool", "call", "int", "binary", "local", "int"]
    );
}

#[test]
fn structural_equality_ignores_spans_only() {
    assert!(sample(0, 2).structurally_eq(&sample(40, 2)));
    assert_ne!(
        sample(0, 2),
        sample(40, 2),
        "derived equality still compares spans"
    );
    assert!(!sample(0, 2).structurally_eq(&sample(0, 3)));
    let a = defs_predicate();
    let mut b = defs_predicate();
    b.span = span(1, 100, 108);
    assert!(a.structurally_eq(&b));
}
