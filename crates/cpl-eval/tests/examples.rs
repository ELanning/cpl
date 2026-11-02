//! The interpreter example table (`spec/toolchain.md` 12.3, the
//! "interpreter examples" row of Section 12; semantics Section 11) on the
//! example projects as the real checker checks them.
//!
//! Each `run_function` row runs twice, with the arguments as the table
//! writes them (constant expressions, the positional arguments of `cpl run`,
//! toolchain 7.2) and as value JSON (`--arg JSON`, 7.3), which must agree.
//! Every field a row names is compared exactly; the ConstraintError payload
//! is the shape of stdlib 1.5 (exactly `type_name` and `message`, the
//! message fixed by semantics 6.10).
//!
//! The functions the table does not name run too, so that every function
//! of the design's examples executes in Milestone 1: `clean`, `label`, and
//! `parse_size` through `run_function`, and the capability-using
//! `load_page`, `load_text`, and `image_width`, which `cpl run` refuses
//! (`cannot_run_effectful`, semantics S39), through
//! `Interpreter::bind_capability` with scripted host bindings, as the
//! interop adapters of Milestones 2-4 will bind real ones (semantics 6.9).

mod common;

use common::{const_args, json_args, try_run};
use cpl_diag::json::{TestKind, TestStatus};
use cpl_eval::{
    on_eval_stack, property_seed, seed_hex, test_result_diagnostic, value_from_json, value_text,
    value_to_json, CapabilityImpl, EvalConfig, Fault, HostContext, Interpreter, RunError,
    RunOutcome, Value,
};
use cpl_hir::{DefId, Program};
use serde_json::{json, Value as Json};
use std::cell::RefCell;
use std::rc::Rc;

/// Run `function` once with constant-expression and once with value-JSON
/// arguments; the two outcomes must render identically. Returns the
/// constant-expression outcome.
fn run_both(p: &Program, function: &str, consts: &[&str], jsons: &[Json]) -> RunOutcome {
    let by_const = try_run(p, function, &const_args(consts))
        .unwrap_or_else(|e| panic!("{function}: refused: {e}"));
    let by_json = try_run(p, function, &json_args(jsons))
        .unwrap_or_else(|e| panic!("{function}: refused: {e}"));
    assert_eq!(
        common::outcome_json(p, &by_const),
        common::outcome_json(p, &by_json),
        "{function}: constant-expression and value-JSON arguments disagree"
    );
    by_const
}

fn ok(payload: Json) -> Json {
    json!({"$type": "Result", "$variant": "Ok", "$payload": [payload]})
}

/// `Err(ConstraintError { type_name, message })` (stdlib 1.5: exactly the
/// two fields; `$type` is the bare prelude name, toolchain 7.3).
fn constraint_error(type_name: &str, message: &str) -> Json {
    json!({"$type": "Result", "$variant": "Err", "$payload": [
        {"$type": "ConstraintError", "type_name": type_name, "message": message}
    ]})
}

/// The source text a fault's span covers.
fn span_text(p: &Program, fault: &cpl_eval::Fault) -> String {
    let span = fault.span.expect("the fault has a span");
    p.source_map().text(span).to_string()
}

#[test]
fn paginate_ints_returns_its_pages_or_a_constraint_error() {
    let p = common::reader();
    let f = "reader.pages.paginate_ints";
    let out = run_both(&p, f, &["[1, 2, 3]", "2"], &[json!([1, 2, 3]), json!(2)]);
    assert_eq!(common::value(&p, &out), ok(json!([[1, 2], [3]])));
    // The table writes the value as value text (semantics S40); no
    // capability exists in Milestone 1, so no effect is traced.
    let v = out.value.as_ref().expect("a value");
    assert_eq!(value_text(&p, v, &out.return_ty), "Ok([[1, 2], [3]])");
    assert!(out.effects.is_empty());
    let out = run_both(&p, f, &["[]", "2"], &[json!([]), json!(2)]);
    assert_eq!(common::value(&p, &out), ok(json!([])));
    // `Err(ConstraintError...)` with the payload shape of stdlib 1.5: a
    // value, not a fault (semantics 6.6, S39 (d)); `paginate` is never
    // entered.
    let out = run_both(&p, f, &["[1]", "0"], &[json!([1]), json!(0)]);
    assert_eq!(
        common::value(&p, &out),
        constraint_error("reader.pages.Positive", "0 does not satisfy self > 0")
    );
    // semantics Section 11 names the same value for `[1, 2, 3]` and `0`.
    let out = run_both(&p, f, &["[1, 2, 3]", "0"], &[json!([1, 2, 3]), json!(0)]);
    assert_eq!(
        common::value(&p, &out),
        constraint_error("reader.pages.Positive", "0 does not satisfy self > 0")
    );
}

#[test]
fn divide_exactly_checks_its_preconditions_in_order() {
    let p = common::reader();
    let f = "reader.pages.divide_exactly";
    let out = run_both(&p, f, &["6", "3"], &[json!(6), json!(3)]);
    assert_eq!(common::value(&p, &out), json!(2));

    // 7, 0: the first requires fails; the second (a % b == 0) is never
    // evaluated, so there is no division_by_zero (semantics 5.2 step 3).
    let out = run_both(&p, f, &["7", "0"], &[json!(7), json!(0)]);
    let d = common::fault(&p, &out);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["category"], "contract");
    assert_eq!(d["severity"], "error");
    assert_eq!(d["clause_kind"], "requires");
    assert_eq!(d["clause"], "b != 0");
    assert_eq!(d["input"], json!({"a": 7, "b": 0}));
    assert_eq!(d["attributed_to"], "caller");
    assert_eq!(d["symbol"], "reader.pages.divide_exactly");
    assert_eq!(d["qualified_name"], "reader.pages.divide_exactly");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["status"], "failed");
    assert!(
        d.get("caller").is_none(),
        "no calling declaration (S39 (c))"
    );
    assert!(d.get("actual").is_none());
    // The span is the clause, from its keyword (toolchain 3.3).
    let fault = common::fault_of(&p, &out);
    assert_eq!(span_text(&p, fault), "requires b != 0");
    assert_eq!(d["span"]["file"], "src/reader/pages.cpl");
    assert_eq!(
        d["backtrace"],
        json!([{"qualified_name": "reader.pages.divide_exactly"}])
    );

    // 7, 2: the first requires held, the second is the first false one.
    let out = run_both(&p, f, &["7", "2"], &[json!(7), json!(2)]);
    let d = common::fault(&p, &out);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause_kind"], "requires");
    assert_eq!(d["clause"], "a % b == 0");
    assert_eq!(d["input"], json!({"a": 7, "b": 2}));
    assert_eq!(d["attributed_to"], "caller");
    assert!(d.get("caller").is_none());
    assert_eq!(
        span_text(&p, common::fault_of(&p, &out)),
        "requires a % b == 0"
    );
}

#[test]
fn positive_size_trims_parses_and_checks() {
    let p = common::reader();
    let f = "reader.pages.positive_size";
    let out = run_both(&p, f, &["\"  12 \""], &[json!("  12 ")]);
    assert_eq!(common::value(&p, &out), ok(json!(12)));
    // `Err(...)`: parse_int accepts the sign (stdlib S25), so the check
    // fails, with the value text of -3 (semantics S40, 6.10).
    let out = run_both(&p, f, &["\"-3\""], &[json!("-3")]);
    assert_eq!(
        common::value(&p, &out),
        constraint_error("reader.pages.Positive", "-3 does not satisfy self > 0")
    );
}

#[test]
fn summarize_reaches_its_hole() {
    let p = common::holes();
    let page = json!({"$type": "holes.pages.Page",
                      "id": {"$type": "holes.pages.PageId", "$payload": ["p"]},
                      "text": "t"});
    let out = run_both(
        &p,
        "holes.pages.summarize",
        &["Page { id: PageId(\"p\"), text: \"t\" }"],
        std::slice::from_ref(&page),
    );
    let d = common::fault(&p, &out);
    assert_eq!(d["code"], "unresolved_hole");
    assert_eq!(d["category"], "hole");
    assert_eq!(d["hole"], "summary");
    assert_eq!(d["symbol"], "holes.pages.summarize");
    assert_eq!(d["facet"], "body");
    assert_eq!(d["expected_type"], "Text");
    assert_eq!(
        d["bindings"],
        json!([{"name": "page", "type": "Page", "kind": "parameter", "value": page}])
    );
    assert_eq!(span_text(&p, common::fault_of(&p, &out)), "?summary");
}

#[test]
fn a_generic_function_cannot_be_run() {
    // `holes.pages.paginate` via `cpl run`: `cannot_run_generic` (exit 2), a
    // refusal before anything runs (semantics S39 (b), toolchain 1.7).
    let p = common::holes();
    match try_run(&p, "holes.pages.paginate", &const_args(&["[1]", "1"])) {
        Err(RunError::Generic(name)) => assert_eq!(name, "holes.pages.paginate"),
        other => panic!("expected RunError::Generic, got {other:?}"),
    }
    let p = common::reader();
    assert!(matches!(
        try_run(&p, "reader.pages.paginate", &const_args(&["[1]", "1"])),
        Err(RunError::Generic(_))
    ));
    // A function with `using` parameters cannot be run either (S39 (b)).
    assert!(matches!(
        try_run(
            &p,
            "reader.pages.load_page",
            &const_args(&["PageId(\"p\")"])
        ),
        Err(RunError::CannotRunEffectful(_))
    ));
}

#[test]
fn cpl_test_on_reader_with_seed_1_tests_all_three() {
    let p = common::reader();
    let report = common::run_all_tests(&p, 1);
    assert_eq!(report.seed, 1);
    assert_eq!(report.cases, 100);
    assert_eq!(
        (report.selected, report.tested, report.failed),
        (3, 3, 0),
        "{:#}",
        common::report_json(&report)
    );
    let names: Vec<&str> = report
        .results
        .iter()
        .map(|r| r.qualified_name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "tests.pagination.\"pagination handles empty input\"",
            "tests.pagination.\"pagination preserves a short tail\"",
            "tests.pagination.\"pagination preserves input\"",
        ]
    );
    for record in &report.results {
        assert_eq!(
            record.status,
            TestStatus::Tested,
            "{}",
            record.qualified_name
        );
        assert!(record.fault.is_none() && record.counterexamples.is_empty());
        assert!(record.failing_case.is_none() && record.shrunk.is_none());
        // The caller (cpl-cli) fills the receipt (toolchain 1.7.2).
        assert!(record.receipt.is_none());
        assert_eq!(record.symbol.as_str(), record.qualified_name);
        let d = test_result_diagnostic(record);
        assert_eq!(d.code.0, "test_result");
        assert_eq!(d.severity, cpl_diag::Severity::Info);
        assert!(d.span.is_none());
        let d = serde_json::to_value(d.to_json(p.source_map())).unwrap();
        assert_eq!(d["status"], "tested");
    }
    let (tests, property) = report.results.split_at(2);
    for t in tests {
        assert_eq!(t.kind, TestKind::Test);
        assert!(t.seed.is_none() && t.property_seed.is_none() && t.cases.is_none());
    }
    let property = &property[0];
    assert_eq!(property.kind, TestKind::Property);
    assert_eq!(property.seed, Some(1));
    assert_eq!(property.cases, Some(100), "the property passes 100 cases");
    assert_eq!(
        property.property_seed.as_deref(),
        Some(seed_hex(&property_seed(1, &property.qualified_name)).as_str())
    );
}

#[test]
fn examples_holes_has_no_tests() {
    // semantics Section 11: `cpl test` on `examples/holes` selects nothing
    // (the CLI turns `selected == 0` into `empty_test_selection`).
    let report = common::run_all_tests(&common::holes(), 1);
    assert_eq!((report.selected, report.tested, report.failed), (0, 0, 0));
    assert!(report.results.is_empty());
}

// ---- the design's other functions ---------------------------------------------

fn page(id: &str, text: &str) -> Json {
    json!({"$type": "reader.pages.Page",
           "id": {"$type": "reader.pages.PageId", "$payload": [id]},
           "text": text})
}

fn err(payload: Json) -> Json {
    json!({"$type": "Result", "$variant": "Err", "$payload": [payload]})
}

fn missing(id: &str) -> Json {
    json!({"$type": "reader.pages.StoreError", "$variant": "Missing",
           "$payload": [{"$type": "reader.pages.PageId", "$payload": [id]}]})
}

fn unavailable() -> Json {
    json!({"$type": "reader.pages.StoreError", "$variant": "Unavailable"})
}

#[test]
fn clean_label_and_parse_size_run_as_the_design_writes_them() {
    let p = common::reader();
    // `clean` trims the text and keeps the id (the struct update `..page`).
    let out = run_both(
        &p,
        "reader.pages.clean",
        &["Page { id: PageId(\"p1\"), text: \"  hello \" }"],
        &[page("p1", "  hello ")],
    );
    assert_eq!(common::value(&p, &out), page("p1", "hello"));
    // `label` matches all three shapes of a `Result<Page, StoreError>`.
    for (arg, expected) in [
        (ok(page("p1", "hello")), "hello"),
        (err(missing("p9")), "Missing page"),
        (err(unavailable()), "Storage unavailable"),
    ] {
        let out = common::run_json(&p, "reader.pages.label", std::slice::from_ref(&arg));
        assert_eq!(common::value(&p, &out), json!(expected), "{arg}");
    }
    // `parse_size`, the corpus glue (grammar D58), trims and parses; a
    // malformed size is an `Err` value, not a fault.
    let out = run_both(
        &p,
        "reader.pages.parse_size",
        &["\" 42 \""],
        &[json!(" 42 ")],
    );
    assert_eq!(common::value(&p, &out), ok(json!(42)));
    let out = run_both(&p, "reader.pages.parse_size", &["\"4x\""], &[json!("4x")]);
    let value = common::value(&p, &out);
    assert_eq!(value["$variant"], "Err", "{value}");
    assert_eq!(value["$payload"][0]["$type"], "ConstraintError", "{value}");
}

/// The answer of a scripted host operation: the operation's name and its
/// arguments as value JSON give the result's value JSON.
type Answer = fn(&str, &[Json]) -> Json;

/// A host binding for one capability: every call is recorded with its
/// arguments as value JSON and answered by `answer`, decoded against the
/// operation's declared return type.
struct Scripted {
    capability: DefId,
    answer: Answer,
    calls: Rc<RefCell<Vec<HostCall>>>,
}

/// One operation call a host received: the operation and its arguments as
/// value JSON.
type HostCall = (String, Vec<Json>);

impl CapabilityImpl for Scripted {
    fn call(
        &mut self,
        op: &str,
        args: Vec<Value>,
        ctx: &mut HostContext<'_>,
    ) -> Result<Value, Fault> {
        let p = ctx.program;
        let op_def = p
            .def(self.capability)
            .kind
            .as_capability()
            .expect("a capability")
            .ops
            .iter()
            .find(|o| o.name == op)
            .expect("a declared operation");
        let args: Vec<Json> = args
            .iter()
            .zip(&op_def.params)
            .map(|(v, param)| value_to_json(p, v, &param.ty))
            .collect();
        let answer = (self.answer)(op, &args);
        self.calls.borrow_mut().push((op.to_string(), args));
        Ok(value_from_json(p, &answer, &op_def.ret).expect("a well-typed answer"))
    }
}

/// What a call with scripted hosts produced: the result's value JSON or the
/// fault's diagnostic object, the traced effects as (operation, the
/// declaration that performed it), and the hosts' calls.
struct Hosted {
    result: Result<Json, Json>,
    effects: Vec<(String, String)>,
    calls: Vec<HostCall>,
}

/// Call `function` with value-JSON arguments and one scripted host per
/// `using` parameter (capability qualified name, answer), in checked mode
/// with effect tracing, on the evaluation stack (toolchain 1.7: a host
/// driving an `Interpreter` itself should).
fn run_hosted(p: &Program, function: &str, args: &[Json], hosts: &[(&str, Answer)]) -> Hosted {
    let f = common::def(p, function);
    let hosts: Vec<(DefId, Answer)> = hosts
        .iter()
        .map(|(capability, answer)| (common::def(p, capability), *answer))
        .collect();
    on_eval_stack(move || {
        let fd = p.def(f).kind.as_fn().expect("a function");
        let values = args
            .iter()
            .zip(&fd.params)
            .map(|(j, param)| value_from_json(p, j, &param.ty).expect("a decodable argument"))
            .collect();
        let config = EvalConfig {
            trace_effects: true,
            ..EvalConfig::default()
        };
        let mut it = Interpreter::new(p, config);
        let calls = Rc::new(RefCell::new(Vec::new()));
        let handles = hosts
            .iter()
            .map(|(capability, answer)| {
                let host = Scripted {
                    capability: *capability,
                    answer: *answer,
                    calls: Rc::clone(&calls),
                };
                it.bind_capability(*capability, Box::new(host), "test host")
            })
            .collect();
        let result = it
            .call(f, values, handles)
            .map(|v| value_to_json(p, &v, &fd.ret))
            .map_err(|fault| common::fault_json(p, &fault));
        let effects = it
            .effects()
            .iter()
            .map(|e| (e.op.clone(), p.def(e.def).stable_id.to_string()))
            .collect();
        let calls = calls.borrow().clone();
        Hosted {
            result,
            effects,
            calls,
        }
    })
}

/// A `PageRead` store: `p1` holds a page, `down` finds the store
/// unavailable, and every other id is missing.
fn page_store(op: &str, args: &[Json]) -> Json {
    assert_eq!(op, "get");
    match args[0]["$payload"][0].as_str() {
        Some("p1") => ok(page("p1", "  hello ")),
        Some("down") => err(unavailable()),
        _ => err(
            json!({"$type": "reader.pages.StoreError", "$variant": "Missing",
                        "$payload": [args[0].clone()]}),
        ),
    }
}

fn page_id(id: &str) -> Json {
    json!({"$type": "reader.pages.PageId", "$payload": [id]})
}

#[test]
fn load_page_and_load_text_run_against_a_host_page_store() {
    let p = common::reader();
    let read = [("reader.pages.PageRead", page_store as Answer)];
    let get = |id: &str| ("get".to_string(), vec![page_id(id)]);
    let effect = |by: &str| ("get".to_string(), format!("reader.pages.{by}"));

    // load_page returns what the store answers, Ok or Err.
    let run = run_hosted(&p, "reader.pages.load_page", &[page_id("p1")], &read);
    assert_eq!(run.result, Ok(ok(page("p1", "  hello "))));
    assert_eq!(run.calls, [get("p1")]);
    assert_eq!(run.effects, [effect("load_page")]);
    let run = run_hosted(&p, "reader.pages.load_page", &[page_id("p9")], &read);
    assert_eq!(run.result, Ok(err(missing("p9"))));

    // load_text forwards its handle (`using db`) to load_page, which
    // performs the operation, and `?` propagates the store's errors.
    let run = run_hosted(&p, "reader.pages.load_text", &[page_id("p1")], &read);
    assert_eq!(run.result, Ok(ok(json!("  hello "))));
    assert_eq!(run.calls, [get("p1")]);
    assert_eq!(run.effects, [effect("load_page")]);
    let run = run_hosted(&p, "reader.pages.load_text", &[page_id("p9")], &read);
    assert_eq!(run.result, Ok(err(missing("p9"))));
    assert_eq!(run.calls, [get("p9")]);
    let run = run_hosted(&p, "reader.pages.load_text", &[page_id("down")], &read);
    assert_eq!(run.result, Ok(err(unavailable())));
}

/// An `ImageCodec`: the PNG signature is a 640 by 480 image, anything else
/// is not an image (`std.foreign.ForeignError`, kind `InvalidArgument`).
fn image_codec(op: &str, args: &[Json]) -> Json {
    assert_eq!(op, "dimensions");
    if args[0]["hex"] == "89504e47" {
        ok(json!({"$type": "reader.images.Dimensions", "width": 640, "height": 480}))
    } else {
        err(not_an_image())
    }
}

fn not_an_image() -> Json {
    json!({"$type": "std.foreign.ForeignError",
           "kind": {"$type": "std.foreign.ForeignErrorKind", "$variant": "InvalidArgument"},
           "message": "not an image",
           "details": []})
}

#[test]
fn image_width_runs_against_a_host_codec() {
    let p = common::reader();
    let codec = [("reader.images.ImageCodec", image_codec as Answer)];
    let bytes = |hex: &str| json!({"$type": "Bytes", "hex": hex});

    let run = run_hosted(
        &p,
        "reader.images.image_width",
        &[bytes("89504e47")],
        &codec,
    );
    assert_eq!(run.result, Ok(ok(json!(640))));
    assert_eq!(
        run.calls,
        [("dimensions".to_string(), vec![bytes("89504e47")])]
    );
    assert_eq!(
        run.effects,
        [(
            "dimensions".to_string(),
            "reader.images.image_width".to_string()
        )]
    );
    // A foreign failure is a typed `Err` value the function propagates with
    // `?`, never a fault (design: foreign boundary semantics).
    let run = run_hosted(&p, "reader.images.image_width", &[bytes("00")], &codec);
    assert_eq!(run.result, Ok(err(not_an_image())));
}
