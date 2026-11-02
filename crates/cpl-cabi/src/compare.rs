//! Semantics S75 between two engines (`spec/toolchain.md` 1.14, 7.2 `--engine`, 12.6). Owner: the native-engine track
//! (N).

use cpl_diag::json::{RunOutput, TestCaseRecord};
use serde_json::{Map, Value as Json};

/// The two outputs as compact JSON after the S75 normalization (toolchain 12.6), and the offset of their first
/// differing byte.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Divergence {
    pub eval: String,
    pub native: String,
    pub offset: usize,
}

fn first_difference(a: &str, b: &str) -> usize {
    a.bytes()
        .zip(b.bytes())
        .position(|(x, y)| x != y)
        .unwrap_or(a.len().min(b.len()))
}

fn divergence(a: &Json, b: &Json) -> Option<Divergence> {
    let (x, y) = (a.to_string(), b.to_string());
    (x != y).then(|| Divergence {
        offset: first_difference(&x, &y),
        eval: x,
        native: y,
    })
}

/// A guard fault carries `depth`; a time fault has `resource: "time"` (rule 2).
fn guard(f: &Json) -> bool {
    f.get("depth").is_some() && f.get("resource").and_then(Json::as_str) == Some("stack")
}

fn time(f: &Json) -> bool {
    f.get("resource").and_then(Json::as_str) == Some("time")
}

fn reduce(f: &Json, keys: &[&str], zero_depth: bool) -> Json {
    let mut m = Map::new();
    for k in keys {
        if let Some(v) = f.get(*k) {
            m.insert((*k).to_string(), v.clone());
        }
    }
    if zero_depth {
        m.insert("depth".to_string(), Json::from(0));
    }
    Json::Object(m)
}

/// Rule 2 on a pair of faults: both reduced, or None when the outcomes are not to be compared.
fn rule2(a: &Json, b: &Json) -> Result<(Json, Json), ()> {
    match (guard(a), guard(b), time(a), time(b)) {
        (true, true, _, _) => {
            let keys = ["code", "category", "severity", "resource", "limit"];
            Ok((reduce(a, &keys, true), reduce(b, &keys, true)))
        }
        (_, _, true, true) => {
            let keys = [
                "code", "category", "severity", "message", "resource", "limit",
            ];
            Ok((reduce(a, &keys, false), reduce(b, &keys, false)))
        }
        (false, false, false, false) => Ok((a.clone(), b.clone())),
        _ => Err(()),
    }
}

/// Rules 1 and 2 of the S75 normalization.
pub fn run_outputs(eval: &RunOutput, native: &RunOutput) -> Option<Divergence> {
    let mut a = serde_json::to_value(eval).expect("a RunOutput serializes");
    let mut b = serde_json::to_value(native).expect("a RunOutput serializes");
    normalize(&mut a);
    normalize(&mut b);
    if let (Some(fa), Some(fb)) = (a.get("fault").cloned(), b.get("fault").cloned()) {
        if !fa.is_null() && !fb.is_null() {
            match rule2(&fa, &fb) {
                Ok((ra, rb)) => {
                    a["fault"] = ra;
                    b["fault"] = rb;
                }
                Err(()) => {
                    for v in [&mut a, &mut b] {
                        if let Json::Object(m) = v {
                            m.remove("result");
                            m.remove("result_text");
                            m.remove("fault");
                        }
                    }
                }
            }
        }
    } else {
        let fa = a.get("fault").cloned().unwrap_or(Json::Null);
        let fb = b.get("fault").cloned().unwrap_or(Json::Null);
        if (guard(&fa) || time(&fa)) != (guard(&fb) || time(&fb)) {
            for v in [&mut a, &mut b] {
                if let Json::Object(m) = v {
                    m.remove("result");
                    m.remove("result_text");
                    m.remove("fault");
                }
            }
        }
    }
    divergence(&a, &b)
}

/// Rules 1 and 3; `sanctioned` says whether an evaluation of the test produced a time or a guard fault on either
/// engine, which the caller observes by wrapping the `TestEngine` of each engine.
pub fn test_records(
    eval: &TestCaseRecord,
    native: &TestCaseRecord,
    sanctioned: bool,
) -> Option<Divergence> {
    let mut a = serde_json::to_value(eval).expect("a TestCaseRecord serializes");
    let mut b = serde_json::to_value(native).expect("a TestCaseRecord serializes");
    normalize(&mut a);
    normalize(&mut b);
    if sanctioned {
        let keys = [
            "symbol",
            "qualified_name",
            "kind",
            "seed",
            "property_seed",
            "receipt",
        ];
        let mut ra = reduce(&a, &keys, false);
        let mut rb = reduce(&b, &keys, false);
        let fa = a.get("fault").cloned().unwrap_or(Json::Null);
        let fb = b.get("fault").cloned().unwrap_or(Json::Null);
        if (guard(&fa) && guard(&fb)) || (time(&fa) && time(&fb)) {
            if let Ok((x, y)) = rule2(&fa, &fb) {
                ra["fault"] = x;
                rb["fault"] = y;
                ra["status"] = a.get("status").cloned().unwrap_or(Json::Null);
                rb["status"] = b.get("status").cloned().unwrap_or(Json::Null);
            }
        }
        return divergence(&ra, &rb);
    }
    divergence(&a, &b)
}

/// Rules 1 and 2 for the diagnostics of two faults of one call (the C host suites, toolchain 12.7): their
/// divergence, or `None` when they agree or rule 2 leaves them uncompared (one a guard or a time fault and the other
/// not, or one a guard fault and the other a time fault).
pub fn faults(eval: &Json, native: &Json) -> Option<Divergence> {
    let (mut a, mut b) = (eval.clone(), native.clone());
    normalize(&mut a);
    normalize(&mut b);
    match rule2(&a, &b) {
        Ok((x, y)) => divergence(&x, &y),
        Err(()) => None,
    }
}

/// Rule 1, for whole envelopes (D3): every `duration_ms` key at any depth, an envelope's `engine` key, and the keys
/// `engine` and `native` of every `check_configuration` object are removed.
pub fn normalize(v: &mut Json) {
    if let Json::Object(m) = v {
        m.remove("engine");
    }
    let mut work: Vec<&mut Json> = vec![v];
    while let Some(x) = work.pop() {
        match x {
            Json::Object(m) => {
                m.remove("duration_ms");
                if let Some(Json::Object(cc)) = m.get_mut("check_configuration") {
                    cc.remove("engine");
                    cc.remove("native");
                }
                for (_, child) in m.iter_mut() {
                    work.push(child);
                }
            }
            Json::Array(items) => {
                for child in items.iter_mut() {
                    work.push(child);
                }
            }
            _ => {}
        }
    }
}
