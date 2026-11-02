//! The driver's scripted host on the evaluator's side (`spec/toolchain.md` 1.14; `spec/cabi.md` 3.8).

use crate::protocol::{self, ScriptAnswer};
use cpl_eval::{CapabilityImpl, Fault, HostContext, Value};
use cpl_hir::DefId;
use cpl_rtspec::messages;
use serde_json::json;

/// A `CapabilityImpl` over a [`protocol::Script`] with the matching rule of the driver's `bind_scripted`
/// (`spec/cabi.md` 3.8): a call takes the first unused response whose operation and arguments equal its own
/// (structural equality) and answers with its result, an ill-typed one included, or reports its foreign error through
/// `cpl_eval::boundary::foreign_failure`; a call that no response matches is the `binding_protocol` fault of semantics
/// S80, its detail rendered from the cpl-rtspec template that both engines render. `label` is the binding name the
/// driver's `bind_scripted` carries; the suites bind the host with `BindOptions { label, validate_results: true }`
/// under the same label, so that both engines bind one script under one binding name (semantics 6.9).
pub struct ScriptHost {
    capability: DefId,
    label: String,
    /// The responses and whether each was used.
    responses: Vec<(protocol::ScriptResponse, bool)>,
}

impl ScriptHost {
    pub fn new(capability: DefId, label: &str, script: protocol::Script) -> ScriptHost {
        ScriptHost {
            capability,
            label: label.to_string(),
            responses: script.responses.into_iter().map(|r| (r, false)).collect(),
        }
    }
}

impl CapabilityImpl for ScriptHost {
    fn call(
        &mut self,
        op: &str,
        args: Vec<Value>,
        ctx: &mut HostContext<'_>,
    ) -> Result<Value, Fault> {
        let p = ctx.program;
        let ops = &p
            .def(self.capability)
            .kind
            .as_capability()
            .expect("a scripted host implements a capability")
            .ops;
        let index = ops
            .iter()
            .position(|o| o.name == op)
            .expect("an operation of the capability");
        let identity = cpl_eval::types::operation_identity(p, self.capability, op);
        let protocol_fault = |detail: String| {
            Fault::internal("native_protocol", detail)
                .with_data("operation", json!(identity.as_str()))
        };
        for (response, used) in &mut self.responses {
            if *used || response.op != op || response.args.len() != args.len() {
                continue;
            }
            let mut all = true;
            for (nodes, arg) in response.args.iter().zip(&args) {
                let v = crate::wire::from_nodes(p, nodes).map_err(|e| protocol_fault(e.message))?;
                if !v.structurally_equal(arg) {
                    all = false;
                    break;
                }
            }
            if !all {
                continue;
            }
            *used = true;
            return match &response.answer {
                ScriptAnswer::Result(nodes) => {
                    crate::wire::from_nodes(p, nodes).map_err(|e| protocol_fault(e.message))
                }
                ScriptAnswer::ForeignError(f) => cpl_eval::boundary::foreign_failure(
                    p,
                    self.capability,
                    index as u32,
                    &self.label,
                    &f.kind,
                    &f.message,
                    &f.details,
                ),
            };
        }
        // semantics S80: the arguments as value text, joined by ", "
        let arguments = args
            .iter()
            .zip(&ops[index].params)
            .map(|(v, param)| cpl_eval::value_text(p, v, &param.ty))
            .collect::<Vec<_>>()
            .join(", ");
        Err(Fault::internal(
            "binding_protocol",
            messages::render(
                &messages::DETAIL_BINDING_PROTOCOL_NO_RESPONSE,
                &[
                    ("binding", self.label.as_str()),
                    ("operation", identity.as_str()),
                    ("arguments", arguments.as_str()),
                ],
            ),
        )
        .with_data("operation", json!(identity.as_str()))
        .with_data("binding", json!(self.label)))
    }
}
