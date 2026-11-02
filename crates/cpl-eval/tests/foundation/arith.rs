//! `Int` arithmetic: semantics 6.3 and S37, stdlib 2.2 and S8, toolchain T9.

use crate::support::json;
use cpl_eval::arith::{binary, div, div_rem, negate, rem};
use cpl_eval::fault::codes;
use cpl_eval::{FaultKind, Value};
use cpl_syntax::ast::BinaryOp;
use num_bigint::BigInt;
use num_traits::{Signed, Zero};

fn n(digits: &str) -> BigInt {
    digits.parse().expect("decimal digits")
}

fn i(v: i64) -> BigInt {
    BigInt::from(v)
}

#[test]
fn worked_values_of_stdlib_2_2_and_s37() {
    // (a, b, a / b, a % b): truncation toward zero, remainder with the
    // dividend's sign (floored division would give -4 / 1 for (-7, 2)).
    let table = [
        (7, 2, 3, 1),
        (-7, 2, -3, -1),
        (7, -2, -3, 1),
        (-7, -2, 3, -1),
        (6, 3, 2, 0),
        (0, 5, 0, 0),
    ];
    for (a, b, q, r) in table {
        assert_eq!(div_rem(&i(a), &i(b)).unwrap(), (i(q), i(r)), "{a} /% {b}");
        assert_eq!(div(&i(a), &i(b)).unwrap(), i(q));
        assert_eq!(rem(&i(a), &i(b)).unwrap(), i(r));
    }
}

#[test]
fn stdlib_12_division_rows() {
    let q: Vec<BigInt> = [(7, 2), (-7, 2), (7, -2), (-7, -2)]
        .iter()
        .map(|&(a, b)| div(&i(a), &i(b)).unwrap())
        .collect();
    assert_eq!(q, vec![i(3), i(-3), i(-3), i(3)]);
    let r: Vec<BigInt> = [(7, 2), (-7, 2), (7, -2), (-7, -2)]
        .iter()
        .map(|&(a, b)| rem(&i(a), &i(b)).unwrap())
        .collect();
    assert_eq!(r, vec![i(1), i(-1), i(1), i(-1)]);
}

fn samples() -> Vec<BigInt> {
    let mut v: Vec<BigInt> = [
        0i64,
        1,
        2,
        3,
        5,
        7,
        10,
        255,
        1_000_000_007,
        i64::MAX,
        i64::MIN,
    ]
    .iter()
    .flat_map(|&x| [i(x), -i(x)])
    .collect();
    let two64: BigInt = BigInt::from(1u8) << 64usize;
    let two200: BigInt = BigInt::from(1u8) << 200usize;
    for x in [
        two64.clone(),
        &two64 - 1,
        &two64 + 1,
        &two200 + 12345,
        n("123456789012345678901234567890123456789"),
    ] {
        v.push(-&x);
        v.push(x);
    }
    v
}

#[test]
fn identity_sign_and_magnitude_laws_hold_everywhere() {
    // S37: a == (a / b) * b + a % b; a % b is 0 or has a's sign; |a % b| < |b|;
    // a % b == 0 exactly when b divides a (the design's divide_exactly).
    let xs = samples();
    for a in &xs {
        for b in &xs {
            if b.is_zero() {
                continue;
            }
            let (q, r) = div_rem(a, b).unwrap();
            assert_eq!(&(&q * b) + &r, *a, "identity for {a} and {b}");
            assert!(
                r.is_zero() || (r.is_negative() == a.is_negative()),
                "sign of {a} % {b}"
            );
            assert!(r.abs() < b.abs(), "|{a} % {b}| < |{b}|");
            // truncation toward zero: |q| == |a| / |b| (floor of magnitudes)
            assert_eq!(q.abs(), a.abs() / b.abs(), "|{a} / {b}|");
            assert_eq!(r.is_zero(), (a.abs() % b.abs()).is_zero());
        }
    }
}

#[test]
fn zero_divisor_is_the_division_by_zero_fault() {
    for a in [i(1), i(0), i(-7), n("99999999999999999999999999")] {
        let f = div_rem(&a, &i(0)).unwrap_err();
        assert_eq!(f.kind, FaultKind::DivisionByZero);
        assert_eq!(f.code(), codes::DIVISION_BY_ZERO);
        assert_eq!(f.data["input"], json(&format!(r#"{{"a": {a}, "b": 0}}"#)));
        assert!(div(&a, &i(0)).is_err());
        assert!(rem(&a, &i(0)).is_err());
        assert!(
            f.symbol.is_none(),
            "the interpreter blames the enclosing declaration"
        );
    }
}

#[test]
fn huge_values_are_exact() {
    // stdlib 12: 12345678901234567890 * 10 == 123456789012345678900.
    assert_eq!(
        binary(BinaryOp::Mul, &n("12345678901234567890"), &i(10)).unwrap(),
        Value::Int(n("123456789012345678900"))
    );
    // i64::MAX + 1 does not wrap.
    assert_eq!(
        binary(BinaryOp::Add, &i(i64::MAX), &i(1)).unwrap(),
        Value::Int(n("9223372036854775808"))
    );
    assert_eq!(
        binary(BinaryOp::Sub, &i(i64::MIN), &i(1)).unwrap(),
        Value::Int(n("-9223372036854775809"))
    );
    assert_eq!(negate(&i(i64::MIN)), n("9223372036854775808"));
    let big = n("340282366920938463463374607431768211456"); // 2^128
    assert_eq!(
        binary(BinaryOp::Div, &big, &n("18446744073709551616")).unwrap(),
        Value::Int(n("18446744073709551616"))
    );
    assert_eq!(
        binary(BinaryOp::Rem, &(-&big - 1), &n("18446744073709551616")).unwrap(),
        Value::Int(i(-1))
    );
}

#[test]
fn every_int_operator() {
    let (a, b) = (i(-7), i(2));
    let cases = [
        (BinaryOp::Add, Value::int(-5)),
        (BinaryOp::Sub, Value::int(-9)),
        (BinaryOp::Mul, Value::int(-14)),
        (BinaryOp::Div, Value::int(-3)),
        (BinaryOp::Rem, Value::int(-1)),
        (BinaryOp::Eq, Value::Bool(false)),
        (BinaryOp::Ne, Value::Bool(true)),
        (BinaryOp::Lt, Value::Bool(true)),
        (BinaryOp::Le, Value::Bool(true)),
        (BinaryOp::Gt, Value::Bool(false)),
        (BinaryOp::Ge, Value::Bool(false)),
    ];
    for (op, expected) in cases {
        assert_eq!(binary(op, &a, &b).unwrap(), expected, "{}", op.symbol());
    }
    // Comparisons are numeric at every magnitude.
    assert_eq!(
        binary(BinaryOp::Lt, &n("-100000000000000000000"), &i(i64::MIN)).unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        binary(BinaryOp::Le, &i(3), &i(3)).unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        binary(BinaryOp::Ge, &i(3), &i(3)).unwrap(),
        Value::Bool(true)
    );
    assert_eq!(negate(&i(5)), i(-5));
    assert_eq!(negate(&i(0)), i(0));
}

#[test]
fn division_operators_fault_through_div_rem() {
    for op in [BinaryOp::Div, BinaryOp::Rem] {
        let f = binary(op, &i(1), &i(0)).unwrap_err();
        assert_eq!(f.kind, FaultKind::DivisionByZero);
        assert_eq!(f.data["input"], json(r#"{"a": 1, "b": 0}"#));
    }
}

#[test]
fn non_int_operators_are_type_confusions() {
    for op in [BinaryOp::And, BinaryOp::Or, BinaryOp::Concat] {
        let f = binary(op, &i(1), &i(2)).unwrap_err();
        assert_eq!(f.kind, FaultKind::Internal);
        assert_eq!(f.data["kind"], "type_confusion");
    }
}
