//! Value JSON and value text: toolchain 7.3 and T30, semantics S40.

use crate::support::{big, bytes, int, ints, json, list, reader, refined, text, texts, Reader};
use cpl_eval::builtins::constraint_error;
use cpl_eval::json::{
    int_to_json, refined_positions, value_text_untyped, value_to_json_untyped, RefinedPosition,
};
use cpl_eval::testing::ProgramBuilder;
use cpl_eval::{value_from_json, value_text, value_to_json, HandleId, Value};
use cpl_hir::{Program, Ty};
use cpl_syntax::ast::BinaryOp;

fn enc(p: &Program, v: &Value, ty: &Ty) -> String {
    serde_json::to_string(&value_to_json(p, v, ty)).unwrap()
}

fn option(r: &Reader, t: Ty) -> Ty {
    Ty::option(&r.p.std_defs, t)
}

fn result(r: &Reader, t: Ty, e: Ty) -> Ty {
    Ty::result(&r.p.std_defs, t, e)
}

/// `ConstraintError`.
fn ce_ty(p: &Program) -> Ty {
    Ty::Adt(p.std_defs.constraint_error, vec![])
}

fn page(r: &Reader, id: &str, t: &str) -> Value {
    Value::new_struct(r.page, vec![Value::wrap(r.page_id, text(id)), text(t)])
}

fn missing(r: &Reader, id: &str) -> Value {
    Value::new_enum(r.store_error, 0, vec![Value::wrap(r.page_id, text(id))])
}

// ===========================================================================
// The encoding table of toolchain 7.3
// ===========================================================================

#[test]
fn int_is_a_number_with_every_digit() {
    let r = reader();
    let p = &r.p;
    assert_eq!(enc(p, &int(42), &Ty::Int), "42");
    assert_eq!(enc(p, &int(-7), &Ty::Int), "-7");
    assert_eq!(enc(p, &int(0), &Ty::Int), "0");
    let huge = "-123456789012345678901234567890123456789012345678901234567890";
    assert_eq!(enc(p, &big(huge), &Ty::Int), huge);
    assert_eq!(
        enc(p, &big("18446744073709551616"), &Ty::Int),
        "18446744073709551616"
    );
    assert_eq!(enc(p, &int(i64::MIN), &Ty::Int), "-9223372036854775808");
    // A refined Int is a plain number (refinements are erased).
    assert_eq!(enc(p, &int(3), &refined(&r.p, r.positive)), "3");
    assert_eq!(
        int_to_json(&"99999999999999999999".parse().unwrap()).to_string(),
        "99999999999999999999"
    );
}

#[test]
fn scalars_unit_and_bytes() {
    let r = reader();
    let p = &r.p;
    assert_eq!(
        value_to_json(p, &Value::Bool(true), &Ty::Bool),
        json("true")
    );
    assert_eq!(
        value_to_json(p, &text("h\u{e9}llo \u{1F600}"), &Ty::Text),
        json(r#""héllo 😀""#)
    );
    assert_eq!(enc(p, &text("\"\\\n"), &Ty::Text), r#""\"\\\n""#);
    assert_eq!(
        value_to_json(p, &Value::unit(p), &Ty::unit(&p.std_defs)),
        json(r#"{"$type": "Unit"}"#)
    );
    assert_eq!(
        value_to_json(p, &bytes(b"hi"), &Ty::Bytes),
        json(r#"{"$type": "Bytes", "hex": "6869"}"#)
    );
    assert_eq!(
        value_to_json(p, &bytes(&[0, 15, 16, 255]), &Ty::Bytes),
        json(r#"{"$type": "Bytes", "hex": "000f10ff"}"#)
    );
    assert_eq!(
        value_to_json(p, &bytes(&[]), &Ty::Bytes),
        json(r#"{"$type": "Bytes", "hex": ""}"#)
    );
}

#[test]
fn lists_structs_enums_wrappers() {
    let r = reader();
    let p = &r.p;
    assert_eq!(
        value_to_json(p, &ints(&[1, 2, 3]), &Ty::list(Ty::Int)),
        json("[1, 2, 3]")
    );
    assert_eq!(value_to_json(p, &ints(&[]), &Ty::list(Ty::Int)), json("[]"));
    // toolchain 7.3: PageId("p1") is {"$type": "reader.pages.PageId", "$payload": ["p1"]}.
    assert_eq!(
        value_to_json(
            p,
            &Value::wrap(r.page_id, text("p1")),
            &Ty::Wrapper(r.page_id)
        ),
        json(r#"{"$type": "reader.pages.PageId", "$payload": ["p1"]}"#)
    );
    assert_eq!(
        value_to_json(p, &page(&r, "p", "t"), &Ty::Adt(r.page, vec![])),
        json(
            r#"{"$type": "reader.pages.Page", "id": {"$type": "reader.pages.PageId", "$payload": ["p"]}, "text": "t"}"#
        )
    );
    assert_eq!(
        value_to_json(p, &missing(&r, "p"), &Ty::Adt(r.store_error, vec![])),
        json(
            r#"{"$type": "reader.pages.StoreError", "$variant": "Missing", "$payload": [{"$type": "reader.pages.PageId", "$payload": ["p"]}]}"#
        )
    );
    // A payload-less variant omits "$payload".
    assert_eq!(
        value_to_json(
            p,
            &Value::new_enum(r.store_error, 1, vec![]),
            &Ty::Adt(r.store_error, vec![])
        ),
        json(r#"{"$type": "reader.pages.StoreError", "$variant": "Unavailable"}"#)
    );
    assert_eq!(
        value_to_json(
            p,
            &Value::new_struct(r.empty, vec![]),
            &Ty::Adt(r.empty, vec![])
        ),
        json(r#"{"$type": "reader.pages.Empty"}"#)
    );
}

#[test]
fn prelude_types_use_bare_names_t30() {
    let r = reader();
    let p = &r.p;
    // toolchain 7.2's example result: Ok([[1, 2], [3]]).
    let ok = Value::ok(p, list(vec![ints(&[1, 2]), ints(&[3])]));
    let ty = result(&r, Ty::list(Ty::list(Ty::Int)), ce_ty(p));
    assert_eq!(
        value_to_json(p, &ok, &ty),
        json(r#"{"$type": "Result", "$variant": "Ok", "$payload": [[[1, 2], [3]]]}"#)
    );
    // Some(Some(1)) nests two tagged objects; None has no "$payload".
    let nested = Value::some(p, Value::some(p, int(1)));
    assert_eq!(
        value_to_json(p, &nested, &option(&r, option(&r, Ty::Int))),
        json(
            r#"{"$type": "Option", "$variant": "Some", "$payload": [{"$type": "Option", "$variant": "Some", "$payload": [1]}]}"#
        )
    );
    assert_eq!(
        value_to_json(p, &Value::none(p), &option(&r, Ty::Int)),
        json(r#"{"$type": "Option", "$variant": "None"}"#)
    );
    let ce = constraint_error(p, "reader.pages.Positive", "0 does not satisfy self > 0");
    assert_eq!(
        value_to_json(p, &Value::err(p, ce), &result(&r, Ty::Int, ce_ty(p))),
        json(
            r#"{"$type": "Result", "$variant": "Err", "$payload": [{"$type": "ConstraintError", "type_name": "reader.pages.Positive", "message": "0 does not satisfy self > 0"}]}"#
        )
    );
}

#[test]
fn std_types_outside_the_prelude_are_qualified_t30() {
    let r = reader();
    let p = &r.p;
    let fe = p.std_defs.foreign_error;
    let kind_def = match &p.def(fe).kind.as_struct().unwrap().fields[0].ty {
        Ty::Adt(d, _) => *d,
        other => panic!("{other:?}"),
    };
    let detail_def = match &p.def(fe).kind.as_struct().unwrap().fields[2].ty {
        Ty::List(t) => match &**t {
            Ty::Adt(d, _) => *d,
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    };
    let v = Value::new_struct(
        fe,
        vec![
            Value::new_enum(kind_def, 3, vec![]),
            text("worker crashed"),
            list(vec![Value::new_struct(
                detail_def,
                vec![text("adapter"), text("py")],
            )]),
        ],
    );
    assert_eq!(
        value_to_json(p, &v, &Ty::Adt(fe, vec![])),
        json(
            r#"{"$type": "std.foreign.ForeignError", "kind": {"$type": "std.foreign.ForeignErrorKind", "$variant": "Transport"}, "message": "worker crashed", "details": [{"$type": "std.foreign.ForeignDetail", "key": "adapter", "value": "py"}]}"#
        )
    );
}

#[test]
fn capability_handles_are_output_only() {
    let r = reader();
    let p = &r.p;
    let h = Value::Capability(HandleId(3));
    assert_eq!(
        value_to_json(p, &h, &Ty::Capability(r.page_read)),
        json(r#"{"$type": "reader.pages.PageRead", "$handle": 3}"#)
    );
    assert_eq!(value_to_json_untyped(p, &h), json(r#"{"$handle": 3}"#));
    assert_eq!(
        value_text(p, &h, &Ty::Capability(r.page_read)),
        "<handle 3: reader.pages.PageRead>"
    );
    assert_eq!(value_text_untyped(p, &h), "<handle 3>");
    let err = value_from_json(
        p,
        &json(r#"{"$type": "reader.pages.PageRead", "$handle": 3}"#),
        &Ty::Capability(r.page_read),
    )
    .unwrap_err();
    assert!(err.message.contains("capability"), "{err}");
}

#[test]
fn generic_positions_encode_by_value() {
    let r = reader();
    let p = &r.p;
    assert_eq!(value_to_json(p, &ints(&[1]), &Ty::Param(0)), json("[1]"));
    let pair = Value::new_struct(r.pair, vec![int(1), int(2)]);
    assert_eq!(
        value_to_json(p, &pair, &Ty::Adt(r.pair, vec![Ty::Int])),
        json(r#"{"$type": "reader.pages.Pair", "first": 1, "second": 2}"#)
    );
}

#[test]
fn object_keys_serialize_sorted() {
    // serde_json's Map is the workspace's sorted map (T4), so a struct's
    // fields serialize after "$type" in name order, not declaration order.
    let r = reader();
    let p = &r.p;
    let dims = Value::new_struct(r.dimensions, vec![int(4), int(3)]);
    assert_eq!(
        enc(p, &dims, &Ty::Adt(r.dimensions, vec![])),
        r#"{"$type":"reader.pages.Dimensions","height":3,"width":4}"#
    );
    assert_eq!(
        enc(p, &missing(&r, "p"), &Ty::Adt(r.store_error, vec![])),
        r#"{"$payload":[{"$payload":["p"],"$type":"reader.pages.PageId"}],"$type":"reader.pages.StoreError","$variant":"Missing"}"#
    );
}

// ===========================================================================
// Value text (semantics S40)
// ===========================================================================

#[test]
fn value_text_forms() {
    let r = reader();
    let p = &r.p;
    let t = |v: &Value| value_text_untyped(p, v);
    assert_eq!(t(&int(42)), "42");
    assert_eq!(t(&int(-5)), "-5");
    assert_eq!(
        t(&big("123456789012345678901234567890")),
        "123456789012345678901234567890"
    );
    assert_eq!(t(&Value::Bool(true)), "true");
    assert_eq!(t(&Value::Bool(false)), "false");
    assert_eq!(t(&Value::unit(p)), "Unit");
    assert_eq!(t(&ints(&[1, 2])), "[1, 2]");
    assert_eq!(t(&ints(&[])), "[]");
    assert_eq!(t(&list(vec![ints(&[1, 2]), ints(&[3])])), "[[1, 2], [3]]");
    assert_eq!(
        t(&page(&r, "p", "t")),
        r#"Page { id: PageId("p"), text: "t" }"#
    );
    assert_eq!(t(&missing(&r, "p")), r#"Missing(PageId("p"))"#);
    assert_eq!(t(&Value::new_enum(r.store_error, 1, vec![])), "Unavailable");
    assert_eq!(t(&Value::wrap(r.page_id, text("p"))), r#"PageId("p")"#);
    assert_eq!(t(&bytes(b"hi")), r#"Bytes("6869")"#);
    assert_eq!(t(&bytes(&[])), r#"Bytes("")"#);
    assert_eq!(t(&Value::new_struct(r.empty, vec![])), "Empty {}");
    assert_eq!(
        t(&Value::ok(p, list(vec![ints(&[1, 2]), ints(&[3])]))),
        "Ok([[1, 2], [3]])"
    );
    assert_eq!(t(&Value::err(p, text("e"))), r#"Err("e")"#);
    assert_eq!(t(&Value::some(p, Value::some(p, int(1)))), "Some(Some(1))");
    assert_eq!(t(&Value::none(p)), "None");
    assert_eq!(
        t(&Value::new_struct(r.pair, vec![int(-1), int(2)])),
        "Pair { first: -1, second: 2 }"
    );
    assert_eq!(
        t(&constraint_error(
            p,
            "Int",
            "\"x\" is not a decimal integer"
        )),
        r#"ConstraintError { type_name: "Int", message: "\"x\" is not a decimal integer" }"#
    );
    assert_eq!(value_text(p, &int(3), &Ty::Int), "3");
}

#[test]
fn text_escapes_of_s40() {
    let r = reader();
    let p = &r.p;
    let t = |s: &str| value_text_untyped(p, &text(s));
    assert_eq!(t(""), r#""""#);
    assert_eq!(t("a\"b"), r#""a\"b""#);
    assert_eq!(t("a\\b"), r#""a\\b""#);
    assert_eq!(t("\n\r\t"), r#""\n\r\t""#);
    // Other C0 controls and DEL as \u{H} with minimal lowercase hex digits.
    assert_eq!(
        t("\u{0}\u{1}\u{1b}\u{1f}\u{7f}"),
        r#""\u{0}\u{1}\u{1b}\u{1f}\u{7f}""#
    );
    // Every other scalar literally (C1 controls, NBSP, combining marks, non-BMP).
    assert_eq!(
        t("\u{85}\u{a0}e\u{301}\u{e9}\u{1F600}\u{200b}"),
        "\"\u{85}\u{a0}e\u{301}\u{e9}\u{1F600}\u{200b}\""
    );
    assert_eq!(
        value_text_untyped(p, &texts(&["a", "\""])),
        r#"["a", "\""]"#
    );
}

// ===========================================================================
// Decoding
// ===========================================================================

fn round_trip(r: &Reader, v: Value, ty: Ty) {
    let p = &r.p;
    let j = value_to_json(p, &v, &ty);
    let text = serde_json::to_string(&j).unwrap();
    let reparsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    let back = value_from_json(p, &reparsed, &ty).unwrap_or_else(|e| panic!("{text}: {e}"));
    assert_eq!(back, v, "round trip of {text}");
}

#[test]
fn round_trips_of_every_form() {
    let r = reader();
    let p = &r.p;
    let std = p.std_defs;
    round_trip(&r, int(0), Ty::Int);
    round_trip(
        &r,
        big("-98765432109876543210987654321098765432109876543210"),
        Ty::Int,
    );
    round_trip(&r, Value::Bool(false), Ty::Bool);
    round_trip(&r, text("h\u{e9}llo \u{1F600} \"q\" \\ \n"), Ty::Text);
    round_trip(&r, Value::unit(p), Ty::unit(&std));
    round_trip(&r, bytes(&[0, 1, 127, 128, 255]), Ty::Bytes);
    round_trip(&r, bytes(&[]), Ty::Bytes);
    round_trip(
        &r,
        list(vec![ints(&[1, 2]), ints(&[])]),
        Ty::list(Ty::list(Ty::Int)),
    );
    round_trip(&r, page(&r, "p", "t"), Ty::Adt(r.page, vec![]));
    round_trip(&r, missing(&r, "x"), Ty::Adt(r.store_error, vec![]));
    round_trip(
        &r,
        Value::new_enum(r.store_error, 1, vec![]),
        Ty::Adt(r.store_error, vec![]),
    );
    round_trip(
        &r,
        Value::wrap(r.page_id, text("p1")),
        Ty::Wrapper(r.page_id),
    );
    round_trip(
        &r,
        Value::new_struct(r.empty, vec![]),
        Ty::Adt(r.empty, vec![]),
    );
    round_trip(
        &r,
        Value::some(p, Value::some(p, int(1))),
        option(&r, option(&r, Ty::Int)),
    );
    round_trip(&r, Value::none(p), option(&r, Ty::Int));
    round_trip(
        &r,
        Value::err(p, constraint_error(p, "Int", "m")),
        result(&r, Ty::Int, Ty::Adt(std.constraint_error, vec![])),
    );
    // Generic struct instantiated with Option<Int>.
    round_trip(
        &r,
        Value::new_struct(r.pair, vec![Value::some(p, int(1)), Value::none(p)]),
        Ty::Adt(r.pair, vec![option(&r, Ty::Int)]),
    );
    // Refined positions decode at their base.
    let positive = refined(&r.p, r.positive);
    round_trip(&r, int(5), positive.clone());
    round_trip(
        &r,
        Value::new_struct(r.dimensions, vec![int(4), int(3)]),
        Ty::Adt(r.dimensions, vec![]),
    );
    round_trip(&r, Value::wrap(r.size, int(2)), Ty::Wrapper(r.size));
    round_trip(
        &r,
        Value::ok(p, list(vec![ints(&[1, 2]), ints(&[3])])),
        result(
            &r,
            Ty::list(Ty::list(Ty::Int)),
            Ty::Adt(std.constraint_error, vec![]),
        ),
    );
}

#[test]
fn decoding_reads_the_corpus_argument_forms() {
    let r = reader();
    let p = &r.p;
    // corpus.reader's label(Err(Missing(PageId("p")))): three nested tagged objects.
    let ty = result(&r, Ty::Adt(r.page, vec![]), Ty::Adt(r.store_error, vec![]));
    let v = value_from_json(
        p,
        &json(r#"{"$type": "Result", "$variant": "Err", "$payload": [{"$type": "reader.pages.StoreError", "$variant": "Missing", "$payload": [{"$type": "reader.pages.PageId", "$payload": ["p"]}]}]}"#),
        &ty,
    )
    .unwrap();
    assert_eq!(v, Value::err(p, missing(&r, "p")));
    // Key order never matters.
    let v = value_from_json(
        p,
        &json(r#"{"text": "t", "id": {"$payload": ["p"], "$type": "reader.pages.PageId"}, "$type": "reader.pages.Page"}"#),
        &Ty::Adt(r.page, vec![]),
    )
    .unwrap();
    assert_eq!(v, page(&r, "p", "t"));
    // -0 is the Int 0; huge numbers keep every digit.
    assert_eq!(value_from_json(p, &json("-0"), &Ty::Int).unwrap(), int(0));
    assert_eq!(
        value_from_json(p, &json("10000000000000000000000"), &Ty::Int).unwrap(),
        big("10000000000000000000000")
    );
}

/// The error of decoding `j` at `ty`: (pointer, message).
fn decode_error(p: &Program, j: &str, ty: &Ty) -> (String, String) {
    let e = value_from_json(p, &json(j), ty).expect_err(j);
    (e.pointer, e.message)
}

#[test]
fn int_decoding_rejects_strings_fractions_and_exponents() {
    let r = reader();
    let p = &r.p;
    for bad in [
        r#""5""#, "5.0", "1.5", "1e3", "1E3", "-2e-1", "true", "null", "[1]",
    ] {
        let (pointer, message) = decode_error(p, bad, &Ty::Int);
        assert_eq!(pointer, "", "{bad}");
        assert!(message.contains("Int"), "{bad}: {message}");
    }
    let (_, message) = decode_error(p, "1.5", &Ty::Int);
    assert!(message.contains("fraction or exponent"), "{message}");
}

#[test]
fn texts_cannot_hold_unpaired_surrogates() {
    // serde_json rejects the document before decoding sees it.
    assert!(serde_json::from_str::<serde_json::Value>(r#""\ud800""#).is_err());
    assert!(serde_json::from_str::<serde_json::Value>(r#""\udc00x""#).is_err());
    let r = reader();
    let (pointer, message) = decode_error(&r.p, "5", &Ty::Text);
    assert_eq!(pointer, "");
    assert!(message.contains("Text"), "{message}");
}

#[test]
fn bytes_decoding_is_strict() {
    let r = reader();
    let p = &r.p;
    assert_eq!(
        decode_error(p, r#"{"$type": "Bytes", "hex": "6A"}"#, &Ty::Bytes).0,
        "/hex"
    );
    assert_eq!(
        decode_error(p, r#"{"$type": "Bytes", "hex": "686"}"#, &Ty::Bytes).0,
        "/hex"
    );
    assert_eq!(
        decode_error(p, r#"{"$type": "Bytes", "hex": 5}"#, &Ty::Bytes).0,
        "/hex"
    );
    assert_eq!(decode_error(p, r#"{"$type": "Bytes"}"#, &Ty::Bytes).0, "");
    assert_eq!(
        decode_error(p, r#"{"$type": "Bytes", "hex": "", "x": 1}"#, &Ty::Bytes).0,
        "/x"
    );
    assert_eq!(
        decode_error(p, r#"{"$type": "bytes", "hex": ""}"#, &Ty::Bytes).0,
        "/$type"
    );
    assert_eq!(decode_error(p, r#""6869""#, &Ty::Bytes).0, "");
}

#[test]
fn unit_decoding_is_exact() {
    let r = reader();
    let p = &r.p;
    let unit = Ty::unit(&p.std_defs);
    assert_eq!(
        value_from_json(p, &json(r#"{"$type": "Unit"}"#), &unit).unwrap(),
        Value::unit(p)
    );
    assert_eq!(
        decode_error(p, r#"{"$type": "Unit", "$variant": "Unit"}"#, &unit).0,
        "/$variant"
    );
    assert_eq!(decode_error(p, r#"{}"#, &unit).0, "");
    assert_eq!(decode_error(p, "null", &unit).0, "");
}

#[test]
fn struct_decoding_requires_every_field_exactly_once() {
    let r = reader();
    let p = &r.p;
    let ty = Ty::Adt(r.page, vec![]);
    let (pointer, message) = decode_error(
        p,
        r#"{"$type": "reader.pages.Page", "id": {"$type": "reader.pages.PageId", "$payload": ["p"]}}"#,
        &ty,
    );
    assert_eq!(pointer, "");
    assert!(message.contains("missing field \"text\""), "{message}");
    let (pointer, message) = decode_error(
        p,
        r#"{"$type": "reader.pages.Page", "id": {"$type": "reader.pages.PageId", "$payload": ["p"]}, "text": "t", "extra": 1}"#,
        &ty,
    );
    assert_eq!(pointer, "/extra");
    assert!(message.contains("unexpected key"), "{message}");
    let (pointer, message) = decode_error(
        p,
        r#"{"$type": "Page", "id": {"$type": "reader.pages.PageId", "$payload": ["p"]}, "text": "t"}"#,
        &ty,
    );
    assert_eq!(pointer, "/$type");
    assert!(message.contains("reader.pages.Page"), "{message}");
    assert_eq!(decode_error(p, r#"{"id": 1, "text": "t"}"#, &ty).0, "");
    assert_eq!(decode_error(p, r#"[]"#, &ty).0, "");
    // A nested failure is located by its JSON pointer.
    let (pointer, _) = decode_error(
        p,
        r#"[{"$type": "reader.pages.Page", "id": {"$type": "reader.pages.PageId", "$payload": ["p"]}, "text": "t"},
            {"$type": "reader.pages.Page", "id": {"$type": "reader.pages.PageId", "$payload": [7]}, "text": "t"}]"#,
        &Ty::list(ty),
    );
    assert_eq!(pointer, "/1/id/$payload/0");
}

#[test]
fn pointer_segments_are_escaped() {
    let r = reader();
    let (pointer, _) = decode_error(
        &r.p,
        r#"{"$type": "reader.pages.Empty", "a/b~c": 1}"#,
        &Ty::Adt(r.empty, vec![]),
    );
    assert_eq!(pointer, "/a~1b~0c");
}

#[test]
fn enum_decoding_checks_variant_and_payload() {
    let r = reader();
    let p = &r.p;
    let ty = Ty::Adt(r.store_error, vec![]);
    let (pointer, message) = decode_error(
        p,
        r#"{"$type": "reader.pages.StoreError", "$variant": "Gone"}"#,
        &ty,
    );
    assert_eq!(pointer, "/$variant");
    assert!(message.contains("Missing, Unavailable"), "{message}");
    assert_eq!(
        decode_error(p, r#"{"$type": "reader.pages.StoreError"}"#, &ty).0,
        ""
    );
    assert_eq!(
        decode_error(
            p,
            r#"{"$type": "reader.pages.StoreError", "$variant": 1}"#,
            &ty
        )
        .0,
        "/$variant"
    );
    // Missing needs its payload, with exactly one value.
    assert_eq!(
        decode_error(
            p,
            r#"{"$type": "reader.pages.StoreError", "$variant": "Missing"}"#,
            &ty
        )
        .0,
        ""
    );
    assert_eq!(
        decode_error(
            p,
            r#"{"$type": "reader.pages.StoreError", "$variant": "Missing", "$payload": []}"#,
            &ty
        )
        .0,
        "/$payload"
    );
    assert_eq!(
        decode_error(
            p,
            r#"{"$type": "reader.pages.StoreError", "$variant": "Missing", "$payload": {}}"#,
            &ty
        )
        .0,
        "/$payload"
    );
    // A payload-less variant must omit "$payload".
    let (pointer, message) = decode_error(
        p,
        r#"{"$type": "reader.pages.StoreError", "$variant": "Unavailable", "$payload": []}"#,
        &ty,
    );
    assert_eq!(pointer, "/$payload");
    assert!(message.contains("omit"), "{message}");
    // Option's "$type" is the bare prelude name.
    let opt = option(&r, Ty::Int);
    assert_eq!(
        decode_error(
            p,
            r#"{"$type": "std.prelude.Option", "$variant": "None"}"#,
            &opt
        )
        .0,
        "/$type"
    );
    assert_eq!(
        decode_error(
            p,
            r#"{"$type": "Option", "$variant": "Some", "$payload": ["1"]}"#,
            &opt
        )
        .0,
        "/$payload/0"
    );
}

#[test]
fn wrapper_decoding_needs_one_payload() {
    let r = reader();
    let p = &r.p;
    let ty = Ty::Wrapper(r.page_id);
    assert_eq!(
        decode_error(
            p,
            r#"{"$type": "reader.pages.PageId", "$payload": ["a", "b"]}"#,
            &ty
        )
        .0,
        "/$payload"
    );
    assert_eq!(
        decode_error(p, r#"{"$type": "reader.pages.PageId"}"#, &ty).0,
        ""
    );
    assert_eq!(decode_error(p, r#""p1""#, &ty).0, "");
    assert_eq!(
        decode_error(
            p,
            r#"{"$type": "reader.pages.PageId", "$payload": ["a"], "$variant": "x"}"#,
            &ty
        )
        .0,
        "/$variant"
    );
}

#[test]
fn undecodable_types() {
    let r = reader();
    let p = &r.p;
    assert!(decode_error(p, "1", &Ty::Param(0)).1.contains("generic"));
    assert!(decode_error(p, "1", &Ty::Gen(Box::new(Ty::Int)))
        .1
        .contains("generator"));
    assert!(decode_error(p, "1", &Ty::Error).1.contains("errors"));
}

// ===========================================================================
// Refined positions
// ===========================================================================

#[test]
fn refined_positions_in_document_order() {
    let r = reader();
    let p = &r.p;
    let positive = refined(&r.p, r.positive);
    let at = |v: &Value, ty: &Ty| -> Vec<(String, cpl_hir::DefId, Value)> {
        refined_positions(p, v, ty)
            .into_iter()
            .map(
                |RefinedPosition {
                     pointer,
                     refined,
                     value,
                 }| (pointer, refined, value),
            )
            .collect()
    };
    assert_eq!(
        at(&int(0), &positive),
        vec![(String::new(), r.positive, int(0))]
    );
    assert_eq!(
        at(&ints(&[1, 2]), &Ty::list(positive.clone())),
        vec![
            ("/0".to_string(), r.positive, int(1)),
            ("/1".to_string(), r.positive, int(2))
        ]
    );
    assert_eq!(
        at(
            &Value::new_struct(r.dimensions, vec![int(4), int(3)]),
            &Ty::Adt(r.dimensions, vec![])
        ),
        vec![
            ("/width".to_string(), r.positive, int(4)),
            ("/height".to_string(), r.positive, int(3))
        ]
    );
    assert_eq!(
        at(&Value::some(p, int(3)), &option(&r, positive.clone())),
        vec![("/$payload/0".to_string(), r.positive, int(3))]
    );
    assert_eq!(at(&Value::none(p), &option(&r, positive.clone())), vec![]);
    assert_eq!(
        at(&Value::wrap(r.size, int(2)), &Ty::Wrapper(r.size)),
        vec![("/$payload/0".to_string(), r.positive, int(2))]
    );
    // A generic struct's field types are instantiated.
    assert_eq!(
        at(
            &Value::new_struct(r.pair, vec![int(1), int(2)]),
            &Ty::Adt(r.pair, vec![positive.clone()])
        ),
        vec![
            ("/first".to_string(), r.positive, int(1)),
            ("/second".to_string(), r.positive, int(2))
        ]
    );
    // A chain is one position: its whole chain is checked there.
    let small = refined(&r.p, r.small);
    assert_eq!(
        at(&int(20), &small),
        vec![(String::new(), r.small, int(20))]
    );
    assert_eq!(at(&ints(&[1]), &Ty::list(Ty::Int)), vec![]);

    // A refinement whose base has refined positions comes after them, so a
    // boundary establishes the base value before evaluating the predicate
    // on it (semantics S39, 6.10: root first).
    let mut b = ProgramBuilder::new();
    let m = b.module("m");
    let pos = b.add_int_refinement(m, "Positive", BinaryOp::Gt, 0);
    let pos_ty = b.refined(pos);
    let rect = b.add_struct(m, "Rect", &[], &[("w", pos_ty.clone()), ("h", pos_ty)]);
    let h = b.hir(m);
    let wide = b.add_refinement(m, "Wide", Ty::Adt(rect, vec![]), h.bool(true), "true");
    let q = b.build();
    let rects = list(vec![Value::new_struct(rect, vec![int(5), int(0)])]);
    let order: Vec<(String, cpl_hir::DefId)> =
        refined_positions(&q, &rects, &Ty::list(refined(&q, wide)))
            .into_iter()
            .map(|position| (position.pointer, position.refined))
            .collect();
    assert_eq!(
        order,
        vec![
            ("/0/w".to_string(), pos),
            ("/0/h".to_string(), pos),
            ("/0".to_string(), wide)
        ]
    );
}
