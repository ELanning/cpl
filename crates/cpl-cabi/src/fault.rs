//! Native faults as evaluator faults (`spec/toolchain.md` 1.14; `spec/cabi.md` 3.8, 7.4). Owner: the native-engine
//! track (N).

use crate::protocol;
use cpl_diag::{FileId, Span, StableId};
use cpl_eval::{ClauseKind, Fault, FaultKind, Frame, Resource};
use cpl_hir::{DefId, Program};

fn span(s: &protocol::SpanRef) -> Span {
    Span::new(FileId(s.file), s.start, s.end)
}

/// The evaluator's `Fault` for a native fault record: the kind, the symbol (`StableId::parse`), spans and frames from
/// the `Program`'s own ids, and the payload parsed with serde_json's recursion limit disabled, on the evaluation thread
/// (T71). From there cpl-cli and the runner render it with the evaluator's code (`Fault::to_diagnostic`, toolchain
/// 1.7.3).
pub fn fault_from_record(p: &Program, r: &protocol::FaultRecord) -> Fault {
    let _ = p;
    let kind = match r.kind.as_str() {
        "contract_violation" => FaultKind::ContractViolation(match r.clause_kind.as_deref() {
            Some("ensures") => ClauseKind::Ensures,
            Some("invariant") => ClauseKind::Invariant,
            Some("decreases") => ClauseKind::Decreases,
            _ => ClauseKind::Requires,
        }),
        "assertion_failed" => FaultKind::AssertionFailed,
        "unresolved_hole" => FaultKind::UnresolvedHole,
        "division_by_zero" => FaultKind::DivisionByZero,
        "refinement_violation" => FaultKind::RefinementViolation,
        "resource_exhaustion" => FaultKind::ResourceExhaustion(match r.resource.as_deref() {
            Some("memory") => Resource::Memory,
            Some("time") => Resource::Time,
            Some("steps") => Resource::Steps,
            _ => Resource::Stack,
        }),
        _ => FaultKind::Internal,
    };
    let mut fault = Fault::new(kind, r.message.clone());
    fault.symbol = r
        .symbol
        .as_ref()
        .and_then(|s| StableId::parse(s.as_str()).ok());
    fault.facet = r.facet;
    fault.span = r.span.as_ref().map(span);
    fault.backtrace = r
        .backtrace
        .iter()
        .map(|f| Frame {
            def: DefId(f.def),
            span: f.span.as_ref().map(span),
        })
        .collect();
    fault.data = r.data.clone();
    fault
}
