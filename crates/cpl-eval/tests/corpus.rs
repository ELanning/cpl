//! The interpreter corpus (`spec/toolchain.md` 12.5, T49) on the real
//! checker's output.
//!
//! Each `tests/corpus/<name>.cpl` is loaded as the module `corpus.<name>`
//! (at its conventional path `src/corpus/<name>.cpl` in a `VirtualTree`
//! with a synthetic `cpl.toml` and the embedded standard library) and
//! checked by `cpl-sema`, which must report no error (T49: every corpus
//! module passes `cpl check`). Every case of the paired `<name>.run.json`
//! then runs through `run_function` with `RunArg::Json` arguments in
//! checked mode with the default `EvalConfig`, a fresh evaluation per case,
//! and no calling declaration (`cpl_eval::testing::corpus::run_case`), and
//! is compared with its expectation: the value JSON of the result at the
//! declared return type exactly, or the fault code. A malformed case
//! object, a module whose header is not `corpus.<name>`, a `function`
//! outside the paired module, and a `.cpl` without a `.run.json` (or the
//! reverse) are harness errors.
//!
//! The run prints `eval corpus: N cases, P pass, F fail` and every failure.

mod common;

use cpl_eval::testing::corpus::{parse_cases, run_case};
use cpl_sema::CheckResult;
use std::collections::BTreeSet;
use std::fs;

/// The corpus module names: the stems of `tests/corpus/*.cpl`, which must be
/// exactly the stems of `tests/corpus/*.run.json`.
fn module_names() -> Vec<String> {
    let mut cpl = BTreeSet::new();
    let mut runs = BTreeSet::new();
    for entry in fs::read_dir(common::corpus_dir()).expect("the corpus directory") {
        let name = entry
            .expect("a directory entry")
            .file_name()
            .into_string()
            .expect("a UTF-8 file name");
        if let Some(stem) = name.strip_suffix(".run.json") {
            runs.insert(stem.to_string());
        } else if let Some(stem) = name.strip_suffix(".cpl") {
            cpl.insert(stem.to_string());
        }
    }
    assert_eq!(
        cpl, runs,
        "harness error: every corpus .cpl needs a .run.json and every .run.json a .cpl"
    );
    cpl.into_iter().collect()
}

/// `corpus.<name>` checked in a project of its own.
fn check_module(name: &str, source: &str) -> CheckResult {
    let path = format!("src/corpus/{name}.cpl");
    let manifest = common::manifest("corpus");
    common::check_files_result(&[("cpl.toml", manifest.as_str()), (path.as_str(), source)])
}

#[test]
fn eval_corpus() {
    let dir = common::corpus_dir();
    let mut total = 0usize;
    let mut failures: Vec<String> = Vec::new();
    for name in module_names() {
        let source = fs::read_to_string(dir.join(format!("{name}.cpl"))).expect("read a module");
        let text =
            fs::read_to_string(dir.join(format!("{name}.run.json"))).expect("read the cases");
        let cases =
            parse_cases(&text).unwrap_or_else(|e| panic!("harness error: {name}.run.json: {e}"));
        let module = format!("corpus.{name}");
        for case in &cases {
            let prefix = format!("{module}.");
            assert!(
                case.function.starts_with(&prefix)
                    && common::path(&case.function).module == ["corpus".to_string(), name.clone()],
                "harness error: {name}.run.json names {}, which is not declared in {module} (T49)",
                case.function
            );
        }
        total += cases.len();
        let result = check_module(&name, &source);
        let module_path = vec!["corpus".to_string(), name.clone()];
        assert!(
            result.program.module_by_path(&module_path).is_some() || result.has_errors(),
            "harness error: {name}.cpl does not declare `module {module}`"
        );
        if result.has_errors() {
            for case in &cases {
                failures.push(format!(
                    "{}: {module} does not check:\n{}",
                    case.function,
                    common::render(&result)
                ));
            }
            continue;
        }
        let program = result.program;
        for case in &cases {
            if let Err(e) = run_case(&program, case) {
                failures.push(e);
            }
        }
    }
    println!(
        "eval corpus: {total} cases, {} pass, {} fail",
        total - failures.len(),
        failures.len()
    );
    for f in &failures {
        println!("  FAIL {f}");
    }
    assert!(
        failures.is_empty(),
        "{} of {total} corpus cases failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert_eq!(
        total, 74,
        "the corpus has 74 cases (tests/corpus/README.md)"
    );
}

/// The corpus module `corpus.<name>`, checked; panics on any error.
fn checked_module(name: &str) -> cpl_hir::Program {
    let source = fs::read_to_string(common::corpus_dir().join(format!("{name}.cpl")))
        .expect("read a module");
    common::checked(check_module(name, &source))
}

/// The fault of `function(args)` in `corpus.<name>` as its diagnostic
/// object.
fn fault_of(p: &cpl_hir::Program, function: &str, args: serde_json::Value) -> serde_json::Value {
    let serde_json::Value::Array(args) = args else {
        panic!("arguments are an array");
    };
    let outcome = common::run_json(p, function, &args);
    common::fault(p, &outcome)
}

/// Every field a `// why:` comment of the corpus states for a fault case
/// (tests/corpus/README.md Section 2), which the T49 format does not
/// compare: the fault each case raises on the checker's program must carry
/// them exactly, and the keys listed as absent must be absent.
#[test]
fn the_fault_fields_the_why_comments_state() {
    use serde_json::json;
    type Row = (
        &'static str,
        &'static str,
        serde_json::Value,
        serde_json::Value,
        &'static [&'static str],
    );
    let rows: Vec<Row> = vec![
        (
            "contracts",
            "corpus.contracts.recip",
            json!([0]),
            json!({"code": "contract_violation", "clause_kind": "requires",
                   "symbol": "corpus.contracts.recip", "facet": "contract",
                   "attributed_to": "caller", "clause": "n != 0", "input": {"n": 0},
                   "status": "failed"}),
            &["caller"],
        ),
        (
            "contracts",
            "corpus.contracts.half",
            json!([5]),
            json!({"code": "contract_violation", "clause_kind": "ensures",
                   "attributed_to": "body", "clause": "result * 2 == n",
                   "input": {"n": 5}, "actual": 2}),
            &[],
        ),
        (
            "contracts",
            "corpus.contracts.passthrough",
            json!([{"$type": "Result", "$variant": "Err", "$payload": ["e"]}]),
            json!({"code": "contract_violation", "clause_kind": "ensures",
                   "clause": "result.is_ok()",
                   "actual": {"$type": "Result", "$variant": "Err", "$payload": ["e"]}}),
            &[],
        ),
        (
            "contracts",
            "corpus.contracts.bad_loop",
            json!([3]),
            json!({"code": "contract_violation", "clause_kind": "invariant",
                   "clause": "i < 3", "attributed_to": "body", "iteration": 3,
                   "input": {"n": 3}, "actual": {"bindings": {"n": 3, "i": 3}}}),
            &[],
        ),
        (
            "contracts",
            "corpus.contracts.stuck",
            json!([2]),
            json!({"code": "contract_violation", "clause_kind": "decreases",
                   "clause": "n", "iteration": 1,
                   "actual": {"bindings": {"n": 2, "i": 1}, "previous": 2, "measure": 2}}),
            &[],
        ),
        (
            "contracts",
            "corpus.contracts.neg_measure",
            json!([1]),
            json!({"code": "contract_violation", "clause_kind": "decreases",
                   "clause": "i - 1", "iteration": 0,
                   "actual": {"bindings": {"n": 1, "i": 0}, "measure": -1}}),
            &[],
        ),
        (
            "contracts",
            "corpus.contracts.bad_rec",
            json!([1]),
            json!({"code": "contract_violation", "clause_kind": "decreases",
                   "clause": "n", "attributed_to": "body", "input": {"n": 1},
                   "actual": {"previous": 1, "measure": 1}}),
            &["iteration"],
        ),
        (
            "contracts",
            "corpus.contracts.faulting_clause",
            json!([0]),
            json!({"code": "division_by_zero", "facet": "contract",
                   "clause": "10 / n > 0", "symbol": "corpus.contracts.faulting_clause",
                   "input": {"a": 10, "b": 0}}),
            &[],
        ),
        (
            "control",
            "corpus.control.clamp_pos",
            json!([-2]),
            json!({"code": "contract_violation", "clause_kind": "ensures",
                   "symbol": "corpus.control.clamp_pos", "facet": "contract",
                   "attributed_to": "body", "clause": "result > 0", "input": {"n": -2},
                   "actual": -2, "status": "failed"}),
            &[],
        ),
        (
            "refine",
            "corpus.refine.to_nz",
            json!([0]),
            json!({"code": "division_by_zero", "symbol": "corpus.refine.Nz",
                   "facet": "contract", "clause": "10 / self > 0",
                   "input": {"a": 10, "b": 0}}),
            &[],
        ),
        (
            "refine",
            "corpus.refine.double",
            json!([0]),
            json!({"code": "refinement_violation", "symbol": "corpus.refine.double",
                   "expected_type": "Positive", "actual": 0, "clause": "self > 0",
                   "parameter": "p", "pointer": ""}),
            &["backtrace"],
        ),
        (
            "faults",
            "corpus.faults.checked_div",
            json!([1, 0]),
            json!({"code": "assertion_failed", "clause": "b != 0",
                   "symbol": "corpus.faults.checked_div", "facet": "body",
                   "bindings": [
                       {"name": "b", "type": "Int", "kind": "parameter", "value": 0},
                       {"name": "a", "type": "Int", "kind": "parameter", "value": 1}]}),
            &[],
        ),
        (
            "holes",
            "corpus.holes.summarize",
            json!([{"$type": "corpus.holes.Page",
                    "id": {"$type": "corpus.holes.PageId", "$payload": ["p"]}, "text": "t"}]),
            json!({"code": "unresolved_hole", "hole": "summary",
                   "symbol": "corpus.holes.summarize", "expected_type": "Text",
                   "bindings": [{"name": "page", "type": "Page", "kind": "parameter",
                                 "value": {"$type": "corpus.holes.Page",
                                           "id": {"$type": "corpus.holes.PageId",
                                                  "$payload": ["p"]},
                                           "text": "t"}}]}),
            &[],
        ),
        (
            "lists",
            "corpus.lists.take_n",
            json!([[1], -1]),
            json!({"code": "contract_violation", "clause_kind": "requires",
                   "symbol": "std.prelude.List.take", "facet": "contract",
                   "attributed_to": "caller", "caller": "corpus.lists.take_n",
                   "clause": "n >= 0", "input": {"self": [1], "n": -1}}),
            &[],
        ),
        (
            "lists",
            "corpus.lists.all_divide",
            json!([[1, 0]]),
            json!({"code": "division_by_zero", "symbol": "corpus.lists.all_divide",
                   "facet": "body", "input": {"a": 10, "b": 0}}),
            &["clause"],
        ),
        (
            "ints",
            "corpus.ints.div",
            json!([1, 0]),
            json!({"code": "division_by_zero", "category": "runtime",
                   "symbol": "corpus.ints.div", "input": {"a": 1, "b": 0}}),
            &[],
        ),
        (
            "enums",
            "corpus.enums.early",
            json!([1, 0]),
            json!({"code": "division_by_zero", "symbol": "corpus.enums.early",
                   "input": {"a": 1, "b": 0}}),
            &[],
        ),
        (
            "reader",
            "corpus.reader.divide_exactly",
            json!([7, 0]),
            json!({"code": "contract_violation", "clause_kind": "requires",
                   "clause": "b != 0", "input": {"a": 7, "b": 0},
                   "attributed_to": "caller"}),
            &["caller"],
        ),
        (
            "reader",
            "corpus.reader.divide_exactly",
            json!([7, 2]),
            json!({"code": "contract_violation", "clause_kind": "requires",
                   "clause": "a % b == 0", "input": {"a": 7, "b": 2}}),
            &["caller"],
        ),
    ];
    let mut programs = std::collections::BTreeMap::new();
    let mut mismatches = Vec::new();
    for (module, function, args, expected, absent) in rows {
        let p = programs
            .entry(module)
            .or_insert_with(|| checked_module(module));
        let fault = fault_of(p, function, args);
        for (key, want) in expected.as_object().expect("an object") {
            if fault.get(key) != Some(want) {
                mismatches.push(format!(
                    "{function}: `{key}` is {}, expected {want}",
                    fault
                        .get(key)
                        .map_or("absent".to_string(), |v| v.to_string())
                ));
            }
        }
        for key in absent {
            if let Some(v) = fault.get(*key) {
                mismatches.push(format!("{function}: `{key}` is {v}, expected absent"));
            }
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

/// tests/corpus/README.md Section 1, "checking": `holes` reports two `hole`
/// infos and one `verification_result` info, `reader` one
/// `verification_result` info, and every other module nothing at all (no
/// warning, which T49 does not forbid but the corpus promises).
#[test]
fn the_modules_check_with_exactly_the_diagnostics_named_here() {
    for name in module_names() {
        let source = fs::read_to_string(common::corpus_dir().join(format!("{name}.cpl")))
            .expect("read a module");
        let result = check_module(&name, &source);
        let mut codes: Vec<String> = result
            .diagnostics
            .iter()
            .map(|d| format!("{}:{}", d.severity.as_str(), d.code.0))
            .collect();
        codes.sort();
        let expected: &[&str] = match name.as_str() {
            "holes" => &["info:hole", "info:hole", "info:verification_result"],
            "reader" => &["info:verification_result"],
            _ => &[],
        };
        assert_eq!(codes, expected, "{name}:\n{}", common::render(&result));
    }
}

/// T49: a case object has exactly the keys `function`, `args`, and `expect`,
/// and `expect` exactly one key, `value` or `fault`; any other shape is a
/// harness error (`parse_cases` refuses it).
#[test]
fn a_malformed_case_object_is_a_harness_error() {
    use cpl_eval::testing::corpus::Expect;
    for bad in [
        r#"{}"#,
        r#"[1]"#,
        r#"[{"function": "a.b", "args": []}]"#,
        r#"[{"function": "a.b", "args": [], "expect": {"value": 1}, "why": "x"}]"#,
        r#"[{"function": "a.b", "args": [], "expect": {"value": 1, "fault": "x"}}]"#,
        r#"[{"function": "a.b", "args": [], "expect": {}}]"#,
        r#"[{"function": "a.b", "args": [], "expect": {"fault": 3}}]"#,
        r#"[{"function": "a.b", "args": [], "expect": {"error": "x"}}]"#,
        r#"[{"function": "a.b", "args": {}, "expect": {"value": 1}}]"#,
        r#"[{"function": 7, "args": [], "expect": {"value": 1}}]"#,
        "not json",
    ] {
        assert!(parse_cases(bad).is_err(), "{bad}");
    }
    let ok = parse_cases(
        r#"[{"function": "a.b", "args": [1], "expect": {"fault": "division_by_zero"}}]"#,
    )
    .expect("a well-formed case");
    assert_eq!(ok[0].function, "a.b");
    assert_eq!(ok[0].args, [serde_json::json!(1)]);
    assert_eq!(ok[0].expect, Expect::Fault("division_by_zero".to_string()));
}
