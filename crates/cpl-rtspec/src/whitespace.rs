//! The `White_Space` set of `trim`, `trim_start`, and `trim_end` (`spec/stdlib.md` S22, S47).
//!
//! Exactly the 25 scalar values that carry the Unicode `White_Space` property, written out so that the set does not
//! drift with the Unicode tables of a Rust or C library release: U+0009-U+000D, U+0020, U+0085, U+00A0, U+1680,
//! U+2000-U+200A, U+2028, U+2029, U+202F, U+205F, U+3000. Not U+200B (zero width space), not U+FEFF. Neither engine
//! uses its host language's classification (S47): the evaluator calls [`is_white_space`], and the C runtime reads the
//! generated `CPLRT_WHITE_SPACE` ranges (`spec/cabi.md` 2.7, B.2).

/// The 25 `White_Space` scalars as inclusive `(lo, hi)` ranges, ascending and disjoint.
pub const RANGES: [(u32, u32); 10] = [
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

/// Whether `c` is one of the 25 `White_Space` scalars of [`RANGES`].
pub fn is_white_space(c: char) -> bool {
    let c = u32::from(c);
    RANGES.iter().any(|&(lo, hi)| lo <= c && c <= hi)
}
