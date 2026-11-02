//! D4's scripted cases (toolchain 12.6): operation clauses and every outcome of semantics S77 with scripted
//! capabilities (valid, ill-typed, and refinement-violating results; foreign errors with and without a `ForeignError`
//! arm), a refinement over a refinement as an argument and as an operation's result, and a call no response matches
//! (semantics S80). Each `protocol::Script` is built here and bound on both engines under one label: the evaluator
//! through `run_function_with_hosts` with `ScriptHost`, the driver through `bind_scripted`.

use crate::{assert_clean_shutdown, driver_config, Gate};
use cpl_cabi::protocol::{ForeignFailure, Script, ScriptAnswer, ScriptResponse};
use cpl_cabi::scripted::ScriptHost;
use cpl_cabi::NativeSession;
use cpl_diag::{Diagnostic, SymbolPath};
use cpl_eval::run::RunHosts;
use cpl_eval::{
    prepare_run, run_function_with_hosts, BindOptions, CapabilityImpl, EvalConfig, RunArg,
};
use cpl_hir::{ParamDef, Program, Ty};
use serde_json::{json, Value as Json};

const CAPS: &str = r#"module caps

use std.foreign.{ForeignError}

pub type Positive = Int where self > 0
pub type Small = Positive where self < 10

pub struct Page {
    id: Int
    text: Text
}

pub capability Store {
    fn load(id: Int) -> Result<Page, ForeignError>
    fn count() -> Int
    fn size(id: Int) -> Positive
    fn small(n: Small) -> Small
    fn checked(limit: Int) -> Result<Small, ForeignError>
    fn next(limit: Int) -> Int
        requires limit > 0
        ensures result >= 0 and result < limit
}

pub fn load_page(id: Int, using store: Store) -> Result<Page, ForeignError>
    effects { store.load }
{
    store.load(id)
}

pub fn count_all(using store: Store) -> Int
    effects { store.count }
{
    store.count()
}

pub fn size_of(id: Int, using store: Store) -> Positive
    effects { store.size }
{
    store.size(id)
}

pub fn small_of(n: Small, using store: Store) -> Small
    effects { store.small }
{
    store.small(n)
}

pub fn checked_of(limit: Int, using store: Store) -> Result<Small, ForeignError>
    effects { store.checked }
{
    store.checked(limit)
}

pub fn draw(limit: Int, using store: Store) -> Int
    effects { store.next }
{
    store.next(limit)
}

// operation clauses that fault while they are evaluated: claimed by op_requires and op_ensures (S34)
pub capability Meter {
    fn read(limit: Int) -> Int
        requires 10 / limit > 0
        ensures 10 / result > 0
}

pub fn measure(limit: Int, using m: Meter) -> Int
    effects { m.read }
{
    m.read(limit)
}

pub fn load_both(using store: Store) -> Result<Int, ForeignError>
    effects { store.load }
{
    let a = store.load(1)?
    let b = store.load(2)?
    Ok(a.id + b.id)
}
"#;

/// One response: the operation, its arguments as (value JSON, type text), and its answer.
enum Answer {
    /// a result: its value JSON decoded under the type of that text (another type than the operation's for an
    /// ill-typed result)
    Value(Json, &'static str),
    Failure(
        &'static str,
        &'static str,
        Vec<(&'static str, &'static str)>,
    ),
}

/// One scripted response: the operation, its arguments as (value JSON, type text), and its answer.
type Response = (&'static str, Vec<(Json, &'static str)>, Answer);

struct Case {
    function: &'static str,
    args: Vec<Json>,
    responses: Vec<Response>,
}

/// The outcomes of semantics S77 the coverage gate requires (toolchain 12.6).
pub(crate) const S77_OUTCOMES: [&str; 7] = [
    "valid",
    "ill_typed_with_arm",
    "ill_typed_without_arm",
    "refinement_with_arm",
    "refinement_without_arm",
    "foreign_with_arm",
    "foreign_without_arm",
];

/// The S77 outcome a case produces (by its position in `cases()`), if it is one.
fn outcome_of(i: usize) -> Option<&'static str> {
    Some(match i {
        0 => "valid",
        1 => "foreign_with_arm",
        2 => "foreign_without_arm",
        3 | 4 => "ill_typed_with_arm",
        5 => "ill_typed_without_arm",
        6 => "refinement_without_arm",
        13 | 14 => "refinement_with_arm",
        _ => return None,
    })
}

fn ty_of(p: &Program, text: &str) -> Ty {
    match text {
        "Int" => Ty::Int,
        "Text" => Ty::Text,
        "Bool" => Ty::Bool,
        _ => {
            let path = SymbolPath::parse(text).unwrap();
            let d = p.lookup(&path).unwrap_or_else(|| panic!("no type {text}"));
            match &p.def(d).kind {
                cpl_hir::DefKind::Struct(_) | cpl_hir::DefKind::Enum(_) => Ty::Adt(d, vec![]),
                _ => panic!("{text}"),
            }
        }
    }
}

/// `Result<T, ForeignError>` with T of the text.
fn foreign_result(p: &Program, ok: &str) -> Ty {
    Ty::Adt(
        p.std_defs.result,
        vec![ty_of(p, ok), Ty::Adt(p.std_defs.foreign_error, vec![])],
    )
}

fn script(p: &Program, case: &Case) -> Script {
    let value = |j: &Json, ty: &str| {
        let ty = match ty.strip_prefix("Result:") {
            Some(ok) => foreign_result(p, ok),
            None => ty_of(p, ty),
        };
        let v = cpl_eval::json::value_from_json(p, j, &ty).unwrap_or_else(|e| panic!("{j}: {e:?}"));
        cpl_cabi::wire::to_nodes(&v)
    };
    Script {
        responses: case
            .responses
            .iter()
            .map(|(op, args, answer)| ScriptResponse {
                op: op.to_string(),
                args: args.iter().map(|(j, ty)| value(j, ty)).collect(),
                answer: match answer {
                    Answer::Value(j, ty) => ScriptAnswer::Result(value(j, ty)),
                    Answer::Failure(kind, message, details) => {
                        ScriptAnswer::ForeignError(ForeignFailure {
                            kind: kind.to_string(),
                            message: message.to_string(),
                            details: details
                                .iter()
                                .map(|(k, v)| (k.to_string(), v.to_string()))
                                .collect(),
                        })
                    }
                },
            })
            .collect(),
    }
}

/// The evaluator's hosts: one scripted host per `using` parameter, all of one script.
struct Scripted {
    script: Script,
    label: &'static str,
}

impl RunHosts for Scripted {
    fn available(&self, _param: &ParamDef) -> bool {
        true
    }

    fn create(
        &mut self,
        param: &ParamDef,
    ) -> Result<(Box<dyn CapabilityImpl>, BindOptions), Vec<Diagnostic>> {
        let Ty::Capability(cap) = param.ty else {
            panic!("a using parameter")
        };
        Ok((
            Box::new(ScriptHost::new(cap, self.label, self.script.clone())),
            BindOptions {
                label: self.label.to_string(),
                validate_results: true,
            },
        ))
    }
}

fn cases() -> Vec<Case> {
    let page = |id: i64, text: &str| json!({"$type": "caps.Page", "id": id, "text": text});
    let ok = |v: Json| json!({"$type": "Result", "$variant": "Ok", "$payload": [v]});
    vec![
        // a valid result
        Case {
            function: "caps.load_page",
            args: vec![json!(1)],
            responses: vec![(
                "load",
                vec![(json!(1), "Int")],
                Answer::Value(ok(page(1, "a")), "Result:caps.Page"),
            )],
        },
        // a reported failure with a ForeignError arm: the runtime's keys, then the host's own
        Case {
            function: "caps.load_page",
            args: vec![json!(2)],
            responses: vec![(
                "load",
                vec![(json!(2), "Int")],
                Answer::Failure("Unavailable", "store down", vec![("retry_safe", "true")]),
            )],
        },
        // a reported failure without a ForeignError arm: binding_error_unrepresentable (S80)
        Case {
            function: "caps.count_all",
            args: vec![],
            responses: vec![(
                "count",
                vec![],
                Answer::Failure("Exception", "boom", vec![]),
            )],
        },
        // an ill-typed result with and without a ForeignError arm (S77)
        Case {
            function: "caps.load_page",
            args: vec![json!(3)],
            responses: vec![(
                "load",
                vec![(json!(3), "Int")],
                Answer::Value(json!(7), "Int"),
            )],
        },
        Case {
            function: "caps.load_page",
            args: vec![json!(4)],
            responses: vec![(
                "load",
                vec![(json!(4), "Int")],
                Answer::Value(ok(json!("x")), "Result:Text"),
            )],
        },
        Case {
            function: "caps.count_all",
            args: vec![],
            responses: vec![("count", vec![], Answer::Value(json!("seven"), "Text"))],
        },
        // a refinement-violating result: without a ForeignError arm a refinement_violation blamed on the operation
        Case {
            function: "caps.size_of",
            args: vec![json!(1)],
            responses: vec![(
                "size",
                vec![(json!(1), "Int")],
                Answer::Value(json!(0), "Int"),
            )],
        },
        Case {
            function: "caps.size_of",
            args: vec![json!(2)],
            responses: vec![(
                "size",
                vec![(json!(2), "Int")],
                Answer::Value(json!(5), "Int"),
            )],
        },
        // a refinement over a refinement: the argument and the result, each one position (S74)
        Case {
            function: "caps.small_of",
            args: vec![json!(3)],
            responses: vec![(
                "small",
                vec![(json!(3), "Int")],
                Answer::Value(json!(4), "Int"),
            )],
        },
        Case {
            function: "caps.small_of",
            args: vec![json!(3)],
            responses: vec![(
                "small",
                vec![(json!(3), "Int")],
                Answer::Value(json!(12), "Int"),
            )],
        },
        Case {
            function: "caps.small_of",
            args: vec![json!(3)],
            responses: vec![(
                "small",
                vec![(json!(3), "Int")],
                Answer::Value(json!(0), "Int"),
            )],
        },
        Case {
            function: "caps.small_of",
            args: vec![json!(12)],
            responses: vec![],
        },
        Case {
            function: "caps.small_of",
            args: vec![json!(-1)],
            responses: vec![],
        },
        // with a ForeignError arm, a refinement failure of the result is InvalidResult (S77)
        Case {
            function: "caps.checked_of",
            args: vec![json!(1)],
            responses: vec![(
                "checked",
                vec![(json!(1), "Int")],
                Answer::Value(ok(json!(12)), "Result:Int"),
            )],
        },
        Case {
            function: "caps.checked_of",
            args: vec![json!(2)],
            responses: vec![(
                "checked",
                vec![(json!(2), "Int")],
                Answer::Value(ok(json!(-4)), "Result:Int"),
            )],
        },
        Case {
            function: "caps.checked_of",
            args: vec![json!(3)],
            responses: vec![(
                "checked",
                vec![(json!(3), "Int")],
                Answer::Value(ok(json!(9)), "Result:Int"),
            )],
        },
        // operation clauses: requires (the caller), ensures (the binding), and both holding
        Case {
            function: "caps.draw",
            args: vec![json!(0)],
            responses: vec![],
        },
        Case {
            function: "caps.draw",
            args: vec![json!(5)],
            responses: vec![(
                "next",
                vec![(json!(5), "Int")],
                Answer::Value(json!(7), "Int"),
            )],
        },
        Case {
            function: "caps.draw",
            args: vec![json!(5)],
            responses: vec![(
                "next",
                vec![(json!(5), "Int")],
                Answer::Value(json!(3), "Int"),
            )],
        },
        // a call no response matches (S80), and two calls each taking the first unused match in order
        Case {
            function: "caps.count_all",
            args: vec![],
            responses: vec![(
                "load",
                vec![(json!(1), "Int")],
                Answer::Value(ok(page(1, "a")), "Result:caps.Page"),
            )],
        },
        Case {
            function: "caps.load_both",
            args: vec![],
            responses: vec![
                (
                    "load",
                    vec![(json!(2), "Int")],
                    Answer::Value(ok(page(20, "b")), "Result:caps.Page"),
                ),
                (
                    "load",
                    vec![(json!(1), "Int")],
                    Answer::Value(ok(page(10, "a")), "Result:caps.Page"),
                ),
            ],
        },
        Case {
            function: "caps.load_both",
            args: vec![],
            responses: vec![
                (
                    "load",
                    vec![(json!(1), "Int")],
                    Answer::Value(ok(page(10, "a")), "Result:caps.Page"),
                ),
                (
                    "load",
                    vec![(json!(2), "Int")],
                    Answer::Failure(
                        "Timeout",
                        "slow",
                        vec![("attempt", "1"), ("retry_safe", "false")],
                    ),
                ),
            ],
        },
        Case {
            function: "caps.load_both",
            args: vec![],
            responses: vec![(
                "load",
                vec![(json!(1), "Int")],
                Answer::Value(ok(page(10, "a")), "Result:caps.Page"),
            )],
        },
        // operation clauses that fault: the requires (before the call), the ensures (on the result)
        Case {
            function: "caps.measure",
            args: vec![json!(0)],
            responses: vec![],
        },
        Case {
            function: "caps.measure",
            args: vec![json!(5)],
            responses: vec![(
                "read",
                vec![(json!(5), "Int")],
                Answer::Value(json!(0), "Int"),
            )],
        },
        Case {
            function: "caps.measure",
            args: vec![json!(5)],
            responses: vec![(
                "read",
                vec![(json!(5), "Int")],
                Answer::Value(json!(2), "Int"),
            )],
        },
    ]
}

/// D4's scripted cases, compared and recorded in the gate.
pub(crate) fn d4_scripted(gate: &mut Gate) {
    let project = crate::project_of("caps", "caps.caps", CAPS);
    let checked = cpl_sema::check(&project);
    assert!(!checked.has_errors(), "{:?}", checked.diagnostics);
    let p = &checked.program;
    let sm = project.source_map.clone();
    let plan = cpl_sema::interop::InteropPlan::empty(project.manifest.c_prefix());
    let cfg = EvalConfig::default();
    let mut session = NativeSession::start(p, &project, &plan, &cfg, &driver_config(gate.sanitize))
        .unwrap_or_else(|e| {
            panic!(
                "the native engine does not start: {:?}",
                e.into_diagnostics()
            )
        });
    session.set_request_timeout(Some(std::time::Duration::from_secs(60)));
    let all = cases();
    // CPL_D4_ONLY=<index> runs one case (a divergence or a leak reproduced alone)
    let only: Option<usize> = std::env::var("CPL_D4_ONLY")
        .ok()
        .and_then(|v| v.parse().ok());
    for (i, case) in all.iter().enumerate() {
        if only.is_some_and(|o| o != i) {
            continue;
        }
        let path = SymbolPath::parse(case.function).unwrap();
        let args: Vec<RunArg> = case.args.iter().cloned().map(RunArg::Json).collect();
        let script = script(p, case);
        let mut hosts = Scripted {
            script: script.clone(),
            label: "fixture:store",
        };
        let eval = run_function_with_hosts(p, &path, &args, &cfg, &mut hosts)
            .expect("the evaluator runs the case");
        let prepared = prepare_run(p, &path, &args, &|_| true).expect("prepare_run");
        let fd = p.def(prepared.function).kind.as_fn().expect("a function");
        let mut handles = Vec::new();
        for param in &fd.using {
            let Ty::Capability(cap) = param.ty else {
                panic!("a using parameter")
            };
            handles.push(
                session
                    .bind_scripted(cap, "fixture:store", script.clone())
                    .expect("bind_scripted"),
            );
        }
        let native = session
            .call(prepared.function, &prepared.arguments, &handles, true)
            .unwrap_or_else(|e| {
                panic!(
                    "{}: the driver failed: {:?}",
                    case.function,
                    e.into_diagnostics()
                )
            });
        let before = gate.divergences.len();
        gate.compare(
            &session,
            p,
            &sm,
            &eval.return_ty,
            &eval.value,
            &native,
            format!("D4 scripted {} {:?}", case.function, case.args),
        );
        if gate.divergences.len() == before {
            if let Some(o) = outcome_of(i) {
                gate.s77.insert(o.to_string());
            }
        }
    }
    gate.absorb(&mut session);
    assert_clean_shutdown(session, "caps");
}
