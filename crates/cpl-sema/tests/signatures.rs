//! Signatures (`spec/semantics.md` 1.5, 1.8, 3.1-3.3, 4.1, S7, S12, S13,
//! S27, S55, S66; toolchain 3.3, 6.5, T17): the `Ty` of every signature
//! position, the well-formedness rules of 1.8, exportability, effect
//! bounds, and interface references.

mod common;

use common::*;
use cpl_diag::{Severity, SymbolPath};
use cpl_hir::{DefKind, EffectRef, RefKind, Ty};
use serde_json::json;
use std::collections::BTreeSet;

fn lookup(result: &cpl_sema::CheckResult, path: &str) -> cpl_hir::DefId {
    result
        .program
        .lookup(&SymbolPath::parse(path).expect("a path"))
        .unwrap_or_else(|| panic!("no {path}"))
}

const SHAPES: &str = "module m.s

pub type PageId(Text)
pub type Positive = Int where self > 0

pub struct Pair<A, B> {
    left: A
    right: List<B>
}

pub enum Tree<T> {
    Leaf(T)
    Node(List<Tree<T>>)
}

pub capability Store {
    fn get(key: PageId) -> Result<Positive, Text>
        ensures result.is_ok() or result.is_err()
}

pub fn f<T>(x: T, using s: Store) -> Pair<T, Int>
    effects { s.get }
{
    ?todo
}
";

#[test]
fn every_signature_position_has_its_type() {
    let result = check_source(SHAPES);
    assert!(!result.has_errors(), "{}", render(&result));
    let p = &result.program;
    let page_id = lookup(&result, "m.s.PageId");
    let positive = lookup(&result, "m.s.Positive");
    let pair = lookup(&result, "m.s.Pair");
    let tree = lookup(&result, "m.s.Tree");
    let store = lookup(&result, "m.s.Store");
    let f = lookup(&result, "m.s.f");
    assert_eq!(
        p.def(page_id).kind.as_wrapper().expect("wrapper").inner,
        Ty::Text
    );
    let r = p.def(positive).kind.as_refinement().expect("refinement");
    assert_eq!(r.base, Ty::Int);
    assert_eq!(r.locals[r.self_local.0 as usize].name, "self");
    assert_eq!(r.locals[0].ty, Ty::Int);
    assert_eq!(r.predicate.text, "self > 0");
    let at = p.source_map().location(r.predicate.span);
    assert_eq!((at.line, at.col), (4, 25), "the clause starts at `where`");
    let s = p.def(pair).kind.as_struct().expect("struct");
    assert_eq!(s.fields[0].ty, Ty::Param(0));
    assert_eq!(s.fields[1].ty, Ty::list(Ty::Param(1)));
    let e = p.def(tree).kind.as_enum().expect("enum");
    assert_eq!(
        e.variants[1].payload,
        [Ty::list(Ty::Adt(tree, vec![Ty::Param(0)]))]
    );
    let c = p.def(store).kind.as_capability().expect("capability");
    let op = &c.ops[0];
    assert_eq!(op.params[0].ty, Ty::Wrapper(page_id));
    let refined = match &op.ret {
        Ty::Adt(d, args) if *d == p.std_defs().result => args[0].clone(),
        other => panic!("{other:?}"),
    };
    assert!(matches!(refined, Ty::Refined(_, d) if d == positive));
    assert_eq!(
        op.locals[op.result_local.expect("result").0 as usize].ty,
        op.ret
    );
    assert_eq!(op.ensures[0].text, "result.is_ok() or result.is_err()");
    let fd = p.def(f).kind.as_fn().expect("fn");
    assert_eq!(fd.params[0].ty, Ty::Param(0));
    assert_eq!(fd.using[0].ty, Ty::Capability(store));
    assert_eq!(fd.ret, Ty::Adt(pair, vec![Ty::Param(0), Ty::Int]));
    let declared: BTreeSet<EffectRef> = [EffectRef {
        param: fd.using[0].local,
        op: "get".into(),
    }]
    .into_iter()
    .collect();
    assert_eq!(fd.effects.declared, Some(declared));
    assert_eq!(fd.locals[fd.using[0].local.0 as usize].name, "s");
}

#[test]
fn signature_references_are_interface_type_uses() {
    let result = check_source(SHAPES);
    let p = &result.program;
    let f = lookup(&result, "m.s.f");
    let refs = p.references_from(f);
    assert!(!refs.is_empty());
    assert!(refs
        .iter()
        .all(|r| r.kind == RefKind::TypeUse && r.interface));
    let targets: Vec<String> = refs.iter().map(|r| p.display_path(r.to)).collect();
    assert!(targets.contains(&"m.s.Store".to_string()), "{targets:?}");
    assert!(targets.contains(&"m.s.Pair".to_string()), "{targets:?}");
    assert!(
        targets.contains(&"std.prelude.Int".to_string()),
        "{targets:?}"
    );
    let mut sorted = p.references.clone();
    sorted.sort_by_key(|r| (r.from, r.span));
    assert_eq!(sorted, p.references);
}

#[test]
fn containment_cycles_are_infinite_types() {
    let result = check_source(
        "module m.i\n\npub struct Left {\n    right: Right\n}\n\npub struct Right {\n    left: Left\n}\n\npub struct Box<T> {\n    value: T\n}\n\npub struct Lazy<T> {\n    items: List<T>\n}\n\npub struct Chain {\n    next: Box<Chain>\n}\n\npub struct Fine {\n    next: Lazy<Fine>\n}\n",
    );
    let got: Vec<(u32, String)> = project_triples(&result)
        .into_iter()
        .map(|(l, _, c)| (l, c))
        .collect();
    assert_eq!(
        got,
        [
            (3, "infinite_type".to_string()),
            (7, "infinite_type".to_string()),
            (19, "infinite_type".to_string()),
        ],
        "{}",
        render(&result)
    );
    let j = json_values(&result);
    assert_eq!(j[0]["type"], json!("Left"));
    assert_eq!(
        j[0]["notes"][0]["message"],
        json!("cycle: Left -> Right -> Left")
    );
    assert_eq!(j[2]["notes"][0]["message"], json!("cycle: Chain -> Chain"));
}

#[test]
fn refinement_cycles_and_dependents() {
    let result = check_source(
        "module m.c\n\ntype A = B where self > 0\ntype B = A where self > 1\ntype C = A where self > 2\n\nfn f(c: C) -> Int {\n    1\n}\n",
    );
    // S66: both declarations of the cycle; C depends on it and is
    // error-typed without a diagnostic of its own (S62).
    let got: Vec<(u32, String)> = project_triples(&result)
        .into_iter()
        .map(|(l, _, c)| (l, c))
        .collect();
    assert_eq!(
        got,
        [
            (3, "cyclic_type_definition".to_string()),
            (4, "cyclic_type_definition".to_string())
        ]
    );
    let j = json_values(&result);
    assert_eq!(j[0]["notes"][0]["message"], json!("cycle: A -> B -> A"));
    let f = lookup(&result, "m.c.f");
    assert_eq!(
        result.program.def(f).kind.as_fn().expect("fn").params[0].ty,
        Ty::Error
    );
}

#[test]
fn value_positions_reject_capabilities() {
    let result = check_source(
        "module m.v\n\npub capability Store {\n    fn get(k: Text) -> Int\n}\n\nfn f(xs: List<Store>) -> Int {\n    1\n}\n\nfn g(k: Text, using n: Int) -> Int\n    effects { n.get }\n{\n    1\n}\n",
    );
    let got: Vec<(u32, String)> = project_triples(&result)
        .into_iter()
        .map(|(l, _, c)| (l, c))
        .collect();
    assert_eq!(
        got,
        [
            (7, "capability_in_value_position".to_string()),
            (11, "using_param_not_capability".to_string()),
        ],
        "{}",
        render(&result)
    );
    let j = json_values(&result);
    assert_eq!(j[0]["span"]["col"], json!(15), "the type argument");
    assert_eq!(j[0]["category"], json!("effect"));
    assert_eq!(j[0]["type"], json!("Store"));
    assert_eq!(j[1]["name"], json!("n"));
    assert_eq!(j[1]["type"], json!("Int"));
    let g = lookup(&result, "m.v.g");
    let fd = result.program.def(g).kind.as_fn().expect("fn");
    assert_eq!(fd.using[0].ty, Ty::Error);
    assert_eq!(fd.effects.declared, Some(BTreeSet::new()));
}

#[test]
fn effect_bound_diagnostics() {
    let result = check_source(
        "module m.e\n\npub capability Store {\n    fn get(k: Text) -> Int\n}\n\nfn f(k: Text, using db: Store) -> Int\n    effects { k.get, db.put, db.get, db.get }\n{\n    1\n}\n",
    );
    let j = json_values(&result);
    let got: Vec<&str> = j
        .iter()
        .map(|d| d["code"].as_str().expect("a code"))
        .collect();
    // The body performs no operation call, so `db` is also unused (S29).
    assert_eq!(
        got,
        [
            "unused_capability",
            "effect_param_not_capability",
            "unknown_effect_operation",
            "duplicate_effect"
        ],
        "{}",
        render(&result)
    );
    assert_eq!(j[0]["name"], json!("db"));
    assert_eq!(j[1]["name"], json!("k"));
    assert_eq!(j[1]["type"], json!("Text"));
    assert_eq!(j[2]["name"], json!("put"));
    assert_eq!(j[2]["type"], json!("Store"));
    assert_eq!(j[3]["severity"], json!("warning"));
    assert_eq!(j[3]["effect"], json!("db.get"));
}

#[test]
fn exportability_fields() {
    let result = check_source(
        "module m.x\n\ntype Secret(Text)\n\npub fn f(xs: List<Secret>) -> Int {\n    1\n}\n",
    );
    let j = json_values(&result);
    assert_eq!(j.len(), 1, "{}", render(&result));
    assert_eq!(j[0]["code"], json!("private_type_in_public_signature"));
    assert_eq!(j[0]["symbol"], json!("m.x.f"));
    assert_eq!(j[0]["qualified_name"], json!("m.x.f"));
    assert_eq!(j[0]["facet"], json!("signature"));
    assert_eq!(j[0]["type"], json!("m.x.Secret"));
    assert_eq!(j[0]["span"]["col"], json!(19));
}

#[test]
fn type_parameters_and_parameters() {
    let result = check_source(
        "module m.t\n\npub struct S<T, U> {\n    v: T\n}\n\nfn f<A, B>(self: A, result: A) -> A\n    ensures true\n{\n    self\n}\n\nfn g(a: Int, a: Int) -> Int {\n    a\n}\n",
    );
    let got: Vec<(u32, Severity, String)> = project_triples(&result);
    assert_eq!(
        got,
        [
            (3, Severity::Error, "unused_type_parameter".to_string()),
            (7, Severity::Warning, "unused_type_parameter".to_string()),
            (7, Severity::Warning, "self_bound".to_string()),
            (7, Severity::Error, "result_name_reserved".to_string()),
            (13, Severity::Error, "duplicate_parameter".to_string()),
        ],
        "{}",
        render(&result)
    );
    let j = json_values(&result);
    assert_eq!(j[0]["name"], json!("U"));
    assert_eq!(j[1]["name"], json!("B"));
}

#[test]
fn type_paths() {
    let result = check_source(
        "module m.p\n\nfn a(x: Gen<Int>) -> Int {\n    1\n}\n\nfn b(x: std.gen.Gen<Int>) -> Int {\n    1\n}\n\nfn c(x: List<Int, Int>) -> gen.Positive {\n    1\n}\n\nfn d(x: std.prelude.Option<Int>, y: nowhere.T, z: std.text.T) -> Int {\n    1\n}\n",
    );
    let got: Vec<(u32, String)> = project_triples(&result)
        .into_iter()
        .map(|(l, _, c)| (l, c))
        .collect();
    assert_eq!(
        got,
        [
            (3, "unresolved_type".to_string()),
            (7, "unresolved_type".to_string()),
            (11, "type_argument_count_mismatch".to_string()),
            (15, "unresolved_module".to_string()),
            (15, "unresolved_type".to_string()),
        ],
        "{}",
        render(&result)
    );
    let j = json_values(&result);
    assert_eq!(j[2]["expected"], json!(1));
    assert_eq!(j[2]["actual"], json!(2));
    assert_eq!(j[4]["module"], json!("std.text"));
    let c = lookup(&result, "m.p.c");
    let ret = &result.program.def(c).kind.as_fn().expect("fn").ret;
    assert!(matches!(ret, Ty::Refined(_, d) if *d == result.program.std_defs().positive));
}

#[test]
fn generators_are_value_positions_only_in_std() {
    // std.gen's stubs take and return Gen<T> (stdlib 8.5); the project
    // cannot write Gen at all.
    let result = check_source("module m.g\n");
    assert!(result.diagnostics.is_empty());
    let list = lookup(&result, "std.gen.list");
    let fd = result.program.def(list).kind.as_fn().expect("fn");
    assert_eq!(fd.params[0].ty, Ty::Gen(Box::new(Ty::Param(0))));
    assert_eq!(fd.ret, Ty::Gen(Box::new(Ty::list(Ty::Param(0)))));
    assert!(matches!(result.program.def(list).kind, DefKind::Fn(_)));
}
