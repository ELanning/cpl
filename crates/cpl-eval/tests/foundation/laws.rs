//! The laws tables of stdlib 2.3 (I), 3.3 (T), 3.4 (B), 4.3/4.5 (L), 5.2
//! (O), and 6.2 (X), checked over fixed samples through the registry:
//! self-test evidence about the builtin implementations (stdlib 10.7),
//! never a proof.

use crate::support::{big, bytes, int, ints, list, ok, text};
use cpl_eval::arith::div_rem;
use cpl_eval::testing::std_program;
use cpl_eval::Value;
use cpl_hir::Program;
use num_bigint::BigInt;

fn int_samples() -> Vec<Value> {
    let mut v: Vec<Value> = [-1000, -7, -2, -1, 0, 1, 2, 3, 7, 255, 1000]
        .iter()
        .map(|&n| int(n))
        .collect();
    v.push(big("-340282366920938463463374607431768211457"));
    v.push(big("340282366920938463463374607431768211455"));
    v
}

fn list_samples() -> Vec<Value> {
    vec![
        ints(&[]),
        ints(&[0]),
        ints(&[1, 2]),
        ints(&[3, -1, 3]),
        ints(&[5, 4, 3, 2, 1, 0]),
    ]
}

fn count_samples() -> Vec<Value> {
    vec![
        int(0),
        int(1),
        int(2),
        int(3),
        int(10),
        big("100000000000000000000"),
    ]
}

fn text_samples() -> Vec<Value> {
    [
        "",
        "a",
        "ab",
        " x ",
        "h\u{e9}llo",
        "e\u{301}",
        "\u{1F600}!",
        "a,b,,c",
        "\u{a0}5\u{a0}",
        "ABC def",
    ]
    .iter()
    .map(|t| text(t))
    .collect()
}

fn concat(a: &Value, b: &Value) -> Value {
    a.concat(b).expect("concatenable")
}

fn b(v: bool) -> Value {
    Value::Bool(v)
}

#[test]
fn int_laws_i1_to_i5() {
    let p = std_program();
    for a in int_samples() {
        let ai = a.as_int().unwrap();
        for bv in int_samples() {
            let bi = bv.as_int().unwrap();
            // I1, I2 (through div_rem, the one home of / and %).
            if let Ok((q, r)) = div_rem(ai, bi) {
                assert_eq!(&q * bi + &r, *ai);
                assert!(num_traits::Signed::abs(&r) < num_traits::Signed::abs(bi));
            }
            // I4.
            let min_ab = ok(&p, "int_min", vec![a.clone(), bv.clone()]);
            let min_ba = ok(&p, "int_min", vec![bv.clone(), a.clone()]);
            let max_ab = ok(&p, "int_max", vec![a.clone(), bv.clone()]);
            let max_ba = ok(&p, "int_max", vec![bv.clone(), a.clone()]);
            assert_eq!(min_ab, min_ba);
            assert_eq!(max_ab, max_ba);
            assert_eq!(min_ab.as_int().unwrap() + max_ab.as_int().unwrap(), ai + bi);
        }
        // I3: parse_int(n.to_text()) == Ok(n).
        let t = ok(&p, "int_to_text", vec![a.clone()]);
        assert_eq!(ok(&p, "text_parse_int", vec![t]), Value::ok(&p, a.clone()));
        // I5: n.abs() == n.max(-n).
        let neg = Value::Int(-ai);
        assert_eq!(
            ok(&p, "int_abs", vec![a.clone()]),
            ok(&p, "int_max", vec![a.clone(), neg])
        );
    }
}

#[test]
fn text_laws_t1_to_t4_and_x1_to_x7() {
    let p = std_program();
    let len = |t: &Value| {
        ok(&p, "text_len", vec![t.clone()])
            .as_int()
            .unwrap()
            .clone()
    };
    for a in text_samples() {
        for bt in text_samples() {
            // T1, X2.
            let ab = concat(&a, &bt);
            assert_eq!(len(&ab), len(&a) + len(&bt));
            assert_eq!(
                ok(&p, "text_starts_with", vec![ab.clone(), a.clone()]),
                b(true)
            );
            assert_eq!(
                ok(&p, "text_ends_with", vec![ab.clone(), bt.clone()]),
                b(true)
            );
            for c in text_samples() {
                // T3, X2 contains.
                assert_eq!(concat(&ab, &c), concat(&a, &concat(&bt, &c)));
                assert_eq!(
                    ok(&p, "text_contains", vec![concat(&ab, &c), bt.clone()]),
                    b(true)
                );
            }
            // X7: chars(a ++ b) == chars(a) ++ chars(b).
            assert_eq!(
                ok(&p, "text_chars", vec![ab]),
                concat(
                    &ok(&p, "text_chars", vec![a.clone()]),
                    &ok(&p, "text_chars", vec![bt.clone()])
                )
            );
        }
        // T2, T4.
        assert_eq!(concat(&text(""), &a), a);
        assert_eq!(concat(&a, &text("")), a);
        assert_eq!(ok(&p, "text_is_empty", vec![a.clone()]), b(a == text("")));
        // X1.
        let trimmed = ok(&p, "text_trim", vec![a.clone()]);
        assert_eq!(ok(&p, "text_trim", vec![trimmed.clone()]), trimmed);
        let inner = ok(&p, "text_trim_end", vec![a.clone()]);
        assert_eq!(ok(&p, "text_trim_start", vec![inner]), trimmed);
        // X3, X4: join(split(t, sep), sep) == t; split(t, sep).len >= 1.
        for sep in [",", "a", "\u{e9}", "ll", " "] {
            let parts = ok(&p, "text_split", vec![a.clone(), text(sep)]);
            assert!(!parts.as_list().unwrap().is_empty());
            assert_eq!(ok(&p, "text_join", vec![parts.clone(), text(sep)]), a);
            for part in parts.as_list().unwrap() {
                assert_eq!(
                    ok(&p, "text_contains", vec![part.clone(), text(sep)]),
                    b(false)
                );
            }
        }
        // X5: to_lower(to_upper(t)) == to_lower(t); case mapping keeps len.
        let upper = ok(&p, "text_to_upper", vec![a.clone()]);
        assert_eq!(len(&upper), len(&a));
        assert_eq!(ok(&p, "text_to_upper", vec![upper.clone()]), upper);
        assert_eq!(
            ok(&p, "text_to_lower", vec![upper]),
            ok(&p, "text_to_lower", vec![a.clone()])
        );
        // chars: one single-scalar text per scalar; join(chars(t), "") == t.
        let cs = ok(&p, "text_chars", vec![a.clone()]);
        assert_eq!(BigInt::from(cs.as_list().unwrap().len()), len(&a));
        assert_eq!(ok(&p, "text_join", vec![cs, text("")]), a);
    }
    // X6: parse_int(from_int(n)) == Ok(n).
    for n in int_samples() {
        let t = ok(&p, "text_from_int", vec![n.clone()]);
        assert_eq!(t, ok(&p, "int_to_text", vec![n.clone()]));
        assert_eq!(ok(&p, "text_parse_int", vec![t]), Value::ok(&p, n));
    }
}

#[test]
fn bytes_laws_b1_to_b3() {
    let p = std_program();
    for vs in [
        ints(&[]),
        ints(&[0]),
        ints(&[1, 2, 255]),
        ints(&[104, 105, 0, 7]),
    ] {
        let bs = ok(&p, "bytes_from_list", vec![vs.clone()]);
        assert_eq!(ok(&p, "bytes_to_list", vec![bs.clone()]), vs);
        assert_eq!(
            ok(
                &p,
                "bytes_from_list",
                vec![ok(&p, "bytes_to_list", vec![bs])]
            ),
            ok(&p, "bytes_from_list", vec![vs])
        );
    }
    for raw in [&[][..], &[0, 1, 2][..], &[0xff, 0x00][..]] {
        let bs = bytes(raw);
        assert_eq!(
            ok(
                &p,
                "bytes_from_list",
                vec![ok(&p, "bytes_to_list", vec![bs.clone()])]
            ),
            bs
        );
    }
    for t in text_samples() {
        let bs = ok(&p, "bytes_from_text", vec![t.clone()]);
        assert_eq!(
            ok(&p, "bytes_to_text", vec![bs.clone()]),
            Value::ok(&p, t.clone())
        );
        let n = ok(&p, "bytes_len", vec![bs]).as_int().unwrap().clone();
        let chars = ok(&p, "text_len", vec![t]).as_int().unwrap().clone();
        assert!(n >= chars && n <= chars * 4);
    }
}

fn take(p: &Program, xs: &Value, n: &Value) -> Value {
    ok(p, "list_take", vec![xs.clone(), n.clone()])
}

fn drop(p: &Program, xs: &Value, n: &Value) -> Value {
    ok(p, "list_drop", vec![xs.clone(), n.clone()])
}

fn len(p: &Program, xs: &Value) -> BigInt {
    ok(p, "list_len", vec![xs.clone()])
        .as_int()
        .unwrap()
        .clone()
}

#[test]
fn list_laws_l1_to_l19() {
    let p = std_program();
    for xs in list_samples() {
        for n in count_samples() {
            // L1, L11, L12.
            assert_eq!(concat(&take(&p, &xs, &n), &drop(&p, &xs, &n)), xs);
            let ni = n.as_int().unwrap();
            assert!(&len(&p, &take(&p, &xs, &n)) <= ni);
            assert!(len(&p, &take(&p, &xs, &n)) <= len(&p, &xs));
            assert!(len(&p, &drop(&p, &xs, &n)) <= len(&p, &xs));
            if ni <= &len(&p, &xs) {
                assert_eq!(&len(&p, &take(&p, &xs, &n)), ni);
            }
            // L12's second half as stdlib 4.3 states it is false at n == 0
            // ([1].drop(0).len == [1].len, by drop's own semantics); it holds
            // for n > 0, the case paginate's `decreases rest.len` relies on.
            // Reported as a spec erratum.
            if &len(&p, &xs) > ni && ni > &BigInt::from(0) {
                assert!(len(&p, &drop(&p, &xs, &n)) < len(&p, &xs));
            }
            // L15.
            for m in count_samples() {
                let min = ok(&p, "int_min", vec![n.clone(), m.clone()]);
                assert_eq!(take(&p, &take(&p, &xs, &n), &m), take(&p, &xs, &min));
                let sum = Value::Int(ni + m.as_int().unwrap());
                assert_eq!(drop(&p, &drop(&p, &xs, &n), &m), drop(&p, &xs, &sum));
            }
        }
        for ys in list_samples() {
            let xy = concat(&xs, &ys);
            // L2, L4.
            let xl = Value::Int(len(&p, &xs));
            assert_eq!(take(&p, &xy, &xl), xs);
            assert_eq!(drop(&p, &xy, &xl), ys);
            assert_eq!(len(&p, &xy), len(&p, &xs) + len(&p, &ys));
            // L3 associativity, L19 contains.
            for zs in list_samples() {
                assert_eq!(concat(&xy, &zs), concat(&xs, &concat(&ys, &zs)));
            }
            for v in [int(0), int(3), int(9)] {
                let either = ok(&p, "list_contains", vec![xs.clone(), v.clone()])
                    .as_bool()
                    .unwrap()
                    || ok(&p, "list_contains", vec![ys.clone(), v.clone()])
                        .as_bool()
                        .unwrap();
                assert_eq!(ok(&p, "list_contains", vec![xy.clone(), v]), b(either));
            }
            // L7: flatten(xss ++ yss) == flatten(xss) ++ flatten(yss).
            let xss = list(vec![xs.clone(), ys.clone()]);
            let yss = list(vec![ys.clone()]);
            assert_eq!(
                ok(&p, "flatten", vec![concat(&xss, &yss)]),
                concat(
                    &ok(&p, "flatten", vec![xss.clone()]),
                    &ok(&p, "flatten", vec![yss])
                )
            );
            // L7: flatten(xss.push(xs)) == flatten(xss) ++ xs.
            let pushed = ok(&p, "list_push", vec![xss.clone(), xs.clone()]);
            assert_eq!(
                ok(&p, "flatten", vec![pushed]),
                concat(&ok(&p, "flatten", vec![xss]), &xs)
            );
        }
        // L3 identities, L5, L6, L13.
        assert_eq!(concat(&ints(&[]), &xs), xs);
        assert_eq!(concat(&xs, &ints(&[])), xs);
        for y in [int(0), int(-4)] {
            let pushed = ok(&p, "list_push", vec![xs.clone(), y.clone()]);
            assert_eq!(pushed, concat(&xs, &list(vec![y.clone()])));
            assert_eq!(ok(&p, "list_drop_last", vec![pushed.clone()]), xs);
            assert_eq!(len(&p, &pushed), len(&p, &xs) + 1);
            // L18: xs.push(y).reverse() == [y] ++ xs.reverse().
            assert_eq!(
                ok(&p, "list_reverse", vec![pushed]),
                concat(&list(vec![y]), &ok(&p, "list_reverse", vec![xs.clone()]))
            );
        }
        assert_eq!(b(len(&p, &xs) == BigInt::from(0)), b(xs == ints(&[])));
        // L7: flatten([xs]) == xs.
        assert_eq!(ok(&p, "flatten", vec![list(vec![xs.clone()])]), xs);
        // L17: reverse twice.
        let rev = ok(&p, "list_reverse", vec![xs.clone()]);
        assert_eq!(ok(&p, "list_reverse", vec![rev]), xs);
        // L16: get(i) == Some(v) iff 0 <= i < len and drop(i).take(1) == [v].
        for i in -2..7i64 {
            let got = ok(&p, "list_get", vec![xs.clone(), int(i)]);
            let in_range = i >= 0 && BigInt::from(i) < len(&p, &xs);
            match got.as_option(&p).unwrap() {
                Some(v) => {
                    assert!(in_range);
                    assert_eq!(
                        take(&p, &drop(&p, &xs, &int(i)), &int(1)),
                        list(vec![v.clone()])
                    );
                }
                None => assert!(!in_range),
            }
        }
        // first/last are get(0) and get(len - 1).
        let last_index = Value::Int(len(&p, &xs) - 1);
        assert_eq!(
            ok(&p, "list_first", vec![xs.clone()]),
            ok(&p, "list_get", vec![xs.clone(), int(0)])
        );
        assert_eq!(
            ok(&p, "list_last", vec![xs.clone()]),
            ok(&p, "list_get", vec![xs.clone(), last_index])
        );
    }
    // L7: flatten([]) == []; L18: [].reverse() == [].
    assert_eq!(ok(&p, "flatten", vec![list(vec![])]), ints(&[]));
    assert_eq!(ok(&p, "list_reverse", vec![ints(&[])]), ints(&[]));
}

#[test]
fn option_and_result_laws_o1_to_o4() {
    let p = std_program();
    for v in [int(1), text("v"), ints(&[])] {
        let d = int(0);
        let e = text("e");
        // O1.
        assert_eq!(
            ok(
                &p,
                "option_unwrap_or",
                vec![Value::some(&p, v.clone()), d.clone()]
            ),
            v
        );
        assert_eq!(
            ok(&p, "option_unwrap_or", vec![Value::none(&p), d.clone()]),
            d
        );
        // O2.
        assert_eq!(
            ok(
                &p,
                "option_ok_or",
                vec![Value::some(&p, v.clone()), e.clone()]
            ),
            Value::ok(&p, v.clone())
        );
        assert_eq!(
            ok(&p, "option_ok_or", vec![Value::none(&p), e.clone()]),
            Value::err(&p, e.clone())
        );
        // O3.
        assert_eq!(
            ok(&p, "result_ok", vec![Value::ok(&p, v.clone())]),
            Value::some(&p, v.clone())
        );
        assert_eq!(
            ok(&p, "result_ok", vec![Value::err(&p, e.clone())]),
            Value::none(&p)
        );
        assert_eq!(
            ok(&p, "result_err", vec![Value::ok(&p, v.clone())]),
            Value::none(&p)
        );
        assert_eq!(
            ok(&p, "result_err", vec![Value::err(&p, e.clone())]),
            Value::some(&p, e.clone())
        );
        // O4: r.is_ok() == r.ok().is_some().
        for r in [Value::ok(&p, v.clone()), Value::err(&p, e)] {
            let some = ok(
                &p,
                "option_is_some",
                vec![ok(&p, "result_ok", vec![r.clone()])],
            );
            assert_eq!(ok(&p, "result_is_ok", vec![r.clone()]), some);
            let is_err = ok(&p, "result_is_err", vec![r.clone()]).as_bool().unwrap();
            assert_eq!(ok(&p, "result_is_ok", vec![r]), b(!is_err));
        }
    }
}
