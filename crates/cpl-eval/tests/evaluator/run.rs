//! `run_function` (toolchain 1.7, 7.2; semantics S39) on a checked
//! program: argument decoding from constant expressions and value JSON,
//! refinement-typed parameters, and every refusal. The 12.3 table itself
//! runs on the example projects (`tests/examples.rs`).

use crate::common;
use crate::support::diag;
use cpl_eval::json::{value_text, value_to_json};
use cpl_eval::run::{ARGUMENT_FILE, USAGE_BAD_ARGUMENT};
use cpl_eval::{run_function, EvalConfig, RunArg, RunError, RunOutcome};
use cpl_hir::{DefId, Program, Ty};
use serde_json::{json, Value as Json};

/// The design's `reader.pages` declarations as `checks.run`, plus small
/// functions with parameters of every value form.
const SOURCE: &str = r#"module checks.run

use std.text.{trim}
use std.text.{parse_int}

pub type PageId(Text)
pub type Positive = Int where self > 0
pub type Small = Positive where self < 10
pub type Nz = Int where 10 / self > 0

pub struct Page {
    id: PageId
    text: Text
}

pub struct Point {
    x: Int
    y: Int
}

pub enum StoreError {
    Missing(PageId)
    Unavailable
}

fn label(result: Result<Page, StoreError>) -> Text {
    match result {
        Ok(page) => page.text
        Err(Missing(_)) => "Missing page"
        Err(Unavailable) => "Storage unavailable"
    }
}

fn parse_size(text: Text) -> Result<Int, ConstraintError> {
    parse_int(trim(text))
}

fn positive_size(text: Text) -> Result<Positive, ConstraintError> {
    let n = parse_size(text)?
    check<Positive>(n)
}

pub fn paginate<T>(items: List<T>, size: Positive) -> List<List<T>>
    ensures flatten(result) == items
    ensures forall page in result: page.len > 0 and page.len <= size
    ensures forall page in result.drop_last(): page.len == size
{
    var pages: List<List<T>> = []
    var rest = items

    while rest.len > size
        invariant flatten(pages) ++ rest == items
        invariant forall page in pages: page.len == size
        decreases rest.len
    {
        pages = pages.push(rest.take(size))
        rest = rest.drop(size)
    }

    if not rest.is_empty() {
        pages = pages.push(rest)
    }

    pages
}

pub fn paginate_ints(
    items: List<Int>,
    size: Int,
) -> Result<List<List<Int>>, ConstraintError>
{
    let checked_size = check<Positive>(size)?
    Ok(paginate(items, checked_size))
}

pub capability PageRead {
    fn get(id: PageId) -> Result<Page, StoreError>
}

pub fn load_page(
    id: PageId,
    using db: PageRead,
) -> Result<Page, StoreError>
    effects { db.get }
{
    db.get(id)
}

fn divide_exactly(a: Int, b: Int) -> Int
    requires b != 0
    requires a % b == 0
    ensures result * b == a
{
    a / b
}

fn double(p: Positive) -> Int {
    p * 2
}

fn total(sizes: List<Positive>) -> Int {
    sizes.len
}

fn small_id(s: Small) -> Int {
    s
}

fn nz_id(n: Nz) -> Int {
    n
}

fn take_point(p: Point) -> Point {
    p
}

fn flip(b: Bool) -> Bool {
    not b
}

fn opt(o: Option<Int>) -> Int {
    match o {
        Some(n) => n
        None => 0
    }
}

fn echo_text(t: Text) -> Text {
    t
}

fn bytes_len(b: Bytes) -> Int {
    b.len
}

fn unit_id(u: Unit) -> Unit {
    u
}

fn sizes_of(ps: List<Page>, ids: List<PageId>) -> Int {
    ps.len + ids.len
}
"#;

/// The fixture's functions, by name.
struct Reader {
    paginate_ints: DefId,
    divide_exactly: DefId,
    positive_size: DefId,
    label: DefId,
    paginate: DefId,
    load_page: DefId,
    page_id: DefId,
    positive: DefId,
    double: DefId,
    total: DefId,
    small_id: DefId,
    nz_id: DefId,
    take_point: DefId,
    flip: DefId,
    opt: DefId,
    echo_text: DefId,
    bytes_len: DefId,
    unit_id: DefId,
    sizes_of: DefId,
}

/// `checks.run`, checked.
fn built() -> (Program, Reader) {
    let p = common::check_source(SOURCE);
    let d = |name: &str| common::def(&p, &format!("checks.run.{name}"));
    let r = Reader {
        paginate_ints: d("paginate_ints"),
        divide_exactly: d("divide_exactly"),
        positive_size: d("positive_size"),
        label: d("label"),
        paginate: d("paginate"),
        load_page: d("load_page"),
        page_id: d("PageId"),
        positive: d("Positive"),
        double: d("double"),
        total: d("total"),
        small_id: d("small_id"),
        nz_id: d("nz_id"),
        take_point: d("take_point"),
        flip: d("flip"),
        opt: d("opt"),
        echo_text: d("echo_text"),
        bytes_len: d("bytes_len"),
        unit_id: d("unit_id"),
        sizes_of: d("sizes_of"),
    };
    (p, r)
}

fn run(p: &Program, f: DefId, args: &[RunArg]) -> Result<RunOutcome, RunError> {
    run_function(p, &p.symbol_path(f), args, &EvalConfig::default())
}

fn c(s: &str) -> RunArg {
    RunArg::ConstExpr(s.to_string())
}

fn j(v: Json) -> RunArg {
    RunArg::Json(v)
}

/// The value JSON of a run that returned.
fn returned(p: &Program, f: DefId, args: &[RunArg]) -> Json {
    let outcome = run(p, f, args).unwrap_or_else(|e| panic!("refused: {e}"));
    match &outcome.value {
        Ok(v) => value_to_json(p, v, &outcome.return_ty),
        Err(fault) => panic!("faulted: {}", diag(p, fault)),
    }
}

/// The fault diagnostic of a run that faulted.
fn faulted(p: &Program, f: DefId, args: &[RunArg]) -> Json {
    let outcome = run(p, f, args).unwrap_or_else(|e| panic!("refused: {e}"));
    match &outcome.value {
        Ok(v) => panic!("returned {}", value_to_json(p, v, &outcome.return_ty)),
        Err(fault) => diag(p, fault),
    }
}

/// The (index, pointer, message) of an ill-typed argument.
fn arg_error(p: &Program, f: DefId, args: &[RunArg]) -> (usize, String, String) {
    match run(p, f, args) {
        Err(RunError::Arg { index, error }) => (index, error.pointer, error.message),
        Err(other) => panic!("expected an argument error, got {other:?}"),
        Ok(_) => panic!("expected an argument error, the call ran"),
    }
}

/// The (index, diagnostic) of an argument that is not a constant expression.
fn not_constant(p: &Program, f: DefId, args: &[RunArg]) -> (usize, cpl_diag::Diagnostic) {
    match run(p, f, args) {
        Err(RunError::ArgNotConstant { index, diagnostic }) => (index, diagnostic),
        Err(other) => panic!("expected ArgNotConstant, got {other:?}"),
        Ok(_) => panic!("expected ArgNotConstant, the call ran"),
    }
}

#[test]
fn json_arguments_decode_against_the_parameters() {
    let (p, r) = built();
    assert_eq!(
        returned(&p, r.paginate_ints, &[j(json!([1, 2, 3])), j(json!(2))]),
        json!({"$type": "Result", "$variant": "Ok", "$payload": [[[1, 2], [3]]]})
    );
    // The two forms may be mixed.
    assert_eq!(
        returned(&p, r.paginate_ints, &[c("[1, 2, 3, 4]"), j(json!(4))]),
        json!({"$type": "Result", "$variant": "Ok", "$payload": [[[1, 2, 3, 4]]]})
    );
    let missing = json!({"$type": "Result", "$variant": "Err", "$payload": [
        {"$type": "checks.run.StoreError", "$variant": "Missing", "$payload": [
            {"$type": "checks.run.PageId", "$payload": ["p"]}]}]});
    assert_eq!(returned(&p, r.label, &[j(missing)]), json!("Missing page"));
    assert_eq!(
        returned(
            &p,
            r.bytes_len,
            &[j(json!({"$type": "Bytes", "hex": "6869"}))]
        ),
        json!(2)
    );
    let (index, pointer, message) =
        arg_error(&p, r.paginate_ints, &[j(json!([1, "x"])), j(json!(1))]);
    assert_eq!((index, pointer.as_str()), (0, "/1"));
    assert!(message.contains("expected `Int`"), "{message}");
}

#[test]
fn a_refinement_typed_parameter_is_checked_s39() {
    let (p, r) = built();
    assert_eq!(returned(&p, r.double, &[c("4")]), json!(8));
    for arg in [c("0"), j(json!(0))] {
        let d = faulted(&p, r.double, &[arg]);
        assert_eq!(d["code"], "refinement_violation");
        assert_eq!(d["category"], "runtime");
        assert_eq!(d["symbol"], "checks.run.double");
        assert_eq!(d["expected_type"], "Positive");
        assert_eq!(d["actual"], json!(0));
        assert_eq!(d["clause"], "self > 0");
        assert_eq!(d["parameter"], "p");
        assert_eq!(d["pointer"], "");
        assert_eq!(d["facet"], "body");
    }
    // A refined position nested in a list.
    assert_eq!(returned(&p, r.total, &[c("[1, 2, 3]")]), json!(3));
    let d = faulted(&p, r.total, &[c("[1, 0, 2]")]);
    assert_eq!(d["code"], "refinement_violation");
    assert_eq!(d["pointer"], "/1");
    assert_eq!(d["actual"], json!(0));
    // A chain: the root predicate first, then the declaration's own.
    assert_eq!(returned(&p, r.small_id, &[c("9")]), json!(9));
    let d = faulted(&p, r.small_id, &[c("0")]);
    assert_eq!(
        (d["expected_type"].clone(), d["clause"].clone()),
        (json!("Small"), json!("self > 0"))
    );
    let d = faulted(&p, r.small_id, &[c("20")]);
    assert_eq!(
        (d["expected_type"].clone(), d["clause"].clone()),
        (json!("Small"), json!("self < 10"))
    );
    // A predicate that faults is that fault, blamed on the refinement (S34).
    let d = faulted(&p, r.nz_id, &[c("0")]);
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["symbol"], "checks.run.Nz");
    assert_eq!(d["clause"], "10 / self > 0");
    assert_eq!(returned(&p, r.nz_id, &[c("5")]), json!(5));
}

#[test]
fn constant_expressions_cover_every_value_form() {
    let (p, r) = built();
    let point = |x: i64, y: i64| json!({"$type": "checks.run.Point", "x": x, "y": y});
    assert_eq!(
        returned(&p, r.take_point, &[c("Point { y: 2, x: -1 }")]),
        point(-1, 2)
    );
    assert_eq!(
        returned(&p, r.take_point, &[c("checks.run.Point { x: 1, y: (2) }")]),
        point(1, 2)
    );
    assert_eq!(returned(&p, r.flip, &[c("not true")]), json!(true));
    assert_eq!(returned(&p, r.flip, &[c("not (not false)")]), json!(true));
    assert_eq!(returned(&p, r.flip, &[c("(true)")]), json!(false));
    assert_eq!(returned(&p, r.opt, &[c("None")]), json!(0));
    assert_eq!(returned(&p, r.opt, &[c("Some(-5)")]), json!(-5));
    assert_eq!(returned(&p, r.opt, &[c("Option.Some(--5)")]), json!(5));
    assert_eq!(
        returned(&p, r.opt, &[c("std.prelude.Option.Some(7)")]),
        json!(7)
    );
    assert_eq!(
        returned(&p, r.echo_text, &[c(r#""a\n\u{e9}""#)]),
        json!("a\n\u{e9}")
    );
    assert_eq!(
        returned(&p, r.unit_id, &[c("Unit")]),
        json!({"$type": "Unit"})
    );
    assert_eq!(
        returned(&p, r.label, &[c("Err(Missing(PageId(\"p\")))")]),
        json!("Missing page")
    );
    assert_eq!(
        returned(&p, r.label, &[c("Err(StoreError.Unavailable)")]),
        json!("Storage unavailable")
    );
    assert_eq!(
        returned(
            &p,
            r.label,
            &[c(
                "Err(checks.run.StoreError.Missing(checks.run.PageId(\"q\")))"
            )]
        ),
        json!("Missing page")
    );
    assert_eq!(
        returned(
            &p,
            r.label,
            &[c("Ok(Page { id: PageId(\"p\"), text: \"body\" })")]
        ),
        json!("body")
    );
    assert_eq!(
        returned(
            &p,
            r.sizes_of,
            &[
                c("[Page { id: PageId(\"a\"), text: \"\" }]"),
                c("[PageId(\"x\"), PageId(\"y\")]")
            ]
        ),
        json!(3)
    );
    let big = "123456789012345678901234567890";
    let outcome = run(&p, r.paginate_ints, &[c(&format!("[{big}]")), c("1")]).unwrap();
    assert_eq!(
        value_text(&p, outcome.value.as_ref().unwrap(), &outcome.return_ty),
        format!("Ok([[{big}]])")
    );
}

#[test]
fn ill_typed_constants_are_argument_errors_with_pointers() {
    let (p, r) = built();
    let (index, pointer, message) = arg_error(&p, r.divide_exactly, &[c("6"), c("\"a\"")]);
    assert_eq!((index, pointer.as_str()), (1, ""));
    assert_eq!(message, "expected `Int`, found a Text literal");
    let (_, pointer, _) = arg_error(&p, r.paginate_ints, &[c("[1, true]"), c("1")]);
    assert_eq!(pointer, "/1");
    let (_, pointer, message) = arg_error(&p, r.label, &[c("Err(Other.Missing(PageId(\"p\")))")]);
    assert_eq!(pointer, "/$payload/0");
    assert!(
        message.contains("the constructor `Other.Missing`"),
        "{message}"
    );
    let (_, pointer, message) = arg_error(&p, r.label, &[c("Err(Missing(\"p\"))")]);
    assert_eq!(pointer, "/$payload/0/$payload/0");
    assert!(message.contains("expected `PageId`"), "{message}");
    let (_, _, message) = arg_error(&p, r.label, &[c("Err(Missing)")]);
    assert!(
        message.contains("takes 1 payload value(s), found 0"),
        "{message}"
    );
    let (_, _, message) = arg_error(&p, r.take_point, &[c("Point { x: 1 }")]);
    assert_eq!(message, "missing field `y` of `Point`");
    let (_, _, message) = arg_error(&p, r.take_point, &[c("Point { x: 1, y: 2, z: 3 }")]);
    assert_eq!(message, "`Point` has no field `z`");
    let (_, _, message) = arg_error(&p, r.take_point, &[c("Point { x: 1, x: 2, y: 3 }")]);
    assert_eq!(message, "the field `x` is given twice");
    let (_, _, message) = arg_error(&p, r.take_point, &[c("Page { x: 1, y: 2 }")]);
    assert!(message.contains("expected `Point`"), "{message}");
    let (_, _, message) = arg_error(&p, r.double, &[c("-true")]);
    assert_eq!(message, "expected `Int`, found a Bool literal");
    // Bytes has no constant form (D8).
    let (_, _, message) = arg_error(&p, r.bytes_len, &[c("\"hi\"")]);
    assert!(message.contains("--arg"), "{message}");
}

#[test]
fn forms_outside_the_constant_grammar_are_refused() {
    let (p, r) = built();
    for (src, start, end) in [
        ("x", 0, 1),
        ("f(1)", 0, 4),
        ("1 + 2", 0, 5),
        ("Point { ..p, x: 1 }", 10, 11),
        ("if true { 1 } else { 2 }", 0, 24),
        ("Some<Int>(1)", 0, 12),
    ] {
        let (index, d) = not_constant(&p, r.double, &[c(src)]);
        assert_eq!(index, 0);
        assert_eq!(d.code, USAGE_BAD_ARGUMENT, "{src}");
        assert_eq!(
            d.span.map(|s| (s.file, s.start, s.end)),
            Some((ARGUMENT_FILE, start, end)),
            "{src}"
        );
        assert_eq!(d.data["argument"], json!(0));
    }
    // A parse failure is the parser's own diagnostic.
    let (_, d) = not_constant(&p, r.double, &[c("[1,")]);
    assert_eq!(d.code, "syntax_error");
    let (_, d) = not_constant(&p, r.double, &[c("1 2")]);
    assert_eq!(d.code, "syntax_error");
}

#[test]
fn runs_that_are_refused() {
    let (p, r) = built();
    let missing = cpl_diag::SymbolPath::parse("checks.run.nothing").unwrap();
    assert!(matches!(
        run_function(&p, &missing, &[], &EvalConfig::default()),
        Err(RunError::NotFound(name)) if name == "checks.run.nothing"
    ));
    assert!(matches!(
        run(&p, r.page_id, &[]),
        Err(RunError::NotAFunction(name)) if name == "checks.run.PageId"
    ));
    assert!(matches!(
        run(&p, r.paginate, &[c("[1]"), c("1")]),
        Err(RunError::Generic(name)) if name == "checks.run.paginate"
    ));
    assert!(matches!(
        run(&p, r.load_page, &[c("PageId(\"p\")")]),
        Err(RunError::CannotRunEffectful(name)) if name == "checks.run.load_page"
    ));
    assert!(matches!(
        run(&p, r.divide_exactly, &[c("1")]),
        Err(RunError::ArgCount {
            expected: 2,
            got: 1
        })
    ));
    // A program with errors runs nothing, not even a function that checked.
    let broken = SOURCE.replace(
        "fn flip(b: Bool) -> Bool {\n    not b\n}",
        "fn flip(b: Bool) -> Bool {\n    1\n}",
    );
    let result = common::check_source_result(&broken);
    assert!(result.has_errors(), "{}", common::render(&result));
    let p = result.program;
    let divide = common::def(&p, "checks.run.divide_exactly");
    assert!(matches!(
        run(&p, divide, &[c("6"), c("3")]),
        Err(RunError::ProgramHasErrors)
    ));
    // So does a program without definitions (a file failed to parse, S52).
    let result = common::check_source_result("module checks.run\n\nfn (\n");
    assert!(result.program.defs.is_empty());
    assert!(matches!(
        run_function(&result.program, &missing, &[], &EvalConfig::default()),
        Err(RunError::ProgramHasErrors)
    ));
}

#[test]
fn the_return_type_is_the_declared_one() {
    let (p, r) = built();
    let outcome = run(&p, r.positive_size, &[c("\"5\"")]).unwrap();
    let cpl_hir::Ty::Adt(def, args) = &outcome.return_ty else {
        panic!("{:?}", outcome.return_ty)
    };
    assert_eq!(*def, p.std_defs.result);
    assert!(matches!(args[0], Ty::Refined(_, d) if d == r.positive));
    assert_eq!(
        value_text(&p, outcome.value.as_ref().unwrap(), &outcome.return_ty),
        "Ok(5)"
    );
}

/// The value JSON of a run's decoded arguments, each at its parameter's
/// type (what `cpl run` reports as `RunOutput::arguments`).
fn decoded(p: &Program, f: DefId, args: &[RunArg]) -> Json {
    let outcome = run(p, f, args).unwrap_or_else(|e| panic!("refused: {e}"));
    let params = &p.def(f).kind.as_fn().expect("a function").params;
    assert_eq!(outcome.arguments.len(), params.len());
    Json::Array(
        outcome
            .arguments
            .iter()
            .zip(params)
            .map(|(value, param)| value_to_json(p, value, &param.ty))
            .collect(),
    )
}

#[test]
fn the_outcome_holds_the_arguments_as_decoded() {
    let (p, r) = built();
    let json = |text: &str| -> Json { serde_json::from_str(text).expect("JSON") };
    // Toolchain 7.2: the two forms are the same values spelled two ways, so
    // they decode to the same value, reported in canonical value JSON
    // (`-0` is the Int 0, `-(-2)` is 2).
    for args in [
        vec![c("[1, 2, 3]"), c("2")],
        vec![j(json!([1, 2, 3])), j(json!(2))],
        vec![c("[1, (2), 3]"), j(json("2"))],
        vec![j(json("[1, 2, 3]")), c("-(-2)")],
    ] {
        assert_eq!(
            decoded(&p, r.paginate_ints, &args),
            json!([[1, 2, 3], 2]),
            "{args:?}"
        );
    }
    assert_eq!(
        decoded(&p, r.paginate_ints, &[j(json("[-0, 0, 7]")), c("-0")]),
        json!([[0, 0, 7], 0])
    );
    let point = json!([{"$type": "checks.run.Point", "x": -1, "y": 2}]);
    assert_eq!(
        decoded(&p, r.take_point, &[c("Point { y: 2, x: -1 }")]),
        point
    );
    assert_eq!(decoded(&p, r.take_point, &[j(point[0].clone())]), point);
    assert_eq!(
        decoded(&p, r.label, &[c("Err(StoreError.Missing(PageId(\"p\")))")]),
        json!([{"$type": "Result", "$variant": "Err", "$payload": [
            {"$type": "checks.run.StoreError", "$variant": "Missing", "$payload": [
                {"$type": "checks.run.PageId", "$payload": ["p"]}]}]}])
    );
    assert_eq!(
        decoded(&p, r.unit_id, &[c("Unit")]),
        json!([{"$type": "Unit"}])
    );
    assert_eq!(
        decoded(
            &p,
            r.bytes_len,
            &[j(json!({"$type": "Bytes", "hex": "6869"}))]
        ),
        json!([{"$type": "Bytes", "hex": "6869"}])
    );
    assert_eq!(decoded(&p, r.flip, &[c("not true")]), json!([false]));
    // A run that faults still holds its arguments: a requires violation,
    // and a refinement violation of the argument itself (S39 (a)).
    assert_eq!(
        faulted(&p, r.divide_exactly, &[c("7"), c("0")])["code"],
        "contract_violation"
    );
    assert_eq!(
        decoded(&p, r.divide_exactly, &[c("7"), c("0")]),
        json!([7, 0])
    );
    assert_eq!(
        faulted(&p, r.total, &[c("[1, 0, 2]")])["code"],
        "refinement_violation"
    );
    assert_eq!(decoded(&p, r.total, &[c("[1, 0, 2]")]), json!([[1, 0, 2]]));
}
