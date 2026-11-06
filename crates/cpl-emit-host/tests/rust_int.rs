//! The Rust runtime's `Int` against `num-bigint`: its own arbitrary-precision arithmetic (the
//! machine-integer fast paths with their overflow checks, schoolbook multiplication, Knuth's
//! division, decimal parsing and printing) on operands around every boundary that matters.

#[path = "../src/runtime/cpl.rs"]
mod cpl;

use cpl::Int;
use num_bigint::BigInt;
use num_traits::{One, Signed, Zero};

/// A small deterministic generator (xorshift64*).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// An operand: near zero, near the `i64` and `u32`/`u64` limb boundaries, or of one to six
/// 32-bit limbs with runs of all-ones and all-zeros limbs (the cases Knuth's division corrects).
fn operand(r: &mut Rng) -> BigInt {
    let two = BigInt::from(2);
    let v = match r.below(8) {
        0 => BigInt::from(r.below(10) as i64 - 5),
        1 => BigInt::from(i64::MAX) - BigInt::from(r.below(4)),
        2 => BigInt::from(i64::MIN) + BigInt::from(r.below(4)),
        3 => two.pow(32 * (1 + r.below(4) as u32)) - BigInt::from(r.below(3)),
        4 => two.pow(63 + r.below(3) as u32) + BigInt::from(r.below(3)) - BigInt::one(),
        _ => {
            let limbs = 1 + r.below(6);
            let mut v = BigInt::zero();
            for _ in 0..limbs {
                let limb = match r.below(4) {
                    0 => 0u64,
                    1 => 0xFFFF_FFFF,
                    2 => 0x8000_0000,
                    _ => r.next() & 0xFFFF_FFFF,
                };
                v = (v << 32) + BigInt::from(limb);
            }
            v
        }
    };
    if r.below(2) == 0 {
        -v
    } else {
        v
    }
}

fn int(v: &BigInt) -> Int {
    Int::parse(&v.to_string()).expect("a decimal")
}

/// Truncating division as CPL defines it (semantics 6.3): the quotient rounds toward zero, the
/// remainder has the dividend's sign — exactly `num-bigint`'s `/` and `%`.
#[test]
fn arithmetic_agrees_with_num_bigint() {
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    for case in 0..200_000 {
        let (a, b) = (operand(&mut r), operand(&mut r));
        let (x, y) = (int(&a), int(&b));
        let ctx = || format!("case {case}: a = {a}, b = {b}");
        assert_eq!(x.to_string(), a.to_string(), "parse and print, {}", ctx());
        assert_eq!(
            x.add(&y).to_string(),
            (&a + &b).to_string(),
            "add, {}",
            ctx()
        );
        assert_eq!(
            x.sub(&y).to_string(),
            (&a - &b).to_string(),
            "sub, {}",
            ctx()
        );
        assert_eq!(
            x.mul(&y).to_string(),
            (&a * &b).to_string(),
            "mul, {}",
            ctx()
        );
        assert_eq!(x.neg().to_string(), (-&a).to_string(), "neg, {}", ctx());
        assert_eq!(x.abs().to_string(), a.abs().to_string(), "abs, {}", ctx());
        assert_eq!(x.cmp(&y), a.cmp(&b), "cmp, {}", ctx());
        assert_eq!(x == y, a == b, "eq, {}", ctx());
        assert_eq!(x.is_negative(), a.is_negative(), "sign, {}", ctx());
        match x.div_rem(&y) {
            None => assert!(b.is_zero(), "div by nonzero, {}", ctx()),
            Some((q, m)) => {
                assert_eq!(q.to_string(), (&a / &b).to_string(), "div, {}", ctx());
                assert_eq!(m.to_string(), (&a % &b).to_string(), "rem, {}", ctx());
            }
        }
        // A value equal to an `i64` is stored as one: equality and hashing see one form.
        let sum = x.add(&y).sub(&y);
        assert_eq!(sum, x, "a value has one representation, {}", ctx());
    }
}

#[test]
fn parsing_follows_the_decimal_grammar() {
    for (text, ok) in [
        ("0", true),
        ("-0", true),
        ("+12", true),
        ("00012", true),
        ("123456789012345678901234567890", true),
        ("-9223372036854775808", true),
        ("9223372036854775808", true),
        ("", false),
        ("-", false),
        ("+", false),
        ("1 ", false),
        (" 1", false),
        ("1_000", false),
        ("0x10", false),
        ("١٢", false),
        ("1e3", false),
        ("--1", false),
    ] {
        assert_eq!(Int::parse(text).is_some(), ok, "{text:?}");
        if ok {
            let n: BigInt = text
                .trim_start_matches('+')
                .parse()
                .expect("num-bigint parses it");
            assert_eq!(
                Int::parse(text).unwrap().to_string(),
                n.to_string(),
                "{text:?}"
            );
        }
    }
}
