//! Runtime values (`spec/toolchain.md` 1.7 and T8; semantics 6.1, 6.4, 6.5;
//! stdlib 10.5).
//!
//! Values are immutable and share structure through [`Rc`] (T8): uniquely owned local text, bytes, and lists may reuse their buffers; shared snapshots copy on write, so no value is ever
//! observed to change (semantics 6.1) and sharing is never visible.
//! Refinements are erased: a `Positive` is a [`Value::Int`] (semantics 6.1,
//! stdlib 10.5). `Unit`, `Option`, and `Result` are the prelude's ordinary
//! enums ([`Value::Enum`]); the constructors below find their variants by
//! name in the program's declarations, so they agree with
//! `std/prelude.cpl` whatever its declaration order.

use crate::interp::HandleId;
use cpl_hir::{DefId, Program};
use num_bigint::BigInt;
use std::rc::Rc;

/// A runtime value of the reference evaluator (`spec/toolchain.md` 1.7).
///
/// `PartialEq` is [`Value::structurally_equal`], the `==` of semantics 6.5.
/// A value implements [`Drop`] so that dropping one never recurses with
/// its nesting depth (see the impl); a pattern therefore cannot move a
/// field out of an owned value: match on a reference and clone the part
/// (an `Rc` clone is a count increment), or use [`Value::into_int`].
#[derive(Clone, Debug)]
pub enum Value {
    /// `Int` and every refinement of it (refinements are erased).
    Int(BigInt),
    /// `Bool`.
    Bool(bool),
    /// `Text`: UTF-8 with capacity retained for uniquely owned local appends.
    Text(Rc<String>),
    /// `Bytes`, with capacity retained for uniquely owned local appends.
    Bytes(Rc<Vec<u8>>),
    /// `List<T>`: a view of a shared buffer ([`ListRef`]), with capacity retained for uniquely
    /// owned local appends.
    List(ListRef),
    /// A struct value; `fields` in declaration order.
    Struct { def: DefId, fields: Rc<[Value]> },
    /// An enum value, the prelude's `Unit`, `Option`, and `Result`
    /// included; `variant` indexes the declaration's variants.
    Enum {
        def: DefId,
        variant: u32,
        payload: Rc<[Value]>,
    },
    /// A nominal wrapper value such as `PageId("x")`, distinct from its
    /// payload (semantics 6.1).
    Wrapper { def: DefId, inner: Rc<Value> },
    /// A host-provided capability handle (none exist in Milestone 1,
    /// semantics 6.9).
    Capability(HandleId),
}

/// The index of the variant called `name` of the enum `def`, or `fallback`
/// when the program does not declare such an enum (a malformed program).
pub(crate) fn variant_index(p: &Program, def: DefId, name: &str, fallback: u32) -> u32 {
    p.defs
        .get(def.0 as usize)
        .and_then(|d| d.kind.as_enum())
        .and_then(|e| e.variants.iter().position(|v| v.name == name))
        .map_or(fallback, |i| i as u32)
}

fn empty() -> Rc<[Value]> {
    Rc::from(Vec::new())
}

impl Value {
    // ---- plain constructors ------------------------------------------------

    /// An `Int`.
    pub fn int(n: impl Into<BigInt>) -> Value {
        Value::Int(n.into())
    }

    /// A `Text`.
    pub fn text(s: &str) -> Value {
        Value::Text(Rc::new(s.to_owned()))
    }

    /// A `Bytes`.
    pub fn bytes(b: &[u8]) -> Value {
        Value::Bytes(Rc::new(b.to_vec()))
    }

    /// A `List`.
    pub fn list(items: Vec<Value>) -> Value {
        Value::List(ListRef::new(items))
    }

    /// The empty `List`.
    pub fn empty_list() -> Value {
        Value::list(Vec::new())
    }

    /// A struct value with `fields` in declaration order.
    pub fn new_struct(def: DefId, fields: Vec<Value>) -> Value {
        Value::Struct {
            def,
            fields: Rc::from(fields),
        }
    }

    /// An enum value of variant index `variant`.
    pub fn new_enum(def: DefId, variant: u32, payload: Vec<Value>) -> Value {
        Value::Enum {
            def,
            variant,
            payload: Rc::from(payload),
        }
    }

    /// The enum value of the variant called `name`, or `None` when `def` is
    /// not an enum with such a variant.
    pub fn variant(p: &Program, def: DefId, name: &str, payload: Vec<Value>) -> Option<Value> {
        let e = p.defs.get(def.0 as usize)?.kind.as_enum()?;
        let index = e.variants.iter().position(|v| v.name == name)?;
        Some(Value::new_enum(def, index as u32, payload))
    }

    /// A nominal wrapper value such as `PageId("x")`.
    pub fn wrap(def: DefId, inner: Value) -> Value {
        Value::Wrapper {
            def,
            inner: Rc::new(inner),
        }
    }

    // ---- the prelude enums (Section 3.6: ordinary enums) -------------------

    /// `Unit`: `Enum { def: std_defs.unit, variant: 0, payload: [] }`.
    pub fn unit(p: &Program) -> Value {
        let def = p.std_defs.unit;
        Value::Enum {
            def,
            variant: variant_index(p, def, "Unit", 0),
            payload: empty(),
        }
    }

    /// `Some(v)`.
    pub fn some(p: &Program, v: Value) -> Value {
        let def = p.std_defs.option;
        Value::new_enum(def, variant_index(p, def, "Some", 0), vec![v])
    }

    /// `None`.
    pub fn none(p: &Program) -> Value {
        let def = p.std_defs.option;
        Value::Enum {
            def,
            variant: variant_index(p, def, "None", 1),
            payload: empty(),
        }
    }

    /// `Ok(v)`.
    pub fn ok(p: &Program, v: Value) -> Value {
        let def = p.std_defs.result;
        Value::new_enum(def, variant_index(p, def, "Ok", 0), vec![v])
    }

    /// `Err(e)`.
    pub fn err(p: &Program, e: Value) -> Value {
        let def = p.std_defs.result;
        Value::new_enum(def, variant_index(p, def, "Err", 1), vec![e])
    }

    /// `Some(Ok(v))` or `Some(Err(e))` for a `Result` value; `None` for any
    /// other value.
    pub fn as_result<'a>(&'a self, p: &Program) -> Option<Result<&'a Value, &'a Value>> {
        let Value::Enum {
            def,
            variant,
            payload,
        } = self
        else {
            return None;
        };
        if *def != p.std_defs.result || payload.len() != 1 {
            return None;
        }
        if *variant == variant_index(p, *def, "Ok", 0) {
            Some(Ok(&payload[0]))
        } else if *variant == variant_index(p, *def, "Err", 1) {
            Some(Err(&payload[0]))
        } else {
            None
        }
    }

    /// `Some(Some(v))` or `Some(None)` for an `Option` value; `None` for any
    /// other value.
    pub fn as_option<'a>(&'a self, p: &Program) -> Option<Option<&'a Value>> {
        let Value::Enum {
            def,
            variant,
            payload,
        } = self
        else {
            return None;
        };
        if *def != p.std_defs.option {
            return None;
        }
        if *variant == variant_index(p, *def, "Some", 0) && payload.len() == 1 {
            Some(Some(&payload[0]))
        } else if *variant == variant_index(p, *def, "None", 1) && payload.is_empty() {
            Some(None)
        } else {
            None
        }
    }

    /// True for the value `Unit`.
    pub fn is_unit(&self, p: &Program) -> bool {
        matches!(self, Value::Enum { def, payload, .. } if *def == p.std_defs.unit && payload.is_empty())
    }

    // ---- accessors -----------------------------------------------------------

    pub fn as_int(&self) -> Option<&BigInt> {
        match self {
            Value::Int(n) => Some(n),
            _ => None,
        }
    }

    /// The `Int` by value, or the value itself when it is not an `Int`. A
    /// `Value` implements [`Drop`] (it tears nested buffers down without
    /// recursion), so a pattern cannot move a field out of an owned value:
    /// match on a reference and clone the part (an `Rc` clone is a count
    /// increment), or use this for the one payload that is not shared.
    pub fn into_int(mut self) -> Result<BigInt, Value> {
        match &mut self {
            Value::Int(n) => Ok(std::mem::take(n)),
            _ => Err(self),
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Value::Text(t) => Some(t),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Bytes(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(xs) => Some(xs),
            _ => None,
        }
    }

    /// The value's form, for internal-error messages: `"Int"`, `"Bool"`,
    /// `"Text"`, `"Bytes"`, `"List"`, `"struct"`, `"enum"`, `"wrapper"`,
    /// `"capability"`.
    pub fn kind_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "Int",
            Value::Bool(_) => "Bool",
            Value::Text(_) => "Text",
            Value::Bytes(_) => "Bytes",
            Value::List(_) => "List",
            Value::Struct { .. } => "struct",
            Value::Enum { .. } => "enum",
            Value::Wrapper { .. } => "wrapper",
            Value::Capability(_) => "capability",
        }
    }

    // ---- operations ------------------------------------------------------------

    /// Structural equality on the erased value (semantics 6.5): numbers by
    /// value; texts as scalar sequences with no normalization; bytes
    /// bytewise; lists, struct fields, and enum payloads pairwise in order
    /// (enums first by variant); wrappers by payload. Values of different
    /// declarations are never equal (a static type error anyway). Never
    /// called on capabilities, which have no `==` (rejected statically);
    /// two handles compare by identity.
    ///
    /// The comparison runs on an explicit stack of remaining slices, so
    /// deeply nested values cannot exhaust the host stack and a wide list
    /// is visited only as far as its first difference. Scalars and shared
    /// buffers need no work allocation.
    pub fn structurally_equal(&self, other: &Value) -> bool {
        let mut work: Vec<(&[Value], &[Value])> = Vec::new();
        let (mut a, mut b) = (self, other);
        loop {
            let same = match (a, b) {
                (Value::Int(x), Value::Int(y)) => x == y,
                (Value::Bool(x), Value::Bool(y)) => x == y,
                (Value::Text(x), Value::Text(y)) => x == y,
                (Value::Bytes(x), Value::Bytes(y)) => x == y,
                (Value::List(xs), Value::List(ys)) => pairwise(xs, ys, xs.same(ys), &mut work),
                (
                    Value::Struct {
                        def: d1,
                        fields: f1,
                    },
                    Value::Struct {
                        def: d2,
                        fields: f2,
                    },
                ) => d1 == d2 && pairwise(f1, f2, Rc::ptr_eq(f1, f2), &mut work),
                (
                    Value::Enum {
                        def: d1,
                        variant: v1,
                        payload: p1,
                    },
                    Value::Enum {
                        def: d2,
                        variant: v2,
                        payload: p2,
                    },
                ) => d1 == d2 && v1 == v2 && pairwise(p1, p2, Rc::ptr_eq(p1, p2), &mut work),
                (Value::Wrapper { def: d1, inner: i1 }, Value::Wrapper { def: d2, inner: i2 }) => {
                    if d1 == d2 && !Rc::ptr_eq(i1, i2) {
                        work.push((std::slice::from_ref(&**i1), std::slice::from_ref(&**i2)));
                    }
                    d1 == d2
                }
                (Value::Capability(h1), Value::Capability(h2)) => h1 == h2,
                _ => false,
            };
            if !same {
                return false;
            }
            let Some((xs, ys)) = work.pop() else {
                return true;
            };
            // Only nonempty, equal-length slices enter the stack. Defer
            // their remaining siblings while visiting the next pair.
            a = &xs[0];
            b = &ys[0];
            if xs.len() > 1 {
                work.push((&xs[1..], &ys[1..]));
            }
        }
    }

    /// `a ++ b` on `List`, `Text`, and `Bytes` (stdlib 3.3, 3.4, 4.2; S12):
    /// the elements of `self` followed by those of `other`. `None` when the
    /// operands are not two values of one of those forms (a type confusion
    /// the interpreter reports as `internal_error`).
    pub fn concat(&self, other: &Value) -> Option<Value> {
        match (self, other) {
            (Value::List(a), Value::List(b)) => Some(if b.is_empty() {
                self.clone()
            } else if a.is_empty() {
                other.clone()
            } else {
                Value::list(a.iter().chain(b.iter()).cloned().collect())
            }),
            (Value::Text(a), Value::Text(b)) => Some(if b.is_empty() {
                self.clone()
            } else if a.is_empty() {
                other.clone()
            } else {
                let mut s = String::with_capacity(a.len() + b.len());
                s.push_str(a);
                s.push_str(b);
                Value::Text(Rc::from(s))
            }),
            (Value::Bytes(a), Value::Bytes(b)) => Some(if b.is_empty() {
                self.clone()
            } else if a.is_empty() {
                other.clone()
            } else {
                let mut v = Vec::with_capacity(a.len() + b.len());
                v.extend_from_slice(a);
                v.extend_from_slice(b);
                Value::Bytes(Rc::from(v))
            }),
            _ => None,
        }
    }

    /// Whether two sequence values share their backing allocation, for assigning `x = x ++ rhs` after rhs runs.
    pub(crate) fn same_buffer(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Text(a), Value::Text(b)) => Rc::ptr_eq(a, b),
            (Value::Bytes(a), Value::Bytes(b)) => Rc::ptr_eq(a, b),
            (Value::List(a), Value::List(b)) => a.same(b),
            _ => false,
        }
    }

    /// Append into an owned sequence, preserving every other reference through copy on write.
    pub(crate) fn append(&mut self, other: &Value) -> bool {
        match (self, other) {
            (Value::Text(a), Value::Text(b)) => {
                if a.is_empty() {
                    *a = Rc::clone(b);
                } else if !b.is_empty() {
                    Rc::make_mut(a).push_str(b);
                }
            }
            (Value::Bytes(a), Value::Bytes(b)) => {
                if a.is_empty() {
                    *a = Rc::clone(b);
                } else if !b.is_empty() {
                    Rc::make_mut(a).extend_from_slice(b);
                }
            }
            (Value::List(a), Value::List(b)) => {
                if a.is_empty() {
                    *a = b.clone();
                } else if !b.is_empty() {
                    a.extend(b);
                }
            }
            _ => return false,
        }
        true
    }
}

/// Queue two nonempty, equal-length sequences; false when the
/// lengths differ. Sequences that are the same elements of one buffer
/// (`same`) are equal to themselves.
fn pairwise<'a>(
    xs: &'a [Value],
    ys: &'a [Value],
    same: bool,
    work: &mut Vec<(&'a [Value], &'a [Value])>,
) -> bool {
    if xs.len() != ys.len() {
        return false;
    }
    if !same && !xs.is_empty() {
        work.push((xs, ys));
    }
    true
}

/// A `List` value: the elements `start..end` of a shared buffer (T8).
/// `take`, `drop`, and `drop_last` are views that copy nothing, so walking
/// a list with `first()` and `drop(1)` is linear. A list whose buffer
/// nothing else holds appends in place: past the elements it views, which
/// nobody else can see, and its dead prefix is compacted away once it is
/// longer than the list, so a queue that drops from the front and pushes
/// at the back stays linear. It reads as a slice of its elements.
#[derive(Clone, Debug)]
pub struct ListRef {
    buf: Rc<Vec<Value>>,
    start: usize,
    end: usize,
}

impl ListRef {
    pub fn new(items: Vec<Value>) -> ListRef {
        let end = items.len();
        ListRef {
            buf: Rc::new(items),
            start: 0,
            end,
        }
    }

    /// The elements `from..to` of this list, sharing its buffer.
    pub fn slice(&self, from: usize, to: usize) -> ListRef {
        debug_assert!(from <= to && to <= self.len());
        ListRef {
            buf: Rc::clone(&self.buf),
            start: self.start + from,
            end: self.start + to,
        }
    }

    /// Whether both are the same elements of one buffer (so equal).
    pub fn same(&self, other: &ListRef) -> bool {
        Rc::ptr_eq(&self.buf, &other.buf) && self.start == other.start && self.end == other.end
    }

    /// The identity of the elements this list views, for sharing: their
    /// address and count.
    pub fn identity(&self) -> (usize, usize) {
        (self.as_ptr() as usize, self.len())
    }

    /// The buffer, holding exactly this list's elements and owned by this
    /// list alone (a copy when it is shared), to append to.
    fn owned(&mut self) -> &mut Vec<Value> {
        if Rc::get_mut(&mut self.buf).is_none() {
            let live = self.buf[self.start..self.end].to_vec();
            self.start = 0;
            self.end = live.len();
            self.buf = Rc::new(live);
        }
        let (start, end) = (self.start, self.end);
        let v = Rc::get_mut(&mut self.buf).expect("an unshared buffer");
        v.truncate(end);
        if start > end - start {
            v.drain(..start);
            self.start = 0;
            self.end = end - start;
        }
        v
    }

    /// Append `item` (copy on write).
    pub fn push(&mut self, item: Value) {
        let v = self.owned();
        v.push(item);
        let len = v.len();
        self.end = len;
    }

    /// Append `items` (copy on write).
    pub fn extend(&mut self, items: &[Value]) {
        let v = self.owned();
        v.extend(items.iter().cloned());
        let len = v.len();
        self.end = len;
    }

    /// Every value the buffer holds, when nothing else holds it (to drop
    /// them without recursion).
    fn buffer_mut(&mut self) -> Option<&mut [Value]> {
        Rc::get_mut(&mut self.buf).map(Vec::as_mut_slice)
    }
}

impl std::ops::Deref for ListRef {
    type Target = [Value];

    fn deref(&self) -> &[Value] {
        &self.buf[self.start..self.end]
    }
}

impl From<Vec<Value>> for ListRef {
    fn from(items: Vec<Value>) -> ListRef {
        ListRef::new(items)
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Value) -> bool {
        self.structurally_equal(other)
    }
}

impl Eq for Value {}

/// Dropping a value never recurses on the host stack (semantics S38: the
/// runtime never ends in a crash): a loop can build a value nested far
/// deeper than any thread's stack could tear down recursively
/// (`struct Node { next: Option<Node> }` grown 10 000 times, or a chain
/// whose every level holds the next one twice), and such a value may be
/// dropped on whatever thread holds it, the caller of
/// [`crate::run_function`] included. A buffer this value owns alone is
/// emptied onto an explicit work list before it is freed, so each nested
/// value is dropped with nothing nested left inside it; a value whose
/// buffer is still shared only releases its count, and the last holder
/// empties it in turn.
impl Drop for Value {
    fn drop(&mut self) {
        let mut work = Vec::new();
        self.take_nested(&mut work);
        while let Some(mut v) = work.pop() {
            v.take_nested(&mut work);
        }
    }
}

impl Value {
    /// When this value owns its buffer alone, move every element of it that
    /// has elements of its own into `out`, leaving `Bool(false)` in its
    /// place.
    fn take_nested(&mut self, out: &mut Vec<Value>) {
        let items = match self {
            Value::List(xs) => xs.buffer_mut(),
            Value::Struct { fields: xs, .. } | Value::Enum { payload: xs, .. } => Rc::get_mut(xs),
            Value::Wrapper { inner, .. } => Rc::get_mut(inner).map(std::slice::from_mut),
            _ => None,
        };
        for item in items.into_iter().flatten() {
            if item.has_nested() {
                out.push(std::mem::replace(item, Value::Bool(false)));
            }
        }
    }

    /// True for a list, struct, enum, or wrapper value that holds values.
    fn has_nested(&self) -> bool {
        match self {
            // a list's buffer may hold values past those it views
            Value::List(xs) => !xs.buf.is_empty(),
            Value::Struct { fields: xs, .. } | Value::Enum { payload: xs, .. } => !xs.is_empty(),
            Value::Wrapper { .. } => true,
            _ => false,
        }
    }
}

impl From<BigInt> for Value {
    fn from(n: BigInt) -> Value {
        Value::Int(n)
    }
}

impl From<i64> for Value {
    fn from(n: i64) -> Value {
        Value::Int(BigInt::from(n))
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Value {
        Value::Bool(b)
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Value {
        Value::text(s)
    }
}

#[cfg(test)]
mod list_views {
    use super::{ListRef, Value};
    use num_bigint::BigInt;

    fn ints(xs: &[i64]) -> ListRef {
        ListRef::new(xs.iter().map(|&x| Value::Int(BigInt::from(x))).collect())
    }

    fn values(l: &ListRef) -> Vec<i64> {
        l.iter()
            .map(|v| match v {
                Value::Int(n) => i64::try_from(n).expect("small"),
                _ => panic!("an Int"),
            })
            .collect()
    }

    #[test]
    fn slices_share_the_buffer_and_copy_nothing() {
        let xs = ints(&[1, 2, 3, 4]);
        let tail = xs.slice(1, 4);
        let mid = tail.slice(1, 2);
        assert_eq!(values(&tail), [2, 3, 4]);
        assert_eq!(values(&mid), [3]);
        assert!(std::rc::Rc::ptr_eq(&xs.buf, &mid.buf));
        assert!(tail.same(&xs.slice(1, 4)));
        assert!(!tail.same(&xs.slice(1, 3)));
        // the same elements of another buffer are equal, not the same
        assert!(!tail.same(&ints(&[2, 3, 4])));
        assert_eq!(Value::List(tail.clone()), Value::List(ints(&[2, 3, 4])));
        // the identity of a view is the elements it views
        assert_eq!(xs.slice(1, 3).identity(), tail.slice(0, 2).identity());
        assert_ne!(xs.slice(0, 2).identity(), xs.slice(0, 3).identity());
        assert_ne!(xs.slice(0, 2).identity(), xs.slice(1, 3).identity());
    }

    #[test]
    fn appending_to_a_shared_view_copies_it() {
        let xs = ints(&[1, 2, 3]);
        let mut tail = xs.slice(1, 3);
        tail.push(Value::Int(BigInt::from(9)));
        assert_eq!(values(&tail), [2, 3, 9]);
        assert_eq!(values(&xs), [1, 2, 3]);
        let mut head = xs.slice(0, 1);
        head.extend(&[Value::Int(BigInt::from(7))]);
        assert_eq!(values(&head), [1, 7]);
        assert_eq!(values(&xs), [1, 2, 3]);
    }

    #[test]
    fn a_unique_view_appends_in_place_past_what_it_views() {
        // a prefix nothing else holds: the hidden tail goes, then the item
        let mut head = ints(&[1, 2, 3]).slice(0, 1);
        head.push(Value::Int(BigInt::from(5)));
        assert_eq!(values(&head), [1, 5]);
        assert_eq!(head.buf.len(), 2);
        // a queue: drop from the front, push at the back; the dead prefix is compacted
        let mut q = ints(&[0]);
        for i in 1..1000 {
            q = q.slice(1, q.len());
            q.push(Value::Int(BigInt::from(i)));
            q.push(Value::Int(BigInt::from(-i)));
            assert!(
                q.buf.len() <= 2 * q.len() + 2,
                "{} for {}",
                q.buf.len(),
                q.len()
            );
        }
        assert_eq!(q.len(), 1000);
        // the last 1000 of 0, 1, -1, 2, -2, ..., 999, -999
        assert_eq!(values(&q)[..3], [500, -500, 501]);
        assert_eq!(values(&q)[997..], [-998, 999, -999]);
    }
}
