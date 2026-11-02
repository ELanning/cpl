//! The JSON reader of `cpl run --arg JSON` (`spec/toolchain.md` 7.2, 7.3).
//!
//! `value_from_json` receives an already-parsed value, in which an object's
//! duplicate keys have collapsed and nesting has been bounded by the
//! parser, so this reader enforces both before decoding: a value nested
//! beyond `serde_json`'s recursion limit and an object with a duplicate key
//! are `usage_bad_argument`, like any text that is not one JSON value.
//! Numbers keep every digit (`arbitrary_precision`, Section 1.3).
//!
//! Reading of 7.2 ("a value nested deeper than the JSON reader's limit (128
//! levels for `serde_json`)"): the limit is the reader's, and `serde_json`'s
//! counter of 128 refuses the 128th level, so the deepest value accepted
//! has 127 levels of arrays and objects ([`MAX_DEPTH`]).

use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::fmt;

/// The deepest nesting of arrays and objects `serde_json`'s reader accepts:
/// its recursion limit of 128 refuses the 128th level (Section 7.2).
pub const MAX_DEPTH: usize = 127;

/// Why an `--arg` value was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JsonArgError {
    /// Not exactly one JSON value; serde_json's message.
    Syntax(String),
    /// Nested deeper than [`MAX_DEPTH`] levels (`serde_json`'s limit).
    TooDeep,
    /// An object with `key` twice; `pointer` is the RFC 6901 position of
    /// that object.
    Duplicate { pointer: String, key: String },
}

impl fmt::Display for JsonArgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JsonArgError::Syntax(message) => write!(f, "not valid JSON: {message}"),
            JsonArgError::TooDeep => write!(
                f,
                "nested too deeply: the JSON reader's limit is 128 levels (at most {MAX_DEPTH} levels of arrays and objects)"
            ),
            JsonArgError::Duplicate { pointer, key } => {
                let at = if pointer.is_empty() {
                    "the top-level object".to_string()
                } else {
                    format!("the object at {pointer}")
                };
                write!(f, "duplicate key {key:?} in {at}")
            }
        }
    }
}

/// Read one `--arg` value strictly.
pub fn read(text: &str) -> Result<serde_json::Value, JsonArgError> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|e| {
        if e.to_string().starts_with("recursion limit exceeded") {
            JsonArgError::TooDeep
        } else {
            JsonArgError::Syntax(e.to_string())
        }
    })?;
    let path = RefCell::new(Vec::new());
    let found = RefCell::new(None);
    let mut de = serde_json::Deserializer::from_str(text);
    let checked = Unique {
        path: &path,
        found: &found,
    }
    .deserialize(&mut de);
    match (checked, found.into_inner()) {
        (_, Some((pointer, key))) => Err(JsonArgError::Duplicate { pointer, key }),
        (Err(e), None) => Err(JsonArgError::Syntax(e.to_string())),
        (Ok(()), None) => Ok(value),
    }
}

/// RFC 6901: `/a/0/b`, with `~` and `/` escaped as `~0` and `~1`.
fn pointer(path: &[String]) -> String {
    path.iter()
        .map(|segment| format!("/{}", segment.replace('~', "~0").replace('/', "~1")))
        .collect()
}

/// A visitor that walks a JSON value and refuses an object with a repeated
/// key, recording the object's pointer and the key.
#[derive(Copy, Clone)]
struct Unique<'a> {
    path: &'a RefCell<Vec<String>>,
    found: &'a RefCell<Option<(String, String)>>,
}

impl<'de> DeserializeSeed<'de> for Unique<'_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Unique<'_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<(), E> {
        Ok(())
    }

    fn visit_i64<E: de::Error>(self, _: i64) -> Result<(), E> {
        Ok(())
    }

    fn visit_u64<E: de::Error>(self, _: u64) -> Result<(), E> {
        Ok(())
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> Result<(), E> {
        Ok(())
    }

    fn visit_str<E: de::Error>(self, _: &str) -> Result<(), E> {
        Ok(())
    }

    fn visit_unit<E: de::Error>(self) -> Result<(), E> {
        Ok(())
    }

    fn visit_none<E: de::Error>(self) -> Result<(), E> {
        Ok(())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        let mut index = 0usize;
        loop {
            self.path.borrow_mut().push(index.to_string());
            let more = seq.next_element_seed(self)?;
            self.path.borrow_mut().pop();
            if more.is_none() {
                return Ok(());
            }
            index += 1;
        }
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        // With `arbitrary_precision` a number arrives as a one-key map,
        // which has no duplicate to find.
        let mut keys = BTreeSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !keys.insert(key.clone()) {
                *self.found.borrow_mut() = Some((pointer(&self.path.borrow()), key));
                return Err(de::Error::custom("duplicate key"));
            }
            self.path.borrow_mut().push(key);
            map.next_value_seed(self)?;
            self.path.borrow_mut().pop();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn values_read_as_serde_json_reads_them() {
        assert_eq!(read("[1, 2, 3]"), Ok(json!([1, 2, 3])));
        assert_eq!(
            read(r#"{"$type": "m.P", "id": {"$payload": ["p"]}}"#),
            Ok(json!({"$type": "m.P", "id": {"$payload": ["p"]}}))
        );
        let big = read("123456789012345678901234567890").expect("a number");
        assert_eq!(big.to_string(), "123456789012345678901234567890");
        assert_eq!(read(" -5 "), Ok(json!(-5)));
    }

    #[test]
    fn duplicate_keys_are_refused_with_the_object_pointer() {
        assert_eq!(
            read(r#"{"a": 1, "a": 2}"#),
            Err(JsonArgError::Duplicate {
                pointer: String::new(),
                key: "a".into()
            })
        );
        assert_eq!(
            read(r#"[0, {"x/y": {"b": [1, {"k": 1, "k": 1}]}}]"#),
            Err(JsonArgError::Duplicate {
                pointer: "/1/x~1y/b/1".into(),
                key: "k".into()
            })
        );
        // Equal values under distinct keys, and repeated keys in sibling
        // objects, are fine.
        assert!(read(r#"[{"a": 1}, {"a": 1}]"#).is_ok());
    }

    #[test]
    fn nesting_beyond_the_limit_is_refused() {
        let deep = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
        assert!(read(&deep(MAX_DEPTH)).is_ok());
        assert_eq!(read(&deep(MAX_DEPTH + 1)), Err(JsonArgError::TooDeep));
        assert_eq!(read(&deep(100_000)), Err(JsonArgError::TooDeep));
    }

    #[test]
    fn other_text_is_a_syntax_error() {
        for text in ["", "[1,", "1 2", "{'a': 1}", "Ok(1)"] {
            assert!(
                matches!(read(text), Err(JsonArgError::Syntax(_))),
                "{text:?}"
            );
        }
    }
}
