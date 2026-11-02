//! Small helpers shared by the manifest, lock, and loader modules.

use std::fmt::Write as _;

/// The 1-based line of a byte offset into `bytes`.
pub(crate) fn line_at(bytes: &[u8], offset: usize) -> u32 {
    let end = offset.min(bytes.len());
    bytes[..end].iter().filter(|&&b| b == b'\n').count() as u32 + 1
}

/// A TOML basic string: `"` and `\` escaped, control characters as escapes,
/// everything else (non-ASCII included) unchanged.
pub(crate) fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A TOML key: bare when it consists of ASCII letters, digits, `_`, and
/// `-`, otherwise a basic string.
pub(crate) fn key(s: &str) -> String {
    let bare = !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if bare {
        s.to_string()
    } else {
        quote(s)
    }
}

/// The dotted path of `name` inside the table at `path` (`""` is the root).
pub(crate) fn join_key(path: &str, name: &str) -> String {
    if path.is_empty() {
        key(name)
    } else {
        format!("{path}.{}", key(name))
    }
}
