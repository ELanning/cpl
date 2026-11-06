//! Value JSON and value text (`spec/toolchain.md` 7.3 and T30; semantics
//! S40): the two canonical renderings of a value, implemented once here.
//!
//! *Value JSON* is the round-trip form (`--arg`, `result`, `input`,
//! `actual`, `bindings`, `failing_case`, `shrunk`, receipts):
//!
//! | CPL type | JSON |
//! | - | - |
//! | `Int` (and a refined `Int`) | a number with every digit (`serde_json`'s `arbitrary_precision`); never a string |
//! | `Bool` | `true` / `false` |
//! | `Text` | a string |
//! | `Unit` | `{"$type": "Unit"}` |
//! | `Bytes` | `{"$type": "Bytes", "hex": "<lowercase hex>"}` |
//! | `List<T>` | an array |
//! | struct | `{"$type": "<name>", "<field>": ..., ...}` |
//! | enum, `Option`, `Result` | `{"$type": "<name>", "$variant": "<Variant>", "$payload": [...]}`, `$payload` omitted for a payload-less variant |
//! | nominal wrapper | `{"$type": "<name>", "$payload": [v]}` |
//! | capability handle | `{"$type": "<capability>", "$handle": n}`, output only |
//!
//! `<name>` is the bare name of a prelude type and the qualified name of
//! any other declaration (T30, [`crate::types::type_name`]).
//!
//! Every JSON object is a `serde_json::Map`, which the workspace builds as
//! a sorted map (T4: no `preserve_order`), so the keys of a struct object
//! serialize in sorted order (`$type` first, then the fields
//! alphabetically) rather than in declaration order as 7.3 says; the
//! object is the same JSON value and decoding never depends on key order.
//!
//! *Value text* is CPL literal syntax for humans (`cpl run`, messages):
//! `42`, `"t\n"` with the escapes of S40, `true`, `Unit`, `[1, 2]`,
//! `Page { id: PageId("p"), text: "t" }`, `Missing(PageId("p"))`,
//! `Unavailable`, `PageId("p")`, `Bytes("6869")`.
//!
//! Both encoders are value-directed: refinements are erased and every other
//! form is visible in the value itself, so the static type contributes only
//! the capability declaration of a handle; the `_untyped` variants serve
//! callers that hold no type (a builtin's `input`, a generic parameter).
//! Decoding is type-directed and strict: [`value_from_json`] checks the base
//! type of a refined position but not its predicate, which only the
//! evaluator can run ([`refined_positions`] lists the positions to check).

use crate::types::{bare_name, instantiate, strip, type_name};
use crate::value::Value;
use cpl_diag::quote_text;
use cpl_hir::{DefId, DefKind, Program, Ty};
use num_bigint::BigInt;
use num_traits::ToPrimitive;
use serde_json::{Map, Number, Value as Json};
use std::fmt::{self, Write as _};
use std::rc::Rc;

// ===========================================================================
// Integers
// ===========================================================================

/// An `Int` as a JSON number with every digit (toolchain 7.3: never a
/// string, any length).
pub fn int_to_json(n: &BigInt) -> Json {
    if let Some(small) = n.to_i64() {
        return Json::from(small);
    }
    let number: Number = n
        .to_string()
        .parse()
        .expect("a decimal integer is a JSON number (arbitrary_precision)");
    Json::Number(number)
}

/// The `Int` a JSON number denotes, or `None` when it has a fraction or an
/// exponent (toolchain 7.3: "decoding rejects fractions and exponents").
pub fn int_from_json(n: &Number) -> Option<BigInt> {
    let text = n.as_str();
    let digits = text.strip_prefix('-').unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

// ===========================================================================
// Value JSON: encoding
// ===========================================================================

/// The value JSON of `v`, whose static type is `ty` (toolchain 7.3,
/// semantics S40). The type matters only for a capability handle, whose
/// `$type` names its capability; see [`value_to_json_untyped`].
pub fn value_to_json(p: &Program, v: &Value, ty: &Ty) -> Json {
    match (v, ty) {
        (Value::Capability(h), Ty::Capability(cap)) => {
            let mut m = Map::new();
            m.insert("$handle".to_string(), Json::from(h.0));
            m.insert("$type".to_string(), Json::String(type_name(p, *cap)));
            Json::Object(m)
        }
        _ => value_to_json_untyped(p, v),
    }
}

/// The value JSON of `v` without a static type: identical to
/// [`value_to_json`] for every data value; a capability handle renders as
/// `{"$handle": n}` because only its type names the capability.
///
/// The encoder runs on an explicit work list, so a value nested deeper
/// than the host stack could recurse (a loop can build one, S38) encodes on
/// any thread.
pub fn value_to_json_untyped(p: &Program, v: &Value) -> Json {
    // Post-order: a value with children is entered (its children are queued
    // above it), then left (its JSON is assembled from its children's JSON,
    // the last entries of `done`).
    let mut done: Vec<Json> = Vec::new();
    let mut work: Vec<(&Value, bool)> = vec![(v, false)];
    while let Some((v, left)) = work.pop() {
        let kids = json_children(v);
        if !left && !kids.is_empty() {
            work.push((v, true));
            work.extend(kids.iter().rev().map(|kid| (kid, false)));
            continue;
        }
        let encoded = done.split_off(done.len() - kids.len());
        done.push(assemble_json(p, v, encoded));
    }
    done.pop().expect("the encoder leaves the root's JSON")
}

/// The values whose JSON is nested in `v`'s, in order.
fn json_children(v: &Value) -> &[Value] {
    match v {
        Value::List(xs) => xs,
        Value::Struct { fields, .. } => fields,
        Value::Enum { payload, .. } => payload,
        Value::Wrapper { inner, .. } => std::slice::from_ref(&**inner),
        _ => &[],
    }
}

/// The JSON of `v`, given the JSON of its [`json_children`] in order.
fn assemble_json(p: &Program, v: &Value, kids: Vec<Json>) -> Json {
    match v {
        Value::Int(n) => int_to_json(n),
        Value::Bool(b) => Json::Bool(*b),
        Value::Text(t) => Json::String(t.to_string()),
        Value::Bytes(b) => {
            let mut m = tagged("Bytes".to_string());
            m.insert("hex".to_string(), Json::String(hex(b)));
            Json::Object(m)
        }
        Value::List(_) => Json::Array(kids),
        Value::Struct { def, .. } => {
            let mut m = tagged(type_name(p, *def));
            for (i, f) in kids.into_iter().enumerate() {
                m.insert(field_name(p, *def, i), f);
            }
            Json::Object(m)
        }
        Value::Enum {
            def,
            variant,
            payload,
        } => {
            let mut m = tagged(type_name(p, *def));
            if *def == p.std_defs.unit && payload.is_empty() {
                return Json::Object(m);
            }
            m.insert(
                "$variant".to_string(),
                Json::String(variant_name(p, *def, *variant)),
            );
            if !payload.is_empty() {
                m.insert("$payload".to_string(), Json::Array(kids));
            }
            Json::Object(m)
        }
        Value::Wrapper { def, .. } => {
            let mut m = tagged(type_name(p, *def));
            m.insert("$payload".to_string(), Json::Array(kids));
            Json::Object(m)
        }
        Value::Capability(h) => {
            let mut m = Map::new();
            m.insert("$handle".to_string(), Json::from(h.0));
            Json::Object(m)
        }
    }
}

fn tagged(name: String) -> Map<String, Json> {
    let mut m = Map::new();
    m.insert("$type".to_string(), Json::String(name));
    m
}

/// The name of field `index` of struct `def` (`<field N>` for a value that
/// does not match its declaration, an internal error elsewhere).
fn field_name(p: &Program, def: DefId, index: usize) -> String {
    p.defs
        .get(def.0 as usize)
        .and_then(|d| d.kind.as_struct())
        .and_then(|s| s.fields.get(index))
        .map_or_else(|| format!("<field {index}>"), |f| f.name.clone())
}

/// The name of variant `index` of enum `def` (`<variant N>` when the value
/// does not match its declaration).
fn variant_name(p: &Program, def: DefId, index: u32) -> String {
    p.defs
        .get(def.0 as usize)
        .and_then(|d| d.kind.as_enum())
        .and_then(|e| e.variants.get(index as usize))
        .map_or_else(|| format!("<variant {index}>"), |v| v.name.clone())
}

// ===========================================================================
// Value text
// ===========================================================================

/// The value text of `v`, whose static type is `ty` (toolchain 7.3,
/// semantics S40): CPL literal syntax. Like [`value_to_json`], only a
/// capability handle uses the type (`<handle 3: reader.pages.PageRead>`).
pub fn value_text(p: &Program, v: &Value, ty: &Ty) -> String {
    match (v, ty) {
        (Value::Capability(h), Ty::Capability(cap)) => {
            format!("<handle {}: {}>", h.0, type_name(p, *cap))
        }
        _ => value_text_untyped(p, v),
    }
}

/// The value text of `v` without a static type (a handle renders as
/// `<handle n>`).
pub fn value_text_untyped(p: &Program, v: &Value) -> String {
    let mut out = String::new();
    write_text(p, v, &mut out);
    out
}

/// One step of [`write_text`]: a value to write, or literal text.
enum Piece<'v> {
    Value(&'v Value),
    Static(&'static str),
    Owned(String),
}

/// The value text of `v`, appended to `out`, on an explicit work list (a
/// deeply nested value renders on any thread, like
/// [`value_to_json_untyped`]).
fn write_text(p: &Program, v: &Value, out: &mut String) {
    let mut work = vec![Piece::Value(v)];
    while let Some(piece) = work.pop() {
        let v = match piece {
            Piece::Static(s) => {
                out.push_str(s);
                continue;
            }
            Piece::Owned(s) => {
                out.push_str(&s);
                continue;
            }
            Piece::Value(v) => v,
        };
        match v {
            Value::Int(n) => {
                let _ = write!(out, "{n}");
            }
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Text(t) => out.push_str(&quote_text(t)),
            Value::Bytes(b) => {
                out.push_str("Bytes(\"");
                out.push_str(&hex(b));
                out.push_str("\")");
            }
            Value::List(xs) => {
                out.push('[');
                work.push(Piece::Static("]"));
                push_items(&mut work, xs);
            }
            Value::Struct { def, fields } => {
                out.push_str(&bare_name(p, *def));
                if fields.is_empty() {
                    out.push_str(" {}");
                    continue;
                }
                out.push_str(" { ");
                work.push(Piece::Static(" }"));
                for (i, f) in fields.iter().enumerate().rev() {
                    work.push(Piece::Value(f));
                    work.push(Piece::Owned(format!("{}: ", field_name(p, *def, i))));
                    if i > 0 {
                        work.push(Piece::Static(", "));
                    }
                }
            }
            Value::Enum {
                def,
                variant,
                payload,
            } => {
                out.push_str(&variant_name(p, *def, *variant));
                if !payload.is_empty() {
                    out.push('(');
                    work.push(Piece::Static(")"));
                    push_items(&mut work, payload);
                }
            }
            Value::Wrapper { def, inner } => {
                out.push_str(&bare_name(p, *def));
                out.push('(');
                work.push(Piece::Static(")"));
                work.push(Piece::Value(inner));
            }
            Value::Capability(h) => {
                let _ = write!(out, "<handle {}>", h.0);
            }
        }
    }
}

/// Queue `items` separated by `", "` so that they pop in order.
fn push_items<'v>(work: &mut Vec<Piece<'v>>, items: &'v [Value]) {
    for (i, x) in items.iter().enumerate().rev() {
        work.push(Piece::Value(x));
        if i > 0 {
            work.push(Piece::Static(", "));
        }
    }
}

// ===========================================================================
// Value JSON: decoding
// ===========================================================================

/// Why a JSON document is not a value of the expected type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArgError {
    /// An RFC 6901 JSON pointer into the decoded document: `""` for the
    /// whole document, `"/1/$payload/0/x"` for a nested position.
    pub pointer: String,
    pub message: String,
}

impl fmt::Display for ArgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.pointer.is_empty() {
            f.write_str(&self.message)
        } else {
            write!(f, "{} (at {})", self.message, self.pointer)
        }
    }
}

impl std::error::Error for ArgError {}

/// Decode value JSON against the type `ty` (toolchain 7.3), strictly: an
/// `Int` must be a JSON number without fraction or exponent; `Text` a JSON
/// string (`serde_json` has already rejected unpaired surrogates such as
/// `"\ud800"` when it parsed the document); `Bytes` exactly
/// `{"$type": "Bytes", "hex": ...}` with an even number of lowercase hex
/// digits; `Unit` exactly `{"$type": "Unit"}`; every tagged object must
/// carry the expected `$type` and no key beyond its form's (a struct needs
/// every field exactly once, an enum's `$payload` has the variant's arity
/// and is absent for a payload-less variant, a wrapper's `$payload` has one
/// element). A refined position is decoded at its root base; its
/// predicates are *not* evaluated here (the caller checks every position
/// [`refined_positions`] lists, `refinement_violation` for `cpl run`).
/// Generic parameters, capabilities, and generators have no decodable form.
pub fn value_from_json(p: &Program, j: &Json, ty: &Ty) -> Result<Value, ArgError> {
    Decoder {
        p,
        pointer: Vec::new(),
    }
    .decode(j, ty)
}

struct Decoder<'p> {
    p: &'p Program,
    pointer: Vec<String>,
}

/// A short description of a JSON value for messages.
fn describe(j: &Json) -> String {
    match j {
        Json::Null => "null".to_string(),
        Json::Bool(b) => format!("the boolean {b}"),
        Json::Number(n) => format!("the number {n}"),
        Json::String(_) => "a string".to_string(),
        Json::Array(_) => "an array".to_string(),
        Json::Object(m) => match m.get("$type").and_then(Json::as_str) {
            Some(t) => format!("an object with \"$type\" {}", Json::String(t.to_string())),
            None => "an object".to_string(),
        },
    }
}

fn escape_segment(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

impl Decoder<'_> {
    fn error(&self, message: impl Into<String>) -> ArgError {
        let mut pointer = String::new();
        for segment in &self.pointer {
            pointer.push('/');
            pointer.push_str(&escape_segment(segment));
        }
        ArgError {
            pointer,
            message: message.into(),
        }
    }

    /// Run `f` with `segment` appended to the pointer.
    fn at<T>(
        &mut self,
        segment: impl Into<String>,
        f: impl FnOnce(&mut Self) -> Result<T, ArgError>,
    ) -> Result<T, ArgError> {
        self.pointer.push(segment.into());
        let result = f(self);
        self.pointer.pop();
        result
    }

    fn expected(&self, ty: &Ty, form: &str, found: &Json) -> ArgError {
        self.error(format!(
            "expected `{}` ({form}), found {}",
            self.p.display_ty(ty),
            describe(found)
        ))
    }

    fn decode(&mut self, j: &Json, ty: &Ty) -> Result<Value, ArgError> {
        match ty {
            Ty::Int => match j {
                Json::Number(n) => int_from_json(n).map(Value::Int).ok_or_else(|| {
                    self.error(format!(
                        "expected `Int`, found the number {n}: an Int has no fraction or exponent"
                    ))
                }),
                other => Err(self.expected(ty, "a JSON number", other)),
            },
            Ty::Bool => match j {
                Json::Bool(b) => Ok(Value::Bool(*b)),
                other => Err(self.expected(ty, "true or false", other)),
            },
            Ty::Text => match j {
                Json::String(s) => Ok(Value::text(s)),
                other => Err(self.expected(ty, "a JSON string", other)),
            },
            Ty::Bytes => self.bytes(j, ty),
            Ty::List(elem) => {
                let Json::Array(items) = j else {
                    return Err(self.expected(ty, "a JSON array", j));
                };
                let mut out = Vec::with_capacity(items.len());
                for (i, item) in items.iter().enumerate() {
                    out.push(self.at(i.to_string(), |d| d.decode(item, elem))?);
                }
                Ok(Value::list(out))
            }
            Ty::Adt(def, args) => self.adt(j, *def, args, ty),
            Ty::Wrapper(def) => self.wrapper(j, *def, ty),
            Ty::Refined(..) => match strip(self.p, ty) {
                Ty::Error => Err(self.error(format!(
                    "the refinement `{}` has no base type",
                    self.p.display_ty(ty)
                ))),
                base => self.decode(j, &base),
            },
            Ty::Param(_) => Err(self.error(
                "a value of a generic type parameter cannot be decoded; run a monomorphic wrapper",
            )),
            Ty::Capability(_) => Err(self.error(format!(
                "a capability handle (`{}`) cannot be supplied as JSON",
                self.p.display_ty(ty)
            ))),
            Ty::Gen(_) => Err(self.error("a generator is not a value")),
            Ty::Error => Err(self.error("the expected type has errors")),
        }
    }

    /// The object of a tagged form, its `$type` checked against `tag`.
    fn tagged_object<'j>(
        &mut self,
        j: &'j Json,
        tag: &str,
        ty: &Ty,
        form: &str,
    ) -> Result<&'j Map<String, Json>, ArgError> {
        let Json::Object(obj) = j else {
            return Err(self.expected(ty, form, j));
        };
        match obj.get("$type") {
            None => Err(self.error(format!(
                "missing \"$type\": expected {}",
                Json::String(tag.to_string())
            ))),
            Some(Json::String(found)) if found == tag => Ok(obj),
            Some(found) => {
                let found = found.clone();
                self.at("$type", |d| {
                    Err(d.error(format!(
                        "expected \"$type\" {}, found {}",
                        Json::String(tag.to_string()),
                        match &found {
                            Json::String(_) => found.to_string(),
                            other => describe(other),
                        }
                    )))
                })
            }
        }
    }

    /// Reject the first key of `obj` that `allowed` refuses.
    fn only_keys(
        &mut self,
        obj: &Map<String, Json>,
        what: &str,
        allowed: impl Fn(&str) -> bool,
    ) -> Result<(), ArgError> {
        match obj.keys().find(|k| !allowed(k)) {
            Some(key) => {
                let key = key.clone();
                self.at(key.clone(), |d| {
                    Err(d.error(format!("unexpected key {} in {what}", Json::String(key))))
                })
            }
            None => Ok(()),
        }
    }

    fn bytes(&mut self, j: &Json, ty: &Ty) -> Result<Value, ArgError> {
        let obj = self.tagged_object(j, "Bytes", ty, "{\"$type\": \"Bytes\", \"hex\": ...}")?;
        self.only_keys(obj, "a Bytes value", |k| k == "$type" || k == "hex")?;
        let Some(hex) = obj.get("hex") else {
            return Err(self.error("missing \"hex\" of a Bytes value"));
        };
        self.at("hex", |d| {
            let Json::String(text) = hex else {
                return Err(d.error(format!(
                    "expected a string of lowercase hex digits, found {}",
                    describe(hex)
                )));
            };
            if text.len() % 2 != 0 {
                return Err(d.error("expected an even number of hex digits"));
            }
            let digit = |c: u8| match c {
                b'0'..=b'9' => Some(c - b'0'),
                b'a'..=b'f' => Some(c - b'a' + 10),
                _ => None,
            };
            let mut out = Vec::with_capacity(text.len() / 2);
            for pair in text.as_bytes().chunks(2) {
                match (digit(pair[0]), digit(pair[1])) {
                    (Some(hi), Some(lo)) => out.push(hi * 16 + lo),
                    _ => {
                        return Err(d.error(format!(
                            "expected lowercase hex digits (0-9, a-f), found {}",
                            String::from_utf8_lossy(pair)
                        )))
                    }
                }
            }
            Ok(Value::Bytes(Rc::from(out)))
        })
    }

    fn adt(&mut self, j: &Json, def: DefId, args: &[Ty], ty: &Ty) -> Result<Value, ArgError> {
        let p = self.p;
        let Some(d) = p.defs.get(def.0 as usize) else {
            return Err(self.error(format!("the program has no declaration {}", def.0)));
        };
        let tag = type_name(p, def);
        match &d.kind {
            DefKind::Struct(s) => {
                let form = format!("an object with \"$type\" {}", Json::String(tag.clone()));
                let obj = self.tagged_object(j, &tag, ty, &form)?;
                let what = format!("a `{tag}` struct");
                self.only_keys(obj, &what, |k| {
                    k == "$type" || s.fields.iter().any(|f| f.name == k)
                })?;
                let mut fields = Vec::with_capacity(s.fields.len());
                for field in &s.fields {
                    let Some(fj) = obj.get(&field.name) else {
                        return Err(self.error(format!(
                            "missing field {} of `{tag}`",
                            Json::String(field.name.clone())
                        )));
                    };
                    let fty = instantiate(&field.ty, args);
                    fields.push(self.at(field.name.clone(), |d| d.decode(fj, &fty))?);
                }
                Ok(Value::Struct {
                    def,
                    fields: Rc::from(fields),
                })
            }
            DefKind::Enum(e) => {
                let form = format!("an object with \"$type\" {}", Json::String(tag.clone()));
                let obj = self.tagged_object(j, &tag, ty, &form)?;
                if def == p.std_defs.unit {
                    self.only_keys(obj, "the Unit value {\"$type\": \"Unit\"}", |k| {
                        k == "$type"
                    })?;
                    return Ok(Value::unit(p));
                }
                let what = format!("a `{tag}` value");
                self.only_keys(obj, &what, |k| {
                    k == "$type" || k == "$variant" || k == "$payload"
                })?;
                let name = match obj.get("$variant") {
                    None => return Err(self.error(format!("missing \"$variant\" of `{tag}`"))),
                    Some(Json::String(name)) => name,
                    Some(other) => {
                        let found = describe(other);
                        return self.at("$variant", |d| {
                            Err(d.error(format!("expected a variant name, found {found}")))
                        });
                    }
                };
                let Some(index) = e.variants.iter().position(|v| &v.name == name) else {
                    let known: Vec<&str> = e.variants.iter().map(|v| v.name.as_str()).collect();
                    let name = name.clone();
                    return self.at("$variant", |d| {
                        Err(d.error(format!(
                            "`{tag}` has no variant {}; its variants are {}",
                            Json::String(name),
                            known.join(", ")
                        )))
                    });
                };
                let variant = &e.variants[index];
                let payload = match obj.get("$payload") {
                    None if variant.payload.is_empty() => Vec::new(),
                    None => {
                        return Err(self.error(format!(
                            "missing \"$payload\" of the variant {} ({} values)",
                            variant.name,
                            variant.payload.len()
                        )))
                    }
                    Some(_) if variant.payload.is_empty() => {
                        let vname = variant.name.clone();
                        return self.at("$payload", |d| {
                            Err(d.error(format!(
                                "the variant {vname} has no payload; omit \"$payload\""
                            )))
                        });
                    }
                    Some(pj) => self.at("$payload", |d| d.payload(pj, &variant.payload, args))?,
                };
                Ok(Value::Enum {
                    def,
                    variant: index as u32,
                    payload: Rc::from(payload),
                })
            }
            _ => Err(self.error(format!(
                "`{}` is not a struct or enum type",
                p.display_ty(ty)
            ))),
        }
    }

    /// A `$payload` array of exactly the given types.
    fn payload(&mut self, j: &Json, types: &[Ty], args: &[Ty]) -> Result<Vec<Value>, ArgError> {
        let Json::Array(items) = j else {
            return Err(self.error(format!(
                "expected an array of {} values, found {}",
                types.len(),
                describe(j)
            )));
        };
        if items.len() != types.len() {
            return Err(self.error(format!(
                "expected {} payload values, found {}",
                types.len(),
                items.len()
            )));
        }
        let mut out = Vec::with_capacity(items.len());
        for (i, (item, t)) in items.iter().zip(types).enumerate() {
            let t = instantiate(t, args);
            out.push(self.at(i.to_string(), |d| d.decode(item, &t))?);
        }
        Ok(out)
    }

    fn wrapper(&mut self, j: &Json, def: DefId, ty: &Ty) -> Result<Value, ArgError> {
        let p = self.p;
        let Some(w) = p.defs.get(def.0 as usize).and_then(|d| d.kind.as_wrapper()) else {
            return Err(self.error(format!("`{}` is not a wrapper type", p.display_ty(ty))));
        };
        let tag = type_name(p, def);
        let form = format!("an object with \"$type\" {}", Json::String(tag.clone()));
        let obj = self.tagged_object(j, &tag, ty, &form)?;
        let what = format!("a `{tag}` value");
        self.only_keys(obj, &what, |k| k == "$type" || k == "$payload")?;
        let Some(pj) = obj.get("$payload") else {
            return Err(self.error(format!("missing \"$payload\" of `{tag}`")));
        };
        let inner_ty = w.inner.clone();
        let mut inner = self.at("$payload", |d| {
            d.payload(pj, std::slice::from_ref(&inner_ty), &[])
        })?;
        Ok(Value::Wrapper {
            def,
            inner: Rc::new(inner.remove(0)),
        })
    }
}

// ===========================================================================
// Refined positions
// ===========================================================================

/// A position of a value whose static type is a refinement: what decoding
/// leaves for the evaluator to check (toolchain 7.3: "decoding evaluates
/// the predicate and rejects values that fail it"; semantics S39 (a)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefinedPosition {
    /// The JSON pointer of the position in the value's JSON (`""` for the
    /// value itself), the same pointer [`value_from_json`] reports.
    pub pointer: String,
    /// The refinement declaration as the type names it there; its whole
    /// chain must hold (`Interpreter::satisfies_refinement`).
    pub refined: DefId,
    /// The (base) value at that position.
    pub value: Value,
}

/// Every refined position of `v` at type `ty`, in the order a boundary
/// checks them: siblings in value order (list elements by index, struct
/// fields in declaration order, payloads by position), and the positions
/// inside a refinement's base before the refinement itself (post-order).
/// Values that do not match `ty` contribute nothing.
///
/// The specifications fix the order within one chain (root first, semantics
/// S39 and 6.10) but not between a refinement and the refined positions
/// inside its base. A predicate may rely on its preceding assumptions
/// (design: contract expressions are "pure and total under their preceding
/// assumptions"), and a value of the base type exists only once its own
/// positions hold: for `type Wide = Rect where self.w / self.h > 1` over
/// `Rect { w: Positive  h: Positive }`, an argument whose `h` is 0 is
/// reported at `/h` (`Positive`) rather than as a division by zero blamed on
/// `Wide`'s predicate, which no `Rect` built by CPL code can make fault.
/// This is the root-first rule of 6.10 applied to a value's structure.
pub fn refined_positions(p: &Program, v: &Value, ty: &Ty) -> Vec<RefinedPosition> {
    let mut out = Vec::new();
    let mut pointer = Vec::new();
    collect_refined(p, v, ty, &mut pointer, &mut out);
    out
}

fn collect_refined(
    p: &Program,
    v: &Value,
    ty: &Ty,
    pointer: &mut Vec<String>,
    out: &mut Vec<RefinedPosition>,
) {
    let render = |pointer: &[String]| {
        let mut s = String::new();
        for segment in pointer {
            s.push('/');
            s.push_str(&escape_segment(segment));
        }
        s
    };
    match ty {
        Ty::Refined(_, def) => {
            // The base value is established first (see `refined_positions`).
            let base = strip(p, ty);
            if !matches!(base, Ty::Refined(..) | Ty::Error) {
                collect_refined(p, v, &base, pointer, out);
            }
            out.push(RefinedPosition {
                pointer: render(pointer),
                refined: *def,
                value: v.clone(),
            });
        }
        Ty::List(elem) => {
            if let Value::List(xs) = v {
                for (i, x) in xs.iter().enumerate() {
                    pointer.push(i.to_string());
                    collect_refined(p, x, elem, pointer, out);
                    pointer.pop();
                }
            }
        }
        Ty::Adt(def, args) => match (p.defs.get(def.0 as usize).map(|d| &d.kind), v) {
            (Some(DefKind::Struct(s)), Value::Struct { fields, .. }) => {
                for (field, value) in s.fields.iter().zip(fields.iter()) {
                    pointer.push(field.name.clone());
                    collect_refined(p, value, &instantiate(&field.ty, args), pointer, out);
                    pointer.pop();
                }
                if s.invariant.is_some() {
                    out.push(RefinedPosition {
                        pointer: render(pointer),
                        refined: *def,
                        value: v.clone(),
                    });
                }
            }
            (
                Some(DefKind::Enum(e)),
                Value::Enum {
                    variant, payload, ..
                },
            ) => {
                if let Some(vd) = e.variants.get(*variant as usize) {
                    for (i, (t, value)) in vd.payload.iter().zip(payload.iter()).enumerate() {
                        pointer.push("$payload".to_string());
                        pointer.push(i.to_string());
                        collect_refined(p, value, &instantiate(t, args), pointer, out);
                        pointer.pop();
                        pointer.pop();
                    }
                }
            }
            _ => {}
        },
        Ty::Wrapper(def) => {
            if let (Some(w), Value::Wrapper { inner, .. }) = (
                p.defs.get(def.0 as usize).and_then(|d| d.kind.as_wrapper()),
                v,
            ) {
                pointer.push("$payload".to_string());
                pointer.push("0".to_string());
                collect_refined(p, inner, &w.inner, pointer, out);
                pointer.pop();
                pointer.pop();
            }
        }
        _ => {}
    }
}
