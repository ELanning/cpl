//! The hand-built programs of `cpl_eval::testing`: the std unit as the
//! stub files declare it (stdlib 11; toolchain 3.6, T15) and user
//! declarations with structural refinement identity (semantics S55).

use crate::support::reader;
use cpl_diag::{DeclName, FileOrigin, StableId};
use cpl_eval::builtins::stub_of;
use cpl_eval::testing::{std_program, FnBuilder, ProgramBuilder, PropertyBuilder, TestBuilder};
use cpl_hir::{BindingKind, DefKind, ExprKind, FnBody, GenExpr, LocalId, Ty};
use cpl_syntax::ast::{BinaryOp, UnaryOp};

#[test]
fn std_files_get_the_lowest_file_ids_in_sorted_order() {
    let p = std_program();
    let files: Vec<(&str, &FileOrigin)> = p
        .source_map()
        .files()
        .map(|f| (f.path.as_str(), &f.origin))
        .collect();
    assert_eq!(
        files,
        [
            ("std/bytes.cpl", &FileOrigin::Std),
            ("std/foreign.cpl", &FileOrigin::Std),
            ("std/gen.cpl", &FileOrigin::Std),
            ("std/prelude.cpl", &FileOrigin::Std),
            ("std/text.cpl", &FileOrigin::Std),
        ]
    );
    let modules: Vec<String> = p.modules().map(|m| m.path.join(".")).collect();
    assert_eq!(
        modules,
        [
            "std.bytes",
            "std.foreign",
            "std.gen",
            "std.prelude",
            "std.text"
        ]
    );
    assert_eq!(p.asts.len(), 5);
    assert!(p.module(p.def(p.std_defs.unit).module).docs.is_some());
}

#[test]
fn std_defs_name_the_prelude_declarations() {
    let p = std_program();
    let s = p.std_defs;
    assert_eq!(p.display_path(s.unit), "std.prelude.Unit");
    assert_eq!(p.display_path(s.option), "std.prelude.Option");
    assert_eq!(p.display_path(s.result), "std.prelude.Result");
    assert_eq!(
        p.display_path(s.constraint_error),
        "std.prelude.ConstraintError"
    );
    assert_eq!(p.display_path(s.foreign_error), "std.foreign.ForeignError");
    assert_eq!(p.display_path(s.positive), "std.gen.Positive");
    let ce = p.def(s.constraint_error).kind.as_struct().unwrap();
    let fields: Vec<&str> = ce.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(fields, ["type_name", "message"]);
    assert!(ce.fields.iter().all(|f| f.ty == Ty::Text));
    let fe = p.def(s.foreign_error).kind.as_struct().unwrap();
    let fields: Vec<&str> = fe.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(fields, ["kind", "message", "details"]);
    let option = p.def(s.option).kind.as_enum().unwrap();
    assert_eq!(option.generics, ["T"]);
    assert_eq!(option.variants[0].payload, [Ty::Param(0)]);
    assert!(option.variants[1].payload.is_empty());
    let result = p.def(s.result).kind.as_enum().unwrap();
    assert_eq!(result.generics, ["T", "E"]);
    assert_eq!(result.variants[1].payload, [Ty::Param(1)]);
    // std.gen.Positive is refinement class 0: Int where self > 0.
    let positive = p.def(s.positive).kind.as_refinement().unwrap();
    assert_eq!(positive.base, Ty::Int);
    assert_eq!(positive.predicate.text, "self > 0");
    assert_eq!(p.refinement_classes[0].members, [s.positive]);
    // The clause runs from its keyword (toolchain 3.3), as the checker spans it.
    assert_eq!(
        p.source_map().text(positive.predicate.span),
        "where self > 0"
    );
}

/// One std definition as the comparison below sees it: everything the
/// evaluator reads of it.
fn std_shape(p: &cpl_hir::Program, d: &cpl_hir::Def) -> String {
    let g = d.kind.generics();
    let ty = |t: &Ty| p.display_ty_in(t, g);
    let clause = |c: &cpl_hir::Clause| format!("{:?}@{}..{}", c.text, c.span.start, c.span.end);
    let mut s = format!(
        "{} #{} {} pub={} file={} decl={} span={}..{} id={} prov={}",
        p.display_path(d.id),
        d.id.0,
        d.kind.id_kind(),
        d.is_pub,
        d.file.0,
        d.decl_index,
        d.span.start,
        d.span.end,
        d.stable_id.as_str(),
        d.provisional
    );
    match &d.kind {
        DefKind::Fn(f) => {
            let params: Vec<String> = f
                .params
                .iter()
                .map(|q| format!("{}:{}@{}", q.name, ty(&q.ty), q.local.0))
                .collect();
            let requires: Vec<String> = f.requires.iter().map(clause).collect();
            let body = match &f.body {
                FnBody::Builtin(id) => id.clone(),
                FnBody::Block(_) => "block".to_string(),
            };
            s += &format!(
                " <{}>({}) -> {} {body} requires {requires:?}",
                f.generics.join(", "),
                params.join(", "),
                ty(&f.ret)
            );
        }
        DefKind::Refinement(r) => {
            s += &format!(" = {} where {}", ty(&r.base), clause(&r.predicate));
        }
        DefKind::Struct(st) => {
            let fields: Vec<String> = st
                .fields
                .iter()
                .map(|f| format!("{}:{}", f.name, ty(&f.ty)))
                .collect();
            s += &format!(" <{}> {{{}}}", st.generics.join(", "), fields.join(", "));
        }
        DefKind::Enum(e) => {
            let variants: Vec<String> = e
                .variants
                .iter()
                .map(|v| {
                    let payload: Vec<String> = v.payload.iter().map(ty).collect();
                    format!("{}({})", v.name, payload.join(", "))
                })
                .collect();
            s += &format!(" <{}> {}", e.generics.join(", "), variants.join(" | "));
        }
        DefKind::BuiltinType(b) => s += &format!(" <{}> {}", b.generics.join(", "), b.builtin),
        DefKind::Wrapper(w) => s += &format!(" ({})", ty(&w.inner)),
        other => s += &format!(" {other:?}"),
    }
    s
}

#[test]
fn the_hand_lowered_std_is_the_checkers_std() {
    // The hand-built programs that tests still use run on this std; it must
    // be what cpl-sema makes of the same stub files: the same DefIds, files,
    // declarations, spans, signatures, `requires` clauses (text and span),
    // registry ids, and refinement classes, so that a builtin's
    // precondition fault, a check<R> chain, and a value's DefIds are the
    // checker's. The one documented difference: the builtins' `ensures`
    // clauses are not lowered by hand (they are never evaluated, stdlib S39).
    let hand = std_program();
    let real = crate::common::check_source(
        "module m

fn f() -> Int {
    1
}
",
    );
    assert_eq!(hand.std_defs, real.std_defs);
    let std_defs = |p: &cpl_hir::Program| -> Vec<String> {
        p.defs
            .iter()
            .filter(|d| p.module(d.module).origin == FileOrigin::Std)
            .map(|d| std_shape(p, d))
            .collect()
    };
    let (hand_defs, real_defs) = (std_defs(&hand), std_defs(&real));
    assert_eq!(hand_defs.len(), real_defs.len());
    for (h, r) in hand_defs.iter().zip(&real_defs) {
        assert_eq!(h, r);
    }
    let classes = |p: &cpl_hir::Program| -> Vec<(String, Vec<String>)> {
        p.refinement_classes
            .iter()
            .map(|c| {
                let members = c.members.iter().map(|&m| p.display_path(m)).collect();
                (p.display_ty(&c.base), members)
            })
            .collect()
    };
    assert_eq!(classes(&hand), classes(&real));
    for (h, r) in hand
        .modules
        .iter()
        .zip(real.modules.iter().filter(|m| m.origin == FileOrigin::Std))
    {
        assert_eq!((&h.path, &h.files, &h.defs), (&r.path, &r.files, &r.defs));
    }
}

#[test]
fn std_identities_are_qualified_names_s43() {
    let p = std_program();
    let path = |id: &str| p.display_path(stub_of(&p, id).unwrap());
    assert_eq!(path("list_take"), "std.prelude.List.take");
    assert_eq!(path("int_min"), "std.prelude.Int.min");
    assert_eq!(path("text_len"), "std.prelude.Text.len");
    assert_eq!(path("bytes_len"), "std.prelude.Bytes.len");
    assert_eq!(path("list_len"), "std.prelude.List.len");
    assert_eq!(path("option_unwrap_or"), "std.prelude.Option.unwrap_or");
    assert_eq!(path("result_unwrap_or"), "std.prelude.Result.unwrap_or");
    assert_eq!(path("flatten"), "std.prelude.flatten");
    assert_eq!(path("text_trim"), "std.text.trim");
    assert_eq!(path("bytes_from_list"), "std.bytes.from_list");
    assert_eq!(path("gen_int"), "std.gen.int");
    assert_eq!(path("gen_range"), "std.gen.range");
    for d in p.defs() {
        assert!(d.provisional);
        assert_eq!(d.stable_id.as_str(), p.display_path(d.id));
        assert_eq!(p.by_stable_id(&d.stable_id), Some(d.id));
    }
    let take = p
        .by_stable_id(&StableId::parse("std.prelude.List.take").unwrap())
        .unwrap();
    assert!(
        matches!(&p.def(take).name, DeclName::Member { receiver, name } if receiver == "List" && name == "take")
    );
    // The AST of a std definition is its own declaration (decl_index).
    assert_eq!(p.ast(take).display_name(), "take");
    assert_eq!(p.ast(p.std_defs.option).display_name(), "Option");
}

#[test]
fn builtin_signatures_follow_the_stubs() {
    let p = std_program();
    let take = p
        .def(stub_of(&p, "list_take").unwrap())
        .kind
        .as_fn()
        .unwrap();
    assert_eq!(take.generics, ["T"]);
    assert_eq!(take.params[0].name, "list");
    assert_eq!(take.params[0].ty, Ty::list(Ty::Param(0)));
    assert_eq!(take.params[1].ty, Ty::Int);
    assert_eq!(take.ret, Ty::list(Ty::Param(0)));
    assert_eq!(take.body, FnBody::Builtin("list_take".into()));
    let parse_int = p
        .def(stub_of(&p, "text_parse_int").unwrap())
        .kind
        .as_fn()
        .unwrap();
    assert_eq!(
        parse_int.ret,
        Ty::result(
            &p.std_defs,
            Ty::Int,
            Ty::Adt(p.std_defs.constraint_error, vec![])
        )
    );
    let positive_int = p
        .def(stub_of(&p, "gen_positive_int").unwrap())
        .kind
        .as_fn()
        .unwrap();
    assert!(positive_int.params.is_empty());
    assert!(
        matches!(&positive_int.ret, Ty::Gen(t) if matches!(**t, Ty::Refined(_, d) if d == p.std_defs.positive))
    );
    let ok_or = p
        .def(stub_of(&p, "option_ok_or").unwrap())
        .kind
        .as_fn()
        .unwrap();
    assert_eq!(ok_or.generics, ["T", "E"]);
    assert_eq!(
        ok_or.ret,
        Ty::result(&p.std_defs, Ty::Param(0), Ty::Param(1))
    );
    let int_ty = p
        .def(stub_of(&p, "list").unwrap())
        .kind
        .as_builtin_type()
        .unwrap();
    assert_eq!(int_ty.generics, ["T"]);
}

#[test]
fn builtin_requires_clauses_are_lowered_with_text_and_span() {
    let p = std_program();
    let requires = |id: &str| -> Vec<(String, String)> {
        p.def(stub_of(&p, id).unwrap())
            .kind
            .as_fn()
            .unwrap()
            .requires
            .iter()
            .map(|c| (c.text.clone(), p.source_map().text(c.span).to_string()))
            .collect()
    };
    let one = |t: &str| vec![(t.to_string(), format!("requires {t}"))];
    assert_eq!(requires("list_take"), one("n >= 0"));
    assert_eq!(requires("list_drop"), one("n >= 0"));
    assert_eq!(requires("text_split"), one("not sep.is_empty()"));
    assert_eq!(
        requires("bytes_from_list"),
        one("forall v in values: v >= 0 and v <= 255")
    );
    assert_eq!(requires("gen_range"), one("lo <= hi"));
    // Every other builtin has no requires clause (no hidden fault sources).
    let with_requires: Vec<String> = p
        .defs()
        .filter(|d| d.kind.as_fn().is_some_and(|f| !f.requires.is_empty()))
        .map(|d| p.display_path(d.id))
        .collect();
    assert_eq!(
        with_requires,
        [
            "std.bytes.from_list",
            "std.gen.range",
            "std.prelude.List.take",
            "std.prelude.List.drop",
            "std.text.split"
        ]
    );
    // The lowered HIR: a forall binder local, a member call of Text.is_empty.
    let from_list = p
        .def(stub_of(&p, "bytes_from_list").unwrap())
        .kind
        .as_fn()
        .unwrap();
    let ExprKind::Forall {
        binder, collection, ..
    } = &from_list.requires[0].expr.kind
    else {
        panic!("a forall")
    };
    assert_eq!(from_list.locals[binder.0 as usize].name, "v");
    assert_eq!(
        from_list.locals[binder.0 as usize].kind,
        BindingKind::Forall
    );
    assert!(matches!(collection.kind, ExprKind::Local(LocalId(0))));
    let split = p
        .def(stub_of(&p, "text_split").unwrap())
        .kind
        .as_fn()
        .unwrap();
    let ExprKind::Unary {
        op: UnaryOp::Not,
        operand,
    } = &split.requires[0].expr.kind
    else {
        panic!("not ...")
    };
    let ExprKind::Call { callee, args, .. } = &operand.kind else {
        panic!("a call")
    };
    assert_eq!(p.display_path(*callee), "std.prelude.Text.is_empty");
    assert_eq!(args.len(), 1);
    assert_eq!(operand.ty, Ty::Bool);
}

#[test]
fn identical_refinements_share_a_class_s55() {
    let mut b = ProgramBuilder::new();
    let m = b.module("reader.pages");
    let positive = b.add_int_refinement(m, "Positive", BinaryOp::Gt, 0);
    let other = b.add_int_refinement(m, "Natural", BinaryOp::Gt, -1);
    let h = b.hir(m);
    // `Int where 0 < self` is a distinct type in 0.1 (S29).
    let flipped = h.binary(BinaryOp::Lt, h.int(0), h.local(LocalId(0), Ty::Int));
    let flipped = b.add_refinement(m, "Flipped", Ty::Int, flipped, "0 < self");
    let std_positive = b.std_defs().positive;
    assert_eq!(b.class_of(positive), b.class_of(std_positive));
    assert_ne!(b.class_of(other), b.class_of(positive));
    assert_ne!(b.class_of(flipped), b.class_of(positive));
    // Ty::Refined compares classes only.
    assert_eq!(b.refined(positive), b.refined(std_positive));
    assert_ne!(b.refined(positive), b.refined(flipped));
    let p = b.build();
    assert_eq!(p.refinement_classes[0].members, [std_positive, positive]);
    assert_eq!(
        p.display_ty(&Ty::list(p_refined(&p, positive))),
        "List<Positive>"
    );
}

fn p_refined(p: &cpl_hir::Program, def: cpl_hir::DefId) -> Ty {
    crate::support::refined(p, def)
}

#[test]
fn user_declarations_functions_tests_properties() {
    let mut b = ProgramBuilder::new();
    let m = b.module("tests.pagination");
    let h = b.hir(m);
    let mut f = FnBuilder::new("twice", Ty::Int).public();
    let n = f.param("n", Ty::Int);
    let r = f.result();
    f.ensures(h.clause(
        h.binary(
            BinaryOp::Eq,
            h.local(r, Ty::Int),
            h.binary(BinaryOp::Mul, h.local(n, Ty::Int), h.int(2)),
        ),
        "result == n * 2",
    ));
    f.body(h.value(h.binary(BinaryOp::Add, h.local(n, Ty::Int), h.local(n, Ty::Int))));
    let twice = b.add_fn(m, f);

    let mut t = TestBuilder::new("pagination handles empty input");
    t.body(h.block(
        vec![cpl_hir::Stmt::Assert {
            expr: h.bool(true),
            text: "true".into(),
        }],
        None,
    ));
    let test = b.add_test(m, t);

    let mut prop = PropertyBuilder::new("pagination preserves input");
    let items = prop.binder(
        "items",
        GenExpr::List(Box::new(GenExpr::Int)),
        Ty::list(Ty::Int),
    );
    let size = prop.binder(
        "size",
        GenExpr::PositiveInt,
        b.refined(b.std_defs().positive),
    );
    prop.body(h.block(vec![], None));
    let property = b.add_property(m, prop);
    b.set_stable_id(property, "prop_k7w3qz2m");

    let p = b.build();
    assert_eq!(p.display_path(twice), "tests.pagination.twice");
    let fd = p.def(twice).kind.as_fn().unwrap();
    assert_eq!(fd.result_local, Some(LocalId(1)));
    assert_eq!(fd.locals[1].kind, BindingKind::Result);
    assert!(p.def(twice).is_pub);
    assert_eq!(
        p.display_path(test),
        "tests.pagination.\"pagination handles empty input\""
    );
    assert!(matches!(p.def(test).kind, DefKind::Test(_)));
    assert_eq!(p.def(property).stable_id.as_str(), "prop_k7w3qz2m");
    assert!(!p.def(property).provisional);
    let pd = p.def(property).kind.as_property().unwrap();
    assert_eq!((pd.binders[0].local, pd.binders[1].local), (items, size));
    assert_eq!(pd.locals[1].kind, BindingKind::For);
    let file = p.source_map().file(p.def(twice).file);
    assert_eq!(file.path, "src/tests/pagination.cpl");
    assert_eq!(file.origin, FileOrigin::Project);
    assert_eq!(
        p.lookup(&cpl_diag::SymbolPath::parse("tests.pagination.twice").unwrap()),
        Some(twice)
    );
}

#[test]
fn the_reader_fixture() {
    let r = reader();
    let p = &r.p;
    assert_eq!(p.display_path(r.page), "reader.pages.Page");
    assert_eq!(p.display_path(r.small), "m.Small");
    assert_eq!(p.def(r.paginate).stable_id.as_str(), "pgn_7c2d");
    assert_eq!(p.display_path(r.paginate), "reader.pages.paginate");
    assert_eq!(
        p.display_ty_in(
            &p.def(r.paginate).kind.as_fn().unwrap().ret,
            &["T".to_string()]
        ),
        "List<List<T>>"
    );
    // The take_n body calls the List.take stub with the receiver first.
    let fd = p.def(r.take_n).kind.as_fn().unwrap();
    let FnBody::Block(body) = &fd.body else {
        panic!()
    };
    let ExprKind::Call { callee, args, .. } = &body.tail.as_ref().unwrap().kind else {
        panic!()
    };
    assert_eq!(p.display_path(*callee), "std.prelude.List.take");
    assert_eq!(args.len(), 2);
    let _ = (r.pages, r.page_read);
}
