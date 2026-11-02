//! The interpreter corpus format (`spec/toolchain.md` 12.5, T49): reading
//! a `<name>.run.json` file and running its cases through
//! [`crate::run_function`] exactly as the corpus harness must.
//!
//! A case object has exactly the keys `function`, `args`, and `expect`, and
//! `expect` has exactly one key, `value` or `fault`; any other shape is a
//! harness error ([`parse_cases`] returns `Err`). [`run_case`] calls the
//! qualified `function` with every argument as `RunArg::Json` in checked
//! mode with the default [`crate::EvalConfig`] and a fresh evaluation, so a
//! refinement-typed argument that fails its predicate is the fault
//! `refinement_violation`; `expect.value` is compared with the value JSON of
//! the result at the declared return type, `expect.fault` with the fault's
//! code only.

use crate::interp::EvalConfig;
use crate::json::value_to_json;
use crate::run::{run_function, RunArg};
use cpl_diag::SymbolPath;
use cpl_hir::Program;
use serde_json::Value as Json;

/// One corpus case.
#[derive(Clone, Debug, PartialEq)]
pub struct CorpusCase {
    /// The qualified name of the function (`corpus.lists.take_n`).
    pub function: String,
    /// One value-JSON element per positional parameter.
    pub args: Vec<Json>,
    pub expect: Expect,
}

/// What a case expects.
#[derive(Clone, Debug, PartialEq)]
pub enum Expect {
    /// The value JSON of the result at the declared return type.
    Value(Json),
    /// A fault code of semantics 6.7.
    Fault(String),
}

/// Parse a `.run.json` document in the closed shape of T49.
pub fn parse_cases(text: &str) -> Result<Vec<CorpusCase>, String> {
    let doc: Json = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    let Json::Array(items) = doc else {
        return Err("a .run.json document is a JSON array of cases".to_string());
    };
    let mut cases = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let at = |m: &str| format!("case {i}: {m}");
        let Json::Object(obj) = item else {
            return Err(at("a case is a JSON object"));
        };
        if let Some(key) = obj
            .keys()
            .find(|k| !matches!(k.as_str(), "function" | "args" | "expect"))
        {
            return Err(at(&format!("unexpected key {key:?}")));
        }
        let Some(Json::String(function)) = obj.get("function") else {
            return Err(at("`function` must be a string"));
        };
        let Some(Json::Array(args)) = obj.get("args") else {
            return Err(at("`args` must be an array"));
        };
        let Some(Json::Object(expect)) = obj.get("expect") else {
            return Err(at("`expect` must be an object"));
        };
        if expect.len() != 1 {
            return Err(at("`expect` has exactly one key, `value` or `fault`"));
        }
        let expect = match (expect.get("value"), expect.get("fault")) {
            (Some(v), None) => Expect::Value(v.clone()),
            (None, Some(Json::String(code))) => Expect::Fault(code.clone()),
            (None, Some(_)) => return Err(at("`expect.fault` must be a code string")),
            _ => return Err(at("`expect` has exactly one key, `value` or `fault`")),
        };
        cases.push(CorpusCase {
            function: function.clone(),
            args: args.clone(),
            expect,
        });
    }
    Ok(cases)
}

/// Run one case; `Err` describes the mismatch.
pub fn run_case(p: &Program, case: &CorpusCase) -> Result<(), String> {
    let path = SymbolPath::parse(&case.function).map_err(|e| format!("bad function: {e}"))?;
    let args: Vec<RunArg> = case.args.iter().cloned().map(RunArg::Json).collect();
    let outcome = run_function(p, &path, &args, &EvalConfig::default())
        .map_err(|e| format!("{}: the run was refused: {e}", case.function))?;
    match (&case.expect, &outcome.value) {
        (Expect::Value(expected), Ok(v)) => {
            let actual = value_to_json(p, v, &outcome.return_ty);
            if &actual == expected {
                Ok(())
            } else {
                Err(format!(
                    "{}: expected the value {expected}, got {actual}",
                    case.function
                ))
            }
        }
        (Expect::Value(expected), Err(fault)) => Err(format!(
            "{}: expected the value {expected}, got the fault {fault}",
            case.function
        )),
        (Expect::Fault(code), Err(fault)) if fault.code() == code.as_str() => Ok(()),
        (Expect::Fault(code), Err(fault)) => Err(format!(
            "{}: expected the fault {code}, got the fault {fault}",
            case.function
        )),
        (Expect::Fault(code), Ok(v)) => Err(format!(
            "{}: expected the fault {code}, got the value {}",
            case.function,
            value_to_json(p, v, &outcome.return_ty)
        )),
    }
}
