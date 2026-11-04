//! Generated differential testing: the old and new versions of a function run on the same
//! type-directed inputs, drawn deterministically from the seed. Inputs outside the old function's
//! domain (an argument refinement or invariant, the function's own `requires`, or an optional
//! filter predicate) are discarded. Agreement on every case is test evidence, never equivalence.
use crate::{error, verify::VerifyConfig, Snapshot};
use cpl_diag::{Diagnostic, StableId};
use cpl_eval::{ClauseKind, FaultKind, RunArg, RunError, Value};
use cpl_hir::{DefKind, Program, Ty};
use num_bigint::BigInt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Differential {
    /// The function compared, by stable identity or name in the base snapshot.
    pub symbol: String,
    /// Valid cases to run; defaults to the change's `verification.cases`.
    #[serde(default)]
    pub cases: Option<u32>,
    /// A pure `Bool` function of the base program taking the same parameters. Inputs for which it
    /// is false are outside the compared domain, such as "existing positive behavior".
    #[serde(default)]
    pub filter: Option<String>,
}

/// A splitmix64 stream: small, deterministic, and independent of any other generator's draws.
pub(crate) struct Rng(u64);
impl Rng {
    pub(crate) fn new(seed: u64, symbol: &StableId, case: u32) -> Rng {
        let mut h = blake3::Hasher::new();
        h.update(b"cpl-differential-v1\0");
        h.update(&seed.to_le_bytes());
        h.update(symbol.as_str().as_bytes());
        h.update(&case.to_le_bytes());
        Rng(u64::from_le_bytes(
            h.finalize().as_bytes()[..8].try_into().unwrap(),
        ))
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next() % n
        }
    }
}

const ALPHABET: [char; 12] = ['a', 'b', 'z', 'A', ' ', '0', '9', '-', '\n', 'é', 'λ', '🙂'];
const MAX_DEPTH: u32 = 24;

/// A value of `ty`; `None` for types without generated values (generics, capabilities).
pub(crate) fn arbitrary(
    p: &Program,
    ty: &Ty,
    rng: &mut Rng,
    size: u32,
    depth: u32,
) -> Option<Value> {
    if depth > MAX_DEPTH {
        return None;
    }
    let size = if depth > 4 { size / 2 } else { size };
    Some(match ty {
        Ty::Int => Value::Int(match rng.below(8) {
            0 => BigInt::from(0),
            1 => BigInt::from(if rng.below(2) == 0 { 1 } else { -1 }),
            2 if size >= 8 => {
                let n = BigInt::from(rng.next()) << (rng.below(64) as usize);
                if rng.below(2) == 0 {
                    n
                } else {
                    -n
                }
            }
            _ => BigInt::from(rng.below(2 * u64::from(size) + 1) as i64 - i64::from(size)),
        }),
        Ty::Bool => Value::Bool(rng.below(2) == 1),
        Ty::Text => {
            let len = rng.below(u64::from(size) + 1);
            let text: String = (0..len)
                .map(|_| ALPHABET[rng.below(ALPHABET.len() as u64) as usize])
                .collect();
            Value::text(&text)
        }
        Ty::Bytes => {
            let len = rng.below(u64::from(size) + 1);
            Value::bytes(&(0..len).map(|_| rng.below(256) as u8).collect::<Vec<_>>())
        }
        Ty::List(element) => {
            let len = rng.below(u64::from(size.min(16)) + 1);
            let items = (0..len)
                .map(|_| arbitrary(p, element, rng, size, depth + 1))
                .collect::<Option<Vec<_>>>()?;
            Value::list(items)
        }
        Ty::Wrapper(def) => {
            let inner = &p.def(*def).kind.as_wrapper()?.inner;
            Value::wrap(*def, arbitrary(p, inner, rng, size, depth + 1)?)
        }
        Ty::Refined(_, def) => {
            // The base type; the boundary rejects values outside the refinement.
            let base = cpl_eval::types::refinement_base(p, *def)?.clone();
            arbitrary(p, &base, rng, size, depth + 1)?
        }
        Ty::Adt(def, args) => match &p.def(*def).kind {
            DefKind::Struct(s) => {
                let fields = s
                    .fields
                    .iter()
                    .map(|f| {
                        let ty = cpl_eval::types::instantiate(&f.ty, args);
                        arbitrary(p, &ty, rng, size, depth + 1)
                    })
                    .collect::<Option<Vec<_>>>()?;
                Value::new_struct(*def, fields)
            }
            DefKind::Enum(e) if !e.variants.is_empty() => {
                // Deep or small values prefer payload-free variants, so recursive types terminate.
                let nullary: Vec<_> = (0..e.variants.len())
                    .filter(|i| e.variants[*i].payload.is_empty())
                    .collect();
                let index = if (depth > 3 || size == 0) && !nullary.is_empty() {
                    nullary[rng.below(nullary.len() as u64) as usize]
                } else {
                    rng.below(e.variants.len() as u64) as usize
                };
                let payload = e.variants[index]
                    .payload
                    .iter()
                    .map(|t| {
                        let ty = cpl_eval::types::instantiate(t, args);
                        arbitrary(p, &ty, rng, size, depth + 1)
                    })
                    .collect::<Option<Vec<_>>>()?;
                Value::new_enum(*def, index as u32, payload)
            }
            _ => return None,
        },
        Ty::Param(_) | Ty::Capability(_) | Ty::Gen(_) | Ty::Error => return None,
    })
}

#[derive(Debug, PartialEq)]
pub(crate) enum Outcome {
    Value(Json),
    /// A fault, compared by diagnostic code; messages and spans can differ between versions.
    Fault(String, Json),
    /// The input is outside the called function's domain.
    Outside,
}
impl Outcome {
    pub(crate) fn json(&self) -> Json {
        match self {
            Outcome::Value(v) => json!({"value": v}),
            Outcome::Fault(_, d) => json!({"fault": d}),
            Outcome::Outside => json!({"outside_domain": true}),
        }
    }
    pub(crate) fn same(&self, other: &Outcome) -> bool {
        match (self, other) {
            (Outcome::Value(a), Outcome::Value(b)) => a == b,
            (Outcome::Fault(a, _), Outcome::Fault(b, _)) => a == b,
            (Outcome::Outside, Outcome::Outside) => true,
            _ => false,
        }
    }
}

pub(crate) fn run(
    s: &Snapshot,
    id: &StableId,
    args: &[Json],
    cfg: &VerifyConfig,
) -> Result<Outcome, Diagnostic> {
    run_counting(s, id, args, cfg, None).map(|(o, _)| o)
}

/// [`run`] with a step budget, also returning the steps taken (`None` when the budget ran out).
pub(crate) fn run_counting(
    s: &Snapshot,
    id: &StableId,
    args: &[Json],
    cfg: &VerifyConfig,
    max_steps: Option<u64>,
) -> Result<(Outcome, Option<u64>), Diagnostic> {
    let p = &s.checked.program;
    let def = p
        .by_stable_id(id)
        .ok_or_else(|| error("differential_target_missing", id.to_string()))?;
    let mut eval = cfg.eval();
    eval.max_steps = max_steps;
    let call = cpl_eval::run_function(
        p,
        &p.symbol_path(def),
        &args.iter().cloned().map(RunArg::Json).collect::<Vec<_>>(),
        &eval,
    );
    let outcome = match call {
        Ok(outcome) => outcome,
        Err(RunError::Arg { .. }) => return Ok((Outcome::Outside, Some(0))),
        Err(e) => return Err(error("differential_not_runnable", e.to_string())),
    };
    let steps = outcome.steps;
    let exhausted = matches!(&outcome.value, Err(f) if f.kind == FaultKind::ResourceExhaustion(cpl_eval::Resource::Steps));
    let result = match outcome.value {
        Ok(v) => Outcome::Value(cpl_eval::value_to_json(p, &v, &outcome.return_ty)),
        Err(f) => {
            // The called function's own precondition, unlike a nested call's, has no caller.
            let boundary = f.symbol.as_ref() == Some(id) && !f.data.contains_key("caller");
            match f.kind {
                FaultKind::RefinementViolation => Outcome::Outside,
                FaultKind::ContractViolation(ClauseKind::Requires) if boundary => Outcome::Outside,
                _ => {
                    let d = f.to_diagnostic(p);
                    Outcome::Fault(d.code.as_str().to_string(), json!(d.to_json(&p.source_map)))
                }
            }
        }
    };
    Ok((result, (!exhausted).then_some(steps)))
}

/// Smaller variants of one argument in value JSON: integers toward zero, shorter texts and lists,
/// and the same applied inside struct fields and enum or wrapper payloads. Tags never change, and
/// every candidate is re-validated by running it, so an invalid candidate is simply skipped.
fn shrinks(v: &Json) -> Vec<Json> {
    match v {
        Json::Number(n) => {
            let Ok(n) = n.to_string().parse::<BigInt>() else {
                return vec![];
            };
            if n == BigInt::from(0) {
                return vec![];
            }
            let mut out = vec![BigInt::from(0), &n / 2];
            out.push(if n > BigInt::from(0) { &n - 1 } else { &n + 1 });
            out.dedup();
            out.into_iter()
                .filter(|c| *c != n)
                .filter_map(|c| serde_json::from_str(&c.to_string()).ok())
                .collect()
        }
        Json::String(text) if !text.is_empty() => {
            let chars: Vec<char> = text.chars().collect();
            vec![
                Json::String(String::new()),
                Json::String(chars[..chars.len() / 2].iter().collect()),
                Json::String(chars[..chars.len() - 1].iter().collect()),
            ]
        }
        Json::Array(items) if !items.is_empty() => {
            let mut out = vec![Json::Array(vec![])];
            for i in 0..items.len().min(8) {
                let mut fewer = items.clone();
                fewer.remove(i);
                out.push(Json::Array(fewer));
            }
            for i in 0..items.len().min(8) {
                for c in shrinks(&items[i]) {
                    let mut smaller = items.clone();
                    smaller[i] = c;
                    out.push(Json::Array(smaller));
                }
            }
            out
        }
        Json::Object(map) => {
            let mut out = vec![];
            for (key, value) in map {
                if key == "$type" || key == "$variant" {
                    continue;
                }
                for c in shrinks(value) {
                    let mut smaller = map.clone();
                    smaller.insert(key.clone(), c);
                    out.push(Json::Object(smaller));
                }
            }
            out
        }
        _ => vec![],
    }
}

/// Compare `base` and `candidate` on generated inputs. Returns behavior records and whether the
/// comparison succeeded: no mismatch, and at least one input inside the domain.
pub fn compare(
    base: &Snapshot,
    candidate: &Snapshot,
    spec: &Differential,
    config: &VerifyConfig,
) -> Result<(Vec<Json>, bool), Diagnostic> {
    // One evaluation stack for every call: each `run_function` then evaluates in place.
    cpl_eval::on_eval_stack(|| compare_on_stack(base, candidate, spec, config))
}

fn compare_on_stack(
    base: &Snapshot,
    candidate: &Snapshot,
    spec: &Differential,
    config: &VerifyConfig,
) -> Result<(Vec<Json>, bool), Diagnostic> {
    let p = &base.checked.program;
    let def = base.resolve(&spec.symbol)?;
    let id = p.def(def).stable_id.clone();
    let f = p.def(def).kind.as_fn().ok_or_else(|| {
        error(
            "differential_not_function",
            "differential testing compares functions",
        )
    })?;
    if !f.generics.is_empty() || !f.using.is_empty() {
        return Err(error(
            "differential_not_runnable",
            "differential testing needs a monomorphic function without capability parameters",
        ));
    }
    let filter = spec
        .filter
        .as_ref()
        .map(|name| {
            let filter = base.resolve(name)?;
            let g = p
                .def(filter)
                .kind
                .as_fn()
                .filter(|g| {
                    g.ret == Ty::Bool
                        && g.params.len() == f.params.len()
                        && g.params.iter().zip(&f.params).all(|(a, b)| a.ty == b.ty)
                        && g.using.is_empty()
                })
                .filter(|_| !p.is_mutating(filter));
            g.map(|_| p.def(filter).stable_id.clone()).ok_or_else(|| {
                error(
                    "differential_filter_invalid",
                    "a filter is a pure Bool function taking the compared function's parameters",
                )
            })
        })
        .transpose()?;
    let wanted = spec.cases.unwrap_or(config.cases);
    if wanted == 0 {
        return Err(error(
            "differential_cases_invalid",
            "differential cases must be positive",
        ));
    }
    let (mut passed, mut discarded, mut attempt) = (0u32, 0u32, 0u32);
    let budget = wanted.saturating_mul(10);
    while passed < wanted && attempt < budget {
        let mut rng = Rng::new(config.seed, &id, attempt);
        let size = attempt.min(30);
        attempt += 1;
        let Some(values) = f
            .params
            .iter()
            .map(|param| arbitrary(p, &param.ty, &mut rng, size, 0))
            .collect::<Option<Vec<_>>>()
        else {
            return Err(error(
                "differential_ungeneratable",
                "a parameter type has no generated values",
            ));
        };
        let args: Vec<Json> = values
            .iter()
            .zip(&f.params)
            .map(|(v, param)| cpl_eval::value_to_json(p, v, &param.ty))
            .collect();
        if let Some(filter) = &filter {
            if run(base, filter, &args, config)? != Outcome::Value(Json::Bool(true)) {
                discarded += 1;
                continue;
            }
        }
        let before = run(base, &id, &args, config)?;
        if before == Outcome::Outside {
            discarded += 1;
            continue;
        }
        let after = run(candidate, &id, &args, config)?;
        if !before.same(&after) {
            // Shrink while the versions still disagree on an input inside the domain.
            let (mut least, mut outcomes, mut steps) = (args.clone(), (before, after), 0);
            'shrink: while steps < 500 {
                for i in 0..least.len() {
                    for c in shrinks(&least[i]) {
                        steps += 1;
                        let mut trial = least.clone();
                        trial[i] = c;
                        if let Some(filter) = &filter {
                            if run(base, filter, &trial, config)?
                                != Outcome::Value(Json::Bool(true))
                            {
                                continue;
                            }
                        }
                        let old = run(base, &id, &trial, config)?;
                        if old == Outcome::Outside {
                            continue;
                        }
                        let new = run(candidate, &id, &trial, config)?;
                        if !old.same(&new) {
                            least = trial;
                            outcomes = (old, new);
                            continue 'shrink;
                        }
                    }
                }
                break;
            }
            return Ok((
                vec![
                    json!({"symbol": id, "method": "differential", "status": "failed",
                    "case": attempt - 1, "generated": args, "arguments": least,
                    "before": outcomes.0.json(), "after": outcomes.1.json(), "cases_passed": passed}),
                ],
                false,
            ));
        }
        passed += 1;
    }
    let ok = passed > 0;
    Ok((
        vec![
            json!({"symbol": id, "method": "differential", "status": if ok { "tested" } else { "untested" },
            "seed": config.seed, "cases_passed": passed, "cases_requested": wanted, "discarded": discarded,
            "equivalence": "unknown"}),
        ],
        ok,
    ))
}
