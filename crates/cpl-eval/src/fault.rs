//! Faults (`spec/toolchain.md` 1.7 and 1.7.3; semantics 5.2-5.6, 6.7, S34,
//! S41, S57).
//!
//! A [`Fault`] is an execution failure that no CPL construct can catch
//! (semantics 6.6). It is built where it is raised: its kind, message, and
//! payload ([`Fault::data`], the code-specific keys of 1.7.3) and, when the
//! raiser knows them, the blamed declaration ([`Fault::symbol`]), the facet,
//! and the span. The interpreter completes it while it propagates: the
//! enclosing declaration for faults raised by an operator or a builtin, the
//! S34 rewrite (`facet: contract`, `clause`, `symbol`) when it escapes a
//! contract clause (never for an `internal_error`, which blames no
//! declaration, semantics 6.7), and the backtrace. [`Fault::to_diagnostic`]
//! turns it into the structured diagnostic of semantics 9.2.
//!
//! Every message is rendered from its `cpl_rtspec::messages` template
//! (toolchain 1.7.4, T70), the one source the native runtime's messages are
//! generated from, so the two engines cannot drift (semantics S75).
//!
//! | [`FaultKind`] | code | category |
//! | - | - | - |
//! | `ContractViolation(k)` | `contract_violation` | `contract` |
//! | `AssertionFailed` | `assertion_failed` | `test` |
//! | `UnresolvedHole` | `unresolved_hole` | `hole` |
//! | `DivisionByZero` | `division_by_zero` | `runtime` |
//! | `RefinementViolation` | `refinement_violation` | `runtime` |
//! | `ResourceExhaustion(r)` | `resource_exhaustion` | `internal` |
//! | `Internal` | `internal_error` | `internal` |

use crate::json::int_to_json;
use cpl_diag::json::BindingKind;
use cpl_diag::{
    BacktraceFrame, Category, DiagCode, Diagnostic, Facet, Span, StableId, RESERVED_KEYS,
};
use cpl_hir::{DefId, Program};
use cpl_rtspec::messages::{self, render};
use num_bigint::BigInt;
use serde::Serialize;
use serde_json::{json, Map, Value as Json};
use std::fmt;

/// The diagnostic codes of the runtime faults (semantics 6.7 and 9.4).
pub mod codes {
    use cpl_diag::DiagCode;

    pub const CONTRACT_VIOLATION: DiagCode = DiagCode("contract_violation");
    pub const ASSERTION_FAILED: DiagCode = DiagCode("assertion_failed");
    pub const UNRESOLVED_HOLE: DiagCode = DiagCode("unresolved_hole");
    pub const DIVISION_BY_ZERO: DiagCode = DiagCode("division_by_zero");
    pub const REFINEMENT_VIOLATION: DiagCode = DiagCode("refinement_violation");
    pub const RESOURCE_EXHAUSTION: DiagCode = DiagCode("resource_exhaustion");
    pub const INTERNAL_ERROR: DiagCode = DiagCode("internal_error");
}

/// One execution fault (`spec/toolchain.md` 1.7).
#[derive(Clone, Debug)]
pub struct Fault {
    pub kind: FaultKind,
    /// One sentence; never the only carrier of a fact a payload key carries.
    pub message: String,
    /// The blamed declaration (Section 1.7.3): the callee whose clause
    /// failed, the enclosing declaration of an operator fault, the function
    /// whose parameter was violated; `None` for `internal_error`.
    pub symbol: Option<StableId>,
    /// `contract` for every contract violation and for a fault raised while
    /// evaluating a clause (semantics S34, S57); `body` otherwise. `None`
    /// lets [`Fault::to_diagnostic`] choose by that rule.
    pub facet: Option<Facet>,
    pub span: Option<Span>,
    /// The active declarations, innermost first.
    pub backtrace: Vec<Frame>,
    /// The Section 1.7.3 payload, flattened into the diagnostic object;
    /// never a key of [`RESERVED_KEYS`].
    pub data: Map<String, Json>,
}

/// One backtrace entry: an active declaration and the span of the call
/// that entered it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub def: DefId,
    pub span: Option<Span>,
}

/// What failed (`spec/toolchain.md` 1.7; semantics 6.7).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum FaultKind {
    ContractViolation(ClauseKind),
    AssertionFailed,
    UnresolvedHole,
    DivisionByZero,
    /// Boundary only: a `cpl run` argument (semantics S39); from Milestone 2
    /// also a C export argument (S81) and the result of a validating binding
    /// (S77, [`crate::boundary::validate_result`]).
    RefinementViolation,
    ResourceExhaustion(Resource),
    /// Includes an operation call on an unbound handle (`kind: "unbound_capability"`).
    Internal,
}

/// The kind of a failed clause: `clause_kind` (semantics 9.2).
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClauseKind {
    Requires,
    Ensures,
    Invariant,
    Decreases,
}

/// The exhausted resource: `resource` (semantics 6.7, S38).
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Resource {
    Stack,
    /// Safe Rust cannot observe allocation failure, so the evaluator's own
    /// code never raises it; from Milestone 2 a native fault rebuilt from a
    /// driver's record carries it, as does a failure the evaluator's adapter
    /// host reports (semantics S76).
    Memory,
    Time,
    /// The opt-in step budget semantics S38 allows (`EvalConfig::max_steps`),
    /// which `cpl-evolve` sets to measure improvements; no CLI flag defines it.
    Steps,
}

/// The party at fault for a contract violation: `attributed_to`
/// (semantics S57).
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttributedTo {
    /// A `requires` clause: the caller had to establish it.
    Caller,
    /// A function's `ensures`, `invariant`, or `decreases` clause.
    Body,
    /// A capability operation's `ensures` clause: the host binding; from
    /// Milestone 2 also the `refinement_violation` of a validated result
    /// (semantics S77).
    Binding,
}

impl ClauseKind {
    /// `"requires"`, `"ensures"`, `"invariant"`, or `"decreases"`.
    pub fn as_str(self) -> &'static str {
        match self {
            ClauseKind::Requires => "requires",
            ClauseKind::Ensures => "ensures",
            ClauseKind::Invariant => "invariant",
            ClauseKind::Decreases => "decreases",
        }
    }

    /// The attribution of a function's clause of this kind (S57): `caller`
    /// for `requires`, `body` for the others. A capability operation's
    /// `ensures` is `binding` instead (S31), which the raiser states.
    pub fn default_attribution(self) -> AttributedTo {
        match self {
            ClauseKind::Requires => AttributedTo::Caller,
            _ => AttributedTo::Body,
        }
    }
}

impl Resource {
    /// `"stack"`, `"memory"`, `"time"`, or `"steps"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Resource::Stack => "stack",
            Resource::Memory => "memory",
            Resource::Time => "time",
            Resource::Steps => "steps",
        }
    }
}

impl AttributedTo {
    /// `"caller"`, `"body"`, or `"binding"`.
    pub fn as_str(self) -> &'static str {
        match self {
            AttributedTo::Caller => "caller",
            AttributedTo::Body => "body",
            AttributedTo::Binding => "binding",
        }
    }
}

impl FaultKind {
    /// The diagnostic code (Section 1.7.3).
    pub fn code(self) -> DiagCode {
        match self {
            FaultKind::ContractViolation(_) => codes::CONTRACT_VIOLATION,
            FaultKind::AssertionFailed => codes::ASSERTION_FAILED,
            FaultKind::UnresolvedHole => codes::UNRESOLVED_HOLE,
            FaultKind::DivisionByZero => codes::DIVISION_BY_ZERO,
            FaultKind::RefinementViolation => codes::REFINEMENT_VIOLATION,
            FaultKind::ResourceExhaustion(_) => codes::RESOURCE_EXHAUSTION,
            FaultKind::Internal => codes::INTERNAL_ERROR,
        }
    }

    /// The diagnostic category (Section 1.7.3).
    pub fn category(self) -> Category {
        match self {
            FaultKind::ContractViolation(_) => Category::Contract,
            FaultKind::AssertionFailed => Category::Test,
            FaultKind::UnresolvedHole => Category::Hole,
            FaultKind::DivisionByZero | FaultKind::RefinementViolation => Category::Runtime,
            FaultKind::ResourceExhaustion(_) | FaultKind::Internal => Category::Internal,
        }
    }
}

impl Fault {
    /// A fault with no symbol, facet, span, backtrace, or payload.
    pub fn new(kind: FaultKind, message: impl Into<String>) -> Fault {
        Fault {
            kind,
            message: message.into(),
            symbol: None,
            facet: None,
            span: None,
            backtrace: Vec::new(),
            data: Map::new(),
        }
    }

    /// `contract_violation` of a clause whose canonical text is `clause`:
    /// `facet: "contract"` and the payload keys `clause_kind`,
    /// `attributed_to`, `clause`, and `status: "failed"` (semantics 5.6).
    /// The raiser adds `symbol`, `span` (the clause), `input`, and, per
    /// clause kind, `actual`, `caller`, `iteration`.
    pub fn contract_violation(
        kind: ClauseKind,
        attributed_to: AttributedTo,
        clause: &str,
    ) -> Fault {
        Fault::new(
            FaultKind::ContractViolation(kind),
            render(
                &messages::CONTRACT_VIOLATION,
                &[("clause_kind", kind.as_str()), ("clause", clause)],
            ),
        )
        .with_facet(Facet::Contract)
        .with_data("clause_kind", json!(kind.as_str()))
        .with_data("attributed_to", json!(attributed_to.as_str()))
        .with_data("clause", json!(clause))
        .with_data("status", json!("failed"))
    }

    /// `assertion_failed` (semantics S41): `clause` is the canonical text of
    /// the asserted expression and `bindings` the visible bindings with
    /// their values, innermost first ([`binding_json`]).
    pub fn assertion_failed(clause: &str, bindings: Vec<Json>) -> Fault {
        Fault::new(
            FaultKind::AssertionFailed,
            render(&messages::ASSERTION_FAILED, &[("clause", clause)]),
        )
        .with_data("clause", json!(clause))
        .with_data("bindings", Json::Array(bindings))
    }

    /// `unresolved_hole` (semantics 6.7, 8.1): `hole` without the `?`,
    /// `expected_type` rendered as written, and the visible bindings with
    /// their values, innermost first ([`binding_json`]).
    pub fn unresolved_hole(hole: &str, expected_type: &str, bindings: Vec<Json>) -> Fault {
        Fault::new(
            FaultKind::UnresolvedHole,
            render(&messages::UNRESOLVED_HOLE, &[("hole", hole)]),
        )
        .with_data("hole", json!(hole))
        .with_data("expected_type", json!(expected_type))
        .with_data("bindings", Json::Array(bindings))
    }

    /// `division_by_zero` (semantics 6.3, stdlib S8) for `a / b` or `a % b`
    /// with `b == 0`: `input: {"a": ..., "b": 0}`.
    pub fn division_by_zero(a: &BigInt, b: &BigInt) -> Fault {
        let mut input = Map::new();
        input.insert("a".to_string(), int_to_json(a));
        input.insert("b".to_string(), int_to_json(b));
        Fault::new(
            FaultKind::DivisionByZero,
            render(&messages::DIVISION_BY_ZERO, &[]),
        )
        .with_data("input", Json::Object(input))
    }

    /// `refinement_violation` (semantics 6.7, S39): a value entered at a
    /// boundary at refinement type `expected_type` (rendered) but `actual`
    /// (value JSON) fails the predicate whose text is `clause`.
    pub fn refinement_violation(expected_type: &str, actual: Json, clause: &str) -> Fault {
        Fault::new(
            FaultKind::RefinementViolation,
            render(
                &messages::REFINEMENT_VIOLATION,
                &[("expected_type", expected_type), ("clause", clause)],
            ),
        )
        .with_data("expected_type", json!(expected_type))
        .with_data("actual", actual)
        .with_data("clause", json!(clause))
    }

    /// `resource_exhaustion` (semantics 6.7, S38): `resource` and `limit`
    /// (the call-depth limit in activations, the time budget in
    /// milliseconds, the step budget in steps).
    pub fn resource_exhaustion(resource: Resource, limit: u64) -> Fault {
        let template = match resource {
            Resource::Stack => &messages::RESOURCE_STACK,
            Resource::Time => &messages::RESOURCE_TIME,
            Resource::Memory => &messages::RESOURCE_MEMORY,
            Resource::Steps => &messages::RESOURCE_STEPS,
        };
        let message = render(template, &[("limit", &limit.to_string())]);
        Fault::new(FaultKind::ResourceExhaustion(resource), message)
            .with_data("resource", json!(resource.as_str()))
            .with_data("limit", json!(limit))
    }

    /// `internal_error` (semantics 6.7): an impossible state, named by the
    /// free-form identifier `kind` (`"type_confusion"`,
    /// `"unbound_capability"`, `"refinement_proof_unsound"`,
    /// `"no_matching_arm"`, ...) and explained by `detail`.
    pub fn internal(kind: &str, detail: impl Into<String>) -> Fault {
        let detail = detail.into();
        Fault::new(
            FaultKind::Internal,
            render(
                &messages::INTERNAL_ERROR,
                &[("kind", kind), ("detail", &detail)],
            ),
        )
        .with_data("kind", json!(kind))
        .with_data("detail", json!(detail))
    }

    // ---- builders --------------------------------------------------------------

    /// Blame the declaration with identity `id`.
    pub fn with_symbol(mut self, id: StableId) -> Fault {
        self.symbol = Some(id);
        self
    }

    /// Blame `def`: its stable identity (an `@id` value or its qualified
    /// name, semantics S2) becomes the symbol.
    pub fn blamed_on(self, p: &Program, def: DefId) -> Fault {
        match p.defs.get(def.0 as usize) {
            Some(d) => self.with_symbol(d.stable_id.clone()),
            None => self,
        }
    }

    pub fn with_facet(mut self, facet: Facet) -> Fault {
        self.facet = Some(facet);
        self
    }

    pub fn with_span(mut self, span: Span) -> Fault {
        self.span = Some(span);
        self
    }

    /// Add (or replace) a payload key. `key` must not be one of
    /// [`RESERVED_KEYS`] (T5): those are the diagnostic's own fields.
    pub fn with_data(mut self, key: &str, value: Json) -> Fault {
        debug_assert!(
            !RESERVED_KEYS.contains(&key),
            "`{key}` is a reserved diagnostic key (T5)"
        );
        self.data.insert(key.to_string(), value);
        self
    }

    /// Append a frame (the next outer activation) to the backtrace.
    pub fn push_frame(&mut self, frame: Frame) {
        self.backtrace.push(frame);
    }

    // ---- queries ---------------------------------------------------------------

    /// The diagnostic code; the shrinker compares it (semantics S45).
    pub fn code(&self) -> DiagCode {
        self.kind.code()
    }

    pub fn category(&self) -> Category {
        self.kind.category()
    }

    /// The `clause` payload, if any.
    pub fn clause(&self) -> Option<&str> {
        self.data.get("clause").and_then(Json::as_str)
    }

    /// The structured diagnostic of this fault (Section 1.7.3, semantics
    /// 9.2): severity `error`; the code and category of the kind; `symbol`
    /// with its `qualified_name` (the qualified name of the declaration the
    /// identity names, or the identity itself when the program has no such
    /// declaration, as for a provisional std identity); `facet` always
    /// `contract` for a contract violation, else as set, else `body` for a
    /// fault that blames a declaration; the backtrace by qualified name; and the
    /// payload, in which the kind's own keys (`clause_kind`, `status:
    /// "failed"`, `resource`) are always present and win over `data`. An
    /// `internal_error` has neither `symbol` nor `facet`, whatever the fault
    /// carries (semantics 6.7; the `-` of the 1.7.3 table).
    pub fn to_diagnostic(&self, p: &Program) -> Diagnostic {
        let mut d = Diagnostic::error(self.code(), self.category(), self.message.clone());
        d.span = self.span;
        let symbol = match self.kind {
            FaultKind::Internal => None,
            _ => self.symbol.as_ref(),
        };
        if let Some(id) = symbol {
            let qualified = match p.by_stable_id(id) {
                Some(def) => p.display_path(def),
                None => id.as_str().to_string(),
            };
            d.symbol = Some(id.clone());
            d.qualified_name = Some(qualified);
        }
        d.facet = match self.kind {
            // Every contract violation blames the contract facet (S57).
            FaultKind::ContractViolation(_) => Some(Facet::Contract),
            // No symbol, hence no facet (semantics 6.7).
            FaultKind::Internal => None,
            _ => self.facet.or(symbol.map(|_| Facet::Body)),
        };
        d.backtrace = self
            .backtrace
            .iter()
            .map(|f| BacktraceFrame {
                qualified_name: def_path(p, f.def),
                span: f.span,
            })
            .collect();
        for (key, value) in &self.data {
            if !RESERVED_KEYS.contains(&key.as_str()) {
                d.data.insert(key.clone(), value.clone());
            }
        }
        match self.kind {
            FaultKind::ContractViolation(kind) => {
                d.data
                    .insert("clause_kind".to_string(), json!(kind.as_str()));
                d.data.insert("status".to_string(), json!("failed"));
                d.data
                    .entry("attributed_to".to_string())
                    .or_insert_with(|| json!(kind.default_attribution().as_str()));
            }
            FaultKind::ResourceExhaustion(resource) => {
                d.data
                    .insert("resource".to_string(), json!(resource.as_str()));
            }
            FaultKind::Internal => {
                d.data
                    .entry("kind".to_string())
                    .or_insert_with(|| json!("unspecified"));
            }
            _ => {}
        }
        d
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code(), self.message)
    }
}

impl std::error::Error for Fault {}

/// The qualified name of `def`, or `{def N}` for an id the program lacks.
fn def_path(p: &Program, def: DefId) -> String {
    match p.defs.get(def.0 as usize) {
        Some(d) if (d.module.0 as usize) < p.modules.len() => p.display_path(def),
        _ => format!("{{def {}}}", def.0),
    }
}

/// One entry of the `bindings` payload of `assertion_failed` and
/// `unresolved_hole` (semantics S41, 6.7): the hole report's binding object
/// of semantics 8.3 (`name`, `type` rendered with type parameters by name,
/// `kind`, and `mutable: true` for a `var`) plus the binding's `value` as
/// value JSON.
pub fn binding_json(name: &str, ty: &str, kind: BindingKind, mutable: bool, value: Json) -> Json {
    let mut m = Map::new();
    m.insert("name".to_string(), json!(name));
    m.insert("type".to_string(), json!(ty));
    m.insert("kind".to_string(), json!(kind.as_str()));
    if mutable {
        m.insert("mutable".to_string(), json!(true));
    }
    m.insert("value".to_string(), value);
    Json::Object(m)
}
