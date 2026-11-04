//! An owned application context: one immutable snapshot, replaced only by its checked machine.
//! `&mut self` serializes dispatch. A failed transition leaves both the snapshot and sequence unchanged.
//! Commands are returned after commit, never executed here; replay cannot repeat an external effect.
//! Like `RunEngine`, drive this API inside `on_eval_stack` when using the reference interpreter.

use crate::json::value_to_json_untyped;
use crate::{value_from_json, Budget, EvalConfig, Interpreter, RunEngine, Value};
use cpl_diag::{Category, DiagCode, Diagnostic};
use cpl_hir::{DefId, DefStatus, MachineSignature, Program, Ty};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};

/// A portable, deterministic transition receipt. Persisting it is the host's responsibility.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransitionRecord {
    pub program_digest: String,
    pub machine: String,
    pub sequence: u64,
    pub event: Json,
    pub before: Json,
    pub after: Json,
    pub commands: Vec<Json>,
}

pub struct MachineContext<'p> {
    program: &'p Program,
    machine: DefId,
    signature: MachineSignature,
    state: Value,
    sequence: u64,
    digest: String,
    config: EvalConfig,
}

impl<'p> MachineContext<'p> {
    pub fn new(
        program: &'p Program,
        machine: DefId,
        state: &Json,
        config: &EvalConfig,
    ) -> Result<Self, Diagnostic> {
        if program
            .defs
            .iter()
            .any(|d| d.status == DefStatus::HasErrors)
        {
            return Err(error(
                "machine_program_invalid",
                "the program has checker errors",
            ));
        }
        let signature = program
            .defs
            .get(machine.0 as usize)
            .and_then(|_| program.machine_signature(machine))
            .ok_or_else(|| {
                error(
                    "usage_not_a_machine",
                    "select a checked machine declaration",
                )
            })?;
        let state = validate(program, state, &signature.state, config)?;
        let mut hash = blake3::Hasher::new();
        hash.update(b"cpl-machine-replay-v1\0");
        hash.update(env!("CARGO_PKG_VERSION").as_bytes());
        hash.update(b"\0");
        hash.update(&[u8::from(config.check_contracts)]);
        for (_, module) in &program.asts {
            // Imports affect resolution even when every declaration is unchanged. Hash the entire formatted
            // module, including imports and annotations, so such a change cannot reuse an old receipt identity.
            let text = cpl_syntax::format_module(module);
            hash.update(&(text.len() as u64).to_le_bytes());
            hash.update(text.as_bytes());
        }
        Ok(Self {
            program,
            machine,
            signature,
            state,
            sequence: 0,
            digest: hash.finalize().to_hex().to_string(),
            config: config.clone(),
        })
    }

    /// A detached immutable snapshot. There is intentionally no state setter or mutable state reference.
    pub fn snapshot(&self) -> Json {
        value_to_json_untyped(self.program, &self.state)
    }

    /// The committed state, for tools that explore transitions from it without committing.
    pub fn state(&self) -> &Value {
        &self.state
    }

    /// Validate an event once, for repeated [`MachineContext::step`]s.
    pub fn event(&self, event: &Json) -> Result<Value, Diagnostic> {
        validate(self.program, event, &self.signature.event, &self.config)
    }

    /// The validated next state of one transition from `state`, which must be a state this
    /// context committed or a previous `step` returned, on a validated `event`. Nothing is
    /// committed and no record is built: bounded exploration calls this per transition.
    pub fn step(
        &self,
        engine: &mut dyn RunEngine,
        state: &Value,
        event: &Value,
    ) -> Result<Value, Diagnostic> {
        let result = engine
            .call_checked(self.machine, vec![state.clone(), event.clone()], vec![])
            .map_err(|f| f.to_diagnostic(self.program))?;
        let return_ty = &self.program.def(self.machine).kind.as_fn().unwrap().ret;
        let result = validate_value(self.program, result, return_ty, &self.config)?;
        match (self.signature.state_field, &result) {
            (Some(si), Value::Struct { fields, .. }) => Ok(fields[si as usize].clone()),
            (None, _) => Ok(result),
            _ => Err(error(
                "machine_result_invalid",
                "the machine returned a value outside its protocol",
            )),
        }
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn program_digest(&self) -> &str {
        &self.digest
    }

    /// Compute, validate, and atomically commit one transition. The caller may handle the returned commands.
    pub fn dispatch(
        &mut self,
        engine: &mut dyn RunEngine,
        event: &Json,
    ) -> Result<TransitionRecord, Diagnostic> {
        let (record, next) = self
            .propose(engine, event)
            .map_err(|d| self.contextualize(d))?;
        self.state = next;
        self.sequence = record.sequence;
        Ok(record)
    }

    /// Recompute a recorded transition without running commands. A stale or divergent receipt cannot commit.
    pub fn replay(
        &mut self,
        engine: &mut dyn RunEngine,
        expected: &TransitionRecord,
    ) -> Result<(), Diagnostic> {
        if expected.program_digest != self.digest
            || expected.machine != self.program.def(self.machine).stable_id.as_str()
            || self.sequence.checked_add(1) != Some(expected.sequence)
            || expected.before != self.snapshot()
        {
            return Err(self.contextualize(error(
                "machine_replay_mismatch",
                "the receipt does not match this program, machine, sequence, or state",
            )));
        }
        let (actual, next) = self
            .propose(engine, &expected.event)
            .map_err(|d| self.contextualize(d))?;
        if actual != *expected {
            return Err(self.contextualize(error(
                "machine_replay_mismatch",
                "replaying the event produced a different state or commands",
            )));
        }
        self.state = next;
        self.sequence = actual.sequence;
        Ok(())
    }

    fn propose(
        &self,
        engine: &mut dyn RunEngine,
        event: &Json,
    ) -> Result<(TransitionRecord, Value), Diagnostic> {
        let sequence = self.sequence.checked_add(1).ok_or_else(|| {
            error(
                "machine_sequence_exhausted",
                "the transition sequence is exhausted",
            )
        })?;
        let event = validate(self.program, event, &self.signature.event, &self.config)?;
        let event_json = value_to_json_untyped(self.program, &event);
        let result = engine
            .call_checked(self.machine, vec![self.state.clone(), event], vec![])
            .map_err(|f| f.to_diagnostic(self.program))?;
        let return_ty = &self.program.def(self.machine).kind.as_fn().unwrap().ret;
        // Validate the entire result, including commands. No partially validated result becomes visible.
        let result = validate_value(self.program, result, return_ty, &self.config)?;
        let (next, commands) = match (
            self.signature.state_field,
            self.signature.commands_field,
            &result,
        ) {
            (Some(si), Some(ci), Value::Struct { fields, .. }) => {
                let Value::List(commands) = &fields[ci as usize] else {
                    return Err(error(
                        "machine_result_invalid",
                        "the machine's commands are not a list",
                    ));
                };
                (
                    fields[si as usize].clone(),
                    commands
                        .iter()
                        .map(|v| value_to_json_untyped(self.program, v))
                        .collect(),
                )
            }
            (None, None, _) => (result, Vec::new()),
            _ => {
                return Err(error(
                    "machine_result_invalid",
                    "the machine returned a value outside its protocol",
                ))
            }
        };
        let record = TransitionRecord {
            program_digest: self.digest.clone(),
            machine: self
                .program
                .def(self.machine)
                .stable_id
                .as_str()
                .to_string(),
            sequence,
            event: event_json,
            before: self.snapshot(),
            after: value_to_json_untyped(self.program, &next),
            commands,
        };
        Ok((record, next))
    }

    fn contextualize(&self, d: Diagnostic) -> Diagnostic {
        d.with_data("machine", json!(self.program.display_path(self.machine)))
            .with_data(
                "transition_sequence",
                json!(self.sequence.saturating_add(1)),
            )
            .with_data("state_unchanged", json!(true))
    }
}

fn error(code: &'static str, message: &str) -> Diagnostic {
    Diagnostic::error(DiagCode(code), Category::Runtime, message)
}

fn validate(p: &Program, json: &Json, ty: &Ty, config: &EvalConfig) -> Result<Value, Diagnostic> {
    let value = value_from_json(p, json, ty).map_err(|e| {
        error("machine_value_invalid", &e.to_string()).with_data("pointer", json!(e.pointer))
    })?;
    validate_value(p, value, ty, config)
}

fn validate_value(
    p: &Program,
    value: Value,
    ty: &Ty,
    config: &EvalConfig,
) -> Result<Value, Diagnostic> {
    let positions = crate::boundary::walk(p, &value, ty).map_err(|e| {
        error(
            "machine_value_invalid",
            &format!("expected {}", p.display_ty(&e.expected)),
        )
        .with_data("pointer", json!(e.pointer))
    })?;
    let mut interpreter = Interpreter::new(p, config.clone());
    for pos in positions {
        if let Some(failed) = interpreter
            .refinement_failure(pos.refined, &pos.value, Budget::Fresh)
            .map_err(|f| f.to_diagnostic(p))?
        {
            let predicate = p.def(failed).kind.checked_predicate().unwrap();
            return Err(crate::Fault::refinement_violation(
                &p.def(pos.refined).name.display(),
                value_to_json_untyped(p, &pos.value),
                &predicate.predicate.text,
            )
            .with_symbol(p.def(failed).stable_id.clone())
            .with_span(predicate.predicate.span)
            .with_data("pointer", json!(pos.pointer))
            .to_diagnostic(p));
        }
    }
    Ok(value)
}
