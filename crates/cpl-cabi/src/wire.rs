//! The node-list codec of values (`spec/toolchain.md` 1.14; `spec/cabi.md` 3.8, 7.4). Owner: the native-engine track
//! (N). Both directions are iterative: a value of any depth crosses without recursion, and a buffer that several
//! positions share is sent once.

use crate::protocol::Node;
use cpl_eval::{HandleId, Value};
use cpl_hir::{DefId, Program};
use num_bigint::BigInt;
use std::collections::HashMap;
use std::rc::Rc;

/// The flat post-order node list of a value; its wire form is the protocol's (`spec/cabi.md` 3.8).
pub use crate::protocol::Nodes;

/// Why a node list is not a value of the program.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct WireError {
    pub message: String,
}

fn err(message: impl Into<String>) -> WireError {
    WireError {
        message: message.into(),
    }
}

/// The identity of a shared buffer: its kind and address (and, for a list, which is a view of
/// its buffer, the count of the elements it views).
fn share_key(v: &Value) -> Option<(u8, usize, usize)> {
    match v {
        Value::Text(t) => Some((1, Rc::as_ptr(t) as *const u8 as usize, 0)),
        Value::Bytes(b) => Some((2, Rc::as_ptr(b) as *const u8 as usize, 0)),
        Value::List(xs) => {
            let (at, len) = xs.identity();
            Some((3, at, len))
        }
        Value::Struct { fields, .. } => Some((4, Rc::as_ptr(fields) as *const Value as usize, 0)),
        Value::Enum { payload, .. } => Some((5, Rc::as_ptr(payload) as *const Value as usize, 0)),
        Value::Wrapper { inner, .. } => Some((6, Rc::as_ptr(inner) as usize, 0)),
        _ => None,
    }
}

fn children(v: &Value) -> &[Value] {
    match v {
        Value::List(xs) => xs,
        Value::Struct { fields, .. } => fields,
        Value::Enum { payload, .. } => payload,
        Value::Wrapper { inner, .. } => std::slice::from_ref(&**inner),
        _ => &[],
    }
}

/// The node list of `v`, iteratively; a shared buffer is sent once.
pub fn to_nodes(v: &Value) -> Nodes {
    let mut nodes: Vec<Node> = Vec::new();
    let mut seen: HashMap<(u8, usize, usize), u32> = HashMap::new();
    // (value, next child, child indices so far)
    let mut stack: Vec<(&Value, usize, Vec<u32>)> = vec![(v, 0, Vec::new())];
    let mut last: u32 = 0;
    while let Some(top) = stack.last_mut() {
        let value = top.0;
        if top.1 == 0 {
            if let Some(&ix) = share_key(value).and_then(|k| seen.get(&k)) {
                stack.pop();
                last = ix;
                if let Some(parent) = stack.last_mut() {
                    parent.2.push(ix);
                }
                continue;
            }
        }
        let kids = children(value);
        if top.1 < kids.len() {
            let child = &kids[top.1];
            top.1 += 1;
            stack.push((child, 0, Vec::new()));
            continue;
        }
        let (value, _, ks) = stack.pop().expect("a frame");
        let node = match value {
            Value::Int(n) => Node::Int(n.clone()),
            Value::Bool(true) => Node::True,
            Value::Bool(false) => Node::False,
            Value::Text(t) => Node::Text(t.to_string()),
            Value::Bytes(b) => Node::Bytes(b.to_vec()),
            Value::List(_) => Node::List(ks),
            Value::Struct { def, .. } => Node::Struct {
                def: def.0,
                fields: ks,
            },
            Value::Enum { def, variant, .. } => Node::Enum {
                def: def.0,
                variant: *variant,
                payload: ks,
            },
            Value::Wrapper { def, .. } => Node::Wrapper {
                def: def.0,
                inner: ks.first().copied().unwrap_or(0),
            },
            Value::Capability(h) => Node::Capability {
                def: u32::MAX,
                handle: h.0,
            },
        };
        let ix = nodes.len() as u32;
        nodes.push(node);
        if let Some(k) = share_key(value) {
            seen.insert(k, ix);
        }
        last = ix;
        if let Some(parent) = stack.last_mut() {
            parent.2.push(ix);
        }
    }
    let _ = last;
    Nodes {
        nodes,
        json: None,
        text: None,
    }
}

/// The value of a node list, iteratively.
pub fn from_nodes(p: &Program, n: &Nodes) -> Result<Value, WireError> {
    if n.nodes.is_empty() {
        return Err(err("a value has at least one node"));
    }
    let mut built: Vec<Value> = Vec::with_capacity(n.nodes.len());
    for (i, node) in n.nodes.iter().enumerate() {
        let get = |k: u32| -> Result<Value, WireError> {
            if (k as usize) < i {
                Ok(built[k as usize].clone())
            } else {
                Err(err(format!(
                    "node {i} refers to node {k}, which is not an earlier node"
                )))
            }
        };
        let def_ok = |d: u32| -> Result<DefId, WireError> {
            if (d as usize) < p.defs.len() {
                Ok(DefId(d))
            } else {
                Err(err(format!(
                    "node {i} names DefId({d}), which the program does not have"
                )))
            }
        };
        let v = match node {
            Node::Int(x) => Value::Int(BigInt::clone(x)),
            Node::True => Value::Bool(true),
            Node::False => Value::Bool(false),
            Node::Text(s) => Value::text(s),
            Node::Bytes(b) => Value::bytes(b),
            Node::List(ks) => Value::list(ks.iter().map(|&k| get(k)).collect::<Result<_, _>>()?),
            Node::Struct { def, fields } => Value::new_struct(
                def_ok(*def)?,
                fields.iter().map(|&k| get(k)).collect::<Result<_, _>>()?,
            ),
            Node::Enum {
                def,
                variant,
                payload,
            } => Value::new_enum(
                def_ok(*def)?,
                *variant,
                payload.iter().map(|&k| get(k)).collect::<Result<_, _>>()?,
            ),
            Node::Wrapper { def, inner } => Value::wrap(def_ok(*def)?, get(*inner)?),
            Node::Capability { handle, .. } => Value::Capability(HandleId(*handle)),
        };
        built.push(v);
    }
    Ok(built.pop().expect("a node"))
}
