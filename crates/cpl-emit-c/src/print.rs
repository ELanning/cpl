//! Printing helpers of the emitter (`spec/cabi.md` 1.3-1.5): C string literals, the prologues of emitted units, and
//! the small formatting rules every unit shares.

use std::fmt::Write as _;

/// A C string literal of `s`: printable ASCII as itself (`"`, `\`, and `?` escaped, the last against trigraphs),
/// every other byte as a three-digit octal escape, so that no escape can swallow a following hex digit and the
/// literal is valid in every C11 compiler whatever the source character set.
pub(crate) fn c_string(s: &str) -> String {
    c_bytes(s.as_bytes())
}

/// A C string literal of arbitrary bytes (see [`c_string`]).
pub(crate) fn c_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() + 2);
    out.push('"');
    for &b in bytes {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'?' => out.push_str("\\?"),
            0x20..=0x7e => out.push(b as char),
            _ => {
                let _ = write!(out, "\\{:03o}", b);
            }
        }
    }
    out.push('"');
    out
}

/// A C string literal, or `NULL` for none.
pub(crate) fn c_opt_string(s: Option<&str>) -> String {
    match s {
        Some(s) => c_string(s),
        None => "NULL".to_string(),
    }
}

/// `n` as a C `uint32_t` constant, `CPLRT_NONE` for `None`.
pub(crate) fn c_index(n: Option<u32>) -> String {
    match n {
        Some(n) => format!("{n}u"),
        None => "CPLRT_NONE".to_string(),
    }
}

/// The namespace prologue of `spec/cabi.md` Appendix B.1 for `prefix`: every unit that includes `cpl_rt.h` begins
/// with it, before its first `#include` (emitted units define no feature-test macro, 1.5).
pub(crate) fn prologue(prefix: &str) -> String {
    format!(
        "#define CPLRT_NS {prefix}\n#define CPLRT(name) cplrt_{prefix}__##name\n#define CPLPKG(name) cplpkg_{prefix}__##name\n"
    )
}

/// A comment's text with `*/` broken, so that CPL text quoted in a comment cannot end it, and every line break
/// replaced by a space.
pub(crate) fn comment_text(s: &str) -> String {
    s.replace("*/", "* /").replace(['\n', '\r'], " ")
}
