use crate::verify::VerifyConfig;
use crate::{error, Snapshot};
use cpl_diag::Diagnostic;
use cpl_eval::{machine::MachineContext, Interpreter, RunEngine, Value};
use cpl_hir::{DefId, DefKind, FunctionMode, Program, Ty};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};
use std::collections::VecDeque;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ExploreConfig {
    pub depth: u32,
    pub max_states: usize,
    pub max_transitions: usize,
    pub verification: VerifyConfig,
    /// Pure `Bool` functions of the state, checked at every reachable state.
    pub invariants: Vec<String>,
    /// A pure `Bool` function of the state; the search reports a shortest trace reaching it.
    pub goal: Option<String>,
}
impl Default for ExploreConfig {
    fn default() -> Self {
        Self {
            depth: 20,
            max_states: 10000,
            max_transitions: 100000,
            verification: VerifyConfig::default(),
            invariants: vec![],
            goal: None,
        }
    }
}
#[derive(Debug, Serialize)]
pub struct Exploration {
    /// `exhaustive`, `bounded_pass`, `incomplete`, `counterexample`, `witness`, or `unreachable`.
    pub result: String,
    pub snapshot: String,
    pub states: usize,
    pub transitions: usize,
    pub depth: u32,
    pub initial: Json,
    /// `supplied`, or `initializer` when the program's unique `initial fn` produced the state.
    pub initial_source: String,
    pub event_alphabet: Vec<Json>,
    /// `supplied`, or `derived` when every value of the event type was enumerated.
    pub alphabet_source: String,
    pub invariants: Vec<String>,
    pub goal: Option<String>,
    pub counterexample: Option<Json>,
    pub witness: Option<Json>,
}
struct Node {
    json: Json,
    value: Value,
    parent: Option<(usize, usize)>,
    depth: u32,
}

/// Every value of a type with finitely many values, or `None` when the type is infinite or the
/// enumeration exceeds `limit`. Tags only: refinements and invariants are validated when used.
fn enumerate(p: &Program, ty: &Ty, limit: usize, depth: u32) -> Option<Vec<Value>> {
    if depth > 8 {
        return None;
    }
    let product = |types: Vec<Ty>| -> Option<Vec<Vec<Value>>> {
        let mut rows = vec![vec![]];
        for t in types {
            let values = enumerate(p, &t, limit, depth + 1)?;
            let mut next = Vec::new();
            for row in &rows {
                for v in &values {
                    let mut r: Vec<Value> = row.clone();
                    r.push(v.clone());
                    next.push(r);
                    if next.len() > limit {
                        return None;
                    }
                }
            }
            rows = next;
        }
        Some(rows)
    };
    match ty {
        Ty::Bool => Some(vec![Value::Bool(false), Value::Bool(true)]),
        Ty::Wrapper(def) => {
            let inner = &p.def(*def).kind.as_wrapper()?.inner;
            Some(
                enumerate(p, inner, limit, depth + 1)?
                    .into_iter()
                    .map(|v| Value::wrap(*def, v))
                    .collect(),
            )
        }
        Ty::Adt(def, args) => match &p.def(*def).kind {
            DefKind::Struct(s) => Some(
                product(
                    s.fields
                        .iter()
                        .map(|f| cpl_eval::types::instantiate(&f.ty, args))
                        .collect(),
                )?
                .into_iter()
                .map(|fields| Value::new_struct(*def, fields))
                .collect(),
            ),
            DefKind::Enum(e) => {
                let mut out = Vec::new();
                for (index, variant) in e.variants.iter().enumerate() {
                    let payloads = product(
                        variant
                            .payload
                            .iter()
                            .map(|t| cpl_eval::types::instantiate(t, args))
                            .collect(),
                    )?;
                    out.extend(
                        payloads
                            .into_iter()
                            .map(|payload| Value::new_enum(*def, index as u32, payload)),
                    );
                    if out.len() > limit {
                        return None;
                    }
                }
                Some(out)
            }
            _ => None,
        },
        _ => None,
    }
}

/// A pure predicate of the machine state, by name.
fn predicate(s: &Snapshot, name: &str, state: &Ty) -> Result<DefId, Diagnostic> {
    let p = &s.checked.program;
    let id = s.resolve(name)?;
    let valid = p.def(id).kind.as_fn().is_some_and(|f| {
        f.generics.is_empty()
            && f.using.is_empty()
            && f.params.len() == 1
            && f.params[0].ty == *state
            && f.ret == Ty::Bool
    }) && !p.is_mutating(id);
    if !valid {
        return Err(error(
            "explore_predicate_invalid",
            format!("{name} must be a pure function from the machine state to Bool"),
        ));
    }
    Ok(id)
}

/// Breadth-first exploration with exact state deduplication and shortest event counterexamples.
/// Completeness is relative to the finite alphabet. No commands are executed.
pub fn explore(
    s: &Snapshot,
    machine: &str,
    initial: Option<&Json>,
    events: Option<&[Json]>,
    config: &ExploreConfig,
) -> Result<Exploration, Diagnostic> {
    config.verification.validate()?;
    if config.max_states == 0
        || config.max_transitions == 0
        || events.is_some_and(<[Json]>::is_empty)
    {
        return Err(error(
            "explore_config_invalid",
            "provide a nonempty finite event alphabet and positive state/transition limits",
        ));
    }
    cpl_eval::on_eval_stack(|| {
        let p = &s.checked.program;
        let id = s.resolve(machine)?;
        let cfg = config.verification.eval();
        let signature = p.machine_signature(id).ok_or_else(|| {
            error(
                "usage_not_a_machine",
                "select a checked machine declaration",
            )
        })?;
        let (initial, initial_source) = match initial {
            Some(state) => (state.clone(), "supplied"),
            None => {
                // The program's unique parameterless initializer of this state type.
                let starts: Vec<_> = p
                    .defs()
                    .filter(|d| {
                        p.function_mode(d.id) == FunctionMode::Initial
                            && d.kind
                                .as_fn()
                                .is_some_and(|f| f.params.is_empty() && f.ret == signature.state)
                    })
                    .map(|d| d.id)
                    .collect();
                let [start] = starts.as_slice() else {
                    return Err(error(
                        "explore_initial_required",
                        "pass --state: the program has no unique parameterless initializer for this state",
                    ));
                };
                let outcome = cpl_eval::run_function(p, &p.symbol_path(*start), &[], &cfg)
                    .map_err(|e| error("explore_initial_failed", e.to_string()))?;
                let state = outcome.value.map_err(|f| f.to_diagnostic(p))?;
                (
                    cpl_eval::value_to_json(p, &state, &signature.state),
                    "initializer",
                )
            }
        };
        let (events, alphabet_source): (Vec<Json>, &str) = match events {
            Some(events) => (events.to_vec(), "supplied"),
            None => {
                let values = enumerate(p, &signature.event, 256, 0).ok_or_else(|| {
                    error(
                        "explore_alphabet_infinite",
                        "the event type has infinitely or too many values; pass --events with payload samples",
                    )
                })?;
                (
                    values
                        .iter()
                        .map(|v| cpl_eval::value_to_json(p, v, &signature.event))
                        .collect(),
                    "derived",
                )
            }
        };
        let invariants = config
            .invariants
            .iter()
            .map(|name| predicate(s, name, &signature.state).map(|id| (name, id)))
            .collect::<Result<Vec<_>, _>>()?;
        let goal = config
            .goal
            .as_ref()
            .map(|name| predicate(s, name, &signature.state))
            .transpose()?;
        let context = MachineContext::new(p, id, &initial, &cfg)?;
        let mut event_validator = Interpreter::new(p, cfg.clone());
        for (n, event) in events.iter().enumerate() {
            let value = cpl_eval::value_from_json(p, event, &signature.event)
                .map_err(|e| error("explore_event_invalid", format!("event {n}: {e}")))?;
            for position in cpl_eval::json::refined_positions(p, &value, &signature.event) {
                if !event_validator
                    .satisfies_refinement(position.refined, &position.value)
                    .map_err(|f| f.to_diagnostic(p))?
                {
                    return Err(error(
                        "explore_event_invalid",
                        format!("event {n} violates its invariant at {}", position.pointer),
                    ));
                }
            }
        }
        // Events are validated once; each transition steps from a stored state value without
        // rebuilding a context, re-validating the state, or re-hashing the program.
        let event_values = events
            .iter()
            .map(|e| context.event(e))
            .collect::<Result<Vec<_>, _>>()?;
        let mut engine = Interpreter::new(p, cfg.clone());
        let mut nodes = vec![Node {
            json: context.snapshot(),
            value: context.state().clone(),
            parent: None,
            depth: 0,
        }];
        let mut seen =
            std::collections::HashSet::from([serde_json::to_string(&nodes[0].json).unwrap()]);
        let mut queue = VecDeque::from([0]);
        let mut out = Exploration {
            result: "exhaustive".into(),
            snapshot: s.digest(),
            states: 1,
            transitions: 0,
            depth: config.depth,
            initial: initial.clone(),
            initial_source: initial_source.into(),
            event_alphabet: events.clone(),
            alphabet_source: alphabet_source.into(),
            invariants: config.invariants.clone(),
            goal: config.goal.clone(),
            counterexample: None,
            witness: None,
        };
        let trace = |nodes: &[Node], mut at: usize, last: Option<&Json>| {
            let mut trace: Vec<Json> = last.into_iter().cloned().collect();
            while let Some((parent, e)) = nodes[at].parent {
                trace.push(events[e].clone());
                at = parent;
            }
            trace.reverse();
            trace
        };
        // `Ok(true)` when the predicate holds; a fault while evaluating it is its diagnostic.
        let holds = |engine: &mut Interpreter, id: DefId, state: &Value| -> Result<bool, Json> {
            match engine.call_checked(id, vec![state.clone()], vec![]) {
                Ok(Value::Bool(b)) => Ok(b),
                Ok(_) => Err(json!({"message": "the predicate did not return a Bool"})),
                Err(f) => Err(json!(f.to_diagnostic(p).to_json(&p.source_map))),
            }
        };
        // Check a newly reached state: invariant violations are counterexamples, a goal is a witness.
        let visit = |engine: &mut Interpreter,
                     nodes: &[Node],
                     n: usize,
                     out: &mut Exploration|
         -> bool {
            let state = &nodes[n].json;
            for (name, inv) in &invariants {
                let failure = match holds(engine, *inv, &nodes[n].value) {
                    Ok(true) => continue,
                    Ok(false) => json!(null),
                    Err(fault) => fault,
                };
                out.result = "counterexample".into();
                out.counterexample = Some(
                    json!({"initial": nodes[0].json, "events": trace(nodes, n, None),
                    "state": state, "invariant": name, "fault": failure}),
                );
                return true;
            }
            if let Some(goal) = goal {
                match holds(engine, goal, &nodes[n].value) {
                    Ok(false) => (),
                    Ok(true) => {
                        out.result = "witness".into();
                        out.witness = Some(
                            json!({"initial": nodes[0].json, "events": trace(nodes, n, None), "state": state}),
                        );
                        return true;
                    }
                    Err(fault) => {
                        out.result = "counterexample".into();
                        out.counterexample = Some(
                            json!({"initial": nodes[0].json, "events": trace(nodes, n, None),
                            "state": state, "goal": config.goal, "fault": fault}),
                        );
                        return true;
                    }
                }
            }
            false
        };
        if visit(&mut engine, &nodes, 0, &mut out) {
            return Ok(out);
        }
        while let Some(n) = queue.pop_front() {
            if nodes[n].depth == config.depth {
                out.result = "bounded_pass".into();
                continue;
            }
            for (event_index, event) in events.iter().enumerate() {
                if out.transitions == config.max_transitions {
                    out.result = "incomplete".into();
                    return Ok(out);
                }
                out.transitions += 1;
                match context.step(&mut engine, &nodes[n].value, &event_values[event_index]) {
                    Err(d) => {
                        if d.code.as_str() == "resource_exhaustion" {
                            out.result = "incomplete".into();
                            return Ok(out);
                        }
                        let d = d
                            .with_data("machine", json!(p.display_path(id)))
                            .with_data("transition_sequence", json!(nodes[n].depth + 1))
                            .with_data("state_unchanged", json!(true));
                        out.result = "counterexample".into();
                        out.counterexample = Some(
                            json!({"initial": nodes[0].json, "events": trace(&nodes, n, Some(event)), "before": nodes[n].json, "fault": d.to_json(&p.source_map)}),
                        );
                        return Ok(out);
                    }
                    Ok(next) => {
                        let json = cpl_eval::json::value_to_json_untyped(p, &next);
                        let key = serde_json::to_string(&json).unwrap();
                        if !seen.contains(&key) {
                            if nodes.len() == config.max_states {
                                out.result = "incomplete".into();
                                return Ok(out);
                            }
                            seen.insert(key);
                            let depth = nodes[n].depth + 1;
                            queue.push_back(nodes.len());
                            nodes.push(Node {
                                json,
                                value: next,
                                parent: Some((n, event_index)),
                                depth,
                            });
                            out.states = nodes.len();
                            if visit(&mut engine, &nodes, nodes.len() - 1, &mut out) {
                                return Ok(out);
                            }
                        }
                    }
                }
            }
        }
        if goal.is_some() && out.result == "exhaustive" {
            out.result = "unreachable".into();
        }
        Ok(out)
    })
}
