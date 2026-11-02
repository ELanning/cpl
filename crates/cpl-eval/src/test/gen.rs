//! Property generators: the draws of `spec/stdlib.md` 8.3 and the shrink
//! candidates of 8.4 (semantics S44, S45; toolchain T45).
//!
//! A [`Gen`] is a `for` range with its `gen.range` bounds already
//! evaluated (once per property, before case 0). Every draw consumes the
//! property's one ChaCha20 stream ([`Stream`]) in the order the
//! distribution table writes; shrinking uses no randomness.

use crate::value::Value;
use cpl_hir::Program;
use num_bigint::{BigInt, BigUint};
use num_traits::{One, Signed, ToPrimitive, Zero};
use rand_chacha::rand_core::{Rng, SeedableRng};
use rand_chacha::ChaCha20Rng;

/// The cap on the size parameter (`max_size`, stdlib 8.2): case `i` runs
/// at size `min(i, 100)`.
pub(crate) const MAX_SIZE: u32 = 100;

/// The fixed scalars of `gen.text`'s fourth bucket (stdlib 8.3), in order.
const TEXT_EXTRAS: [char; 10] = [
    '\u{e9}',
    '\u{df}',
    '\u{130}',
    '\u{3a9}',
    '\u{4e2d}',
    '\u{1f600}',
    '\u{200b}',
    '\u{301}',
    '\u{fffd}',
    '\u{a0}',
];

/// A generator of the `GenExpr` vocabulary (toolchain 3.5) with its
/// `gen.range` bounds evaluated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Gen {
    Int,
    PositiveInt,
    Bool,
    Text,
    List(Box<Gen>),
    Option(Box<Gen>),
    /// `gen.range(lo, hi)` with `lo <= hi` checked.
    Range(BigInt, BigInt),
}

/// A property's random stream: `ChaCha20Rng::from_seed(property_seed)`
/// (stream 0, block 0) and the primitive draws of stdlib 8.3.
pub(crate) struct Stream {
    rng: ChaCha20Rng,
}

impl Stream {
    pub(crate) fn new(seed: [u8; 32]) -> Stream {
        Stream {
            rng: ChaCha20Rng::from_seed(seed),
        }
    }

    /// `u()`: the next `u32` word of the stream.
    fn u(&mut self) -> u32 {
        self.rng.next_u32()
    }

    /// One word of the stream (`u()` of stdlib 8.3).
    pub(crate) fn word(&mut self) -> u32 {
        self.u()
    }

    /// `bits(b)`: `ceil(b / 32)` words assembled little-endian (the first
    /// word least significant), keeping the low `b` bits.
    fn bits(&mut self, b: u64) -> BigUint {
        if b == 0 {
            return BigUint::zero();
        }
        let mut digits: Vec<u32> = (0..b.div_ceil(32)).map(|_| self.u()).collect();
        let rem = b % 32;
        if rem != 0 {
            if let Some(top) = digits.last_mut() {
                *top &= (1u32 << rem) - 1;
            }
        }
        BigUint::new(digits)
    }

    /// `bits(b)` for `b <= 64`, with the same draws.
    fn bits_u64(&mut self, b: u32) -> u64 {
        if b == 0 {
            return 0;
        }
        let low = u64::from(self.u());
        let x = if b > 32 {
            low | (u64::from(self.u()) << 32)
        } else {
            low
        };
        if b == 64 {
            x
        } else {
            x & ((1u64 << b) - 1)
        }
    }

    /// `below(k)` for `1 <= k <= u64::MAX`: with `b` the bit length of
    /// `k - 1`, `bits(b)` repeated until it is below `k` (no modulo bias;
    /// `below(1)` is 0 and draws nothing).
    pub(crate) fn below_u64(&mut self, k: u64) -> u64 {
        debug_assert!(k >= 1, "below(k) needs k >= 1");
        let b = 64 - (k - 1).leading_zeros();
        loop {
            let x = self.bits_u64(b);
            if x < k {
                return x;
            }
        }
    }

    /// `below(k)` for any `k >= 1`.
    fn below(&mut self, k: &BigUint) -> BigUint {
        if let Some(small) = k.to_u64() {
            return BigUint::from(self.below_u64(small));
        }
        let b = (k - 1u32).bits();
        loop {
            let x = self.bits(b);
            if &x < k {
                return x;
            }
        }
    }

    /// `sign()`: one word; odd makes the value negative.
    fn negative(&mut self) -> bool {
        self.u() % 2 == 1
    }
}

/// One draw of `g` at size `size` (stdlib 8.3).
pub(crate) fn draw(s: &mut Stream, g: &Gen, size: u32, p: &Program) -> Value {
    match g {
        Gen::Int => Value::Int(draw_int(s, size)),
        Gen::PositiveInt => Value::Int(BigInt::one() + draw_int(s, size).abs()),
        Gen::Bool => Value::Bool(s.u() % 2 == 1),
        Gen::Text => Value::text(&draw_text(s, size)),
        Gen::List(elem) => {
            let len = s.below_u64(u64::from(size) + 1);
            let items = (0..len).map(|_| draw(s, elem, size, p)).collect();
            Value::list(items)
        }
        Gen::Option(elem) => {
            if s.u().is_multiple_of(4) {
                Value::none(p)
            } else {
                let x = draw(s, elem, size, p);
                Value::some(p, x)
            }
        }
        Gen::Range(lo, hi) => {
            // lo <= hi was checked at setup, so the span is at least 1.
            let span: BigInt = hi - lo + 1;
            let k = span.magnitude().clone().max(BigUint::one());
            Value::Int(lo + BigInt::from(s.below(&k)))
        }
    }
}

/// `gen.int` at size `s`: bucket `u() % 8`; 0 gives 0, 1 gives ±1, 2 at
/// sizes of 8 or more a large value (bit length `64 + below(8s + 1)` with
/// the top bit set), everything else uniform in `[-s, s]`.
fn draw_int(st: &mut Stream, size: u32) -> BigInt {
    let s = u64::from(size);
    match st.u() % 8 {
        0 => BigInt::zero(),
        1 => {
            if st.negative() {
                -BigInt::one()
            } else {
                BigInt::one()
            }
        }
        2 if s >= 8 => {
            let length = 64 + st.below_u64(8 * s + 1);
            let magnitude = (BigUint::one() << (length - 1)) + st.bits(length - 1);
            let value = BigInt::from(magnitude);
            if st.negative() {
                -value
            } else {
                value
            }
        }
        _ => BigInt::from(st.below_u64(2 * s + 1)) - BigInt::from(s),
    }
}

/// `gen.text` at size `s`: a length `below(s + 1)`, then each scalar by
/// bucket `u() % 4` (`a`-`z` twice as often as printable ASCII and the
/// fixed list of [`TEXT_EXTRAS`]).
fn draw_text(st: &mut Stream, size: u32) -> String {
    let len = st.below_u64(u64::from(size) + 1);
    let mut out = String::new();
    for _ in 0..len {
        let c = match st.u() % 4 {
            0 | 1 => char::from(b'a' + st.below_u64(26) as u8),
            2 => char::from(0x20 + st.below_u64(95) as u8),
            _ => TEXT_EXTRAS[st.below_u64(10) as usize],
        };
        out.push(c);
    }
    out
}

/// A lazy sequence of shrink candidates: a list's candidates are built only
/// as far as the shrinker consumes them.
pub(crate) type Candidates<'a> = Box<dyn Iterator<Item = Value> + 'a>;

/// The shrink candidates of `v` drawn from `g`, in the order of stdlib 8.4,
/// each different from `v` (duplicates among them are kept: the table
/// lists rules, and every rerun counts as one shrink step).
pub(crate) fn candidates<'a>(g: &'a Gen, v: &'a Value, p: &'a Program) -> Candidates<'a> {
    Box::new(raw_candidates(g, v, p).filter(move |c| !c.structurally_equal(v)))
}

fn raw_candidates<'a>(g: &'a Gen, v: &'a Value, p: &'a Program) -> Candidates<'a> {
    match (g, v) {
        // `0`; `v / 2` (truncating); one step toward 0.
        (Gen::Int, Value::Int(n)) => {
            let mut c = vec![BigInt::zero(), n / 2];
            if n.is_positive() {
                c.push(n - 1);
            } else if n.is_negative() {
                c.push(n + 1);
            }
            Box::new(c.into_iter().map(Value::Int))
        }
        // `1`; `1 + (v - 1) / 2`; `v - 1`; only candidates `>= 1`.
        (Gen::PositiveInt, Value::Int(n)) => {
            let one = BigInt::one();
            let c: Vec<BigInt> = [one.clone(), &one + (n - &one) / 2, n - &one]
                .into_iter()
                .filter(|c| c >= &one)
                .collect();
            Box::new(c.into_iter().map(Value::Int))
        }
        (Gen::Bool, Value::Bool(true)) => Box::new(std::iter::once(Value::Bool(false))),
        // `""`; the first half; the second half; without the last scalar;
        // the first scalar that is not `a` replaced by `a`.
        (Gen::Text, Value::Text(t)) => {
            let chars: Vec<char> = t.chars().collect();
            let n = chars.len();
            let text = |cs: &[char]| Value::text(&cs.iter().collect::<String>());
            let mut c = vec![
                Value::text(""),
                text(&chars[..n / 2]),
                text(&chars[n / 2..]),
            ];
            if n > 0 {
                c.push(text(&chars[..n - 1]));
            }
            if let Some(i) = chars.iter().position(|&ch| ch != 'a') {
                let mut replaced = chars.clone();
                replaced[i] = 'a';
                c.push(text(&replaced));
            }
            Box::new(c.into_iter())
        }
        // `[]`; the first half; the second half; without element `i` for
        // every `i` ascending; then element `i` replaced by each of its own
        // candidates, `i` ascending.
        (Gen::List(elem), Value::List(items)) => {
            let n = items.len();
            let fixed = [
                Value::empty_list(),
                Value::list(items[..n / 2].to_vec()),
                Value::list(items[n / 2..].to_vec()),
            ];
            let removals = (0..n).map(move |i| {
                let mut without = items.to_vec();
                without.remove(i);
                Value::list(without)
            });
            let replacements = items.iter().enumerate().flat_map(move |(i, item)| {
                candidates(elem, item, p).map(move |x| {
                    let mut replaced = items.to_vec();
                    replaced[i] = x;
                    Value::list(replaced)
                })
            });
            Box::new(fixed.into_iter().chain(removals).chain(replacements))
        }
        // `Some(x)`: `None`, then `Some(x')` for each candidate of `x`.
        (Gen::Option(elem), v) => match v.as_option(p) {
            Some(Some(x)) => Box::new(
                std::iter::once(Value::none(p))
                    .chain(candidates(elem, x, p).map(move |x| Value::some(p, x))),
            ),
            _ => Box::new(std::iter::empty()),
        },
        // With `o = 0.max(lo).min(hi)`: `o`; `o + (v - o) / 2`; one step
        // from `v` toward `o`; every candidate in `[lo, hi]`.
        (Gen::Range(lo, hi), Value::Int(n)) => {
            let zero = BigInt::zero();
            let o = zero.max(lo.clone()).min(hi.clone());
            let mut c = vec![o.clone(), &o + (n - &o) / 2];
            if n > &o {
                c.push(n - 1);
            } else if n < &o {
                c.push(n + 1);
            }
            Box::new(
                c.into_iter()
                    .filter(move |x| x >= lo && x <= hi)
                    .map(Value::Int),
            )
        }
        _ => Box::new(std::iter::empty()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::std_program;

    /// An independent reading of stdlib 8.3's primitives over the raw stream.
    struct Replay(ChaCha20Rng);

    impl Replay {
        fn new(seed: [u8; 32]) -> Replay {
            Replay(ChaCha20Rng::from_seed(seed))
        }
        fn u(&mut self) -> u32 {
            self.0.next_u32()
        }
        fn bits(&mut self, b: u64) -> BigUint {
            let words = b.div_ceil(32);
            let mut x = BigUint::zero();
            for i in 0..words {
                x += BigUint::from(self.u()) << (32 * i);
            }
            x % (BigUint::one() << b)
        }
        fn below(&mut self, k: u64) -> u64 {
            let b = if k == 1 {
                0
            } else {
                64 - (k - 1).leading_zeros() as u64
            };
            loop {
                let x = self.bits(b).to_u64().unwrap();
                if x < k {
                    return x;
                }
            }
        }
        fn int(&mut self, s: u64) -> BigInt {
            match self.u() % 8 {
                0 => BigInt::zero(),
                1 => {
                    if self.u() % 2 == 1 {
                        BigInt::from(-1)
                    } else {
                        BigInt::from(1)
                    }
                }
                2 if s >= 8 => {
                    let l = 64 + self.below(8 * s + 1);
                    let m = BigInt::from((BigUint::one() << (l - 1)) + self.bits(l - 1));
                    if self.u() % 2 == 1 {
                        -m
                    } else {
                        m
                    }
                }
                _ => BigInt::from(self.below(2 * s + 1) as i64 - s as i64),
            }
        }
    }

    #[test]
    fn every_draw_follows_the_distribution_table() {
        let p = std_program();
        for seed in [[0u8; 32], [7; 32], [0xa5; 32]] {
            for size in [0u32, 1, 3, 8, 37, 100] {
                let s64 = u64::from(size);
                let mut st = Stream::new(seed);
                let mut r = Replay::new(seed);
                for _ in 0..60 {
                    // gen.int
                    assert_eq!(draw(&mut st, &Gen::Int, size, &p), Value::Int(r.int(s64)));
                    // gen.positive_int: 1 + |v|
                    let v = r.int(s64);
                    assert_eq!(
                        draw(&mut st, &Gen::PositiveInt, size, &p),
                        Value::Int(BigInt::one() + v.abs())
                    );
                    // gen.bool
                    assert_eq!(
                        draw(&mut st, &Gen::Bool, size, &p),
                        Value::Bool(r.u() % 2 == 1)
                    );
                    // gen.text
                    let len = r.below(s64 + 1);
                    let mut t = String::new();
                    for _ in 0..len {
                        t.push(match r.u() % 4 {
                            0 | 1 => char::from_u32('a' as u32 + r.below(26) as u32).unwrap(),
                            2 => char::from_u32(0x20 + r.below(95) as u32).unwrap(),
                            _ => TEXT_EXTRAS[r.below(10) as usize],
                        });
                    }
                    assert_eq!(draw(&mut st, &Gen::Text, size, &p), Value::text(&t));
                    // gen.list(gen.bool)
                    let len = r.below(s64 + 1);
                    let items: Vec<Value> = (0..len).map(|_| Value::Bool(r.u() % 2 == 1)).collect();
                    assert_eq!(
                        draw(&mut st, &Gen::List(Box::new(Gen::Bool)), size, &p),
                        Value::list(items)
                    );
                    // gen.option(gen.int)
                    let expected = if r.u().is_multiple_of(4) {
                        Value::none(&p)
                    } else {
                        Value::some(&p, Value::Int(r.int(s64)))
                    };
                    assert_eq!(
                        draw(&mut st, &Gen::Option(Box::new(Gen::Int)), size, &p),
                        expected
                    );
                    // gen.range(-5, 5_000_000_000): the size is ignored; 33 bits as two words.
                    let range = Gen::Range(BigInt::from(-5), BigInt::from(5_000_000_000i64));
                    let x = r.below(5_000_000_006);
                    assert_eq!(
                        draw(&mut st, &range, size, &p),
                        Value::Int(BigInt::from(x as i64 - 5))
                    );
                }
            }
        }
    }

    #[test]
    fn large_ints_have_the_top_bit_of_their_length_set() {
        let p = std_program();
        let mut st = Stream::new([9; 32]);
        let mut large = 0;
        for _ in 0..400 {
            if let Value::Int(n) = &draw(&mut st, &Gen::Int, 100, &p) {
                if n.magnitude().bits() > 64 {
                    large += 1;
                    assert!(n.magnitude().bits() <= 64 + 800);
                }
                if n.magnitude().bits() <= 64 {
                    assert!(n.abs() <= BigInt::from(100), "{n}");
                }
            }
        }
        assert!(large > 20, "{large}");
    }

    fn ints(ns: &[i64]) -> Vec<Value> {
        ns.iter().map(|&n| Value::int(n)).collect()
    }

    #[test]
    fn shrink_candidates_follow_the_table_in_order() {
        let p = std_program();
        let c = |g: &Gen, v: Value| candidates(g, &v, &p).collect::<Vec<_>>();
        assert_eq!(c(&Gen::Int, Value::int(10)), ints(&[0, 5, 9]));
        assert_eq!(c(&Gen::Int, Value::int(-7)), ints(&[0, -3, -6]));
        // Duplicates are kept: every rule yields 0 for 1.
        assert_eq!(c(&Gen::Int, Value::int(1)), ints(&[0, 0, 0]));
        assert!(c(&Gen::Int, Value::int(0)).is_empty());
        assert_eq!(c(&Gen::PositiveInt, Value::int(5)), ints(&[1, 3, 4]));
        assert_eq!(c(&Gen::PositiveInt, Value::int(2)), ints(&[1, 1, 1]));
        assert!(c(&Gen::PositiveInt, Value::int(1)).is_empty());
        assert_eq!(c(&Gen::Bool, Value::Bool(true)), vec![Value::Bool(false)]);
        assert!(c(&Gen::Bool, Value::Bool(false)).is_empty());
        let texts = |ts: &[&str]| ts.iter().map(|t| Value::text(t)).collect::<Vec<_>>();
        assert_eq!(
            c(&Gen::Text, Value::text("ba")),
            texts(&["", "b", "a", "b", "aa"])
        );
        assert_eq!(
            c(&Gen::Text, Value::text("abc")),
            texts(&["", "a", "bc", "ab", "aac"])
        );
        // "", the first half "", and without its last scalar "" (the second half is "a" itself).
        assert_eq!(c(&Gen::Text, Value::text("a")), texts(&["", "", ""]));
        assert!(c(&Gen::Text, Value::text("")).is_empty());
        let list = |ns: &[i64]| Value::list(ints(ns));
        assert_eq!(
            c(&Gen::List(Box::new(Gen::Int)), list(&[3, 0])),
            vec![
                list(&[]),
                list(&[3]),
                list(&[0]),
                list(&[0]),
                list(&[3]),
                list(&[0, 0]),
                list(&[1, 0]),
                list(&[2, 0]),
            ]
        );
        assert_eq!(
            c(&Gen::List(Box::new(Gen::Int)), list(&[1, 2, 3])),
            vec![
                list(&[]),
                list(&[1]),
                list(&[2, 3]),
                list(&[2, 3]),
                list(&[1, 3]),
                list(&[1, 2]),
                list(&[0, 2, 3]),
                list(&[0, 2, 3]),
                list(&[0, 2, 3]),
                list(&[1, 0, 3]),
                list(&[1, 1, 3]),
                list(&[1, 1, 3]),
                list(&[1, 2, 0]),
                list(&[1, 2, 1]),
                list(&[1, 2, 2]),
            ]
        );
        assert!(c(&Gen::List(Box::new(Gen::Int)), list(&[])).is_empty());
        let some = |n: i64| Value::some(&p, Value::int(n));
        assert_eq!(
            c(&Gen::Option(Box::new(Gen::Int)), some(2)),
            vec![Value::none(&p), some(0), some(1), some(1)]
        );
        assert!(c(&Gen::Option(Box::new(Gen::Int)), Value::none(&p)).is_empty());
        let range = |lo: i64, hi: i64| Gen::Range(BigInt::from(lo), BigInt::from(hi));
        assert_eq!(c(&range(5, 10), Value::int(8)), ints(&[5, 6, 7]));
        assert_eq!(c(&range(-10, -2), Value::int(-9)), ints(&[-2, -5, -8]));
        assert_eq!(c(&range(-10, 10), Value::int(-4)), ints(&[0, -2, -3]));
        assert!(c(&range(5, 10), Value::int(5)).is_empty());
    }

    /// The fast `below` path draws exactly what the general one draws.
    #[test]
    fn below_fast_path_matches_the_general_definition() {
        for k in [
            1u64,
            2,
            3,
            26,
            95,
            201,
            801,
            1 << 32,
            (1 << 32) + 1,
            u64::MAX,
        ] {
            let mut fast = Stream::new([7; 32]);
            let mut slow = Stream::new([7; 32]);
            for _ in 0..50 {
                let a = fast.below_u64(k);
                let big = BigUint::from(k);
                let b = (&big - 1u32).bits();
                let x = loop {
                    let x = slow.bits(b);
                    if x < big {
                        break x;
                    }
                };
                assert_eq!(BigUint::from(a), x, "k = {k}");
            }
        }
    }
}
