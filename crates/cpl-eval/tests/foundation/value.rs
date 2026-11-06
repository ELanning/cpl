//! Values: the prelude constructors and structural equality (semantics 6.1,
//! 6.5; stdlib 1.7), and `++` (stdlib 3.3, 3.4, 4.2).

use crate::support::{big, bytes, int, ints, list, reader, text};
use cpl_eval::testing::std_program;
use cpl_eval::Value;

#[test]
fn prelude_constructors_use_the_stub_variants() {
    let p = std_program();
    let std = p.std_defs;
    let variants = |def: cpl_hir::DefId| -> Vec<String> {
        p.def(def)
            .kind
            .as_enum()
            .unwrap()
            .variants
            .iter()
            .map(|v| v.name.clone())
            .collect()
    };
    assert_eq!(variants(std.option), ["Some", "None"]);
    assert_eq!(variants(std.result), ["Ok", "Err"]);
    assert_eq!(variants(std.unit), ["Unit"]);

    let unit = Value::unit(&p);
    assert!(
        matches!(&unit, Value::Enum { def, variant: 0, payload } if *def == std.unit && payload.is_empty())
    );
    assert!(unit.is_unit(&p));
    assert!(
        matches!(Value::some(&p, int(1)), Value::Enum { def, variant: 0, .. } if def == std.option)
    );
    assert!(matches!(Value::none(&p), Value::Enum { def, variant: 1, .. } if def == std.option));
    assert!(
        matches!(Value::ok(&p, int(1)), Value::Enum { def, variant: 0, .. } if def == std.result)
    );
    assert!(
        matches!(Value::err(&p, int(1)), Value::Enum { def, variant: 1, .. } if def == std.result)
    );
    assert_eq!(
        Value::variant(&p, std.option, "Some", vec![int(4)]),
        Some(Value::some(&p, int(4)))
    );
    assert_eq!(Value::variant(&p, std.option, "Nope", vec![]), None);
}

#[test]
fn as_option_and_as_result() {
    let p = std_program();
    let some = Value::some(&p, int(3));
    let none = Value::none(&p);
    let ok = Value::ok(&p, text("x"));
    let err = Value::err(&p, int(2));
    assert_eq!(some.as_option(&p), Some(Some(&int(3))));
    assert_eq!(none.as_option(&p), Some(None));
    assert_eq!(ok.as_option(&p), None);
    assert_eq!(int(3).as_option(&p), None);
    assert_eq!(ok.as_result(&p), Some(Ok(&text("x"))));
    assert_eq!(err.as_result(&p), Some(Err(&int(2))));
    assert_eq!(some.as_result(&p), None);
    assert_eq!(Value::unit(&p).as_result(&p), None);
    assert!(!some.is_unit(&p));
}

#[test]
fn accessors() {
    assert_eq!(int(5).as_int(), Some(&5.into()));
    assert_eq!(Value::Bool(true).as_bool(), Some(true));
    assert_eq!(text("é").as_text(), Some("é"));
    assert_eq!(bytes(&[1, 2]).as_bytes(), Some(&[1u8, 2][..]));
    assert_eq!(ints(&[1]).as_list().map(<[Value]>::len), Some(1));
    assert_eq!(text("x").as_int(), None);
    assert_eq!(int(1).kind_name(), "Int");
    assert_eq!(Value::empty_list().kind_name(), "List");
}

#[test]
fn equality_of_scalars() {
    assert_eq!(int(7), int(7));
    assert_ne!(int(7), int(-7));
    assert_eq!(
        big("123456789012345678901234567890"),
        big("123456789012345678901234567890")
    );
    assert_ne!(
        big("123456789012345678901234567890"),
        big("123456789012345678901234567891")
    );
    assert_eq!(Value::Bool(false), Value::Bool(false));
    assert_ne!(Value::Bool(false), Value::Bool(true));
    // Text: scalar sequences, no normalization (stdlib 3.3, 12).
    assert_eq!(text("héllo"), text("héllo"));
    assert_ne!(text("\u{e9}"), text("e\u{301}"));
    assert_ne!(text("a"), text("a "));
    assert_eq!(bytes(&[0, 255]), bytes(&[0, 255]));
    assert_ne!(bytes(&[0]), bytes(&[0, 0]));
    // Different forms are never equal.
    assert_ne!(int(1), Value::Bool(true));
    assert_ne!(text("1"), int(1));
    assert_ne!(text(""), bytes(&[]));
    assert_ne!(Value::empty_list(), text(""));
}

#[test]
fn equality_of_lists() {
    // stdlib 12: [[1], [2]] == [[1], [2]] and [1, 2] != [2, 1].
    let a = list(vec![ints(&[1]), ints(&[2])]);
    let b = list(vec![ints(&[1]), ints(&[2])]);
    assert!(a.structurally_equal(&b));
    assert!(!ints(&[1, 2]).structurally_equal(&ints(&[2, 1])));
    assert!(!ints(&[1, 2]).structurally_equal(&ints(&[1, 2, 3])));
    assert!(Value::empty_list().structurally_equal(&ints(&[])));
    // A shared buffer is equal to itself without being walked.
    let shared = ints(&[1, 2, 3]);
    assert!(shared.structurally_equal(&shared.clone()));
    // The first difference decides, wherever it is.
    assert!(
        !list(vec![ints(&[1]), ints(&[2]), ints(&[3])]).structurally_equal(&list(vec![
            ints(&[1]),
            ints(&[2]),
            ints(&[4])
        ]))
    );
}

#[test]
fn equality_of_structs_enums_wrappers() {
    let r = reader();
    let p = &r.p;
    let page = |id: &str, t: &str| {
        Value::new_struct(r.page, vec![Value::wrap(r.page_id, text(id)), text(t)])
    };
    assert_eq!(page("p", "t"), page("p", "t"));
    assert_ne!(page("p", "t"), page("p", "u"));
    assert_ne!(page("p", "t"), page("q", "t"));
    // Wrappers compare by payload (stdlib 12: PageId("a") == PageId("a")).
    assert_eq!(
        Value::wrap(r.page_id, text("a")),
        Value::wrap(r.page_id, text("a"))
    );
    assert_ne!(
        Value::wrap(r.page_id, text("a")),
        Value::wrap(r.page_id, text("b"))
    );
    // A wrapper is distinct from its payload and from another wrapper.
    assert_ne!(Value::wrap(r.page_id, text("a")), text("a"));
    assert_ne!(Value::wrap(r.size, int(1)), Value::wrap(r.page_id, int(1)));
    // Enums: variant first, then payload.
    let missing =
        |id: &str| Value::new_enum(r.store_error, 0, vec![Value::wrap(r.page_id, text(id))]);
    let unavailable = Value::new_enum(r.store_error, 1, vec![]);
    assert_eq!(missing("p"), missing("p"));
    assert_ne!(missing("p"), missing("q"));
    assert_ne!(missing("p"), unavailable);
    assert_eq!(Value::unit(p), Value::unit(p));
    assert_eq!(
        Value::some(p, Value::some(p, int(1))),
        Value::some(p, Value::some(p, int(1)))
    );
    assert_ne!(Value::some(p, int(1)), Value::none(p));
    assert_ne!(Value::ok(p, int(1)), Value::err(p, int(1)));
    // Different declarations are never equal.
    assert_ne!(
        Value::new_struct(r.empty, vec![]),
        Value::new_struct(r.page, vec![])
    );
}

/// `enum Nat { Z  S(Nat) }` values `depth` constructors deep.
fn nat(def: cpl_hir::DefId, depth: usize, last: i64) -> Value {
    let mut v = Value::new_enum(def, 0, vec![int(last)]);
    for _ in 0..depth {
        v = Value::new_enum(def, 1, vec![v]);
    }
    v
}

#[test]
fn equality_of_deep_values_does_not_recurse() {
    // 200 000 nested payloads: a recursive comparison would need far more
    // than the 2 MiB of a test thread's stack. Dropping them does not
    // recurse either (see `dropping_a_deep_value_does_not_recurse`).
    let r = reader();
    let a = nat(r.store_error, 200_000, 1);
    let b = nat(r.store_error, 200_000, 1);
    let c = nat(r.store_error, 200_000, 2);
    assert!(a.structurally_equal(&b));
    assert!(!a.structurally_equal(&c));
}

#[test]
fn equality_checks_remaining_siblings_after_nested_and_shared_values() {
    let r = reader();
    let shared = ints(&[1, 2, 3]);
    let tree = |last| {
        list(vec![
            nat(r.store_error, 2_000, 7),
            shared.clone(),
            Value::empty_list(),
            Value::wrap(r.page_id, text("same")),
            ints(&[last]),
        ])
    };
    let a = tree(5);
    assert!(a.structurally_equal(&tree(5)));
    assert!(!a.structurally_equal(&tree(6)));
    let wide = list((0..10_000).map(int).collect());
    assert!(wide.structurally_equal(&list((0..10_000).map(int).collect())));
    assert!(!wide.structurally_equal(&list((-1..9_999).map(int).collect())));
}

/// Run `f` on a thread with a 256 KiB stack, far less than a recursion over
/// the 200 000 levels of the values below would need.
fn on_small_stack(f: impl FnOnce() + Send) {
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(256 << 10)
            .spawn_scoped(s, f)
            .expect("a thread")
            .join()
            .expect("the small-stack thread did not panic");
    });
}

#[test]
fn dropping_a_deep_value_does_not_recurse() {
    // Semantics S38 (the runtime never ends in a crash): a loop can build a
    // value nested deeper than a stack could tear down recursively, and the
    // thread holding it drops it. Each value is built and dropped on a
    // 256 KiB stack: a payload chain, a list nested in lists, a wrapper
    // chain, and a chain whose every level holds the level below twice
    // (its buffers become unique one holder at a time).
    let r = reader();
    let def = r.store_error;
    let page_id = r.page_id;
    on_small_stack(move || {
        drop(nat(def, 200_000, 1));
        let mut xs = Value::empty_list();
        for _ in 0..200_000 {
            xs = Value::list(vec![int(0), xs, text("t")]);
        }
        drop(xs);
        let mut w = text("p");
        for _ in 0..200_000 {
            w = Value::wrap(page_id, w);
        }
        drop(w);
        let mut twice = Value::empty_list();
        for _ in 0..200_000 {
            twice = Value::list(vec![twice.clone(), twice]);
        }
        drop(twice);
    });
}

#[test]
fn dropping_a_value_keeps_the_buffers_it_shares() {
    // Only the buffers a value owns alone are torn down: a deep part still
    // held elsewhere survives the drop of a value that contains it.
    let r = reader();
    let def = r.store_error;
    on_small_stack(move || {
        let inner = nat(def, 200_000, 7);
        let outer = Value::list(vec![inner.clone(), int(1)]);
        drop(outer);
        assert!(inner.structurally_equal(&nat(def, 200_000, 7)));
        assert!(!inner.structurally_equal(&nat(def, 200_000, 8)));
    });
}

#[test]
fn concat_of_lists_texts_bytes() {
    // stdlib 12: [1] ++ [2, 3], [] ++ [], "a" ++ "b".
    assert_eq!(ints(&[1]).concat(&ints(&[2, 3])), Some(ints(&[1, 2, 3])));
    assert_eq!(ints(&[]).concat(&ints(&[])), Some(ints(&[])));
    assert_eq!(text("a").concat(&text("b")), Some(text("ab")));
    assert_eq!(text("").concat(&text("é")), Some(text("é")));
    assert_eq!(text("é").concat(&text("")), Some(text("é")));
    assert_eq!(bytes(&[1]).concat(&bytes(&[2])), Some(bytes(&[1, 2])));
    assert_eq!(bytes(&[]).concat(&bytes(&[])), Some(bytes(&[])));
    // The operands are unchanged (immutability).
    let xs = ints(&[1]);
    let ys = xs.concat(&ints(&[2])).unwrap();
    assert_eq!(xs, ints(&[1]));
    assert_eq!(ys, ints(&[1, 2]));
    // An empty operand shares the other buffer.
    if let (Value::List(a), Some(Value::List(b))) = (&ys, &ys.concat(&ints(&[]))) {
        assert!(a.same(b));
    } else {
        panic!("lists expected");
    }
    // Mixed forms are not concatenable.
    assert_eq!(text("a").concat(&ints(&[1])), None);
    assert_eq!(int(1).concat(&int(2)), None);
    assert_eq!(bytes(&[1]).concat(&text("a")), None);
}
