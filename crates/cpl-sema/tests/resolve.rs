//! Name resolution (`spec/semantics.md` 1.5, 1.6, 2.1-2.5, S5, S6, S8-S11,
//! S62, S67): imports and their diagnostics, qualified chains, constructor
//! resolution, local scopes, and the fields of the diagnostics (S49, 9.3,
//! 9.4).

mod common;

use common::*;
use cpl_diag::{Facet, Severity};
use cpl_hir::ImportTarget;
use serde_json::json;

fn errors_only(result: &cpl_sema::CheckResult) -> Vec<Triple> {
    project_triples(result)
}

#[test]
fn imports_are_file_scoped_and_recorded() {
    let lib = "module lib.a\n\npub fn f() -> Int {\n    1\n}\n\npub type Id(Text)\n";
    let user = "module app.user\n\nuse lib.a.{f, Id}\nuse lib.{a}\n\nfn g(id: Id) -> Int {\n    f() + a.f()\n}\n";
    let other = "module app.user\n\nfn h() -> Int {\n    f()\n}\n";
    let result = check_files(&[
        ("src/lib/a.cpl", lib),
        ("src/app/user.cpl", user),
        ("src/app/user/other.cpl", other),
    ]);
    // S5: the sibling file of app.user does not see the imports.
    assert_eq!(
        errors_only(&result),
        [(4, Severity::Error, "unresolved_name".to_string())]
    );
    let p = &result.program;
    let targets: Vec<&ImportTarget> = p.imports.iter().map(|i| &i.target).collect();
    assert_eq!(targets.len(), 3);
    assert!(matches!(targets[0], ImportTarget::Def(d) if p.display_path(*d) == "lib.a.f"));
    assert!(matches!(targets[1], ImportTarget::Def(d) if p.display_path(*d) == "lib.a.Id"));
    assert!(matches!(targets[2], ImportTarget::Module(m) if p.module(*m).path == ["lib", "a"]));
}

#[test]
fn namespace_aliases_and_unused_imports() {
    let lib = "module ns.deep.x\n\npub fn f() -> Int {\n    1\n}\n";
    let user = "module app.u\n\nuse ns.{deep}\nuse std.text.{trim, to_upper}\n\nfn g() -> Int {\n    deep.x.f()\n}\n\nfn h(t: Text) -> Text {\n    trim(t)\n}\n";
    let result = check_files(&[("src/ns/deep/x.cpl", lib), ("src/app/u.cpl", user)]);
    assert_eq!(
        errors_only(&result),
        [(4, Severity::Warning, "unused_import".to_string())]
    );
    let unused = json_values(&result).remove(0);
    assert_eq!(unused["name"], json!("to_upper"));
    let p = &result.program;
    assert!(p
        .imports
        .iter()
        .any(|i| i.target == ImportTarget::Namespace(vec!["ns".into(), "deep".into()])));
}

#[test]
fn import_diagnostics_carry_their_fields() {
    let lib = "module lib.a\n\nfn hidden() -> Int {\n    1\n}\n";
    let user = "module app.u\n\nuse lib.a.{hidden}\nuse lib.a.{nothing}\nuse lib.zz.{x}\nuse std.gen.{list}\nuse std.prelude.{flatten}\n\nfn g() -> Int {\n    hidden() + nothing()\n}\n";
    let result = check_files(&[("src/lib/a.cpl", lib), ("src/app/u.cpl", user)]);
    let got: Vec<(u32, String)> = errors_only(&result)
        .into_iter()
        .map(|(l, _, c)| (l, c))
        .collect();
    assert_eq!(
        got,
        [
            (3, "private_item".to_string()),
            (4, "unresolved_name".to_string()),
            (5, "unresolved_module".to_string()),
            (6, "generator_import".to_string()),
            (7, "unused_import".to_string()),
        ],
        "{}",
        render(&result)
    );
    let j = json_values(&result);
    assert!(j.iter().all(|d| d["category"] == json!("resolve")));
    assert_eq!(j[0]["symbol"], json!("lib.a.hidden"));
    assert_eq!(j[0]["qualified_name"], json!("lib.a.hidden"));
    assert_eq!(j[1]["name"], json!("nothing"));
    assert_eq!(j[1]["module"], json!("lib.a"));
    assert_eq!(j[2]["module"], json!("lib.zz"));
    assert_eq!(j[3]["name"], json!("list"));
}

#[test]
fn import_conflicts_and_prelude_names() {
    let result = check_source(
        "module m.c\n\nuse std.text.{trim}\nuse std.text.{trim}\nuse std.bytes.{to_list}\n\nfn to_list() -> Int {\n    1\n}\n\nfn tidy(t: Text) -> Text {\n    trim(t)\n}\n",
    );
    let got: Vec<(u32, String)> = errors_only(&result)
        .into_iter()
        .map(|(l, _, c)| (l, c))
        .collect();
    assert_eq!(
        got,
        [
            (4, "duplicate_import".to_string()),
            (5, "import_conflicts_with_declaration".to_string()),
        ]
    );
    let j = json(&result);
    assert_eq!(j[0].notes.len(), 1, "the earlier import");
    assert_eq!(j[0].notes[0].span.as_ref().map(|s| s.line), Some(3));
    assert_eq!(j[1].notes[0].span.as_ref().map(|s| s.line), Some(7));
    let result = check_source("module m.p\n\nfn check(n: Int) -> Int {\n    n\n}\n\npub struct Float64 {\n    v: Int\n}\n");
    let j = json(&result);
    assert_eq!(j.len(), 2);
    assert!(j.iter().all(|d| d.code == "prelude_name_redefined"));
    assert_eq!(j[1].notes[0].message, "reserved for a later edition");
}

#[test]
fn chains_through_modules_and_types() {
    let lib = "module lib.a\n\npub enum E {\n    V(Int)\n    W\n}\n\npub type Id(Text)\n\npub fn f(e: E) -> Int {\n    1\n}\n";
    let user = "module app.u\n\nfn g() -> Int {\n    lib.a.f(lib.a.E.V(1)) + lib.a.f(lib.a.E.W)\n}\n\nfn h() -> lib.a.Id {\n    lib.a.Id(\"x\")\n}\n\nfn bad() -> Int {\n    lib.a.E.X + lib.a.f.x + lib.b.f() + lib.a.nothing\n}\n";
    let result = check_files(&[("src/lib/a.cpl", lib), ("src/app/u.cpl", user)]);
    let got: Vec<String> = errors_only(&result)
        .into_iter()
        .map(|(_, _, c)| c)
        .collect();
    assert_eq!(
        got,
        [
            "unresolved_name",
            "unresolved_name",
            "unresolved_module",
            "unresolved_name"
        ],
        "{}",
        render(&result)
    );
    let j = json_values(&result);
    assert_eq!(j[2]["module"], json!("lib.b"));
    assert_eq!(j[3]["module"], json!("lib.a"));
}

#[test]
fn not_a_value_and_not_a_constructor() {
    let result = check_source(
        "module m.v\n\npub struct S {\n    x: Int\n}\n\ntype R = Int where self > 0\n\nfn f<T>(t: T) -> Int {\n    let _ = std\n    let _ = S\n    let _ = R\n    let _ = T\n    let _ = R { x: 1 }\n    let _ = gen.int\n    1\n}\n",
    );
    let got: Vec<(u32, String)> = errors_only(&result)
        .into_iter()
        .map(|(l, _, c)| (l, c))
        .collect();
    // `gen.int` is a module-level property: not a `not_a_value` (S54); in a
    // `let` initializer it is the typer's `generator_outside_for` (S14).
    assert_eq!(
        got,
        [
            (10, "not_a_value".to_string()),
            (11, "not_a_constructor".to_string()),
            (12, "not_a_constructor".to_string()),
            (13, "not_a_constructor".to_string()),
            (14, "not_a_constructor".to_string()),
            (15, "generator_outside_for".to_string()),
        ],
        "{}",
        render(&result)
    );
}

#[test]
fn constructors_without_an_expected_type_are_decided() {
    let result = check_source(
        "module m.k\n\npub enum A {\n    Same\n}\n\npub enum B {\n    Same\n}\n\nfn f() -> Int {\n    let _ = Same\n    let _ = Nowhere\n    1\n}\n\nfn g() -> A {\n    Same\n}\n",
    );
    let got: Vec<(u32, String)> = errors_only(&result)
        .into_iter()
        .map(|(l, _, c)| (l, c))
        .collect();
    // `Same` in the tail of g has an expected type: the typer decides it.
    assert_eq!(
        got,
        [
            (12, "ambiguous_constructor".to_string()),
            (13, "unresolved_name".to_string()),
        ]
    );
    let j = json_values(&result);
    assert_eq!(j[0]["candidates"], json!(["m.k.A.Same", "m.k.B.Same"]));
}

#[test]
fn local_scopes() {
    let result = check_source(
        "module m.s\n\nfn f(a: Int, b: Int) -> Int\n    ensures result > a\n{\n    let c = a\n    let c = b\n    var d = 0\n    d = 1\n    let _e = c\n    let result = 3\n    match Some(a) {\n        Some(x) => x\n        None => b\n    }\n}\n",
    );
    let got: Vec<(u32, Severity, String)> = errors_only(&result);
    assert_eq!(
        got,
        [
            (7, Severity::Error, "shadowed_binding".to_string()),
            (8, Severity::Warning, "unused_binding".to_string()),
            (11, Severity::Error, "result_name_reserved".to_string()),
            (11, Severity::Warning, "unused_binding".to_string()),
        ],
        "{}",
        render(&result)
    );
    let shadow = &json(&result)[0];
    assert_eq!(shadow.notes[0].span.as_ref().map(|s| s.line), Some(6));
}

#[test]
fn result_and_self_rules() {
    let result = check_source(
        "module m.r\n\nfn f(n: Int) -> Int\n    requires n > 0 and (\n        result > 0)\n{\n    let self = n\n    self + 1\n}\n\nfn g(n: Int) -> Int {\n    self\n}\n",
    );
    let got: Vec<(u32, Severity, String)> = errors_only(&result);
    // `result_outside_ensures` in a clause spans the clause (S63 (2)).
    assert_eq!(
        got,
        [
            (4, Severity::Error, "result_outside_ensures".to_string()),
            (7, Severity::Warning, "self_bound".to_string()),
            (12, Severity::Error, "self_outside_where".to_string()),
        ],
        "{}",
        render(&result)
    );
}

#[test]
fn struct_literals() {
    let result = check_source(
        "module m.l\n\npub struct P {\n    x: Int\n    y: Int\n}\n\nfn f() -> P {\n    P { x: 1, x: 2, z: 3 }\n}\n",
    );
    let got: Vec<String> = errors_only(&result)
        .into_iter()
        .map(|(_, _, c)| c)
        .collect();
    assert_eq!(
        got,
        ["missing_field", "duplicate_field", "unknown_field"],
        "{}",
        render(&result)
    );
    let j = json_values(&result);
    let missing = j
        .iter()
        .find(|d| d["code"] == json!("missing_field"))
        .expect("missing_field");
    assert_eq!(missing["missing"], json!(["y"]));
    assert_eq!(missing["type"], json!("P"));
}

#[test]
fn private_in_public_contract_fields() {
    let result = check_source(
        "module m.c\n\nfn small(n: Int) -> Bool {\n    n < 10\n}\n\npub fn f(n: Int) -> Int\n    requires small(n) and small(n + 1)\n{\n    n\n}\n",
    );
    let j = json_values(&result);
    assert_eq!(j.len(), 1, "one per callee and clause");
    let d = &j[0];
    assert_eq!(d["code"], json!("private_in_public_contract"));
    assert_eq!(d["symbol"], json!("m.c.f"));
    assert_eq!(d["facet"], json!(Facet::Contract.as_str()));
    assert_eq!(d["clause"], json!("small(n) and small(n + 1)"));
    assert_eq!(d["callee"], json!("m.c.small"));
    assert_eq!(d["span"]["line"], json!(8));
    assert_eq!(d["span"]["col"], json!(5));
}

#[test]
fn tests_and_properties_are_not_functions() {
    let result = check_source(
        "module m.t\n\nuse std.text.{parse_int}\n\ntest \"t\" {\n    let _n = parse_int(\"1\")?\n    return\n}\n\nproperty \"p\"\n    for n in gen.range(0, n)\n    for n in gen.int\n{\n    assert true\n}\n",
    );
    let got: Vec<(u32, String)> = errors_only(&result)
        .into_iter()
        .map(|(l, _, c)| (l, c))
        .collect();
    assert_eq!(
        got,
        [
            (6, "propagate_outside_function".to_string()),
            (7, "return_outside_function".to_string()),
            (11, "unresolved_name".to_string()),
            (12, "shadowed_binding".to_string()),
        ],
        "{}",
        render(&result)
    );
}

#[test]
fn locals_hide_universe_names_and_callees_note_the_hidden_item() {
    let result = check_source(
        "module m.h\n\nuse std.text.{trim}\n\nfn f(t: Text) -> Int {\n    let gen = 1\n    let std = 2\n    let trim = 3\n    gen + std + trim + std.x\n}\n\nfn g(t: Text) -> Text {\n    trim(t)\n}\n",
    );
    // `std.x` is a member of a local, not a module chain: the typer's
    // `unknown_member` on `Int` is the only diagnostic.
    assert_eq!(
        triples(&result),
        [(9, cpl_diag::Severity::Error, "unknown_member".to_string())],
        "{}",
        render(&result)
    );
}

#[test]
fn members_are_not_free_functions() {
    let result =
        check_source("module m.f\n\nfn f(xs: List<Int>) -> Int {\n    len(take(xs, 1))\n}\n");
    let j = json(&result);
    assert_eq!(j.len(), 2, "{}", render(&result));
    assert!(j.iter().all(|d| d.code == "unresolved_name"));
    assert_eq!(
        j[0].notes[0].message,
        "`len` is a property of Text, Bytes, List, not a free function; write `x.len`"
    );
    assert_eq!(
        j[1].notes[0].message,
        "`take` is a method of List, not a free function; write `x.take(...)`"
    );
}
