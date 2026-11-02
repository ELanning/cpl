//! cpl-driver protocol 1 (`spec/cabi.md` 3.8, C17): the messages between the toolchain and a driver, the program's
//! emitted C compiled with `-DCPL_WITH_TESTS=1` and the runtime, which evaluates for the native engine (`--engine
//! native` of `cpl run` and `cpl test`, the differential suites) and grants backend-`c` adapters to the evaluator's
//! `CAdapter` (cabi.md 7.2).
//!
//! The transport is line-delimited compact JSON: one [`Request`] per LF-terminated line on the driver's stdin, one
//! [`Response`] per line on its stdout, and before the first request the driver's [`Banner`]; stderr carries only
//! crash and sanitizer reports. Objects are read by key, so key order is not part of the protocol (these types write
//! their keys in field order). A `def`, a `file`, and a position are the program's own `DefId.0`, `FileId.0`, and
//! byte offset; a handle is a number the driver gave out (`bind`, `bind_scripted`), from 0 in bind order after each
//! `configure`.
//!
//! The answers a request can get:
//!
//! | Request | Response |
//! | - | - |
//! | [`Request::Configure`] | `Ok` with `null`, or `Error` (`interop_runtime_in_use` when a value of the old runtime is still live) |
//! | [`Request::Bind`] | `Handle`, or `Error` (`interop_bind_failed`, numbering no handle) |
//! | [`Request::BindScripted`] | `Handle` |
//! | [`Request::Call`] | `Ok` with a [`Nodes`], `Fault` without `violation`, or `Error` |
//! | [`Request::RunBody`] | `Ok` with `null`, or `Fault` with `violation` (an object or `null`) |
//! | [`Request::FailingRefinement`] | `Ok` with a `def` or `null` (the first refinement of the chain, root first, whose predicate is false), or `Fault` |
//! | [`Request::RangeBound`] | `Ok` with a [`Nodes`], or `Fault` with `violation` |
//! | [`Request::CapCall`] | `Ok` with a [`Nodes`], `ForeignError`, `Fault` (a fault of the binding layer: `binding_protocol`, memory), or `Error` |
//! | [`Request::Tables`] | `Ok` with entry `index` of the table as an object (its struct's member names; strings, integers, `null` for `CPLRT_NONE` and `NULL`, `true` for a pointer to a static value, arrays, nested objects; function pointers left out; a `files` entry is its path), or without `index` its length |
//! | [`Request::Coverage`] | `Ok` with a [`CoverageCounts`], or `Error` (`interop_option_unsupported`, `option` `"coverage"`) from a driver built without coverage |
//! | [`Request::FromJson`] | `Ok` with a [`Nodes`], or `Error` (`interop_type_unknown`, `interop_json_invalid`) |
//! | [`Request::Render`] | `Ok` with a [`Nodes`] carrying `json` and `text` |
//! | [`Request::Stats`] | `Ok` with a [`StatsCounts`] |
//! | [`Request::Shutdown`] | `Ok` with the counters of `Stats` after the driver dropped every handle it numbered; the driver then closes its runtime and exits 0 |
//!
//! Every request can also get `Error`: a malformed line (not JSON, an unknown `op`, a missing or ill-typed key, an
//! unknown binding, a `def`, `handle`, `binder`, `side`, `operation`, table, or index out of range) answers
//! `interop_json_invalid` with `pointer` and `detail`, and the driver reads the next line; after `CPL_E_INTERNAL` the
//! answer is `internal_error` kind `runtime_invariant`, and every later request but `configure` and `shutdown`
//! answers `interop_runtime_poisoned`.
//!
//! Until the first `configure` the driver runs with `max_call_depth` 10,000, no time budget, contracts checked, and no
//! heap budget. `call` and `run_body` are each one top-level evaluation with a fresh deadline (the evaluator's
//! `Budget::Fresh`; with `check_arguments` each refined position is first checked in an evaluation of its own with a
//! fresh deadline, as `run_function` checks it), `range_bound` runs with no deadline (`Budget::None`), and
//! `failing_refinement` uses its `budget`.
//!
//! This module is a frozen contract (the contracts engineer's file): the types are complete, and their serde forms
//! are the protocol. The client (`NativeSession`, `CAdapter`) reads a fault record's `data`, which can be as deep as
//! the values it holds, with serde_json's recursion limit disabled, on the evaluation thread (toolchain T71).

use cpl_diag::json::DiagnosticJson;
use cpl_diag::{Facet, StableId};
use num_bigint::BigInt;
use serde::de::{self, SeqAccess, Visitor};
use serde::ser::SerializeSeq;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value as Json};
use std::collections::BTreeMap;

/// The protocol version of this module, the `protocol` of the banner: `cpl_rtspec::limits::DRIVER_PROTOCOL`.
pub const PROTOCOL: u32 = cpl_rtspec::limits::DRIVER_PROTOCOL;

/// The keys of [`CoverageCounts::claims`]: the kinds of the claim sites of `spec/cabi.md` 3.3.1, and
/// `already_claimed` for the S70 path that finds a fault already claimed.
pub const CLAIM_KINDS: [&str; 10] = [
    "requires",
    "ensures",
    "header_decreases",
    "invariant",
    "loop_decreases",
    "where",
    "op_requires",
    "op_ensures",
    "range",
    "already_claimed",
];

/// The driver's first line, `{"protocol": 1, "runtime": "0.1.0", "api": "0.1", "program": "<64 hex>"}`. The client
/// refuses a program digest other than the one it built (`spec/cabi.md` 3.8) and a protocol other than [`PROTOCOL`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Banner {
    pub protocol: u32,
    /// `cpl_rt::RUNTIME_VERSION`.
    pub runtime: String,
    /// The C API version `"<major>.<minor>"` (`cpl_rtspec::limits::C_API_VERSION`).
    pub api: String,
    /// The program digest of `spec/cabi.md` 1.6, 64 lowercase hex characters.
    pub program: String,
}

/// One request line, tagged by its `op` key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// `{"op": "configure", "max_call_depth": n, "time_budget_ms": n | null, "check_contracts": b,
    /// "max_heap_bytes": n}`: the driver closes its runtime, releasing every capability it bound, and opens a new one
    /// with this configuration; handles are numbered from 0 again. `check_contracts: false` is the unchecked mode
    /// that only the driver accepts (`spec/cabi.md` 3.7); `max_heap_bytes` 0 is no heap budget.
    Configure {
        max_call_depth: u32,
        time_budget_ms: Option<u64>,
        check_contracts: bool,
        max_heap_bytes: u64,
    },
    /// `{"op": "bind", "binding": "<name>"}`: grants the compiled-in backend-`c` adapter of that binding
    /// (`spec/cabi.md` 6.1); its `init` runs now. From Milestone 3 also a backend-`python` binding, in a worker
    /// (`spec/python.md` 6).
    Bind { binding: String },
    /// `{"op": "workers", "python": "<path>", "env": {"KEY": "value"}}` (Milestone 3, `spec/python.md` 6.5): the
    /// interpreter and the environment additions of the driver's later Python workers.
    Workers {
        python: String,
        env: std::collections::BTreeMap<String, String>,
    },
    /// `{"op": "bind_scripted", "capability": def, "label": "<label>", "responses": [...]}`: a scripted host of the
    /// capability, whose results are validated as every Milestone 2 binding's are (`spec/cabi.md` 6.4). `label` is the
    /// handle's binding name (semantics 6.9).
    BindScripted {
        capability: u32,
        label: String,
        #[serde(flatten)]
        script: Script,
    },
    /// `{"op": "call", "function": def, "args": [V...], "using": [n...], "check_arguments": b}`: one top-level
    /// evaluation of the function, with no calling declaration, through its entry trampoline; with
    /// `check_arguments` the refined positions of the arguments are checked first, as `cpl run` checks them
    /// (semantics S39 (a), S74).
    Call {
        function: u32,
        args: Vec<Nodes>,
        using: Vec<u32>,
        check_arguments: bool,
    },
    /// `{"op": "run_body", "test": def, "binders": [V...]}`: a test or property body with its `for` binders bound
    /// in clause order.
    RunBody { test: u32, binders: Vec<Nodes> },
    /// `{"op": "failing_refinement", "refined": def, "value": V, "budget": "fresh" | "none"}`.
    FailingRefinement {
        refined: u32,
        value: Nodes,
        budget: Budget,
    },
    /// `{"op": "range_bound", "property": def, "binder": i, "side": "lo" | "hi", "clause": "<range text>"}`: bound
    /// `side` of the `gen.range` of `for` binder `binder` (clause order) of the property, a fault escaping it claimed
    /// with the range text the runner computed in Rust (`clause::range_text`, semantics S71).
    RangeBound {
        property: u32,
        binder: u32,
        side: Side,
        clause: String,
    },
    /// `{"op": "cap_call", "handle": n, "operation": i, "args": [V...]}`: the binding layer only (the adapter's
    /// function and the foreign-failure mapping), with no clauses and no validation, for the evaluator's `CAdapter`
    /// (`spec/cabi.md` 7.2).
    CapCall {
        handle: u32,
        operation: u32,
        args: Vec<Nodes>,
    },
    /// `{"op": "tables", "table": "<table>", "index": i}`: entry `index` of the compiled table, or without `index`
    /// the table's length; the table-fidelity test reads every entry (`spec/cabi.md` 3.2).
    Tables {
        table: Table,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<u32>,
    },
    /// `{"op": "coverage"}`: the counters of a coverage driver (`spec/cabi.md` 1.4).
    Coverage,
    /// `{"op": "from_json", "type": "<qualified type text>", "json": "<text>"}`: `cpl_value_from_json` of the text at
    /// the type that `cpl_type_lookup` finds, for the round-trip tests of the C reader (toolchain 12.6).
    FromJson {
        #[serde(rename = "type")]
        ty: String,
        json: String,
    },
    /// `{"op": "render", "value": V}`: the value with the C writer's renderings.
    Render { value: Nodes },
    /// `{"op": "stats"}`: the counters of the driver runtime's `cpl_stats`.
    Stats,
    /// `{"op": "shutdown"}`.
    Shutdown,
}

/// The `budget` of [`Request::FailingRefinement`]: the evaluator's `Budget` (toolchain 1.7.4).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Budget {
    /// A top-level evaluation with its own time budget.
    Fresh,
    /// The runner's unbudgeted evaluations.
    None,
}

/// The `side` of [`Request::RangeBound`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Lo,
    Hi,
}

/// The compiled tables of `cplrt_program` that [`Request::Tables`] reads (`spec/cabi.md` Appendix B.1).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Table {
    Defs,
    Files,
    Sites,
    Clauses,
    Bsites,
    Holes,
    Refinements,
    Types,
    Exports,
    Grants,
}

/// The responses of a scripted host (`bind_scripted`, `spec/cabi.md` 3.8). An operation call takes the first unused
/// response whose `op` and `args` equal the call's (structural equality) and answers with its `result`, which may
/// be ill-typed on purpose, or reports its foreign error; a call no response matches is the fault `internal_error`
/// kind `binding_protocol` with `operation`, `binding` (the label), the operation call's span, and the `detail` of
/// semantics S80 (`cpl_rtspec::messages::DETAIL_BINDING_PROTOCOL_NO_RESPONSE`). `cpl_cabi::scripted::ScriptHost`
/// applies the same rule on the evaluator's side.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Script {
    pub responses: Vec<ScriptResponse>,
}

/// `{"op": "<name>", "args": [V...], "result": V}` or `{"op": "<name>", "args": [V...], "foreign_error": F}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptResponse {
    /// The operation's name.
    pub op: String,
    pub args: Vec<Nodes>,
    #[serde(flatten)]
    pub answer: ScriptAnswer,
}

/// What a scripted response answers: a result or a reported foreign failure.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptAnswer {
    Result(Nodes),
    ForeignError(ForeignFailure),
}

/// `F`, a foreign failure: `{"kind": "<ForeignErrorKind>", "message": "...", "details": [["key", "value"], ...]}`.
/// In a script, the failure the host reports. In a [`Response::ForeignError`] of `cap_call`, the reported kind, the
/// final message (the adapter's, the one stdlib.md S45 gives a declarative binding, or the binding layer's own), and,
/// in stdlib.md S45's order, every detail after the runtime's keys `adapter`, `capability`, and `operation`: the
/// declarative pair, the adapter's own entries in the order it added them, and, for an `InvalidResult` of the binding
/// layer, `pointer` and `expected_type`. The `CAdapter` prepends the runtime's three keys as `cplrt_cap_invoke` does
/// (`spec/cabi.md` 6.5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForeignFailure {
    /// A variant of `std.foreign.ForeignErrorKind` (`cpl_rtspec::foreign::KINDS`).
    pub kind: String,
    pub message: String,
    pub details: Vec<(String, String)>,
}

/// One response line. A response does not name its request: the client reads it as the answer of the request it
/// sent (the table of the module documentation), and the variants are told apart by their keys.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Response {
    /// `{"fault": R}`, and for `run_body` and `range_bound` `{"fault": R, "violation": {...} | null}`.
    Fault(Box<FaultAnswer>),
    /// `{"foreign_error": F}`, of `cap_call` only.
    ForeignError { foreign_error: ForeignFailure },
    /// `{"handle": n}`, of `bind` and `bind_scripted`.
    Handle { handle: u32 },
    /// `{"error": E}`: the S49 object of an `interop_*` error (`spec/cabi.md` 4.9), or of `internal_error` kind
    /// `runtime_invariant` when the driver's runtime hit `CPL_E_INTERNAL` (cabi.md 4.6).
    Error { error: Box<DiagnosticJson> },
    /// `{"ok": ...}`: the request's result, whose shape the request decides (the module documentation): `null`, a
    /// [`Nodes`], a `def` or `null`, a table entry or length, a [`CoverageCounts`], or a [`StatsCounts`];
    /// `serde_json::from_value` reads it as the expected type.
    Ok { ok: Json },
}

/// The fault answer of a request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaultAnswer {
    pub fault: FaultRecord,
    /// `None`: no `violation` key (`call`, `failing_refinement`, `cap_call`); `Some(None)`: `"violation": null`;
    /// `Some(Some(v))`: the last `ensures`, `invariant`, or `decreases` violation of the evaluation, as raised
    /// (`run_body`, `range_bound`; semantics S46; `spec/cabi.md` 3.3.1).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "present_or_null"
    )]
    pub violation: Option<Option<Violation>>,
}

/// `{"def": n, "fault": R}`: a violation record and the declaration it is of.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Violation {
    pub def: u32,
    pub fault: FaultRecord,
}

/// `R`, a native fault record (`spec/cabi.md` 3.3.1): `{"kind": "<code>", "clause_kind"?: "...", "resource"?: "...",
/// "message": "...", "symbol"?: "<stable id>", "facet"?: "contract", "span"?: [file, start, end], "backtrace":
/// [[def, file, start, end] | [def], ...], "data": {...}, "diagnostic": "<S49 JSON>"}`. `cpl_cabi::fault_from_record`
/// rebuilds the evaluator's `cpl_eval::Fault` from it; `diagnostic`, the C runtime's own S49 rendering of the fault,
/// is what a C host sees and is compared byte for byte (`spec/cabi.md` 3.9).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaultRecord {
    /// The fault code of semantics 6.7: `contract_violation`, `assertion_failed`, `unresolved_hole`,
    /// `division_by_zero`, `refinement_violation`, `resource_exhaustion`, or `internal_error`.
    pub kind: String,
    /// A `contract_violation`'s `requires`, `ensures`, `invariant`, or `decreases`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clause_kind: Option<String>,
    /// A `resource_exhaustion`'s `stack`, `memory`, `time`, or `steps`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    /// Rendered once, from the template of the kind (`cpl_rtspec::messages`).
    pub message: String,
    /// The blamed declaration's stable id (`StableId::parse` of the C text); none for `internal_error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<StableId>,
    /// `contract` when set; unset otherwise (the diagnostic then says `body` when there is a symbol).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facet: Option<Facet>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<SpanRef>,
    /// Innermost frame first.
    pub backtrace: Vec<FrameRef>,
    /// The payload keys of toolchain 1.7.3 as the C runtime renders them.
    pub data: Map<String, Json>,
    /// The C runtime's S49 rendering of the fault, compact JSON.
    pub diagnostic: String,
}

/// A span as the program's own ids: `[file, start, end]` (`FileId.0` and byte offsets).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(from = "(u32, u32, u32)", into = "(u32, u32, u32)")]
pub struct SpanRef {
    pub file: u32,
    pub start: u32,
    pub end: u32,
}

impl From<(u32, u32, u32)> for SpanRef {
    fn from((file, start, end): (u32, u32, u32)) -> SpanRef {
        SpanRef { file, start, end }
    }
}

impl From<SpanRef> for (u32, u32, u32) {
    fn from(s: SpanRef) -> (u32, u32, u32) {
        (s.file, s.start, s.end)
    }
}

/// A backtrace frame: `[def, file, start, end]`, or `[def]` for a frame without a span.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "Vec<u32>", into = "Vec<u32>")]
pub struct FrameRef {
    pub def: u32,
    pub span: Option<SpanRef>,
}

impl TryFrom<Vec<u32>> for FrameRef {
    type Error = String;

    fn try_from(items: Vec<u32>) -> Result<FrameRef, String> {
        match items.as_slice() {
            [def] => Ok(FrameRef {
                def: *def,
                span: None,
            }),
            [def, file, start, end] => Ok(FrameRef {
                def: *def,
                span: Some(SpanRef {
                    file: *file,
                    start: *start,
                    end: *end,
                }),
            }),
            _ => Err(format!(
                "a backtrace frame is [def, file, start, end] or [def], not {} numbers",
                items.len()
            )),
        }
    }
}

impl From<FrameRef> for Vec<u32> {
    fn from(f: FrameRef) -> Vec<u32> {
        match f.span {
            Some(s) => vec![f.def, s.file, s.start, s.end],
            None => vec![f.def],
        }
    }
}

/// `V`, a value as a flat post-order node list, `{"nodes": [[tag, ...], ...]}`, the root last and children referenced
/// by the index of an earlier node, so that a shared child array is sent once and neither side recurses on value
/// depth (`spec/cabi.md` 3.8, 7.4). An output value adds `json` and `text`: the C writer's value JSON and value text of
/// it, which the suites compare byte for byte with the Rust renderings (toolchain 12.6). `cpl_cabi::wire` converts
/// node lists to and from `cpl_eval::Value`, and checks that every child index refers to an earlier node.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Nodes {
    pub nodes: Vec<Node>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// One node of a [`Nodes`] list. Children are indices of earlier nodes; `def` is a `DefId.0`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    /// `["i", "<canonical decimal>"]`: an optional `-`, then digits without leading zeros, `0` for zero.
    Int(BigInt),
    /// `["t"]`.
    True,
    /// `["f"]`.
    False,
    /// `["s", "<text>"]`.
    Text(String),
    /// `["y", "<lowercase hex>"]`, two digits per byte.
    Bytes(Vec<u8>),
    /// `["l", k...]`: a list's elements.
    List(Vec<u32>),
    /// `["r", def, k...]`: a struct's fields in declaration order.
    Struct { def: u32, fields: Vec<u32> },
    /// `["e", def, variant, k...]`: an enum value's payload.
    Enum {
        def: u32,
        variant: u32,
        payload: Vec<u32>,
    },
    /// `["w", def, k]`: a wrapper.
    Wrapper { def: u32, inner: u32 },
    /// `["c", def, handle]`: a capability handle, in outputs only.
    Capability { def: u32, handle: u32 },
}

// Stream the flat wire arrays directly. Going through Vec<Json> clones the entire node when writing and builds
// a temporary JSON tree when reading; neither is needed to preserve the protocol's tagged-array representation.
impl Serialize for Node {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (tag, len) = match self {
            Node::Int(_) => ("i", 2),
            Node::True => ("t", 1),
            Node::False => ("f", 1),
            Node::Text(_) => ("s", 2),
            Node::Bytes(_) => ("y", 2),
            Node::List(items) => ("l", 1 + items.len()),
            Node::Struct { fields, .. } => ("r", 2 + fields.len()),
            Node::Enum { payload, .. } => ("e", 3 + payload.len()),
            Node::Wrapper { .. } => ("w", 3),
            Node::Capability { .. } => ("c", 3),
        };
        let mut seq = serializer.serialize_seq(Some(len))?;
        seq.serialize_element(tag)?;
        match self {
            Node::Int(n) => seq.serialize_element(&n.to_string())?,
            Node::True | Node::False => {}
            Node::Text(s) => seq.serialize_element(s)?,
            Node::Bytes(bytes) => seq.serialize_element(&encode_hex(bytes))?,
            Node::List(items) => {
                for item in items {
                    seq.serialize_element(item)?;
                }
            }
            Node::Struct { def, fields } => {
                seq.serialize_element(def)?;
                for field in fields {
                    seq.serialize_element(field)?;
                }
            }
            Node::Enum {
                def,
                variant,
                payload,
            } => {
                seq.serialize_element(def)?;
                seq.serialize_element(variant)?;
                for item in payload {
                    seq.serialize_element(item)?;
                }
            }
            Node::Wrapper { def, inner } => {
                seq.serialize_element(def)?;
                seq.serialize_element(inner)?;
            }
            Node::Capability { def, handle } => {
                seq.serialize_element(def)?;
                seq.serialize_element(handle)?;
            }
        }
        seq.end()
    }
}

enum NodeTag {
    Int,
    True,
    False,
    Text,
    Bytes,
    List,
    Struct,
    Enum,
    Wrapper,
    Capability,
}

impl<'de> Deserialize<'de> for NodeTag {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct TagVisitor;
        impl Visitor<'_> for TagVisitor {
            type Value = NodeTag;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a node tag string")
            }
            fn visit_str<E: de::Error>(self, tag: &str) -> Result<NodeTag, E> {
                match tag {
                    "i" => Ok(NodeTag::Int),
                    "t" => Ok(NodeTag::True),
                    "f" => Ok(NodeTag::False),
                    "s" => Ok(NodeTag::Text),
                    "y" => Ok(NodeTag::Bytes),
                    "l" => Ok(NodeTag::List),
                    "r" => Ok(NodeTag::Struct),
                    "e" => Ok(NodeTag::Enum),
                    "w" => Ok(NodeTag::Wrapper),
                    "c" => Ok(NodeTag::Capability),
                    _ => Err(E::custom(format!("unknown node tag `{tag}`"))),
                }
            }
        }
        deserializer.deserialize_str(TagVisitor)
    }
}

impl<'de> Deserialize<'de> for Node {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NodeVisitor;
        impl<'de> Visitor<'de> for NodeVisitor {
            type Value = Node;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a tagged node array")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Node, A::Error> {
                fn required<'de, T: Deserialize<'de>, A: SeqAccess<'de>>(
                    seq: &mut A,
                ) -> Result<T, A::Error> {
                    seq.next_element()?
                        .ok_or_else(|| de::Error::custom("missing node operand"))
                }
                fn indices<'de, A: SeqAccess<'de>>(seq: &mut A) -> Result<Vec<u32>, A::Error> {
                    let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(4096));
                    while let Some(item) = seq.next_element()? {
                        items.push(item);
                    }
                    Ok(items)
                }
                let node = match required::<NodeTag, _>(&mut seq)? {
                    NodeTag::Int => Node::Int(
                        canonical_int(&required::<String, _>(&mut seq)?)
                            .map_err(de::Error::custom)?,
                    ),
                    NodeTag::True => Node::True,
                    NodeTag::False => Node::False,
                    NodeTag::Text => Node::Text(required(&mut seq)?),
                    NodeTag::Bytes => Node::Bytes(
                        lowercase_hex(&required::<String, _>(&mut seq)?)
                            .map_err(de::Error::custom)?,
                    ),
                    NodeTag::List => return Ok(Node::List(indices(&mut seq)?)),
                    NodeTag::Struct => {
                        return Ok(Node::Struct {
                            def: required(&mut seq)?,
                            fields: indices(&mut seq)?,
                        })
                    }
                    NodeTag::Enum => {
                        return Ok(Node::Enum {
                            def: required(&mut seq)?,
                            variant: required(&mut seq)?,
                            payload: indices(&mut seq)?,
                        })
                    }
                    NodeTag::Wrapper => Node::Wrapper {
                        def: required(&mut seq)?,
                        inner: required(&mut seq)?,
                    },
                    NodeTag::Capability => Node::Capability {
                        def: required(&mut seq)?,
                        handle: required(&mut seq)?,
                    },
                };
                if seq.next_element::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::custom("extra node operand"));
                }
                Ok(node)
            }
        }
        deserializer.deserialize_seq(NodeVisitor)
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[usize::from(byte >> 4)] as char);
        out.push(HEX[usize::from(byte & 15)] as char);
    }
    out
}

/// The canonical decimal of an `Int`, or why `text` is not one.
fn canonical_int(text: &str) -> Result<BigInt, String> {
    let digits = text.strip_prefix('-').unwrap_or(text);
    let canonical = !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && (digits == "0" || !digits.starts_with('0'))
        && text != "-0";
    if !canonical {
        return Err(format!("`{text}` is not the canonical decimal of an Int"));
    }
    text.parse()
        .map_err(|_| format!("`{text}` is not the canonical decimal of an Int"))
}

/// The bytes of lowercase hex `text`, or why it is not that.
fn lowercase_hex(text: &str) -> Result<Vec<u8>, String> {
    let digit = |b: u8| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    };
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return Err(format!("`{text}` has an odd number of hex digits"));
    }
    bytes
        .chunks(2)
        .map(|pair| match (digit(pair[0]), digit(pair[1])) {
            (Some(hi), Some(lo)) => Ok(hi << 4 | lo),
            _ => Err(format!("`{text}` is not lowercase hex")),
        })
        .collect()
}

impl TryFrom<Vec<Json>> for Node {
    type Error = String;

    fn try_from(items: Vec<Json>) -> Result<Node, String> {
        let (tag, rest) = match items.split_first() {
            Some((Json::String(tag), rest)) => (tag.as_str(), rest),
            _ => return Err("a node is an array that starts with its tag".to_string()),
        };
        let index = |v: &Json| -> Result<u32, String> {
            v.as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| format!("`{v}` is not an index or a def"))
        };
        let indices = |vs: &[Json]| -> Result<Vec<u32>, String> { vs.iter().map(index).collect() };
        match (tag, rest) {
            ("i", [Json::String(d)]) => canonical_int(d).map(Node::Int),
            ("t", []) => Ok(Node::True),
            ("f", []) => Ok(Node::False),
            ("s", [Json::String(s)]) => Ok(Node::Text(s.clone())),
            ("y", [Json::String(h)]) => lowercase_hex(h).map(Node::Bytes),
            ("l", ks) => Ok(Node::List(indices(ks)?)),
            ("r", [def, ks @ ..]) => Ok(Node::Struct {
                def: index(def)?,
                fields: indices(ks)?,
            }),
            ("e", [def, variant, ks @ ..]) => Ok(Node::Enum {
                def: index(def)?,
                variant: index(variant)?,
                payload: indices(ks)?,
            }),
            ("w", [def, k]) => Ok(Node::Wrapper {
                def: index(def)?,
                inner: index(k)?,
            }),
            ("c", [def, handle]) => Ok(Node::Capability {
                def: index(def)?,
                handle: index(handle)?,
            }),
            _ => Err(format!(
                "`[\"{tag}\", ...]` with {} operand(s) is not a node",
                rest.len()
            )),
        }
    }
}

impl From<Node> for Vec<Json> {
    fn from(node: Node) -> Vec<Json> {
        fn tagged(tag: &str, mut operands: Vec<Json>) -> Vec<Json> {
            operands.insert(0, Json::String(tag.to_string()));
            operands
        }
        let numbers = |ns: Vec<u32>| -> Vec<Json> { ns.into_iter().map(Json::from).collect() };
        match node {
            Node::Int(n) => tagged("i", vec![Json::String(n.to_string())]),
            Node::True => tagged("t", Vec::new()),
            Node::False => tagged("f", Vec::new()),
            Node::Text(s) => tagged("s", vec![Json::String(s)]),
            Node::Bytes(b) => {
                let hex = encode_hex(&b);
                tagged("y", vec![Json::String(hex)])
            }
            Node::List(items) => tagged("l", numbers(items)),
            Node::Struct { def, fields } => {
                let mut operands = vec![Json::from(def)];
                operands.extend(numbers(fields));
                tagged("r", operands)
            }
            Node::Enum {
                def,
                variant,
                payload,
            } => {
                let mut operands = vec![Json::from(def), Json::from(variant)];
                operands.extend(numbers(payload));
                tagged("e", operands)
            }
            Node::Wrapper { def, inner } => tagged("w", vec![Json::from(def), Json::from(inner)]),
            Node::Capability { def, handle } => {
                tagged("c", vec![Json::from(def), Json::from(handle)])
            }
        }
    }
}

/// The `ok` of a [`Request::Coverage`]: the counters of `spec/cabi.md` 1.4 summed by HIR variant
/// (`"ExprKind::Call"`, `"Stmt::While"`, `"PatternKind::Variant"`), the calls of each builtin by registry id, and the
/// claims by the kind of their claim site ([`CLAIM_KINDS`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageCounts {
    pub nodes: BTreeMap<String, u64>,
    pub builtins: BTreeMap<String, u64>,
    pub claims: BTreeMap<String, u64>,
}

/// The `ok` of a [`Request::Stats`]: the driver runtime's `cpl_stats` by member name, without `struct_size` and
/// `reserved` (`spec/cabi.md` 4.5).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatsCounts {
    pub live_handles: u64,
    pub live_caps: u64,
    pub live_objects: u64,
    pub live_bytes: u64,
    pub peak_bytes: u64,
    pub active_calls: u64,
    pub live_buffers: u64,
}

/// The `violation` key of a [`FaultAnswer`], which is absent, `null`, or an object.
mod present_or_null {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<T: Serialize, S: Serializer>(
        value: &Option<Option<T>>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(inner) => inner.serialize(s),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Option<Option<T>>, D::Error> {
        Option::<T>::deserialize(d).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn int(n: i64) -> Node {
        Node::Int(BigInt::from(n))
    }

    /// `value` serializes to exactly `expected` (compact, keys in field order) and reads back from it.
    fn exact<T>(value: &T, expected: &str)
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        assert_eq!(serde_json::to_string(value).unwrap(), expected);
        assert_eq!(&serde_json::from_str::<T>(expected).unwrap(), value);
    }

    #[test]
    fn requests_have_the_shapes_of_cabi_3_8() {
        exact(
            &Request::Configure {
                max_call_depth: 10_000,
                time_budget_ms: None,
                check_contracts: true,
                max_heap_bytes: 0,
            },
            r#"{"op":"configure","max_call_depth":10000,"time_budget_ms":null,"check_contracts":true,"max_heap_bytes":0}"#,
        );
        exact(
            &Request::Bind {
                binding: "image_codec".to_string(),
            },
            r#"{"op":"bind","binding":"image_codec"}"#,
        );
        let page_id = Nodes {
            nodes: vec![
                Node::Text("p".to_string()),
                Node::Wrapper { def: 30, inner: 0 },
            ],
            ..Nodes::default()
        };
        let page = Nodes {
            nodes: vec![
                Node::Text("p".to_string()),
                Node::Wrapper { def: 30, inner: 0 },
                Node::Text("t".to_string()),
                Node::Struct {
                    def: 31,
                    fields: vec![1, 2],
                },
                Node::Enum {
                    def: 7,
                    variant: 0,
                    payload: vec![3],
                },
            ],
            ..Nodes::default()
        };
        exact(
            &Request::BindScripted {
                capability: 61,
                label: "pages".to_string(),
                script: Script {
                    responses: vec![
                        ScriptResponse {
                            op: "get".to_string(),
                            args: vec![page_id.clone()],
                            answer: ScriptAnswer::Result(page),
                        },
                        ScriptResponse {
                            op: "get".to_string(),
                            args: vec![page_id.clone()],
                            answer: ScriptAnswer::ForeignError(ForeignFailure {
                                kind: "Unavailable".to_string(),
                                message: "down".to_string(),
                                details: vec![("retry_safe".to_string(), "true".to_string())],
                            }),
                        },
                    ],
                },
            },
            concat!(
                r#"{"op":"bind_scripted","capability":61,"label":"pages","responses":["#,
                r#"{"op":"get","args":[{"nodes":[["s","p"],["w",30,0]]}],"#,
                r#""result":{"nodes":[["s","p"],["w",30,0],["s","t"],["r",31,1,2],["e",7,0,3]]}},"#,
                r#"{"op":"get","args":[{"nodes":[["s","p"],["w",30,0]]}],"#,
                r#""foreign_error":{"kind":"Unavailable","message":"down","details":[["retry_safe","true"]]}}]}"#
            ),
        );
        exact(
            &Request::Call {
                function: 44,
                args: vec![page_id.clone()],
                using: vec![0, 1],
                check_arguments: true,
            },
            r#"{"op":"call","function":44,"args":[{"nodes":[["s","p"],["w",30,0]]}],"using":[0,1],"check_arguments":true}"#,
        );
        exact(
            &Request::RunBody {
                test: 90,
                binders: Vec::new(),
            },
            r#"{"op":"run_body","test":90,"binders":[]}"#,
        );
        exact(
            &Request::FailingRefinement {
                refined: 23,
                value: Nodes {
                    nodes: vec![int(0)],
                    ..Nodes::default()
                },
                budget: Budget::None,
            },
            r#"{"op":"failing_refinement","refined":23,"value":{"nodes":[["i","0"]]},"budget":"none"}"#,
        );
        exact(
            &Request::RangeBound {
                property: 91,
                binder: 0,
                side: Side::Hi,
                clause: "gen.range(1, 10)".to_string(),
            },
            r#"{"op":"range_bound","property":91,"binder":0,"side":"hi","clause":"gen.range(1, 10)"}"#,
        );
        exact(
            &Request::CapCall {
                handle: 0,
                operation: 0,
                args: vec![Nodes {
                    nodes: vec![Node::Bytes(vec![0x49, 0x4d, 0xff])],
                    ..Nodes::default()
                }],
            },
            r#"{"op":"cap_call","handle":0,"operation":0,"args":[{"nodes":[["y","494dff"]]}]}"#,
        );
        exact(
            &Request::Tables {
                table: Table::Bsites,
                index: Some(3),
            },
            r#"{"op":"tables","table":"bsites","index":3}"#,
        );
        exact(
            &Request::Tables {
                table: Table::Defs,
                index: None,
            },
            r#"{"op":"tables","table":"defs"}"#,
        );
        exact(&Request::Coverage, r#"{"op":"coverage"}"#);
        exact(
            &Request::FromJson {
                ty: "List<Int>".to_string(),
                json: "[1, 2]".to_string(),
            },
            r#"{"op":"from_json","type":"List<Int>","json":"[1, 2]"}"#,
        );
        exact(
            &Request::Render {
                value: Nodes {
                    nodes: vec![Node::True, Node::False, Node::List(vec![0, 1])],
                    ..Nodes::default()
                },
            },
            r#"{"op":"render","value":{"nodes":[["t"],["f"],["l",0,1]]}}"#,
        );
        exact(&Request::Stats, r#"{"op":"stats"}"#);
        exact(&Request::Shutdown, r#"{"op":"shutdown"}"#);
    }

    #[test]
    fn requests_are_read_by_key() {
        let reordered = r#"{"check_contracts":false,"max_heap_bytes":4096,"time_budget_ms":250,"max_call_depth":0,"op":"configure"}"#;
        assert_eq!(
            serde_json::from_str::<Request>(reordered).unwrap(),
            Request::Configure {
                max_call_depth: 0,
                time_budget_ms: Some(250),
                check_contracts: false,
                max_heap_bytes: 4096,
            }
        );
        let scripted = r#"{"responses":[],"label":"l","op":"bind_scripted","capability":5}"#;
        assert_eq!(
            serde_json::from_str::<Request>(scripted).unwrap(),
            Request::BindScripted {
                capability: 5,
                label: "l".to_string(),
                script: Script::default(),
            }
        );
        for malformed in [
            r#"{"op":"launch"}"#,
            r#"{"op":"bind"}"#,
            r#"{"op":"tables","table":"rows"}"#,
            r#"{"op":"range_bound","property":1,"binder":0,"side":"middle","clause":""}"#,
            r#"{"op":"call","function":-1,"args":[],"using":[],"check_arguments":false}"#,
        ] {
            assert!(
                serde_json::from_str::<Request>(malformed).is_err(),
                "{malformed}"
            );
        }
    }

    #[test]
    fn nodes_encode_every_tag() {
        let all = Nodes {
            nodes: vec![
                int(-42),
                Node::Int("123456789012345678901234567890".parse().unwrap()),
                Node::True,
                Node::False,
                Node::Text("a\"\u{0}é".to_string()),
                Node::Bytes(Vec::new()),
                Node::Bytes(vec![0, 15, 16, 255]),
                Node::List(Vec::new()),
                Node::List(vec![0, 1, 0]),
                Node::Struct {
                    def: 3,
                    fields: Vec::new(),
                },
                Node::Enum {
                    def: 4,
                    variant: 70_000,
                    payload: Vec::new(),
                },
                Node::Wrapper { def: 5, inner: 8 },
                Node::Capability { def: 6, handle: 2 },
            ],
            json: Some("{\"$type\":\"Unit\"}".to_string()),
            text: Some("Unit".to_string()),
        };
        exact(
            &all,
            concat!(
                r#"{"nodes":[["i","-42"],["i","123456789012345678901234567890"],["t"],["f"],["s","a\"\u0000é"],"#,
                r#"["y",""],["y","000f10ff"],["l"],["l",0,1,0],["r",3],["e",4,70000],["w",5,8],["c",6,2]],"#,
                r#""json":"{\"$type\":\"Unit\"}","text":"Unit"}"#
            ),
        );
        exact(&Nodes::default(), r#"{"nodes":[]}"#);
        for node in all.nodes {
            let json = Json::Array(Vec::<Json>::from(node.clone()));
            assert_eq!(serde_json::to_value(&node).unwrap(), json);
            assert_eq!(serde_json::from_value::<Node>(json).unwrap(), node);
        }
    }

    #[test]
    fn malformed_nodes_are_refused() {
        for bad in [
            r#"[]"#,
            r#"[1]"#,
            r#"[{"i":null}, "0"]"#,
            r#"[{"t":null}]"#,
            r#"["i"]"#,
            r#"["i", 5]"#,
            r#"["i", "007"]"#,
            r#"["i", "-0"]"#,
            r#"["i", "+1"]"#,
            r#"["i", ""]"#,
            r#"["i", "-"]"#,
            r#"["i", "1e3"]"#,
            r#"["t", 0]"#,
            r#"["s"]"#,
            r#"["s", 1]"#,
            r#"["y", "0"]"#,
            r#"["y", "AB"]"#,
            r#"["y", "zz"]"#,
            r#"["l", -1]"#,
            r#"["l", 4294967296]"#,
            r#"["l", "0"]"#,
            r#"["l", 1.0]"#,
            r#"["r"]"#,
            r#"["e", 1]"#,
            r#"["w", 1]"#,
            r#"["w", 1, 2, 3]"#,
            r#"["c", 1]"#,
            r#"["x"]"#,
        ] {
            assert!(
                serde_json::from_str::<Node>(bad).is_err(),
                "{bad} must be refused"
            );
        }
        assert_eq!(
            serde_json::from_str::<Node>(r#"["i","0"]"#).unwrap(),
            int(0)
        );
        assert_eq!(
            serde_json::from_str::<Node>(r#"["i","-10"]"#).unwrap(),
            int(-10)
        );
    }

    fn record(kind: &str) -> FaultRecord {
        let mut data = Map::new();
        data.insert("detail".to_string(), json!("no arm of the match matches 3"));
        data.insert("kind".to_string(), json!("no_matching_arm"));
        FaultRecord {
            kind: kind.to_string(),
            clause_kind: None,
            resource: None,
            message: "internal error (no_matching_arm): no arm of the match matches 3".to_string(),
            symbol: None,
            facet: None,
            span: Some(SpanRef {
                file: 5,
                start: 100,
                end: 140,
            }),
            backtrace: vec![
                FrameRef {
                    def: 46,
                    span: Some(SpanRef {
                        file: 5,
                        start: 10,
                        end: 20,
                    }),
                },
                FrameRef {
                    def: 44,
                    span: None,
                },
            ],
            data,
            diagnostic: "{}".to_string(),
        }
    }

    #[test]
    fn fault_records_and_violations() {
        let r = record("internal_error");
        let line = concat!(
            r#"{"fault":{"kind":"internal_error","message":"internal error (no_matching_arm): no arm of the match "#,
            r#"matches 3","span":[5,100,140],"backtrace":[[46,5,10,20],[44]],"data":{"detail":"no arm of the match "#,
            r#"matches 3","kind":"no_matching_arm"},"diagnostic":"{}"}}"#
        );
        exact(
            &Response::Fault(Box::new(FaultAnswer {
                fault: r.clone(),
                violation: None,
            })),
            line,
        );
        let with_null = format!("{},\"violation\":null}}", &line[..line.len() - 1]);
        let answer: Response = serde_json::from_str(&with_null).unwrap();
        assert_eq!(
            answer,
            Response::Fault(Box::new(FaultAnswer {
                fault: r.clone(),
                violation: Some(None),
            }))
        );
        assert_eq!(serde_json::to_string(&answer).unwrap(), with_null);

        let mut v = record("contract_violation");
        v.clause_kind = Some("ensures".to_string());
        v.symbol = Some(StableId::parse("reader.pages.divide_exactly").unwrap());
        v.facet = Some(Facet::Contract);
        let answer = Response::Fault(Box::new(FaultAnswer {
            fault: v.clone(),
            violation: Some(Some(Violation { def: 41, fault: v })),
        }));
        let text = serde_json::to_string(&answer).unwrap();
        assert!(text.contains(
            r#""violation":{"def":41,"fault":{"kind":"contract_violation","clause_kind":"ensures""#
        ));
        assert!(text.contains(r#""symbol":"reader.pages.divide_exactly","facet":"contract""#));
        assert_eq!(serde_json::from_str::<Response>(&text).unwrap(), answer);

        let mut memory = record("resource_exhaustion");
        memory.resource = Some("memory".to_string());
        memory.span = None;
        memory.backtrace.clear();
        let text = serde_json::to_string(&memory).unwrap();
        assert!(
            text.contains(r#""resource":"memory","message":"#)
                && text.contains(r#""backtrace":[]"#)
        );
        assert!(!text.contains("\"span\""));
        assert_eq!(serde_json::from_str::<FaultRecord>(&text).unwrap(), memory);

        for bad in [r#"[]"#, r#"[1, 2]"#, r#"[1, 2, 3]"#, r#"[1, 2, 3, 4, 5]"#] {
            assert!(serde_json::from_str::<FrameRef>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn responses_are_told_apart_by_their_keys() {
        exact(&Response::Ok { ok: Json::Null }, r#"{"ok":null}"#);
        exact(&Response::Ok { ok: json!(23) }, r#"{"ok":23}"#);
        exact(&Response::Handle { handle: 3 }, r#"{"handle":3}"#);
        exact(
            &Response::ForeignError {
                foreign_error: ForeignFailure {
                    kind: "InvalidResult".to_string(),
                    message: "m".to_string(),
                    details: vec![("pointer".to_string(), "/$payload/0/width".to_string())],
                },
            },
            r#"{"foreign_error":{"kind":"InvalidResult","message":"m","details":[["pointer","/$payload/0/width"]]}}"#,
        );
        let error = r#"{"error":{"code":"internal_error","category":"internal","severity":"error","message":"internal error (runtime_invariant): a slot was freed twice","detail":"a slot was freed twice","kind":"runtime_invariant"}}"#;
        let parsed: Response = serde_json::from_str(error).unwrap();
        match &parsed {
            Response::Error { error } => {
                assert_eq!(error.code, "internal_error");
                assert_eq!(error.data["kind"], json!("runtime_invariant"));
            }
            other => panic!("not an error response: {other:?}"),
        }
        assert_eq!(serde_json::to_string(&parsed).unwrap(), error);

        // The ok payloads, read as the types their requests give them.
        let nodes = Nodes {
            nodes: vec![int(640)],
            json: Some("640".to_string()),
            text: Some("640".to_string()),
        };
        let line = serde_json::to_string(&Response::Ok {
            ok: serde_json::to_value(&nodes).unwrap(),
        })
        .unwrap();
        // A payload built as a serde_json::Value has its keys sorted; key order is not part of the protocol.
        assert_eq!(
            line,
            r#"{"ok":{"json":"640","nodes":[["i","640"]],"text":"640"}}"#
        );
        let Response::Ok { ok } = serde_json::from_str(&line).unwrap() else {
            panic!("an ok response")
        };
        assert_eq!(serde_json::from_value::<Nodes>(ok).unwrap(), nodes);
        let Response::Ok { ok } = serde_json::from_str(r#"{"ok":null}"#).unwrap() else {
            panic!("an ok response")
        };
        assert_eq!(serde_json::from_value::<Option<u32>>(ok).unwrap(), None);
        let stats = r#"{"ok":{"live_handles":0,"live_caps":0,"live_objects":0,"live_bytes":0,"peak_bytes":4096,"active_calls":0,"live_buffers":0}}"#;
        let Response::Ok { ok } = serde_json::from_str(stats).unwrap() else {
            panic!("an ok response")
        };
        assert_eq!(
            serde_json::from_value::<StatsCounts>(ok)
                .unwrap()
                .peak_bytes,
            4096
        );
        let coverage = r#"{"ok":{"nodes":{"ExprKind::Call":3},"builtins":{"list_take":1},"claims":{"already_claimed":2}}}"#;
        let Response::Ok { ok } = serde_json::from_str(coverage).unwrap() else {
            panic!("an ok response")
        };
        let counts: CoverageCounts = serde_json::from_value(ok).unwrap();
        assert_eq!(counts.claims["already_claimed"], 2);
        assert!(CLAIM_KINDS.contains(&"already_claimed"));

        assert!(serde_json::from_str::<Response>(r#"{"nope":1}"#).is_err());
        assert!(serde_json::from_str::<Response>(r#"{"handle":-1}"#).is_err());
    }

    #[test]
    fn a_deep_fault_payload_reads_once_the_recursion_limit_is_disabled() {
        // A fault record's payload can be as deep as the values it holds; the client reads it with serde_json's
        // recursion limit disabled (the workspace feature `unbounded_depth`, toolchain T71), on the evaluation thread.
        let depth = 300;
        let deep = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        let line = format!(
            r#"{{"fault":{{"kind":"assertion_failed","message":"m","backtrace":[],"data":{{"bindings":{deep}}},"diagnostic":"{{}}"}}}}"#
        );
        assert!(
            serde_json::from_str::<Response>(&line).is_err(),
            "serde_json's default limit is 128 levels"
        );
        let mut reader = serde_json::Deserializer::from_str(&line);
        reader.disable_recursion_limit();
        let response =
            Response::deserialize(&mut reader).expect("a deep payload reads without the limit");
        reader.end().expect("one response per line");
        let Response::Fault(answer) = response else {
            panic!("a fault answer")
        };
        let mut level = &answer.fault.data["bindings"];
        let mut seen = 1;
        while let Some(inner) = level.as_array().and_then(|a| a.first()) {
            level = inner;
            seen += 1;
        }
        assert_eq!(seen, depth);
    }

    #[test]
    fn payload_numbers_of_any_length_survive_every_response_form() {
        // A fault payload holds value JSON, whose Int is a JSON number of any length (toolchain 7.3), and an error
        // object's payload holds counts: both pass through the untagged Response and the flattened DiagnosticJson,
        // which buffer their input, and must come out as the digits the driver wrote (serde_json's
        // `arbitrary_precision`, toolchain 1.3).
        let big = "-123456789012345678901234567890";
        let line = format!(
            r#"{{"fault":{{"kind":"refinement_violation","message":"m","backtrace":[],"data":{{"actual":{big},"limit":18446744073709551615,"input":{{"a":{big},"b":0}}}},"diagnostic":"{{}}"}},"violation":null}}"#
        );
        let Response::Fault(answer) = serde_json::from_str(&line).unwrap() else {
            panic!("a fault answer")
        };
        assert_eq!(answer.fault.data["actual"].to_string(), big);
        assert_eq!(answer.fault.data["input"]["a"].to_string(), big);
        assert_eq!(
            answer.fault.data["limit"].to_string(),
            "18446744073709551615"
        );
        let mut reader = serde_json::Deserializer::from_str(&line);
        reader.disable_recursion_limit();
        let Response::Fault(answer) = Response::deserialize(&mut reader).unwrap() else {
            panic!("a fault answer")
        };
        assert_eq!(answer.fault.data["actual"].to_string(), big);

        // An error object's numeric payload passes through the flattened payload of DiagnosticJson. (The usage errors
        // of the C API have the category `interop`, which cpl-diag's `Category` gains in Milestone 2, toolchain T76;
        // until then this reads the object of `runtime_invariant`, with the payload count of `interop_runtime_busy`.)
        let error = format!(
            r#"{{"error":{{"code":"internal_error","category":"internal","severity":"error","message":"m","active_calls":2,"detail":"d","kind":"runtime_invariant","value":{}}}}}"#,
            &big[1..]
        );
        let Response::Error { error: e } = serde_json::from_str(&error).unwrap() else {
            panic!("an error answer")
        };
        assert_eq!(e.data["active_calls"], json!(2));
        assert_eq!(e.data["value"].to_string(), &big[1..]);
        assert_eq!(
            serde_json::to_string(&Response::Error { error: e }).unwrap(),
            error
        );
        let ok = format!(r#"{{"ok":{big}}}"#);
        let Response::Ok { ok: v } = serde_json::from_str(&ok).unwrap() else {
            panic!("an ok answer")
        };
        assert_eq!(v.to_string(), big);
    }

    #[test]
    fn the_banner() {
        exact(
            &Banner {
                protocol: PROTOCOL,
                runtime: "0.1.0".to_string(),
                api: "0.1".to_string(),
                program: "0".repeat(64),
            },
            &format!(
                r#"{{"protocol":1,"runtime":"0.1.0","api":"0.1","program":"{}"}}"#,
                "0".repeat(64)
            ),
        );
    }
}
