//! The runtime of CPL code compiled to Rust (`spec/hosts.md`): the values (`Int`, `Text`,
//! `Bytes`, `List<T>`, the standard library's error types), faults, the calling context with its
//! call-depth and stack guards, the builtins of the standard library, value text, and value JSON.
//! It has no dependencies, so a generated crate builds with `cargo` alone.

#![allow(dead_code, clippy::all)]

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
/// The reference of a type that contains itself (`spec/hosts.md` 2.2): `enum Tree { Leaf,
/// Node(Arc<Tree>, Int) }` where the CPL type has `Node(Tree, Int)`.
pub use std::sync::Arc;

// ===========================================================================
// Int: arbitrary precision, i64 while the value fits
// ===========================================================================

/// A CPL `Int`: an arbitrary-precision integer. Values that fit in an `i64` are stored inline,
/// so arithmetic on them is a checked machine operation.
#[derive(Clone)]
pub struct Int(Repr);

#[derive(Clone)]
enum Repr {
    S(i64),
    /// Never a value that fits in an `i64`.
    B(Arc<Big>),
}

#[derive(Clone, PartialEq, Eq)]
struct Big {
    neg: bool,
    /// Little-endian 32-bit limbs, no leading zero limb.
    mag: Vec<u32>,
}

const BASE: u64 = 1 << 32;

fn trim(m: &mut Vec<u32>) {
    while m.last() == Some(&0) {
        m.pop();
    }
}

fn mag_from_u64(v: u64) -> Vec<u32> {
    let mut m = vec![v as u32, (v >> 32) as u32];
    trim(&mut m);
    m
}

fn mag_cmp(a: &[u32], b: &[u32]) -> Ordering {
    if a.len() != b.len() {
        return a.len().cmp(&b.len());
    }
    for i in (0..a.len()).rev() {
        if a[i] != b[i] {
            return a[i].cmp(&b[i]);
        }
    }
    Ordering::Equal
}

fn mag_add(a: &[u32], b: &[u32]) -> Vec<u32> {
    let (a, b) = if a.len() >= b.len() { (a, b) } else { (b, a) };
    let mut out = Vec::with_capacity(a.len() + 1);
    let mut carry = 0u64;
    for i in 0..a.len() {
        let s = a[i] as u64 + *b.get(i).unwrap_or(&0) as u64 + carry;
        out.push(s as u32);
        carry = s >> 32;
    }
    if carry > 0 {
        out.push(carry as u32);
    }
    out
}

/// `a - b` for `a >= b`.
fn mag_sub(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut out = Vec::with_capacity(a.len());
    let mut borrow = 0i64;
    for i in 0..a.len() {
        let d = a[i] as i64 - *b.get(i).unwrap_or(&0) as i64 - borrow;
        if d < 0 {
            out.push((d + BASE as i64) as u32);
            borrow = 1;
        } else {
            out.push(d as u32);
            borrow = 0;
        }
    }
    trim(&mut out);
    out
}

fn mag_mul(a: &[u32], b: &[u32]) -> Vec<u32> {
    if a.is_empty() || b.is_empty() {
        return Vec::new();
    }
    let mut out = vec![0u32; a.len() + b.len()];
    for i in 0..a.len() {
        let mut carry = 0u64;
        for j in 0..b.len() {
            let t = a[i] as u64 * b[j] as u64 + out[i + j] as u64 + carry;
            out[i + j] = t as u32;
            carry = t >> 32;
        }
        out[i + b.len()] = carry as u32;
    }
    trim(&mut out);
    out
}

fn mag_divrem_small(a: &[u32], d: u32) -> (Vec<u32>, u32) {
    let mut q = vec![0u32; a.len()];
    let mut r = 0u64;
    for i in (0..a.len()).rev() {
        let cur = (r << 32) | a[i] as u64;
        q[i] = (cur / d as u64) as u32;
        r = cur % d as u64;
    }
    trim(&mut q);
    (q, r as u32)
}

/// Knuth's algorithm D (Hacker's Delight `divmnu`): `(a / b, a % b)` for `b` nonzero.
fn mag_divrem(a: &[u32], b: &[u32]) -> (Vec<u32>, Vec<u32>) {
    if mag_cmp(a, b) == Ordering::Less {
        return (Vec::new(), a.to_vec());
    }
    if b.len() == 1 {
        let (q, r) = mag_divrem_small(a, b[0]);
        return (q, if r == 0 { Vec::new() } else { vec![r] });
    }
    let n = b.len();
    let m = a.len() - n;
    let s = b[n - 1].leading_zeros();
    let shl = |x: &[u32], extra: bool| -> Vec<u32> {
        let mut out = Vec::with_capacity(x.len() + 1);
        if s == 0 {
            out.extend_from_slice(x);
            if extra {
                out.push(0);
            }
            return out;
        }
        let mut carry = 0u32;
        for &w in x {
            out.push((w << s) | carry);
            carry = w >> (32 - s);
        }
        if extra {
            out.push(carry);
        }
        out
    };
    let vn = shl(b, false);
    let mut un = shl(a, true);
    let mut q = vec![0u32; m + 1];
    for j in (0..=m).rev() {
        let num = ((un[j + n] as u64) << 32) | un[j + n - 1] as u64;
        let mut qhat = num / vn[n - 1] as u64;
        let mut rhat = num - qhat * vn[n - 1] as u64;
        while qhat >= BASE || qhat * vn[n - 2] as u64 > ((rhat << 32) | un[j + n - 2] as u64) {
            qhat -= 1;
            rhat += vn[n - 1] as u64;
            if rhat >= BASE {
                break;
            }
        }
        let mut k: i64 = 0;
        let mut t: i64;
        for i in 0..n {
            let p = qhat * vn[i] as u64;
            t = un[i + j] as i64 - k - (p & 0xFFFF_FFFF) as i64;
            un[i + j] = t as u32;
            k = (p >> 32) as i64 - (t >> 32);
        }
        t = un[j + n] as i64 - k;
        un[j + n] = t as u32;
        q[j] = qhat as u32;
        if t < 0 {
            q[j] = q[j].wrapping_sub(1);
            let mut k: i64 = 0;
            for i in 0..n {
                let t = un[i + j] as i64 + vn[i] as i64 + k;
                un[i + j] = t as u32;
                k = t >> 32;
            }
            un[j + n] = (un[j + n] as i64 + k) as u32;
        }
    }
    let mut r = vec![0u32; n];
    if s == 0 {
        r.copy_from_slice(&un[..n]);
    } else {
        for i in 0..n - 1 {
            r[i] = (un[i] >> s) | (un[i + 1] << (32 - s));
        }
        r[n - 1] = un[n - 1] >> s;
    }
    trim(&mut q);
    trim(&mut r);
    (q, r)
}

impl Int {
    pub const ZERO: Int = Int(Repr::S(0));
    pub const ONE: Int = Int(Repr::S(1));

    pub const fn small(v: i64) -> Int {
        Int(Repr::S(v))
    }

    pub fn from_i64(v: i64) -> Int {
        Int(Repr::S(v))
    }

    pub fn from_usize(v: usize) -> Int {
        match i64::try_from(v) {
            Ok(v) => Int(Repr::S(v)),
            Err(_) => Int::from_parts(false, mag_from_u64(v as u64)),
        }
    }

    fn from_parts(neg: bool, mag: Vec<u32>) -> Int {
        if mag.len() <= 2 {
            let u = mag.first().copied().unwrap_or(0) as u64
                | (mag.get(1).copied().unwrap_or(0) as u64) << 32;
            if !neg && u <= i64::MAX as u64 {
                return Int(Repr::S(u as i64));
            }
            if neg && u <= 1u64 << 63 {
                return Int(Repr::S((u as i64).wrapping_neg()));
            }
        }
        Int(Repr::B(Arc::new(Big {
            neg: neg && !mag.is_empty(),
            mag,
        })))
    }

    /// The magnitude, borrowed where it is a big one's (no copy for a read).
    fn mag(&self) -> std::borrow::Cow<'_, [u32]> {
        match &self.0 {
            Repr::S(v) => std::borrow::Cow::Owned(mag_from_u64(v.unsigned_abs())),
            Repr::B(b) => std::borrow::Cow::Borrowed(&b.mag),
        }
    }

    fn parts(&self) -> (bool, Vec<u32>) {
        match &self.0 {
            Repr::S(v) => (*v < 0, mag_from_u64(v.unsigned_abs())),
            Repr::B(b) => (b.neg, b.mag.clone()),
        }
    }

    pub fn to_i64(&self) -> Option<i64> {
        match self.0 {
            Repr::S(v) => Some(v),
            Repr::B(_) => None,
        }
    }

    pub fn to_usize(&self) -> Option<usize> {
        self.to_i64().and_then(|v| usize::try_from(v).ok())
    }

    pub fn is_negative(&self) -> bool {
        match &self.0 {
            Repr::S(v) => *v < 0,
            Repr::B(b) => b.neg,
        }
    }

    pub fn is_zero(&self) -> bool {
        matches!(self.0, Repr::S(0))
    }

    fn add_parts(a: (bool, Vec<u32>), b: (bool, Vec<u32>)) -> Int {
        if a.0 == b.0 {
            return Int::from_parts(a.0, mag_add(&a.1, &b.1));
        }
        match mag_cmp(&a.1, &b.1) {
            Ordering::Equal => Int::ZERO,
            Ordering::Greater => Int::from_parts(a.0, mag_sub(&a.1, &b.1)),
            Ordering::Less => Int::from_parts(b.0, mag_sub(&b.1, &a.1)),
        }
    }

    // Each operation is a machine operation inlined where it is used, and calls its
    // arbitrary-precision path, kept out of line, only when an operand or the result does not
    // fit in an `i64` (H4).

    #[inline]
    pub fn add(&self, o: &Int) -> Int {
        if let (Repr::S(a), Repr::S(b)) = (&self.0, &o.0) {
            if let Some(c) = a.checked_add(*b) {
                return Int(Repr::S(c));
            }
        }
        self.add_big(o)
    }

    #[cold]
    #[inline(never)]
    fn add_big(&self, o: &Int) -> Int {
        Int::add_parts(self.parts(), o.parts())
    }

    #[inline]
    pub fn sub(&self, o: &Int) -> Int {
        if let (Repr::S(a), Repr::S(b)) = (&self.0, &o.0) {
            if let Some(c) = a.checked_sub(*b) {
                return Int(Repr::S(c));
            }
        }
        self.sub_big(o)
    }

    #[cold]
    #[inline(never)]
    fn sub_big(&self, o: &Int) -> Int {
        let (neg, mag) = o.parts();
        Int::add_parts(self.parts(), (!neg && !mag.is_empty(), mag))
    }

    #[inline]
    pub fn mul(&self, o: &Int) -> Int {
        if let (Repr::S(a), Repr::S(b)) = (&self.0, &o.0) {
            if let Some(c) = a.checked_mul(*b) {
                return Int(Repr::S(c));
            }
        }
        self.mul_big(o)
    }

    #[cold]
    #[inline(never)]
    fn mul_big(&self, o: &Int) -> Int {
        let (am, bm) = (self.mag(), o.mag());
        Int::from_parts(self.is_negative() != o.is_negative(), mag_mul(&am, &bm))
    }

    #[inline]
    pub fn neg(&self) -> Int {
        if let Repr::S(a) = self.0 {
            if let Some(c) = a.checked_neg() {
                return Int(Repr::S(c));
            }
        }
        self.neg_big()
    }

    #[cold]
    #[inline(never)]
    fn neg_big(&self) -> Int {
        let (n, m) = self.parts();
        Int::from_parts(!n && !m.is_empty(), m)
    }

    pub fn abs(&self) -> Int {
        if self.is_negative() {
            self.neg()
        } else {
            self.clone()
        }
    }

    /// Truncating division and remainder (semantics 6.3): the quotient rounds toward zero and
    /// the remainder has the dividend's sign.
    pub fn div_rem(&self, o: &Int) -> Option<(Int, Int)> {
        if o.is_zero() {
            return None;
        }
        if let (Repr::S(a), Repr::S(b)) = (&self.0, &o.0) {
            if let (Some(q), Some(r)) = (a.checked_div(*b), a.checked_rem(*b)) {
                return Some((Int(Repr::S(q)), Int(Repr::S(r))));
            }
        }
        let (an, bn) = (self.is_negative(), o.is_negative());
        let (q, r) = mag_divrem(&self.mag(), &o.mag());
        Some((Int::from_parts(an != bn, q), Int::from_parts(an, r)))
    }

    #[inline]
    pub fn div(&self, o: &Int, symbol: &str) -> Result<Int, Fault> {
        if let (Repr::S(a), Repr::S(b)) = (&self.0, &o.0) {
            if let Some(q) = a.checked_div(*b) {
                return Ok(Int(Repr::S(q)));
            }
        }
        self.div_big(o, symbol)
    }

    /// Also division by zero, and `i64::MIN / -1`.
    #[cold]
    #[inline(never)]
    fn div_big(&self, o: &Int, symbol: &str) -> Result<Int, Fault> {
        self.div_rem(o)
            .map(|(q, _)| q)
            .ok_or_else(|| Fault::division_by_zero(symbol))
    }

    #[inline]
    pub fn rem(&self, o: &Int, symbol: &str) -> Result<Int, Fault> {
        if let (Repr::S(a), Repr::S(b)) = (&self.0, &o.0) {
            if let Some(r) = a.checked_rem(*b) {
                return Ok(Int(Repr::S(r)));
            }
        }
        self.rem_big(o, symbol)
    }

    #[cold]
    #[inline(never)]
    fn rem_big(&self, o: &Int, symbol: &str) -> Result<Int, Fault> {
        self.div_rem(o)
            .map(|(_, r)| r)
            .ok_or_else(|| Fault::division_by_zero(symbol))
    }

    /// The decimal grammar of `parse_int` (stdlib S25): `("+" | "-")? ASCII_DIGIT+`.
    pub fn parse(t: &str) -> Option<Int> {
        let (neg, digits) = match t.as_bytes().first() {
            Some(b'+') => (false, &t[1..]),
            Some(b'-') => (true, &t[1..]),
            _ => (false, t),
        };
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        if digits.len() <= 18 {
            let v: i64 = digits.parse().ok()?;
            return Some(Int(Repr::S(if neg { -v } else { v })));
        }
        let mut mag: Vec<u32> = Vec::new();
        for chunk in digits.as_bytes().chunks(9) {
            let mut scale = 1u64;
            let mut value = 0u64;
            for &d in chunk {
                scale *= 10;
                value = value * 10 + (d - b'0') as u64;
            }
            // mag = mag * scale + value
            let mut carry = value;
            for limb in mag.iter_mut() {
                let t = *limb as u64 * scale + carry;
                *limb = t as u32;
                carry = t >> 32;
            }
            while carry > 0 {
                mag.push(carry as u32);
                carry >>= 32;
            }
        }
        trim(&mut mag);
        Some(Int::from_parts(neg, mag))
    }

    /// A literal of the program.
    pub fn lit(t: &str) -> Int {
        Int::parse(t).expect("an integer literal")
    }
}

impl PartialEq for Int {
    #[inline]
    fn eq(&self, o: &Int) -> bool {
        match (&self.0, &o.0) {
            (Repr::S(a), Repr::S(b)) => a == b,
            (Repr::B(a), Repr::B(b)) => a == b,
            _ => false,
        }
    }
}
impl Eq for Int {}

impl Hash for Int {
    fn hash<H: Hasher>(&self, h: &mut H) {
        match &self.0 {
            Repr::S(v) => v.hash(h),
            Repr::B(b) => {
                b.neg.hash(h);
                b.mag.hash(h);
            }
        }
    }
}

impl Int {
    #[cold]
    #[inline(never)]
    fn cmp_big(&self, o: &Int) -> Ordering {
        let (am, bm) = (self.mag(), o.mag());
        match (self.is_negative(), o.is_negative()) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, false) => mag_cmp(&am, &bm),
            (true, true) => mag_cmp(&bm, &am),
        }
    }
}

impl Ord for Int {
    #[inline]
    fn cmp(&self, o: &Int) -> Ordering {
        match (&self.0, &o.0) {
            (Repr::S(a), Repr::S(b)) => a.cmp(b),
            _ => self.cmp_big(o),
        }
    }
}
impl PartialOrd for Int {
    #[inline]
    fn partial_cmp(&self, o: &Int) -> Option<Ordering> {
        Some(self.cmp(o))
    }
    #[inline]
    fn lt(&self, o: &Int) -> bool {
        match (&self.0, &o.0) {
            (Repr::S(a), Repr::S(b)) => a < b,
            _ => self.cmp_big(o).is_lt(),
        }
    }
    #[inline]
    fn le(&self, o: &Int) -> bool {
        match (&self.0, &o.0) {
            (Repr::S(a), Repr::S(b)) => a <= b,
            _ => self.cmp_big(o).is_le(),
        }
    }
    #[inline]
    fn gt(&self, o: &Int) -> bool {
        match (&self.0, &o.0) {
            (Repr::S(a), Repr::S(b)) => a > b,
            _ => self.cmp_big(o).is_gt(),
        }
    }
    #[inline]
    fn ge(&self, o: &Int) -> bool {
        match (&self.0, &o.0) {
            (Repr::S(a), Repr::S(b)) => a >= b,
            _ => self.cmp_big(o).is_ge(),
        }
    }
}

impl fmt::Display for Int {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Repr::S(v) => write!(f, "{v}"),
            Repr::B(b) => {
                use std::fmt::Write as _;
                // nine digits at a time, the magnitude divided in place
                let mut m = b.mag.clone();
                let mut chunks: Vec<u32> = Vec::with_capacity(m.len() * 32 / 29 + 1);
                while !m.is_empty() {
                    let mut r = 0u64;
                    for x in m.iter_mut().rev() {
                        let cur = (r << 32) | *x as u64;
                        *x = (cur / 1_000_000_000) as u32;
                        r = cur % 1_000_000_000;
                    }
                    while m.last() == Some(&0) {
                        m.pop();
                    }
                    chunks.push(r as u32);
                }
                let mut s = String::with_capacity(chunks.len() * 9 + 1);
                if b.neg {
                    s.push('-');
                }
                let _ = write!(s, "{}", chunks.last().copied().unwrap_or(0));
                for c in chunks.iter().rev().skip(1) {
                    let _ = write!(s, "{c:09}");
                }
                f.write_str(&s)
            }
        }
    }
}
impl fmt::Debug for Int {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
impl From<i64> for Int {
    fn from(v: i64) -> Int {
        Int(Repr::S(v))
    }
}
impl From<i32> for Int {
    fn from(v: i32) -> Int {
        Int(Repr::S(v as i64))
    }
}
impl From<u64> for Int {
    fn from(v: u64) -> Int {
        Int::from_parts(false, mag_from_u64(v))
    }
}
impl From<usize> for Int {
    fn from(v: usize) -> Int {
        Int::from_usize(v)
    }
}
impl TryFrom<&Int> for i64 {
    type Error = Fault;
    fn try_from(v: &Int) -> Result<i64, Fault> {
        v.to_i64().ok_or_else(|| {
            Fault::new(
                "interop_int_out_of_range",
                None,
                "",
                "",
                &format!("{v} does not fit in an i64"),
            )
        })
    }
}

// ===========================================================================
// Text, Bytes, List
// ===========================================================================

/// A CPL `Text`: a sequence of Unicode scalar values (Rust's `str`), shared and immutable.
#[derive(Clone)]
pub struct Text(TextRepr);

/// How many bytes a text holds inline, with no allocation (a word, a number): `Text` stays three words.
const TEXT_INLINE: usize = 22;

#[derive(Clone)]
enum TextRepr {
    Static(&'static str),
    /// A short text: its length and its bytes, always those of a `&str` (`Text::inline`).
    Small(u8, [u8; TEXT_INLINE]),
    Shared(Arc<str>),
    /// A text being appended to: spare capacity, used in place while nothing else holds it.
    Growing(Arc<String>),
}

impl Text {
    pub const fn lit(s: &'static str) -> Text {
        Text(TextRepr::Static(s))
    }
    /// `s` held inline, when it is short enough.
    #[inline]
    fn inline(s: &str) -> Option<Text> {
        if s.len() > TEXT_INLINE {
            return None;
        }
        let mut b = [0u8; TEXT_INLINE];
        b[..s.len()].copy_from_slice(s.as_bytes());
        Some(Text(TextRepr::Small(s.len() as u8, b)))
    }
    pub fn new(s: &str) -> Text {
        if s.is_empty() {
            return Text::lit("");
        }
        Text::inline(s).unwrap_or_else(|| Text(TextRepr::Shared(Arc::from(s))))
    }
    pub fn from_string(s: String) -> Text {
        if s.is_empty() {
            return Text::lit("");
        }
        Text::inline(&s).unwrap_or_else(|| Text(TextRepr::Shared(Arc::from(s))))
    }
    /// The decimal digits of `v`, inline (an `i64` has at most 20).
    pub fn of_i64(v: i64) -> Text {
        let mut b = [0u8; TEXT_INLINE];
        let mut i = TEXT_INLINE;
        let mut m = v.unsigned_abs();
        loop {
            i -= 1;
            b[i] = b'0' + (m % 10) as u8;
            m /= 10;
            if m == 0 {
                break;
            }
        }
        if v < 0 {
            i -= 1;
            b[i] = b'-';
        }
        let n = TEXT_INLINE - i;
        b.copy_within(i.., 0);
        Text(TextRepr::Small(n as u8, b))
    }
    #[inline]
    pub fn as_str(&self) -> &str {
        match &self.0 {
            TextRepr::Static(s) => s,
            // SAFETY: a `Small` holds the first `n` bytes of a `&str` (`inline`) or ASCII digits (`of_i64`), so they
            // are valid UTF-8.
            TextRepr::Small(n, b) => unsafe { std::str::from_utf8_unchecked(&b[..*n as usize]) },
            TextRepr::Shared(s) => s,
            TextRepr::Growing(s) => s,
        }
    }
    /// `self = self ++ more`: in place when `self` alone holds a buffer with room, else into a
    /// new buffer of twice the length, so a loop of appends is linear.
    pub fn append_mut(&mut self, more: &str) {
        if more.is_empty() {
            return;
        }
        if let TextRepr::Growing(s) = &mut self.0 {
            if let Some(s) = Arc::get_mut(s) {
                s.push_str(more);
                return;
            }
        }
        let mut s = String::with_capacity((self.len() + more.len()) * 2);
        s.push_str(self);
        s.push_str(more);
        self.0 = TextRepr::Growing(Arc::new(s));
    }
}
impl std::ops::Deref for Text {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}
impl Default for Text {
    fn default() -> Text {
        Text::lit("")
    }
}
impl PartialEq for Text {
    fn eq(&self, o: &Text) -> bool {
        self.as_str() == o.as_str()
    }
}
impl Eq for Text {}
impl Hash for Text {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.as_str().hash(h)
    }
}
impl fmt::Debug for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}
impl fmt::Display for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
impl From<&str> for Text {
    fn from(s: &str) -> Text {
        Text::new(s)
    }
}
impl From<String> for Text {
    fn from(s: String) -> Text {
        Text::from_string(s)
    }
}

/// A CPL `Bytes`: an immutable byte string.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Bytes(Arc<[u8]>);

impl Bytes {
    pub fn new(b: &[u8]) -> Bytes {
        Bytes(Arc::from(b))
    }
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }
}
impl fmt::Debug for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Bytes(\"{}\")", hex(&self.0))
    }
}
impl From<&[u8]> for Bytes {
    fn from(b: &[u8]) -> Bytes {
        Bytes::new(b)
    }
}
impl From<Vec<u8>> for Bytes {
    fn from(b: Vec<u8>) -> Bytes {
        Bytes(Arc::from(b))
    }
}

/// A CPL `List<T>`: an immutable sequence, a view of a shared buffer. `take`, `drop`, and
/// `drop_last` are views, so they cost nothing; appending to a list that holds the only
/// reference to its buffer and ends where the buffer ends happens in place, so building a list
/// in a loop is linear.
#[derive(Clone)]
pub struct List<T> {
    buf: Arc<Vec<T>>,
    start: usize,
    len: usize,
}

impl<T> List<T> {
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn as_slice(&self) -> &[T] {
        &self.buf[self.start..self.start + self.len]
    }
    pub fn get(&self, i: usize) -> Option<&T> {
        self.as_slice().get(i)
    }
    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.as_slice().iter()
    }
    /// The elements `from..to` of this list, sharing its buffer.
    pub fn view(&self, from: usize, to: usize) -> List<T> {
        List {
            buf: Arc::clone(&self.buf),
            start: self.start + from,
            len: to - from,
        }
    }
}

impl<T: Clone> List<T> {
    pub fn new() -> List<T> {
        List::from_vec(Vec::new())
    }
    pub fn from_vec(v: Vec<T>) -> List<T> {
        let len = v.len();
        List {
            buf: Arc::new(v),
            start: 0,
            len,
        }
    }
    /// The buffer to append to in place: this list's, when nothing else refers to it and the
    /// list ends where it does.
    fn tail_mut(&mut self) -> Option<&mut Vec<T>> {
        if self.start + self.len != self.buf.len() {
            return None;
        }
        Arc::get_mut(&mut self.buf)
    }
    pub fn push_mut(&mut self, v: T) {
        if let Some(vec) = self.tail_mut() {
            vec.push(v);
            self.len += 1;
            return;
        }
        let mut vec = Vec::with_capacity((self.len + 1) * 2);
        vec.extend_from_slice(self.as_slice());
        vec.push(v);
        *self = List::from_vec(vec);
    }
    /// `self = self ++ more`, in place where it can.
    pub fn append_mut(&mut self, more: &List<T>) {
        if more.is_empty() {
            return;
        }
        if self.is_empty() {
            *self = more.clone();
            return;
        }
        if let Some(vec) = self.tail_mut() {
            vec.extend_from_slice(more.as_slice());
            self.len += more.len;
            return;
        }
        let mut vec = Vec::with_capacity((self.len + more.len) * 2);
        vec.extend_from_slice(self.as_slice());
        vec.extend_from_slice(more.as_slice());
        *self = List::from_vec(vec);
    }
    pub fn to_vec(&self) -> Vec<T> {
        self.as_slice().to_vec()
    }
}
impl<T: PartialEq> PartialEq for List<T> {
    fn eq(&self, o: &List<T>) -> bool {
        self.as_slice() == o.as_slice()
    }
}
impl<T: Eq> Eq for List<T> {}
impl<T: Hash> Hash for List<T> {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.as_slice().hash(h)
    }
}
impl<T: Clone> Default for List<T> {
    fn default() -> List<T> {
        List::new()
    }
}
impl<T: fmt::Debug> fmt::Debug for List<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}
impl<T: Clone> From<Vec<T>> for List<T> {
    fn from(v: Vec<T>) -> List<T> {
        List::from_vec(v)
    }
}
impl<T: Clone> FromIterator<T> for List<T> {
    fn from_iter<I: IntoIterator<Item = T>>(it: I) -> List<T> {
        List::from_vec(it.into_iter().collect())
    }
}
impl<'a, T> IntoIterator for &'a List<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

// ===========================================================================
// The standard library's data types
// ===========================================================================

/// `ConstraintError` (stdlib 1.5): the error of `check<R>` and of the standard parsers.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ConstraintError {
    pub type_name: Text,
    pub message: Text,
}

/// `std.foreign.ForeignErrorKind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ForeignErrorKind {
    Exception,
    InvalidArgument,
    InvalidResult,
    Transport,
    Timeout,
    Cancelled,
    Unavailable,
}

impl ForeignErrorKind {
    const NAMES: [&'static str; 7] = [
        "Exception",
        "InvalidArgument",
        "InvalidResult",
        "Transport",
        "Timeout",
        "Cancelled",
        "Unavailable",
    ];
    const ALL: [ForeignErrorKind; 7] = [
        ForeignErrorKind::Exception,
        ForeignErrorKind::InvalidArgument,
        ForeignErrorKind::InvalidResult,
        ForeignErrorKind::Transport,
        ForeignErrorKind::Timeout,
        ForeignErrorKind::Cancelled,
        ForeignErrorKind::Unavailable,
    ];
    pub fn name(self) -> &'static str {
        Self::NAMES[self as usize]
    }
}

/// `std.foreign.ForeignDetail`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ForeignDetail {
    pub key: Text,
    pub value: Text,
}

/// `std.foreign.ForeignError`: a foreign failure as a typed CPL error.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ForeignError {
    pub kind: ForeignErrorKind,
    pub message: Text,
    pub details: List<ForeignDetail>,
}

// ===========================================================================
// Faults and the calling context
// ===========================================================================

/// A fault (semantics 6.7): an execution that cannot continue. It is an error of the program,
/// not a value: a contract that failed, an assertion, a division by zero, a hole, an exhausted
/// resource.
#[derive(Clone, PartialEq, Eq)]
pub struct Fault(Box<FaultInfo>);

/// What a [`Fault`] says; a `Fault` derefs to it (`fault.code`, `fault.message`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaultInfo {
    /// The diagnostic code: `contract_violation`, `assertion_failed`, `division_by_zero`,
    /// `unresolved_hole`, `resource_exhaustion`, `refinement_violation`, ...
    pub code: &'static str,
    /// For a contract violation: `requires`, `ensures`, `invariant`, or `decreases`.
    pub clause_kind: Option<&'static str>,
    /// The failing clause's text (an assertion's expression, a hole's name).
    pub clause: String,
    /// The stable identity of the blamed declaration.
    pub symbol: String,
    pub message: String,
}

// A fault is one pointer, so `Result<T, Fault>` stays small on the path without one.
impl std::ops::Deref for Fault {
    type Target = FaultInfo;
    fn deref(&self) -> &FaultInfo {
        &self.0
    }
}
impl std::ops::DerefMut for Fault {
    fn deref_mut(&mut self) -> &mut FaultInfo {
        &mut self.0
    }
}
impl fmt::Debug for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&*self.0, f)
    }
}

impl Fault {
    pub fn new(
        code: &'static str,
        clause_kind: Option<&'static str>,
        clause: &str,
        symbol: &str,
        message: &str,
    ) -> Fault {
        Fault(Box::new(FaultInfo {
            code,
            clause_kind,
            clause: clause.to_string(),
            symbol: symbol.to_string(),
            message: message.to_string(),
        }))
    }
    pub fn division_by_zero(symbol: &str) -> Fault {
        Fault::new("division_by_zero", None, "", symbol, "division by zero")
    }
    pub fn requires(clause: &str, symbol: &str) -> Fault {
        Fault::new(
            "contract_violation",
            Some("requires"),
            clause,
            symbol,
            &format!("requires clause failed: {clause}"),
        )
    }
    pub fn stack(limit: u32) -> Fault {
        Fault::new(
            "resource_exhaustion",
            None,
            "",
            "",
            &format!("the call depth limit of {limit} activations was exceeded"),
        )
    }
    pub fn refinement(expected: &str, clause: &str, symbol: &str) -> Fault {
        Fault::new(
            "refinement_violation",
            None,
            clause,
            symbol,
            &format!("the value does not satisfy {expected}: {clause}"),
        )
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for Fault {}

/// The default call-depth limit (semantics S38).
pub const MAX_CALL_DEPTH: u32 = 10_000;

/// The context of one call from the host: the call depth, the native stack's guard, and the
/// measures of the active recursive functions (S35). Generated functions take it last.
pub struct Cx {
    depth: u32,
    limit: u32,
    stack_floor: usize,
    /// The activations of functions with a `decreases` clause, innermost last, with their
    /// measures once recorded.
    /// (Only a recursive function with a `decreases` clause pushes one: a call without one
    /// allocates nothing.)
    measures: Vec<(u32, Option<Int>)>,
    /// The owners of the contract expressions being evaluated, innermost last (S34).
    clauses: Stack<&'static str, 8>,
    /// Speculative forms that bailed in this call (`q_ok`).
    q_bails: u32,
}

/// A stack whose first `N` entries live in place: a call that nests no deeper allocates
/// nothing.
struct Stack<T, const N: usize> {
    inline: [T; N],
    len: usize,
    spill: Vec<T>,
}

impl<T: Default, const N: usize> Stack<T, N> {
    #[inline]
    fn push(&mut self, v: T) {
        if self.len < N {
            self.inline[self.len] = v;
        } else {
            self.spill.push(v);
        }
        self.len += 1;
    }
    #[inline]
    fn pop(&mut self) {
        self.len -= 1;
        if self.len >= N {
            self.spill.pop();
        } else {
            self.inline[self.len] = T::default();
        }
    }
    #[inline]
    fn get(&self, i: usize) -> &T {
        if i < N {
            &self.inline[i]
        } else {
            &self.spill[i - N]
        }
    }
}

thread_local! {
    static STACK_FLOOR: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The lowest address this thread's stack may reach before the guard faults.
#[inline]
fn stack_floor(here: usize) -> usize {
    STACK_FLOOR.with(|f| {
        if f.get() == 0 {
            f.set(thread_stack_floor(here));
        }
        f.get()
    })
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn thread_stack_floor(here: usize) -> usize {
    extern "C" {
        fn pthread_self() -> usize;
        fn pthread_getattr_np(thread: usize, attr: *mut [u64; 16]) -> i32;
        fn pthread_attr_getstack(attr: *const [u64; 16], addr: *mut usize, size: *mut usize)
            -> i32;
        fn pthread_attr_destroy(attr: *mut [u64; 16]) -> i32;
    }
    let mut attr = [0u64; 16];
    let (mut addr, mut size) = (0usize, 0usize);
    // SAFETY: the attribute buffer is larger than glibc's pthread_attr_t on every target.
    let ok = unsafe {
        pthread_getattr_np(pthread_self(), &mut attr) == 0
            && pthread_attr_getstack(&attr, &mut addr, &mut size) == 0
    };
    if ok {
        unsafe { pthread_attr_destroy(&mut attr) };
        // Keep 256 KiB for the host's own frames and the fault's construction.
        if addr != 0 && here > addr {
            return addr + (256 << 10);
        }
    }
    here.saturating_sub(1 << 20)
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn thread_stack_floor(here: usize) -> usize {
    here.saturating_sub(1 << 20)
}

impl Cx {
    #[inline]
    pub fn new() -> Cx {
        Cx::with_depth(MAX_CALL_DEPTH)
    }
    #[inline]
    pub fn with_depth(limit: u32) -> Cx {
        let here = 0u8;
        let at = &here as *const u8 as usize;
        Cx {
            depth: 0,
            limit,
            stack_floor: stack_floor(at),
            measures: Vec::new(),
            clauses: Stack {
                inline: [""; 8],
                len: 0,
                spill: Vec::new(),
            },
            q_bails: 0,
        }
    }
    /// The speculative forms (spec/hosts.md): whether to try one (not after 16 bails in this call).
    #[inline]
    pub fn q_ok(&self) -> bool {
        self.q_bails < 16
    }
    #[inline]
    pub fn q_bail(&mut self) {
        self.q_bails += 1;
    }
    /// The function activations in progress.
    #[inline]
    pub fn depth(&self) -> u32 {
        self.depth
    }
    /// A speculative call's checks, `qd` the activations already in progress: the depth limit, and every 16
    /// levels the stack's guard (a speculative frame is small; the guard keeps far more than 16 of them).
    #[inline]
    pub fn q_enter(&self, qd: u32) -> bool {
        if qd >= self.limit {
            return false;
        }
        if qd & 15 == 15 {
            let here = 0u8;
            if (&here as *const u8 as usize) < self.stack_floor {
                return false;
            }
        }
        true
    }
    /// The measure of the nearest activation of `f` as an `i64`: `i64::MIN` when there is none or it has none
    /// yet, `i64::MAX` for a bigint (always positive, so above every `i64`).
    pub fn q_prev(&self, f: u32) -> i64 {
        for (g, m) in self.measures.iter().rev() {
            if *g == f {
                return match m {
                    None => i64::MIN,
                    Some(m) => m.to_i64().unwrap_or(i64::MAX),
                };
            }
        }
        i64::MIN
    }
    /// A function activation (semantics S38): the depth limit, and the native stack's guard.
    #[inline]
    pub fn enter(&mut self) -> Result<(), Fault> {
        self.depth += 1;
        if self.depth > self.limit {
            return Err(Fault::stack(self.limit));
        }
        let here = 0u8;
        if (&here as *const u8 as usize) < self.stack_floor {
            return Err(Fault::new(
                "resource_exhaustion",
                None,
                "",
                "",
                &format!(
                    "the stack is exhausted at call depth {} (limit {})",
                    self.depth, self.limit
                ),
            ));
        }
        Ok(())
    }
    #[inline]
    pub fn leave(&mut self) {
        self.depth -= 1;
    }
    /// The activation of function `f`, which has a `decreases` clause, begins: its measure is
    /// recorded by [`Cx::measure_set`] once its `requires` clauses held.
    #[inline]
    pub fn measure_push(&mut self, f: u32) {
        self.measures.push((f, None));
    }
    /// S35: the measure of the current activation of `f` is `>= 0` and below the measure
    /// recorded by the nearest enclosing activation of `f` (none while that activation is
    /// still checking its `requires` clauses or computing its measure).
    pub fn measure_set(&mut self, f: u32, m: Int, clause: &str, symbol: &str) -> Result<(), Fault> {
        let n = self.measures.len();
        let previous = self.measures[..n - 1]
            .iter()
            .rev()
            .find(|(g, _)| *g == f)
            .and_then(|(_, m)| m.as_ref());
        if m.is_negative() || previous.is_some_and(|prev| m >= *prev) {
            return Err(Fault::new(
                "contract_violation",
                Some("decreases"),
                clause,
                symbol,
                &format!("decreases clause failed: {clause}"),
            ));
        }
        self.measures[n - 1].1 = Some(m);
        Ok(())
    }
    /// The activation that began with the last [`Cx::measure_push`] ends, after its `ensures`.
    #[inline]
    pub fn measure_pop(&mut self) {
        self.measures.pop();
    }
    /// The evaluation of a contract expression of `owner` begins (S34).
    #[inline]
    pub fn clause_enter(&mut self, owner: &'static str) {
        self.clauses.push(owner);
    }
    #[inline]
    pub fn clause_leave(&mut self) {
        self.clauses.pop();
    }
    /// S34: a fault that escaped a contract expression is its owner's. A fault ends the call,
    /// so the innermost expression still being evaluated when it arrives is the one it escaped.
    pub fn attribute(&self, mut f: Fault) -> Fault {
        if self.clauses.len > 0 {
            f.symbol = self.clauses.get(self.clauses.len - 1).to_string();
        }
        f
    }
}

impl Default for Cx {
    fn default() -> Cx {
        Cx::new()
    }
}

/// `?name`: the fault of a hole reached at run time (semantics 8.1).
pub fn hole<T>(name: &str, symbol: &str) -> Result<T, Fault> {
    Err(Fault::new(
        "unresolved_hole",
        None,
        name,
        symbol,
        &format!("evaluation reached the unresolved hole ?{name}"),
    ))
}

// ===========================================================================
// Value text (semantics S40)
// ===========================================================================

/// CPL literal syntax for a value: what `check<R>` quotes in its error message.
pub trait ValueText {
    fn write_text(&self, out: &mut String);
    fn value_text(&self) -> String {
        let mut s = String::new();
        self.write_text(&mut s);
        s
    }
}

/// A text as a CPL literal, with the grammar's escapes.
pub fn quote_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\x7f' => {
                out.push_str(&format!("\\u{{{:x}}}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push_str(&format!("{x:02x}"));
    }
    s
}

impl ValueText for Int {
    fn write_text(&self, out: &mut String) {
        out.push_str(&self.to_string())
    }
}
impl ValueText for bool {
    fn write_text(&self, out: &mut String) {
        out.push_str(if *self { "true" } else { "false" })
    }
}
impl ValueText for Text {
    fn write_text(&self, out: &mut String) {
        out.push_str(&quote_text(self))
    }
}
impl ValueText for Bytes {
    fn write_text(&self, out: &mut String) {
        out.push_str("Bytes(\"");
        out.push_str(&hex(&self.0));
        out.push_str("\")");
    }
}
impl ValueText for () {
    fn write_text(&self, out: &mut String) {
        out.push_str("Unit")
    }
}
impl<T: ValueText> ValueText for List<T> {
    fn write_text(&self, out: &mut String) {
        out.push('[');
        for (i, x) in self.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            x.write_text(out);
        }
        out.push(']');
    }
}
impl<T: ValueText> ValueText for Option<T> {
    fn write_text(&self, out: &mut String) {
        match self {
            Some(x) => {
                out.push_str("Some(");
                x.write_text(out);
                out.push(')');
            }
            None => out.push_str("None"),
        }
    }
}
impl<T: ValueText, E: ValueText> ValueText for Result<T, E> {
    fn write_text(&self, out: &mut String) {
        match self {
            Ok(x) => {
                out.push_str("Ok(");
                x.write_text(out);
                out.push(')');
            }
            Err(e) => {
                out.push_str("Err(");
                e.write_text(out);
                out.push(')');
            }
        }
    }
}
impl ValueText for ConstraintError {
    fn write_text(&self, out: &mut String) {
        out.push_str("ConstraintError { type_name: ");
        self.type_name.write_text(out);
        out.push_str(", message: ");
        self.message.write_text(out);
        out.push_str(" }");
    }
}
impl ValueText for ForeignErrorKind {
    fn write_text(&self, out: &mut String) {
        out.push_str(self.name())
    }
}
impl ValueText for ForeignDetail {
    fn write_text(&self, out: &mut String) {
        out.push_str("ForeignDetail { key: ");
        self.key.write_text(out);
        out.push_str(", value: ");
        self.value.write_text(out);
        out.push_str(" }");
    }
}
impl ValueText for ForeignError {
    fn write_text(&self, out: &mut String) {
        out.push_str("ForeignError { kind: ");
        self.kind.write_text(out);
        out.push_str(", message: ");
        self.message.write_text(out);
        out.push_str(", details: ");
        self.details.write_text(out);
        out.push_str(" }");
    }
}

/// `check<R>`'s error when the predicate of `type_name` fails on `value` (semantics 6.10).
pub fn check_failure<T: ValueText, U>(
    value: &T,
    type_name: &'static str,
    predicate: &'static str,
) -> Result<U, ConstraintError> {
    Err(ConstraintError {
        type_name: Text::lit(type_name),
        message: Text::from_string(format!(
            "{} does not satisfy {predicate}",
            value.value_text()
        )),
    })
}

// ===========================================================================
// Value JSON (toolchain 7.3)
// ===========================================================================

/// A JSON document whose numbers keep every digit.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(String),
    Str(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn object(mut fields: Vec<(String, Json)>) -> Json {
        fields.sort_by(|a, b| a.0.cmp(&b.0));
        Json::Object(fields)
    }
    pub fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Number(n) => out.push_str(n),
            Json::Str(s) => write_json_string(s, out),
            Json::Array(items) => {
                out.push('[');
                for (i, x) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    x.write(out);
                }
                out.push(']');
            }
            Json::Object(fields) => {
                out.push('{');
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_json_string(k, out);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        self.write(&mut s);
        s
    }
    /// Read a JSON document.
    pub fn parse(text: &str) -> Result<Json, String> {
        let mut p = JsonParser {
            s: text.as_bytes(),
            i: 0,
        };
        let v = p.value(0)?;
        p.ws();
        if p.i != p.s.len() {
            return Err(format!("trailing characters at {}", p.i));
        }
        Ok(v)
    }
}

fn write_json_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

struct JsonParser<'a> {
    s: &'a [u8],
    i: usize,
}

impl JsonParser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }
    fn value(&mut self, depth: usize) -> Result<Json, String> {
        if depth > 512 {
            return Err("JSON nested too deeply".into());
        }
        self.ws();
        match self.s.get(self.i) {
            None => Err("unexpected end of JSON".into()),
            Some(b'n') => self.word("null", Json::Null),
            Some(b't') => self.word("true", Json::Bool(true)),
            Some(b'f') => self.word("false", Json::Bool(false)),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b'[') => {
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.s.get(self.i) == Some(&b']') {
                    self.i += 1;
                    return Ok(Json::Array(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    self.ws();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Json::Array(items));
                        }
                        _ => return Err(format!("expected , or ] at {}", self.i)),
                    }
                }
            }
            Some(b'{') => {
                self.i += 1;
                let mut fields = Vec::new();
                self.ws();
                if self.s.get(self.i) == Some(&b'}') {
                    self.i += 1;
                    return Ok(Json::Object(fields));
                }
                loop {
                    self.ws();
                    if self.s.get(self.i) != Some(&b'"') {
                        return Err(format!("expected a key at {}", self.i));
                    }
                    let k = self.string()?;
                    self.ws();
                    if self.s.get(self.i) != Some(&b':') {
                        return Err(format!("expected : at {}", self.i));
                    }
                    self.i += 1;
                    let v = self.value(depth + 1)?;
                    fields.push((k, v));
                    self.ws();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Json::Object(fields));
                        }
                        _ => return Err(format!("expected , or }} at {}", self.i)),
                    }
                }
            }
            Some(c) if *c == b'-' || c.is_ascii_digit() => {
                let start = self.i;
                self.i += 1;
                while self.i < self.s.len()
                    && matches!(
                        self.s[self.i],
                        b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-'
                    )
                {
                    self.i += 1;
                }
                Ok(Json::Number(
                    String::from_utf8_lossy(&self.s[start..self.i]).into_owned(),
                ))
            }
            Some(c) => Err(format!("unexpected {:?} at {}", *c as char, self.i)),
        }
    }
    fn word(&mut self, w: &str, v: Json) -> Result<Json, String> {
        if self.s[self.i..].starts_with(w.as_bytes()) {
            self.i += w.len();
            Ok(v)
        } else {
            Err(format!("unexpected text at {}", self.i))
        }
    }
    fn hex4(&mut self) -> Result<u32, String> {
        let h = self.s.get(self.i..self.i + 4).ok_or("short \\u escape")?;
        self.i += 4;
        u32::from_str_radix(std::str::from_utf8(h).map_err(|e| e.to_string())?, 16)
            .map_err(|e| e.to_string())
    }
    fn string(&mut self) -> Result<String, String> {
        self.i += 1;
        let mut out = String::new();
        loop {
            let c = *self.s.get(self.i).ok_or("unterminated string")?;
            match c {
                b'"' => {
                    self.i += 1;
                    return Ok(out);
                }
                b'\\' => {
                    let e = *self.s.get(self.i + 1).ok_or("unterminated escape")?;
                    self.i += 2;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hi = self.hex4()?;
                            let code = if (0xD800..0xDC00).contains(&hi) {
                                if self.s.get(self.i..self.i + 2) != Some(b"\\u") {
                                    return Err("an unpaired surrogate".into());
                                }
                                self.i += 2;
                                let lo = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&lo) {
                                    return Err("an unpaired surrogate".into());
                                }
                                0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                            } else if (0xDC00..0xE000).contains(&hi) {
                                return Err("an unpaired surrogate".into());
                            } else {
                                hi
                            };
                            out.push(char::from_u32(code).ok_or("an invalid scalar")?);
                        }
                        _ => return Err("an unknown escape".into()),
                    }
                }
                _ => {
                    let start = self.i;
                    while self.i < self.s.len() && self.s[self.i] != b'"' && self.s[self.i] != b'\\'
                    {
                        self.i += 1;
                    }
                    out.push_str(
                        std::str::from_utf8(&self.s[start..self.i]).map_err(|e| e.to_string())?,
                    );
                }
            }
        }
    }
}

/// The value JSON of a value.
pub trait ToJson {
    fn to_json(&self) -> Json;
}

/// A value from its value JSON; refinements are not checked here.
pub trait FromJson: Sized {
    fn from_json(j: &Json) -> Result<Self, String>;
}

pub fn tagged(type_name: &str, variant: Option<&str>, payload: Option<Vec<Json>>) -> Json {
    let mut fields = vec![("$type".to_string(), Json::Str(type_name.to_string()))];
    if let Some(v) = variant {
        fields.push(("$variant".to_string(), Json::Str(v.to_string())));
    }
    if let Some(p) = payload {
        fields.push(("$payload".to_string(), Json::Array(p)));
    }
    Json::object(fields)
}

/// The `$variant` and `$payload` of a tagged value of `type_name`.
pub fn untag<'a>(j: &'a Json, type_name: &str) -> Result<(&'a str, &'a [Json]), String> {
    match j.get("$type") {
        Some(Json::Str(t)) if t == type_name => {}
        _ => return Err(format!("expected a {type_name}")),
    }
    let variant = match j.get("$variant") {
        Some(Json::Str(v)) => v.as_str(),
        _ => "",
    };
    let payload = match j.get("$payload") {
        Some(Json::Array(p)) => p.as_slice(),
        _ => &[],
    };
    Ok((variant, payload))
}

/// A field of a struct's value JSON.
pub fn field<T: FromJson>(j: &Json, name: &str) -> Result<T, String> {
    T::from_json(j.get(name).ok_or_else(|| format!("missing field {name}"))?)
}

impl ToJson for Int {
    fn to_json(&self) -> Json {
        Json::Number(self.to_string())
    }
}
impl FromJson for Int {
    fn from_json(j: &Json) -> Result<Int, String> {
        match j {
            Json::Number(n) => Int::parse(n).ok_or_else(|| format!("{n} is not an integer")),
            _ => Err("expected an integer".into()),
        }
    }
}
impl ToJson for bool {
    fn to_json(&self) -> Json {
        Json::Bool(*self)
    }
}
impl FromJson for bool {
    fn from_json(j: &Json) -> Result<bool, String> {
        match j {
            Json::Bool(b) => Ok(*b),
            _ => Err("expected a boolean".into()),
        }
    }
}
impl ToJson for Text {
    fn to_json(&self) -> Json {
        Json::Str(self.as_str().to_string())
    }
}
impl FromJson for Text {
    fn from_json(j: &Json) -> Result<Text, String> {
        match j {
            Json::Str(s) => Ok(Text::new(s)),
            _ => Err("expected a text".into()),
        }
    }
}
impl ToJson for Bytes {
    fn to_json(&self) -> Json {
        Json::object(vec![
            ("$type".into(), Json::Str("Bytes".into())),
            ("hex".into(), Json::Str(hex(&self.0))),
        ])
    }
}
impl FromJson for Bytes {
    fn from_json(j: &Json) -> Result<Bytes, String> {
        match (j.get("$type"), j.get("hex")) {
            (Some(Json::Str(t)), Some(Json::Str(h))) if t == "Bytes" && h.len() % 2 == 0 => {
                let mut out = Vec::with_capacity(h.len() / 2);
                for i in (0..h.len()).step_by(2) {
                    out.push(u8::from_str_radix(&h[i..i + 2], 16).map_err(|e| e.to_string())?);
                }
                Ok(Bytes::from(out))
            }
            _ => Err("expected Bytes".into()),
        }
    }
}
impl ToJson for () {
    fn to_json(&self) -> Json {
        tagged("Unit", None, None)
    }
}
impl FromJson for () {
    fn from_json(j: &Json) -> Result<(), String> {
        untag(j, "Unit").map(|_| ())
    }
}
impl<T: ToJson> ToJson for List<T> {
    fn to_json(&self) -> Json {
        Json::Array(self.iter().map(ToJson::to_json).collect())
    }
}
impl<T: FromJson + Clone> FromJson for List<T> {
    fn from_json(j: &Json) -> Result<List<T>, String> {
        match j {
            Json::Array(items) => items
                .iter()
                .map(T::from_json)
                .collect::<Result<Vec<_>, _>>()
                .map(List::from_vec),
            _ => Err("expected a list".into()),
        }
    }
}
impl<T: ToJson> ToJson for Option<T> {
    fn to_json(&self) -> Json {
        match self {
            Some(x) => tagged("Option", Some("Some"), Some(vec![x.to_json()])),
            None => tagged("Option", Some("None"), None),
        }
    }
}
impl<T: FromJson> FromJson for Option<T> {
    fn from_json(j: &Json) -> Result<Option<T>, String> {
        match untag(j, "Option")? {
            ("Some", [x]) => Ok(Some(T::from_json(x)?)),
            ("None", []) => Ok(None),
            _ => Err("expected an Option".into()),
        }
    }
}
impl<T: ToJson, E: ToJson> ToJson for Result<T, E> {
    fn to_json(&self) -> Json {
        match self {
            Ok(x) => tagged("Result", Some("Ok"), Some(vec![x.to_json()])),
            Err(e) => tagged("Result", Some("Err"), Some(vec![e.to_json()])),
        }
    }
}
impl<T: FromJson, E: FromJson> FromJson for Result<T, E> {
    fn from_json(j: &Json) -> Result<Result<T, E>, String> {
        match untag(j, "Result")? {
            ("Ok", [x]) => Ok(Ok(T::from_json(x)?)),
            ("Err", [e]) => Ok(Err(E::from_json(e)?)),
            _ => Err("expected a Result".into()),
        }
    }
}
impl ToJson for ConstraintError {
    fn to_json(&self) -> Json {
        Json::object(vec![
            ("$type".into(), Json::Str("ConstraintError".into())),
            ("message".into(), self.message.to_json()),
            ("type_name".into(), self.type_name.to_json()),
        ])
    }
}
impl FromJson for ConstraintError {
    fn from_json(j: &Json) -> Result<ConstraintError, String> {
        Ok(ConstraintError {
            type_name: field(j, "type_name")?,
            message: field(j, "message")?,
        })
    }
}
impl ToJson for ForeignErrorKind {
    fn to_json(&self) -> Json {
        tagged("std.foreign.ForeignErrorKind", Some(self.name()), None)
    }
}
impl FromJson for ForeignErrorKind {
    fn from_json(j: &Json) -> Result<ForeignErrorKind, String> {
        let (v, _) = untag(j, "std.foreign.ForeignErrorKind")?;
        ForeignErrorKind::NAMES
            .iter()
            .position(|n| *n == v)
            .map(|i| ForeignErrorKind::ALL[i])
            .ok_or_else(|| "expected a ForeignErrorKind".into())
    }
}
impl ToJson for ForeignDetail {
    fn to_json(&self) -> Json {
        Json::object(vec![
            (
                "$type".into(),
                Json::Str("std.foreign.ForeignDetail".into()),
            ),
            ("key".into(), self.key.to_json()),
            ("value".into(), self.value.to_json()),
        ])
    }
}
impl FromJson for ForeignDetail {
    fn from_json(j: &Json) -> Result<ForeignDetail, String> {
        Ok(ForeignDetail {
            key: field(j, "key")?,
            value: field(j, "value")?,
        })
    }
}
impl ToJson for ForeignError {
    fn to_json(&self) -> Json {
        Json::object(vec![
            ("$type".into(), Json::Str("std.foreign.ForeignError".into())),
            ("details".into(), self.details.to_json()),
            ("kind".into(), self.kind.to_json()),
            ("message".into(), self.message.to_json()),
        ])
    }
}
impl FromJson for ForeignError {
    fn from_json(j: &Json) -> Result<ForeignError, String> {
        Ok(ForeignError {
            kind: field(j, "kind")?,
            message: field(j, "message")?,
            details: field(j, "details")?,
        })
    }
}

/// A field, payload, or wrapped value that holds a value of its own declaration (`spec/hosts.md`
/// H6): shared like an `Arc`, and dropped without recursion. Dropping the last holder of a value
/// nested a million levels deep (a list kept as a chain of nodes) would otherwise recurse once
/// per level and exhaust the thread's stack; here each level is queued and dropped in turn.
pub struct Boxed<T: 'static>(Option<BoxedRepr<T>>);

enum BoxedRepr<T: 'static> {
    Shared(Arc<T>),
    /// A value that needs no allocation: an option's `None` (`Boxed::opt`), a leaf of a tree.
    Static(&'static T),
}

impl<T: 'static> Clone for BoxedRepr<T> {
    fn clone(&self) -> BoxedRepr<T> {
        match self {
            BoxedRepr::Shared(a) => BoxedRepr::Shared(a.clone()),
            BoxedRepr::Static(s) => BoxedRepr::Static(s),
        }
    }
}

impl<T: 'static> Boxed<T> {
    pub fn new(v: T) -> Boxed<T> {
        Boxed(Some(BoxedRepr::Shared(Arc::new(v))))
    }
}
impl<X: 'static> Boxed<Option<X>> {
    /// An option, held with no allocation when it is `None`.
    #[inline]
    pub fn opt(v: Option<X>) -> Boxed<Option<X>> {
        match v {
            None => Boxed(Some(BoxedRepr::Static(&None))),
            v => Boxed::new(v),
        }
    }
}
impl<T: 'static> std::ops::Deref for Boxed<T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        match self.0.as_ref().expect("a live Boxed") {
            BoxedRepr::Shared(a) => a,
            BoxedRepr::Static(s) => s,
        }
    }
}
impl<T: Clone + 'static> Boxed<T> {
    /// The value, moved out where this is its only reference (no count of what it holds is taken), else a copy.
    #[inline]
    pub fn into_inner(mut self) -> T {
        match self.0.take().expect("a live Boxed") {
            BoxedRepr::Shared(a) => Arc::try_unwrap(a).unwrap_or_else(|a| (*a).clone()),
            BoxedRepr::Static(s) => s.clone(),
        }
    }
}
impl<T: 'static> Clone for Boxed<T> {
    fn clone(&self) -> Boxed<T> {
        Boxed(self.0.clone())
    }
}
impl<T: PartialEq + 'static> PartialEq for Boxed<T> {
    fn eq(&self, o: &Boxed<T>) -> bool {
        match (&self.0, &o.0) {
            (Some(BoxedRepr::Shared(a)), Some(BoxedRepr::Shared(b))) if Arc::ptr_eq(a, b) => true,
            (Some(_), Some(_)) => **self == **o,
            _ => false,
        }
    }
}
impl<T: Eq + 'static> Eq for Boxed<T> {}
impl<T: Hash + 'static> Hash for Boxed<T> {
    fn hash<H: Hasher>(&self, h: &mut H) {
        (**self).hash(h)
    }
}
impl<T: fmt::Debug + 'static> fmt::Debug for Boxed<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}
impl<T: 'static> Drop for Boxed<T> {
    fn drop(&mut self) {
        if let Some(BoxedRepr::Shared(a)) = self.0.take() {
            if let Ok(v) = Arc::try_unwrap(a) {
                drop_nested(v);
            }
        }
    }
}

/// How many levels of `Boxed` values are dropped by plain recursion before the rest are queued.
const INLINE_DROP_DEPTH: u32 = 128;

std::thread_local! {
    static DROP_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static PENDING: std::cell::RefCell<Vec<Box<dyn std::any::Any>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Drop `v`, the value of a `Boxed` that held its last reference. Up to `INLINE_DROP_DEPTH`
/// levels deep this recurses as any drop does; deeper values are queued, and the outermost call
/// drops the queue one value at a time, so the stack stays bounded however deep the value is.
fn drop_nested<T: 'static>(v: T) {
    // while the thread's locals are being destroyed (a value dropped at thread exit): plainly
    let Ok(depth) = DROP_DEPTH.try_with(|d| d.get()) else {
        return drop(v);
    };
    if depth >= INLINE_DROP_DEPTH {
        // (if the queue is gone, the closure, and v in it, is dropped plainly)
        let _ = PENDING.try_with(move |p| p.borrow_mut().push(Box::new(v)));
        return;
    }
    DROP_DEPTH.with(|d| d.set(depth + 1));
    drop(v);
    DROP_DEPTH.with(|d| d.set(depth));
    if depth == 0 {
        while let Some(next) = PENDING.with(|p| p.borrow_mut().pop()) {
            DROP_DEPTH.with(|d| d.set(1));
            drop(next);
            DROP_DEPTH.with(|d| d.set(0));
        }
    }
}

impl<T: ValueText + 'static> ValueText for Boxed<T> {
    fn write_text(&self, out: &mut String) {
        (**self).write_text(out)
    }
}
impl<T: ToJson + 'static> ToJson for Boxed<T> {
    fn to_json(&self) -> Json {
        (**self).to_json()
    }
}
impl<T: FromJson + 'static> FromJson for Boxed<T> {
    fn from_json(j: &Json) -> Result<Boxed<T>, String> {
        T::from_json(j).map(Boxed::new)
    }
}

impl<T: ValueText> ValueText for Arc<T> {
    fn write_text(&self, out: &mut String) {
        (**self).write_text(out)
    }
}
impl<T: ToJson> ToJson for Arc<T> {
    fn to_json(&self) -> Json {
        (**self).to_json()
    }
}
impl<T: FromJson> FromJson for Arc<T> {
    fn from_json(j: &Json) -> Result<Arc<T>, String> {
        T::from_json(j).map(Arc::new)
    }
}

/// What every CPL value of a generic position supports.
pub trait Value:
    Clone + PartialEq + Eq + Hash + fmt::Debug + ValueText + ToJson + FromJson + 'static
{
}
impl<T> Value for T where
    T: Clone + PartialEq + Eq + Hash + fmt::Debug + ValueText + ToJson + FromJson + 'static
{
}

// ===========================================================================
// The builtins of the standard library (stdlib 1.4-7)
// ===========================================================================

/// The 25 `White_Space` scalars (stdlib S22).
pub fn is_white_space(c: char) -> bool {
    const RANGES: [(u32, u32); 10] = [
        (0x0009, 0x000D),
        (0x0020, 0x0020),
        (0x0085, 0x0085),
        (0x00A0, 0x00A0),
        (0x1680, 0x1680),
        (0x2000, 0x200A),
        (0x2028, 0x2029),
        (0x202F, 0x202F),
        (0x205F, 0x205F),
        (0x3000, 0x3000),
    ];
    let c = c as u32;
    RANGES.iter().any(|&(lo, hi)| lo <= c && c <= hi)
}

pub fn concat_text(a: Text, b: Text) -> Text {
    if b.is_empty() {
        return a;
    }
    if a.is_empty() {
        return b;
    }
    let mut s = String::with_capacity(a.len() + b.len());
    s.push_str(&a);
    s.push_str(&b);
    Text::from_string(s)
}

pub fn concat_bytes(a: Bytes, b: Bytes) -> Bytes {
    if b.0.is_empty() {
        return a;
    }
    if a.0.is_empty() {
        return b;
    }
    let mut v = Vec::with_capacity(a.0.len() + b.0.len());
    v.extend_from_slice(&a.0);
    v.extend_from_slice(&b.0);
    Bytes::from(v)
}

pub fn concat_list<T: Clone>(mut a: List<T>, b: List<T>) -> List<T> {
    a.append_mut(&b);
    a
}

pub fn flatten<T: Clone>(lists: List<List<T>>) -> List<T> {
    let mut out = Vec::new();
    for l in lists.iter() {
        out.extend(l.iter().cloned());
    }
    List::from_vec(out)
}

pub fn int_to_text(v: Int) -> Text {
    match v.to_i64() {
        Some(x) => Text::of_i64(x),
        None => Text::from_string(v.to_string()),
    }
}
pub fn int_abs(v: Int) -> Int {
    v.abs()
}
pub fn int_min(a: Int, b: Int) -> Int {
    if b < a {
        b
    } else {
        a
    }
}
pub fn int_max(a: Int, b: Int) -> Int {
    if b > a {
        b
    } else {
        a
    }
}
pub fn text_len(t: &Text) -> Int {
    Int::from_usize(t.chars().count())
}
pub fn text_is_empty(t: &Text) -> bool {
    t.is_empty()
}
pub fn bytes_len(b: &Bytes) -> Int {
    Int::from_usize(b.0.len())
}
pub fn bytes_is_empty(b: &Bytes) -> bool {
    b.0.is_empty()
}
pub fn list_len<T: Clone>(l: &List<T>) -> Int {
    Int::from_usize(l.len())
}
pub fn list_is_empty<T: Clone>(l: &List<T>) -> bool {
    l.is_empty()
}
fn count(n: &Int, len: usize) -> usize {
    match n.to_usize() {
        Some(m) if m < len => m,
        _ => len,
    }
}
pub fn list_take<T: Clone>(l: &List<T>, n: Int) -> Result<List<T>, Fault> {
    if n.is_negative() {
        return Err(Fault::requires("n >= 0", "std.prelude.List.take"));
    }
    let k = count(&n, l.len());
    Ok(if k == l.len() {
        l.clone()
    } else {
        l.view(0, k)
    })
}
pub fn list_drop<T: Clone>(l: &List<T>, n: Int) -> Result<List<T>, Fault> {
    if n.is_negative() {
        return Err(Fault::requires("n >= 0", "std.prelude.List.drop"));
    }
    let k = count(&n, l.len());
    Ok(if k == 0 {
        l.clone()
    } else {
        l.view(k, l.len())
    })
}
pub fn list_drop_last<T: Clone>(l: &List<T>) -> List<T> {
    if l.is_empty() {
        l.clone()
    } else {
        l.view(0, l.len() - 1)
    }
}
pub fn list_push<T: Clone>(mut l: List<T>, v: T) -> List<T> {
    l.push_mut(v);
    l
}
pub fn list_get<T: Clone>(l: &List<T>, i: Int) -> Option<T> {
    i.to_usize().and_then(|i| l.get(i).cloned())
}

/// The list reads of the speculative forms (spec/hosts.md H14): element `i` as an `i64`, `Some(None)` past the
/// ends, `None` (a bail) for an element that is a bigint.
/// An element type of a list of lists in a speculative form: its empty list, shared.
#[doc(hidden)]
pub trait QLeaf: Sized + Clone + 'static {
    fn q_empty() -> &'static List<Self>;
}
impl QLeaf for Int {
    fn q_empty() -> &'static List<Int> {
        static E: std::sync::OnceLock<List<Int>> = std::sync::OnceLock::new();
        E.get_or_init(List::new)
    }
}
impl QLeaf for bool {
    fn q_empty() -> &'static List<bool> {
        static E: std::sync::OnceLock<List<bool>> = std::sync::OnceLock::new();
        E.get_or_init(List::new)
    }
}
/// `xs.get(i).unwrap_or([])` of a list of lists, read in place (no count taken): the row, or the empty list.
#[doc(hidden)]
#[inline]
pub fn q_row<T: QLeaf>(l: &List<List<T>>, i: i64) -> &List<T> {
    if i < 0 || i as u64 >= l.len() as u64 {
        return T::q_empty();
    }
    &l.as_slice()[i as usize]
}
#[doc(hidden)]
#[inline]
pub fn q_get(l: &List<Int>, i: i64) -> Option<Option<i64>> {
    if i < 0 || i as u64 >= l.len() as u64 {
        return Some(None);
    }
    l.as_slice()[i as usize].to_i64().map(Some)
}
#[doc(hidden)]
#[inline]
pub fn q_get_bool(l: &List<bool>, i: i64) -> Option<bool> {
    if i < 0 || i as u64 >= l.len() as u64 {
        return None;
    }
    Some(l.as_slice()[i as usize])
}
/// `take(k)` and `drop(k)`: `None` (a bail) for a negative count, whose fault the generic form raises.
#[doc(hidden)]
#[inline]
pub fn q_take<T: Clone>(l: &List<T>, k: i64) -> Option<List<T>> {
    if k < 0 {
        return None;
    }
    Some(l.view(0, (k as u64).min(l.len() as u64) as usize))
}
#[doc(hidden)]
#[inline]
pub fn q_drop<T: Clone>(l: &List<T>, k: i64) -> Option<List<T>> {
    if k < 0 {
        return None;
    }
    Some(l.view((k as u64).min(l.len() as u64) as usize, l.len()))
}
#[doc(hidden)]
#[inline]
pub fn q_drop_last<T: Clone>(l: &List<T>) -> List<T> {
    l.view(0, l.len().saturating_sub(1))
}
/// `xs = xs.take(k)`, `xs = xs.drop(k)`, `xs = xs.drop_last()` of a local: the view narrowed in place, with no
/// count of its buffer taken or given back.
#[doc(hidden)]
#[inline]
pub fn q_take_mut<T>(l: &mut List<T>, k: i64) -> Option<()> {
    if k < 0 {
        return None;
    }
    l.len = (k as u64).min(l.len as u64) as usize;
    Some(())
}
#[doc(hidden)]
#[inline]
pub fn q_drop_mut<T>(l: &mut List<T>, k: i64) -> Option<()> {
    if k < 0 {
        return None;
    }
    let n = (k as u64).min(l.len as u64) as usize;
    l.start += n;
    l.len -= n;
    Some(())
}
#[doc(hidden)]
#[inline]
pub fn q_drop_last_mut<T>(l: &mut List<T>) {
    l.len = l.len.saturating_sub(1);
}
pub fn list_first<T: Clone>(l: &List<T>) -> Option<T> {
    l.get(0).cloned()
}
pub fn list_last<T: Clone>(l: &List<T>) -> Option<T> {
    l.as_slice().last().cloned()
}
pub fn list_reverse<T: Clone>(l: List<T>) -> List<T> {
    l.iter().rev().cloned().collect()
}
pub fn list_contains<T: Clone + PartialEq>(l: &List<T>, v: &T) -> bool {
    l.iter().any(|x| x == v)
}
pub fn option_is_some<T>(o: &Option<T>) -> bool {
    o.is_some()
}
pub fn option_is_none<T>(o: &Option<T>) -> bool {
    o.is_none()
}
pub fn option_unwrap_or<T>(o: Option<T>, d: T) -> T {
    o.unwrap_or(d)
}
pub fn option_ok_or<T, E>(o: Option<T>, e: E) -> Result<T, E> {
    o.ok_or(e)
}
pub fn result_is_ok<T, E>(r: &Result<T, E>) -> bool {
    r.is_ok()
}
pub fn result_is_err<T, E>(r: &Result<T, E>) -> bool {
    r.is_err()
}
pub fn result_unwrap_or<T, E>(r: Result<T, E>, d: T) -> T {
    r.unwrap_or(d)
}
pub fn result_ok<T, E>(r: Result<T, E>) -> Option<T> {
    r.ok()
}
pub fn result_err<T, E>(r: Result<T, E>) -> Option<E> {
    r.err()
}
fn same_or_new(t: &Text, r: &str) -> Text {
    if r.len() == t.len() {
        t.clone()
    } else {
        Text::new(r)
    }
}
pub fn text_trim(t: Text) -> Text {
    same_or_new(&t, t.trim_matches(is_white_space))
}
pub fn text_trim_start(t: Text) -> Text {
    same_or_new(&t, t.trim_start_matches(is_white_space))
}
pub fn text_trim_end(t: Text) -> Text {
    same_or_new(&t, t.trim_end_matches(is_white_space))
}
pub fn text_starts_with(t: &Text, p: &Text) -> bool {
    t.starts_with(p.as_str())
}
pub fn text_ends_with(t: &Text, s: &Text) -> bool {
    t.ends_with(s.as_str())
}
pub fn text_contains(t: &Text, n: &Text) -> bool {
    t.contains(n.as_str())
}
pub fn text_split(t: Text, sep: Text) -> Result<List<Text>, Fault> {
    if sep.is_empty() {
        return Err(Fault::requires("not sep.is_empty()", "std.text.split"));
    }
    Ok(t.split(sep.as_str()).map(Text::new).collect())
}
pub fn text_join(parts: List<Text>, sep: Text) -> Text {
    if parts.len() == 1 {
        return parts.as_slice()[0].clone();
    }
    let mut out = String::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            out.push_str(&sep);
        }
        out.push_str(p);
    }
    Text::from_string(out)
}
pub fn text_to_upper(t: Text) -> Text {
    if t.bytes().any(|b| b.is_ascii_lowercase()) {
        Text::from_string(t.to_ascii_uppercase())
    } else {
        t
    }
}
pub fn text_to_lower(t: Text) -> Text {
    if t.bytes().any(|b| b.is_ascii_uppercase()) {
        Text::from_string(t.to_ascii_lowercase())
    } else {
        t
    }
}
pub fn text_parse_int(t: Text) -> Result<Int, ConstraintError> {
    Int::parse(&t).ok_or_else(|| ConstraintError {
        type_name: Text::lit("Int"),
        message: Text::from_string(format!("{} is not a decimal integer", quote_text(&t))),
    })
}
pub fn text_from_int(v: Int) -> Text {
    Text::from_string(v.to_string())
}
pub fn text_chars(t: Text) -> List<Text> {
    let mut buf = [0u8; 4];
    t.chars()
        .map(|c| Text::new(c.encode_utf8(&mut buf)))
        .collect()
}
pub fn bytes_from_list(values: List<Int>) -> Result<Bytes, Fault> {
    let mut out = Vec::with_capacity(values.len());
    for v in values.iter() {
        match v.to_i64() {
            Some(b @ 0..=255) => out.push(b as u8),
            _ => {
                return Err(Fault::requires(
                    "forall v in values: v >= 0 and v <= 255",
                    "std.bytes.from_list",
                ))
            }
        }
    }
    Ok(Bytes::from(out))
}
pub fn bytes_to_list(b: Bytes) -> List<Int> {
    b.0.iter().map(|&x| Int::from_i64(x as i64)).collect()
}
pub fn bytes_from_text(t: Text) -> Bytes {
    Bytes::new(t.as_bytes())
}
pub fn bytes_to_text(b: Bytes) -> Result<Text, ConstraintError> {
    match std::str::from_utf8(&b.0) {
        Ok(t) => Ok(Text::new(t)),
        Err(e) => Err(ConstraintError {
            type_name: Text::lit("Text"),
            message: Text::from_string(format!("invalid UTF-8 at byte {}", e.valid_up_to())),
        }),
    }
}
