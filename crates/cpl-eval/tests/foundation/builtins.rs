//! The builtin registry and every builtin: stdlib 1.4-1.7, 2-6, 10.4-10.6,
//! and the acceptance rows of stdlib 12; semantics 6.10, S58.

use crate::support::{
    big, bytes, call, call_from, fault, int, ints, json, list, ok, reader, text, texts, Reader,
};
use cpl_diag::{Facet, StableId};
use cpl_eval::builtins::{
    check_failure, check_gen_range, check_order, constraint_error, constraint_type_name, ids,
    lookup, stub_of,
};
use cpl_eval::testing::std_program;
use cpl_eval::{ClauseKind, EvalConfig, Fault, FaultKind, Interpreter, Value};
use cpl_hir::{DefKind, ExprKind, FnBody, Lit, Program};
use cpl_syntax::ast::BinaryOp;
use num_bigint::BigInt;
use std::collections::BTreeSet;

/// The complete id set of stdlib 10.6, in the table's order.
const SPEC_IDS: [&str; 60] = [
    "int",
    "bool",
    "text",
    "bytes",
    "list",
    "gen",
    "context",
    "flatten",
    "int_to_text",
    "int_abs",
    "int_min",
    "int_max",
    "text_len",
    "text_is_empty",
    "bytes_len",
    "bytes_is_empty",
    "list_len",
    "list_is_empty",
    "list_take",
    "list_drop",
    "list_drop_last",
    "list_push",
    "list_get",
    "list_first",
    "list_last",
    "list_reverse",
    "list_contains",
    "option_is_some",
    "option_is_none",
    "option_unwrap_or",
    "option_ok_or",
    "result_is_ok",
    "result_is_err",
    "result_unwrap_or",
    "result_ok",
    "result_err",
    "text_trim",
    "text_trim_start",
    "text_trim_end",
    "text_starts_with",
    "text_ends_with",
    "text_contains",
    "text_split",
    "text_join",
    "text_to_upper",
    "text_to_lower",
    "text_parse_int",
    "text_from_int",
    "text_chars",
    "bytes_from_list",
    "bytes_to_list",
    "bytes_from_text",
    "bytes_to_text",
    "gen_int",
    "gen_positive_int",
    "gen_bool",
    "gen_text",
    "gen_list",
    "gen_option",
    "gen_range",
];

// ===========================================================================
// The registry
// ===========================================================================

#[test]
fn registry_is_exactly_stdlib_10_6() {
    let registered: Vec<&str> = ids().collect();
    assert_eq!(registered.len(), 60);
    let mut sorted = registered.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted, registered, "sorted and unique (binary search)");
    let spec: BTreeSet<&str> = SPEC_IDS.into_iter().collect();
    assert_eq!(spec.len(), 60);
    assert_eq!(registered.into_iter().collect::<BTreeSet<_>>(), spec);
    for id in SPEC_IDS {
        assert!(lookup(id).is_some(), "{id}");
    }
}

#[test]
fn the_stubs_and_the_registry_name_the_same_ids() {
    // The std_builtin_missing consistency rule of stdlib 10.2, both ways.
    let p = std_program();
    let mut from_stubs = BTreeSet::new();
    for d in p.defs() {
        match &d.kind {
            DefKind::Fn(f) => match &f.body {
                FnBody::Builtin(id) => {
                    assert!(from_stubs.insert(id.clone()), "{id} used twice");
                }
                FnBody::Block(_) => panic!("std has no bodied fn (S36): {}", p.display_path(d.id)),
            },
            DefKind::BuiltinType(t) => {
                assert!(
                    from_stubs.insert(t.builtin.clone()),
                    "{} used twice",
                    t.builtin
                );
            }
            _ => {}
        }
    }
    let registered: BTreeSet<String> = ids().map(str::to_string).collect();
    assert_eq!(from_stubs, registered);
}

#[test]
fn unknown_ids_are_not_found() {
    for id in [
        "",
        "list_map",
        "List_take",
        "list_take ",
        "gen_float",
        "check",
    ] {
        assert!(lookup(id).is_none(), "{id:?}");
    }
}

#[test]
fn stub_of_finds_the_declaring_stub() {
    let p = std_program();
    let take = stub_of(&p, "list_take").unwrap();
    assert_eq!(p.display_path(take), "std.prelude.List.take");
    assert_eq!(
        p.display_path(stub_of(&p, "int").unwrap()),
        "std.prelude.Int"
    );
    assert_eq!(p.display_path(stub_of(&p, "gen").unwrap()), "std.gen.Gen");
    assert_eq!(
        p.display_path(stub_of(&p, "text_split").unwrap()),
        "std.text.split"
    );
    assert_eq!(stub_of(&p, "nope"), None);
}

#[test]
fn types_and_generators_are_not_callable() {
    let p = std_program();
    for id in ["int", "bool", "text", "bytes", "list", "gen"] {
        let f = fault(&p, id, vec![]);
        assert_eq!(f.kind, FaultKind::Internal, "{id}");
        assert_eq!(f.data["kind"], "builtin_type_called");
    }
    for id in [
        "gen_int",
        "gen_positive_int",
        "gen_bool",
        "gen_text",
        "gen_list",
        "gen_option",
    ] {
        let f = fault(&p, id, vec![]);
        assert_eq!(f.data["kind"], "generator_not_a_value", "{id}");
    }
    let f = fault(&p, "gen_range", vec![int(1), int(5)]);
    assert_eq!(f.data["kind"], "generator_not_a_value");
    let f = fault(&p, "gen_range", vec![int(1), int(1)]);
    assert_eq!(f.data["kind"], "generator_not_a_value");
}

#[test]
fn type_confusions_are_internal_errors() {
    let p = std_program();
    let f = fault(&p, "list_take", vec![text("x"), int(1)]);
    assert_eq!(f.kind, FaultKind::Internal);
    assert_eq!(f.data["kind"], "type_confusion");
    assert!(f.data["detail"].as_str().unwrap().contains("list_take"));
    let f = fault(&p, "list_len", vec![]);
    assert_eq!(f.data["kind"], "builtin_arity");
    let f = fault(&p, "flatten", vec![ints(&[1])]);
    assert_eq!(f.data["kind"], "type_confusion");
    let f = fault(&p, "text_join", vec![ints(&[1]), text(",")]);
    assert_eq!(f.data["kind"], "type_confusion");
    let f = fault(&p, "option_is_some", vec![int(1)]);
    assert_eq!(f.data["kind"], "type_confusion");
    let f = fault(&p, "result_ok", vec![Value::some(&p, int(1))]);
    assert_eq!(f.data["kind"], "type_confusion");
    let f = fault(&p, "bytes_from_list", vec![texts(&["a"])]);
    assert_eq!(f.data["kind"], "type_confusion");
}

// ===========================================================================
// Preconditions (stdlib 10.4, S39; semantics 6.10, S58)
// ===========================================================================

/// The common shape of a builtin precondition fault.
fn assert_requires(p: &Program, f: &Fault, symbol: &str, clause: &str, input: &str) {
    assert_eq!(f.kind, FaultKind::ContractViolation(ClauseKind::Requires));
    assert_eq!(f.symbol, Some(StableId::parse(symbol).unwrap()));
    assert_eq!(f.facet, Some(Facet::Contract));
    let d = f.to_diagnostic(p);
    assert_eq!(d.code, "contract_violation");
    assert_eq!(d.qualified_name.as_deref(), Some(symbol));
    assert_eq!(d.symbol.as_ref().map(StableId::as_str), Some(symbol));
    assert_eq!(d.facet, Some(Facet::Contract));
    assert_eq!(d.data["clause_kind"], "requires");
    assert_eq!(d.data["attributed_to"], "caller");
    assert_eq!(d.data["clause"], clause);
    assert_eq!(d.data["status"], "failed");
    assert_eq!(d.data["input"], json(input));
    assert!(!d.data.contains_key("actual"));
    assert_eq!(d.message, format!("requires clause failed: {clause}"));
    // The span is the clause in the stub file, from its keyword.
    let span = f.span.expect("the stub's clause span");
    assert_eq!(p.source_map().text(span), format!("requires {clause}"));
    assert!(p.source_map().file(span.file).path.starts_with("std/"));
}

#[test]
fn take_and_drop_of_a_negative_count() {
    // stdlib 12 faults: [1].take(-1), [1].drop(-1), clause n >= 0.
    let r = reader();
    let p = &r.p;
    let f = fault(p, "list_take", vec![ints(&[1]), int(-1)]);
    assert_requires(
        p,
        &f,
        "std.prelude.List.take",
        "n >= 0",
        r#"{"self": [1], "n": -1}"#,
    );
    assert!(
        !f.data.contains_key("caller"),
        "no caller at the top level of cpl run"
    );
    let f = fault(p, "list_drop", vec![ints(&[1]), int(-1)]);
    assert_requires(
        p,
        &f,
        "std.prelude.List.drop",
        "n >= 0",
        r#"{"self": [1], "n": -1}"#,
    );
    // A huge negative count is still negative.
    let f = fault(
        p,
        "list_take",
        vec![ints(&[]), big("-100000000000000000000")],
    );
    assert_requires(
        p,
        &f,
        "std.prelude.List.take",
        "n >= 0",
        r#"{"self": [], "n": -100000000000000000000}"#,
    );
    assert_eq!(
        p.source_map().location(f.span.unwrap()).file,
        "std/prelude.cpl"
    );
}

#[test]
fn the_caller_is_the_enclosing_declaration() {
    // corpus.lists.take_n([1], -1): caller "corpus.lists.take_n".
    let r = reader();
    let p = &r.p;
    let f = call_from(p, r.take_n, "list_take", vec![ints(&[1]), int(-1)]).unwrap_err();
    assert_eq!(f.data["caller"], "reader.pages.take_n");
    let d = f.to_diagnostic(p);
    assert_eq!(d.data["caller"], "reader.pages.take_n");
    // An @id caller is still named by its qualified name (semantics 9.3).
    let f = call_from(p, r.paginate, "text_split", vec![text("a"), text("")]).unwrap_err();
    assert_eq!(f.data["caller"], "reader.pages.paginate");
}

#[test]
fn split_with_an_empty_separator() {
    let r = reader();
    let p = &r.p;
    let f = fault(p, "text_split", vec![text("a"), text("")]);
    assert_requires(
        p,
        &f,
        "std.text.split",
        "not sep.is_empty()",
        r#"{"text": "a", "sep": ""}"#,
    );
    assert_eq!(
        p.source_map().location(f.span.unwrap()).file,
        "std/text.cpl"
    );
}

#[test]
fn from_list_out_of_range() {
    // stdlib 12: from_list([256]) and from_list([-1]).
    let r = reader();
    let p = &r.p;
    let clause = "forall v in values: v >= 0 and v <= 255";
    for (values, input) in [
        (ints(&[256]), r#"{"values": [256]}"#),
        (ints(&[-1]), r#"{"values": [-1]}"#),
        (ints(&[0, 255, 1000]), r#"{"values": [0, 255, 1000]}"#),
        (
            list(vec![big("100000000000000000000000")]),
            r#"{"values": [100000000000000000000000]}"#,
        ),
    ] {
        let f = fault(p, "bytes_from_list", vec![values]);
        assert_requires(p, &f, "std.bytes.from_list", clause, input);
    }
}

#[test]
fn gen_range_with_lo_above_hi() {
    // stdlib 12: `for x in gen.range(5, 1)` faults at property setup.
    let r = reader();
    let p = &r.p;
    let it = Interpreter::new(p, EvalConfig::default());
    let f = check_gen_range(&it, &BigInt::from(5), &BigInt::from(1)).unwrap_err();
    assert_requires(p, &f, "std.gen.range", "lo <= hi", r#"{"lo": 5, "hi": 1}"#);
    assert!(check_gen_range(&it, &BigInt::from(1), &BigInt::from(1)).is_ok());
    assert!(check_gen_range(&it, &BigInt::from(-5), &BigInt::from(5)).is_ok());
    let f = fault(p, "gen_range", vec![int(5), int(1)]);
    assert_requires(p, &f, "std.gen.range", "lo <= hi", r#"{"lo": 5, "hi": 1}"#);
}

#[test]
fn preconditions_without_stubs_fall_back_to_the_spec_texts() {
    // A hand-built program with no std at all still gets the 10.4 shape.
    let sm = std::sync::Arc::new(cpl_diag::SourceMap::new());
    let p = Program::empty(sm);
    let f = fault(&p, "list_take", vec![ints(&[1]), int(-1)]);
    assert_eq!(
        f.symbol,
        Some(StableId::parse("std.prelude.List.take").unwrap())
    );
    assert_eq!(f.data["clause"], "n >= 0");
    assert_eq!(f.data["input"], json(r#"{"self": [1], "n": -1}"#));
    assert_eq!(f.span, None);
    let f = fault(&p, "text_split", vec![text("a"), text("")]);
    assert_eq!(f.data["input"], json(r#"{"text": "a", "sep": ""}"#));
}

// ===========================================================================
// flatten and the Int members
// ===========================================================================

#[test]
fn flatten_concatenates_in_order() {
    let p = std_program();
    // stdlib 12: flatten([[1, 2], [], [3]]) and flatten([]).
    assert_eq!(
        ok(
            &p,
            "flatten",
            vec![list(vec![ints(&[1, 2]), ints(&[]), ints(&[3])])]
        ),
        ints(&[1, 2, 3])
    );
    assert_eq!(ok(&p, "flatten", vec![list(vec![])]), ints(&[]));
    assert_eq!(
        ok(&p, "flatten", vec![list(vec![ints(&[]), ints(&[])])]),
        ints(&[])
    );
    assert_eq!(
        ok(&p, "flatten", vec![list(vec![ints(&[4, 5])])]),
        ints(&[4, 5])
    );
    assert_eq!(
        ok(
            &p,
            "flatten",
            vec![list(vec![ints(&[1]), ints(&[2]), ints(&[3, 4])])]
        ),
        ints(&[1, 2, 3, 4])
    );
    // Nested lists stay nested one level down.
    let xss = list(vec![
        list(vec![ints(&[1])]),
        list(vec![ints(&[2]), ints(&[])]),
    ]);
    assert_eq!(
        ok(&p, "flatten", vec![xss]),
        list(vec![ints(&[1]), ints(&[2]), ints(&[])])
    );
}

#[test]
fn int_members() {
    let p = std_program();
    // stdlib 12: (-5).abs(), 3.min(5), (-3).max(0), 4.min(4).
    assert_eq!(ok(&p, "int_abs", vec![int(-5)]), int(5));
    assert_eq!(ok(&p, "int_abs", vec![int(0)]), int(0));
    assert_eq!(ok(&p, "int_abs", vec![int(7)]), int(7));
    assert_eq!(
        ok(&p, "int_abs", vec![big("-123456789012345678901234567890")]),
        big("123456789012345678901234567890")
    );
    assert_eq!(ok(&p, "int_min", vec![int(3), int(5)]), int(3));
    assert_eq!(ok(&p, "int_min", vec![int(5), int(3)]), int(3));
    assert_eq!(ok(&p, "int_max", vec![int(-3), int(0)]), int(0));
    assert_eq!(ok(&p, "int_min", vec![int(4), int(4)]), int(4));
    assert_eq!(ok(&p, "int_max", vec![int(4), int(4)]), int(4));
    assert_eq!(
        ok(
            &p,
            "int_max",
            vec![big("-1"), big("-100000000000000000000")]
        ),
        int(-1)
    );
    // stdlib 12: (-5).to_text(), 0.to_text(), 1_000.to_text().
    assert_eq!(ok(&p, "int_to_text", vec![int(-5)]), text("-5"));
    assert_eq!(ok(&p, "int_to_text", vec![int(0)]), text("0"));
    assert_eq!(ok(&p, "int_to_text", vec![int(1_000)]), text("1000"));
    assert_eq!(
        ok(&p, "int_to_text", vec![big("-98765432109876543210")]),
        text("-98765432109876543210")
    );
}

// ===========================================================================
// Text and Bytes members
// ===========================================================================

#[test]
fn text_len_counts_scalar_values() {
    let p = std_program();
    // stdlib 12: "héllo".len, "😀".len, "e\u{301}".len, "".len.
    for (t, n) in [
        ("h\u{e9}llo", 5),
        ("\u{1F600}", 1),
        ("e\u{301}", 2),
        ("", 0),
        ("\u{4e2d}\u{6587}", 2),
    ] {
        assert_eq!(ok(&p, "text_len", vec![text(t)]), int(n), "{t:?}");
    }
    assert_eq!(ok(&p, "text_is_empty", vec![text("")]), Value::Bool(true));
    assert_eq!(ok(&p, "text_is_empty", vec![text(" ")]), Value::Bool(false));
}

#[test]
fn bytes_members() {
    let p = std_program();
    assert_eq!(ok(&p, "bytes_len", vec![bytes(&[1, 2, 3])]), int(3));
    assert_eq!(ok(&p, "bytes_len", vec![bytes(&[])]), int(0));
    assert_eq!(
        ok(&p, "bytes_is_empty", vec![bytes(&[])]),
        Value::Bool(true)
    );
    assert_eq!(
        ok(&p, "bytes_is_empty", vec![bytes(&[0])]),
        Value::Bool(false)
    );
}

// ===========================================================================
// List members (stdlib 4.2, 4.5, S15, S16, S18)
// ===========================================================================

#[test]
fn take_clamps_at_the_length() {
    let p = std_program();
    let xs = ints(&[1, 2, 3]);
    // stdlib 12: take(2), take(5), take(0).
    assert_eq!(ok(&p, "list_take", vec![xs.clone(), int(2)]), ints(&[1, 2]));
    assert_eq!(
        ok(&p, "list_take", vec![xs.clone(), int(5)]),
        ints(&[1, 2, 3])
    );
    assert_eq!(ok(&p, "list_take", vec![xs.clone(), int(0)]), ints(&[]));
    assert_eq!(
        ok(&p, "list_take", vec![xs.clone(), int(3)]),
        ints(&[1, 2, 3])
    );
    assert_eq!(ok(&p, "list_take", vec![ints(&[]), int(0)]), ints(&[]));
    assert_eq!(ok(&p, "list_take", vec![ints(&[]), int(4)]), ints(&[]));
    // Clamp before converting the count (stdlib 4.1): beyond u64 and usize.
    assert_eq!(
        ok(
            &p,
            "list_take",
            vec![ints(&[1, 2]), big("10000000000000000000000")]
        ),
        ints(&[1, 2])
    );
    assert_eq!(
        ok(
            &p,
            "list_take",
            vec![xs.clone(), big("18446744073709551616")]
        ),
        xs
    );
}

#[test]
fn drop_clamps_at_the_length() {
    let p = std_program();
    let xs = ints(&[1, 2, 3]);
    // stdlib 12: drop(2), drop(5), drop(0).
    assert_eq!(ok(&p, "list_drop", vec![xs.clone(), int(2)]), ints(&[3]));
    assert_eq!(ok(&p, "list_drop", vec![xs.clone(), int(5)]), ints(&[]));
    assert_eq!(
        ok(&p, "list_drop", vec![xs.clone(), int(0)]),
        ints(&[1, 2, 3])
    );
    assert_eq!(ok(&p, "list_drop", vec![xs.clone(), int(3)]), ints(&[]));
    assert_eq!(ok(&p, "list_drop", vec![ints(&[]), int(0)]), ints(&[]));
    assert_eq!(
        ok(&p, "list_drop", vec![xs, big("10000000000000000000000")]),
        ints(&[])
    );
}

#[test]
fn drop_last_push_len_is_empty() {
    let p = std_program();
    // stdlib 12: [1, 2, 3].drop_last(), [].drop_last().
    assert_eq!(
        ok(&p, "list_drop_last", vec![ints(&[1, 2, 3])]),
        ints(&[1, 2])
    );
    assert_eq!(ok(&p, "list_drop_last", vec![ints(&[])]), ints(&[]));
    assert_eq!(ok(&p, "list_drop_last", vec![ints(&[9])]), ints(&[]));
    // stdlib 12: [1].push(2); the receiver is unchanged (immutability).
    let xs = ints(&[1]);
    let ys = ok(&p, "list_push", vec![xs.clone(), int(2)]);
    assert_eq!(ys, ints(&[1, 2]));
    assert_eq!(xs, ints(&[1]));
    assert_eq!(ok(&p, "list_push", vec![ints(&[]), int(7)]), ints(&[7]));
    assert_eq!(
        ok(&p, "list_push", vec![list(vec![]), ints(&[])]),
        list(vec![ints(&[])])
    );
    // stdlib 12: [1, 2, 3].len, [].len, [].is_empty(), [1].is_empty().
    assert_eq!(ok(&p, "list_len", vec![ints(&[1, 2, 3])]), int(3));
    assert_eq!(ok(&p, "list_len", vec![ints(&[])]), int(0));
    assert_eq!(ok(&p, "list_is_empty", vec![ints(&[])]), Value::Bool(true));
    assert_eq!(
        ok(&p, "list_is_empty", vec![ints(&[1])]),
        Value::Bool(false)
    );
}

#[test]
fn get_first_last() {
    let p = std_program();
    let xs = ints(&[1, 2, 3]);
    let some = |v: Value| Value::some(&p, v);
    let none = Value::none(&p);
    // stdlib 12: get(1), get(3), get(-1); first(), last(), [].last().
    assert_eq!(ok(&p, "list_get", vec![xs.clone(), int(1)]), some(int(2)));
    assert_eq!(ok(&p, "list_get", vec![xs.clone(), int(3)]), none);
    assert_eq!(ok(&p, "list_get", vec![xs.clone(), int(-1)]), none);
    assert_eq!(ok(&p, "list_get", vec![xs.clone(), int(0)]), some(int(1)));
    assert_eq!(
        ok(
            &p,
            "list_get",
            vec![xs.clone(), big("100000000000000000000")]
        ),
        none
    );
    assert_eq!(
        ok(
            &p,
            "list_get",
            vec![xs.clone(), big("-100000000000000000000")]
        ),
        none
    );
    assert_eq!(ok(&p, "list_get", vec![ints(&[]), int(0)]), none);
    assert_eq!(ok(&p, "list_first", vec![xs.clone()]), some(int(1)));
    assert_eq!(ok(&p, "list_last", vec![xs]), some(int(3)));
    assert_eq!(ok(&p, "list_first", vec![ints(&[])]), none);
    assert_eq!(ok(&p, "list_last", vec![ints(&[])]), none);
    assert_eq!(ok(&p, "list_last", vec![ints(&[8])]), some(int(8)));
}

#[test]
fn reverse_and_contains() {
    let p = std_program();
    // stdlib 12: reverse(), contains(2), contains(4).
    assert_eq!(
        ok(&p, "list_reverse", vec![ints(&[1, 2, 3])]),
        ints(&[3, 2, 1])
    );
    assert_eq!(ok(&p, "list_reverse", vec![ints(&[])]), ints(&[]));
    assert_eq!(
        ok(&p, "list_contains", vec![ints(&[1, 2, 3]), int(2)]),
        Value::Bool(true)
    );
    assert_eq!(
        ok(&p, "list_contains", vec![ints(&[1, 2, 3]), int(4)]),
        Value::Bool(false)
    );
    assert_eq!(
        ok(&p, "list_contains", vec![ints(&[]), int(4)]),
        Value::Bool(false)
    );
    // Structural equality, nested values included; no normalization.
    let xss = list(vec![ints(&[1]), ints(&[2, 3])]);
    assert_eq!(
        ok(&p, "list_contains", vec![xss.clone(), ints(&[2, 3])]),
        Value::Bool(true)
    );
    assert_eq!(
        ok(&p, "list_contains", vec![xss, ints(&[3, 2])]),
        Value::Bool(false)
    );
    assert_eq!(
        ok(
            &p,
            "list_contains",
            vec![texts(&["\u{e9}"]), text("e\u{301}")]
        ),
        Value::Bool(false)
    );
}

// ===========================================================================
// Option and Result members (stdlib 5)
// ===========================================================================

#[test]
fn option_members() {
    let p = std_program();
    let some1 = Value::some(&p, int(1));
    let none = Value::none(&p);
    // stdlib 12 rows.
    assert_eq!(
        ok(&p, "option_unwrap_or", vec![some1.clone(), int(0)]),
        int(1)
    );
    assert_eq!(
        ok(&p, "option_unwrap_or", vec![none.clone(), int(0)]),
        int(0)
    );
    assert_eq!(
        ok(&p, "option_is_some", vec![some1.clone()]),
        Value::Bool(true)
    );
    assert_eq!(
        ok(&p, "option_is_some", vec![none.clone()]),
        Value::Bool(false)
    );
    assert_eq!(
        ok(&p, "option_is_none", vec![none.clone()]),
        Value::Bool(true)
    );
    assert_eq!(
        ok(&p, "option_is_none", vec![some1.clone()]),
        Value::Bool(false)
    );
    assert_eq!(
        ok(&p, "option_ok_or", vec![Value::some(&p, int(2)), text("e")]),
        Value::ok(&p, int(2))
    );
    assert_eq!(
        ok(&p, "option_ok_or", vec![none, text("e")]),
        Value::err(&p, text("e"))
    );
    // A nested option unwraps one level.
    let nested = Value::some(&p, Value::none(&p));
    assert_eq!(
        ok(&p, "option_unwrap_or", vec![nested, some1]),
        Value::none(&p)
    );
}

#[test]
fn result_members() {
    let p = std_program();
    let ok1 = Value::ok(&p, int(1));
    let err_x = Value::err(&p, text("x"));
    // stdlib 12 rows.
    assert_eq!(
        ok(&p, "result_unwrap_or", vec![ok1.clone(), int(0)]),
        int(1)
    );
    assert_eq!(
        ok(&p, "result_unwrap_or", vec![err_x.clone(), int(0)]),
        int(0)
    );
    assert_eq!(ok(&p, "result_is_ok", vec![ok1.clone()]), Value::Bool(true));
    assert_eq!(
        ok(&p, "result_is_ok", vec![err_x.clone()]),
        Value::Bool(false)
    );
    assert_eq!(
        ok(&p, "result_is_err", vec![err_x.clone()]),
        Value::Bool(true)
    );
    assert_eq!(
        ok(&p, "result_is_err", vec![ok1.clone()]),
        Value::Bool(false)
    );
    assert_eq!(
        ok(&p, "result_ok", vec![ok1.clone()]),
        Value::some(&p, int(1))
    );
    assert_eq!(ok(&p, "result_ok", vec![err_x.clone()]), Value::none(&p));
    assert_eq!(
        ok(&p, "result_err", vec![err_x]),
        Value::some(&p, text("x"))
    );
    assert_eq!(ok(&p, "result_err", vec![ok1]), Value::none(&p));
}

// ===========================================================================
// std.text (stdlib 6)
// ===========================================================================

/// The 25 White_Space scalars of S22.
const WHITE_SPACE: [char; 25] = [
    '\u{9}', '\u{a}', '\u{b}', '\u{c}', '\u{d}', '\u{20}', '\u{85}', '\u{a0}', '\u{1680}',
    '\u{2000}', '\u{2001}', '\u{2002}', '\u{2003}', '\u{2004}', '\u{2005}', '\u{2006}', '\u{2007}',
    '\u{2008}', '\u{2009}', '\u{200a}', '\u{2028}', '\u{2029}', '\u{202f}', '\u{205f}', '\u{3000}',
];

#[test]
fn trim_uses_exactly_the_white_space_set() {
    let p = std_program();
    // stdlib 12: trim("  5 "), trim("\u{a0}x\u{a0}"), trim("\u{200b}x").
    assert_eq!(ok(&p, "text_trim", vec![text("  5 ")]), text("5"));
    assert_eq!(ok(&p, "text_trim", vec![text("\u{a0}x\u{a0}")]), text("x"));
    assert_eq!(
        ok(&p, "text_trim", vec![text("\u{200b}x")]),
        text("\u{200b}x")
    );
    assert_eq!(
        ok(&p, "text_trim", vec![text("\u{feff}x\u{feff}")]),
        text("\u{feff}x\u{feff}")
    );
    assert_eq!(ok(&p, "text_trim", vec![text("")]), text(""));
    assert_eq!(ok(&p, "text_trim", vec![text("a b")]), text("a b"));
    for c in WHITE_SPACE {
        let padded = format!("{c}{c}x{c}y{c}");
        assert_eq!(
            ok(&p, "text_trim", vec![text(&padded)]),
            text(&format!("x{c}y")),
            "U+{:04X}",
            c as u32
        );
    }
    let all: String = WHITE_SPACE.iter().collect();
    assert_eq!(ok(&p, "text_trim", vec![text(&all)]), text(""));
    // Not whitespace: zero width space, BOM, NEL-like letters, combining marks.
    for c in [
        '\u{200b}', '\u{feff}', '\u{180e}', '\u{301}', 'a', '\u{0}', '\u{1c}',
    ] {
        let t = format!("{c}x{c}");
        assert_eq!(
            ok(&p, "text_trim", vec![text(&t)]),
            text(&t),
            "U+{:04X}",
            c as u32
        );
    }
    // stdlib 12: trim_start("  a "), trim_end("  a ").
    assert_eq!(ok(&p, "text_trim_start", vec![text("  a ")]), text("a "));
    assert_eq!(ok(&p, "text_trim_end", vec![text("  a ")]), text("  a"));
    assert_eq!(
        ok(&p, "text_trim_start", vec![text("\u{3000}a")]),
        text("a")
    );
    assert_eq!(ok(&p, "text_trim_end", vec![text("a\u{2029}")]), text("a"));
}

#[test]
fn prefix_suffix_and_contains() {
    let p = std_program();
    let b = |id: &str, a: &str, x: &str| ok(&p, id, vec![text(a), text(x)]);
    // stdlib 12 rows.
    assert_eq!(b("text_starts_with", "abc", "ab"), Value::Bool(true));
    assert_eq!(b("text_starts_with", "abc", ""), Value::Bool(true));
    assert_eq!(b("text_starts_with", "a", "ab"), Value::Bool(false));
    assert_eq!(b("text_ends_with", "abc", "bc"), Value::Bool(true));
    assert_eq!(b("text_contains", "abc", "b"), Value::Bool(true));
    assert_eq!(b("text_contains", "abc", ""), Value::Bool(true));
    // Boundaries.
    assert_eq!(b("text_ends_with", "abc", ""), Value::Bool(true));
    assert_eq!(b("text_ends_with", "c", "bc"), Value::Bool(false));
    assert_eq!(b("text_contains", "", ""), Value::Bool(true));
    assert_eq!(b("text_contains", "abc", "d"), Value::Bool(false));
    assert_eq!(b("text_contains", "abc", "abcd"), Value::Bool(false));
    // Scalar sequences, no normalization: "é" (U+00E9) does not start with "e".
    assert_eq!(
        b("text_starts_with", "\u{e9}t\u{e9}", "e"),
        Value::Bool(false)
    );
    assert_eq!(
        b("text_contains", "h\u{e9}llo", "\u{e9}l"),
        Value::Bool(true)
    );
    assert_eq!(
        b("text_contains", "he\u{301}llo", "\u{e9}"),
        Value::Bool(false)
    );
    assert_eq!(
        b("text_ends_with", "x\u{1F600}", "\u{1F600}"),
        Value::Bool(true)
    );
}

#[test]
fn split_is_leftmost_first_and_non_overlapping() {
    let p = std_program();
    let s = |t: &str, sep: &str| ok(&p, "text_split", vec![text(t), text(sep)]);
    // stdlib 12 and S24 worked values.
    assert_eq!(s("a,b,,c", ","), texts(&["a", "b", "", "c"]));
    assert_eq!(s("", ","), texts(&[""]));
    assert_eq!(s("a,", ","), texts(&["a", ""]));
    assert_eq!(s(",a", ","), texts(&["", "a"]));
    assert_eq!(s("aaa", "aa"), texts(&["", "a"]));
    assert_eq!(s("abc", ","), texts(&["abc"]));
    assert_eq!(s(",", ","), texts(&["", ""]));
    assert_eq!(s("a::b::", "::"), texts(&["a", "b", ""]));
    assert_eq!(s("x\u{e9}y\u{e9}", "\u{e9}"), texts(&["x", "y", ""]));
}

#[test]
fn join_texts() {
    let p = std_program();
    let j = |parts: &[&str], sep: &str| ok(&p, "text_join", vec![texts(parts), text(sep)]);
    // stdlib 12 rows.
    assert_eq!(j(&["a", "b"], ", "), text("a, b"));
    assert_eq!(j(&[], ","), text(""));
    assert_eq!(j(&["x"], ","), text("x"));
    assert_eq!(j(&["", ""], ","), text(","));
    assert_eq!(j(&["a", "b", "c"], ""), text("abc"));
}

#[test]
fn case_mapping_is_ascii_only() {
    let p = std_program();
    // stdlib 12: to_upper("abc\u{e9}"), to_lower("ABC").
    assert_eq!(
        ok(&p, "text_to_upper", vec![text("abc\u{e9}")]),
        text("ABC\u{e9}")
    );
    assert_eq!(ok(&p, "text_to_lower", vec![text("ABC")]), text("abc"));
    // S23: no Unicode case mapping, length preserved.
    assert_eq!(
        ok(&p, "text_to_upper", vec![text("stra\u{df}e")]),
        text("STRA\u{df}E")
    );
    assert_eq!(
        ok(&p, "text_to_lower", vec![text("\u{130}I")]),
        text("\u{130}i")
    );
    assert_eq!(ok(&p, "text_to_upper", vec![text("")]), text(""));
    assert_eq!(
        ok(&p, "text_to_lower", vec![text("123 xyz")]),
        text("123 xyz")
    );
}

#[test]
fn parse_int_accepts_exactly_the_s25_grammar() {
    let p = std_program();
    let parsed = |t: &str| ok(&p, "text_parse_int", vec![text(t)]);
    // stdlib 12 rows.
    assert_eq!(parsed("42"), Value::ok(&p, int(42)));
    assert_eq!(parsed("-0"), Value::ok(&p, int(0)));
    assert_eq!(parsed("+7"), Value::ok(&p, int(7)));
    assert_eq!(parsed("007"), Value::ok(&p, int(7)));
    assert_eq!(
        parsed("99999999999999999999"),
        Value::ok(&p, big("99999999999999999999"))
    );
    assert_eq!(
        parsed("-123456789012345678901234567890"),
        Value::ok(&p, big("-123456789012345678901234567890"))
    );
    assert_eq!(parsed("0"), Value::ok(&p, int(0)));
    for bad in [
        " 42", "", "4_2", "\u{ff11}", "-", "+", "42 ", "1.0", "0x10", "--1", "+-1", "1e3",
        "\u{661}", "\u{a0}1",
    ] {
        let v = parsed(bad);
        let Some(Err(e)) = v.as_result(&p) else {
            panic!("{bad:?} parsed: {v:?}");
        };
        // S44: the bare prelude name; the message names the input.
        let Value::Struct { fields, .. } = e else {
            panic!("{e:?}")
        };
        assert_eq!(fields[0], text("Int"), "{bad:?}");
        let message = fields[1].as_text().unwrap();
        assert!(message.ends_with("is not a decimal integer"), "{message}");
    }
    let v = parsed("12a");
    let Some(Err(Value::Struct { fields, .. })) = v.as_result(&p) else {
        panic!()
    };
    assert_eq!(fields[1], text("\"12a\" is not a decimal integer"));
}

#[test]
fn from_int_and_chars() {
    let p = std_program();
    // stdlib 12: from_int(-12), chars("h\u{e9}y").
    assert_eq!(ok(&p, "text_from_int", vec![int(-12)]), text("-12"));
    assert_eq!(ok(&p, "text_from_int", vec![int(0)]), text("0"));
    assert_eq!(
        ok(&p, "text_from_int", vec![big("12345678901234567890")]),
        text("12345678901234567890")
    );
    assert_eq!(
        ok(&p, "text_chars", vec![text("h\u{e9}y")]),
        texts(&["h", "\u{e9}", "y"])
    );
    assert_eq!(ok(&p, "text_chars", vec![text("")]), texts(&[]));
    assert_eq!(
        ok(&p, "text_chars", vec![text("e\u{301}\u{1F600}")]),
        texts(&["e", "\u{301}", "\u{1F600}"])
    );
}

// ===========================================================================
// std.bytes (stdlib 3.4)
// ===========================================================================

#[test]
fn bytes_conversions() {
    let p = std_program();
    // stdlib 12 rows.
    let hi = ok(&p, "bytes_from_list", vec![ints(&[104, 105])]);
    assert_eq!(hi, bytes(b"hi"));
    assert_eq!(ok(&p, "bytes_len", vec![hi]), int(2));
    let b12 = ok(&p, "bytes_from_list", vec![ints(&[1, 2])]);
    assert_eq!(ok(&p, "bytes_to_list", vec![b12]), ints(&[1, 2]));
    assert_eq!(ok(&p, "bytes_from_list", vec![ints(&[])]), bytes(&[]));
    assert_eq!(
        ok(&p, "bytes_from_list", vec![ints(&[0, 255])]),
        bytes(&[0, 255])
    );
    assert_eq!(
        ok(&p, "bytes_to_list", vec![bytes(&[0, 128, 255])]),
        ints(&[0, 128, 255])
    );
    let e_acute = ok(&p, "bytes_from_text", vec![text("\u{e9}")]);
    assert_eq!(e_acute, bytes(&[0xc3, 0xa9]));
    assert_eq!(ok(&p, "bytes_len", vec![e_acute.clone()]), int(2));
    assert_eq!(
        ok(&p, "bytes_to_text", vec![e_acute]),
        Value::ok(&p, text("\u{e9}"))
    );
    assert_eq!(ok(&p, "bytes_from_text", vec![text("")]), bytes(&[]));
    assert_eq!(
        ok(&p, "bytes_from_text", vec![text("\u{1F600}")]),
        bytes(&[0xf0, 0x9f, 0x98, 0x80])
    );
}

#[test]
fn to_text_is_strict_utf8() {
    let p = std_program();
    let invalid = |b: &[u8], offset: usize| {
        let v = ok(&p, "bytes_to_text", vec![bytes(b)]);
        let Some(Err(Value::Struct { fields, .. })) = v.as_result(&p) else {
            panic!("{b:?} decoded: {v:?}")
        };
        // S44: type_name "Text".
        assert_eq!(fields[0], text("Text"));
        assert_eq!(
            fields[1],
            text(&format!("invalid UTF-8 at byte {offset}")),
            "{b:?}"
        );
    };
    invalid(&[255], 0); // stdlib 12
    invalid(&[0x61, 0xc3], 1); // truncated sequence
    invalid(&[0xc0, 0x80], 0); // overlong NUL
    invalid(&[0xe0, 0x80, 0x80], 0); // overlong
    invalid(&[0x61, 0xed, 0xa0, 0x80], 1); // an encoded surrogate
    invalid(&[0xf4, 0x90, 0x80, 0x80], 0); // above U+10FFFF
    invalid(&[0x80], 0); // a lone continuation byte
    assert_eq!(
        ok(&p, "bytes_to_text", vec![bytes(&[])]),
        Value::ok(&p, text(""))
    );
    assert_eq!(
        ok(&p, "bytes_to_text", vec![bytes(&[0xf4, 0x8f, 0xbf, 0xbf])]),
        Value::ok(&p, text("\u{10ffff}"))
    );
}

// ===========================================================================
// check<R> and ConstraintError (stdlib 1.4, 1.5, S44; semantics 6.10)
// ===========================================================================

/// Evaluate a predicate of the form `self <op> <int literal>`.
fn holds(r: &Reader, def: cpl_hir::DefId, v: i64) -> bool {
    let pred = &r.p.def(def).kind.as_refinement().unwrap().predicate.expr;
    let ExprKind::Binary { op, rhs, .. } = &pred.kind else {
        panic!()
    };
    let ExprKind::Lit(Lit::Int(k)) = &rhs.kind else {
        panic!()
    };
    let v = BigInt::from(v);
    match op {
        BinaryOp::Gt => &v > k,
        BinaryOp::Lt => &v < k,
        other => panic!("{other:?}"),
    }
}

/// `check<R>(v)` with the predicates evaluated by [`holds`].
fn check(r: &Reader, refined: cpl_hir::DefId, v: i64) -> Value {
    for def in check_order(&r.p, refined) {
        if !holds(r, def, v) {
            return check_failure(&r.p, def, &int(v));
        }
    }
    Value::ok(&r.p, int(v))
}

/// The (type_name, message) of an `Err(ConstraintError)`.
fn constraint(r: &Reader, v: &Value) -> (String, String) {
    let Some(Err(Value::Struct { def, fields })) = v.as_result(&r.p) else {
        panic!("not an Err(ConstraintError): {v:?}")
    };
    assert_eq!(*def, r.p.std_defs.constraint_error);
    (
        fields[0].as_text().unwrap().to_string(),
        fields[1].as_text().unwrap().to_string(),
    )
}

#[test]
fn check_order_is_the_chain_root_first() {
    let r = reader();
    let p = &r.p;
    assert_eq!(check_order(p, r.positive), vec![r.positive]);
    assert_eq!(check_order(p, r.small), vec![r.positive, r.small]);
    assert_eq!(
        check_order(p, p.std_defs.positive),
        vec![p.std_defs.positive]
    );
    assert_eq!(check_order(p, r.page), vec![]);
}

#[test]
fn check_results_and_type_names_of_s44() {
    let r = reader();
    let p = &r.p;
    // stdlib 12: check<Positive>(3) == Ok(3); check<Positive>(0).is_err().
    assert_eq!(check(&r, r.positive, 3), Value::ok(p, int(3)));
    // In reader.pages: check<Positive>(0) names reader.pages.Positive.
    assert_eq!(
        constraint(&r, &check(&r, r.positive, 0)),
        (
            "reader.pages.Positive".to_string(),
            "0 does not satisfy self > 0".to_string()
        )
    );
    // check<gen.Positive>(0) names std.gen.Positive (one type, S29).
    assert_eq!(
        constraint(&r, &check(&r, p.std_defs.positive, 0)),
        (
            "std.gen.Positive".to_string(),
            "0 does not satisfy self > 0".to_string()
        )
    );
    // type Small = Positive where self < 10 in m: the root predicate fails first.
    assert_eq!(
        constraint(&r, &check(&r, r.small, 0)),
        (
            "reader.pages.Positive".to_string(),
            "0 does not satisfy self > 0".to_string()
        )
    );
    assert_eq!(
        constraint(&r, &check(&r, r.small, 20)),
        (
            "m.Small".to_string(),
            "20 does not satisfy self < 10".to_string()
        )
    );
    assert_eq!(check(&r, r.small, 5), Value::ok(p, int(5)));
    // A negative value renders in decimal with its sign (value text).
    assert_eq!(
        constraint(&r, &check(&r, r.positive, -3)).1,
        "-3 does not satisfy self > 0"
    );
}

#[test]
fn constraint_errors() {
    let r = reader();
    let p = &r.p;
    assert_eq!(constraint_type_name(p, r.positive), "reader.pages.Positive");
    assert_eq!(
        constraint_type_name(p, p.std_defs.positive),
        "std.gen.Positive"
    );
    assert_eq!(constraint_type_name(p, p.std_defs.option), "Option");
    assert_eq!(
        constraint_type_name(p, p.std_defs.constraint_error),
        "ConstraintError"
    );
    assert_eq!(constraint_type_name(p, stub_of(p, "int").unwrap()), "Int");
    assert_eq!(
        constraint_type_name(p, p.std_defs.foreign_error),
        "std.foreign.ForeignError"
    );
    // Fields in declaration order: type_name, then message.
    let ce = constraint_error(p, "reader.pages.Size", "too large");
    assert_eq!(
        ce,
        Value::new_struct(
            p.std_defs.constraint_error,
            vec![text("reader.pages.Size"), text("too large")]
        )
    );
    assert_eq!(
        cpl_eval::value_to_json(
            p,
            &ce,
            &cpl_hir::Ty::Adt(p.std_defs.constraint_error, vec![])
        ),
        json(
            r#"{"$type": "ConstraintError", "type_name": "reader.pages.Size", "message": "too large"}"#
        )
    );
    // stdlib 12: match std.text.parse_int("x") { Err(e) => e.type_name ... } is "Int".
    let v = call(p, "text_parse_int", vec![text("x")]).unwrap();
    assert_eq!(constraint(&r, &v).0, "Int");
}

#[test]
fn unit_and_wrapper_equality_rows() {
    // stdlib 12: Unit == Unit, PageId("a") == PageId("a").
    let r = reader();
    assert!(Value::unit(&r.p).structurally_equal(&Value::unit(&r.p)));
    assert!(
        Value::wrap(r.page_id, text("a")).structurally_equal(&Value::wrap(r.page_id, text("a")))
    );
}
