//! D4's hand-built programs (toolchain 12.6): states no checked program reaches, built with
//! `cpl_eval::testing::ProgramBuilder` and emitted with a synthetic `Project` (`root` None, the manifest of a minimal
//! `cpl.toml`, the program's source map) and `InteropPlan::empty` (toolchain 1.13): a generator used as a value, an
//! unsound `Refine`, and a `match` whose arms match nothing. Each is an `internal_error` on both engines.

use crate::{assert_clean_shutdown, driver_config, Gate};
use cpl_diag::{FileOrigin, SymbolPath};
use cpl_eval::testing::{FnBuilder, ProgramBuilder};
use cpl_eval::{prepare_run, run_function, EvalConfig, RunArg};
use cpl_hir::{BindingKind, GenExpr, PatternKind, Stmt, Ty};
use cpl_project::{load_project_from_tree, LoadOptions, VirtualTree};
use cpl_syntax::ast::BinaryOp;

pub(crate) fn d4_builder(gate: &mut Gate) {
    let mut b = ProgramBuilder::new();
    let m = b.module("built.cases");
    let h = b.hir(m);
    // fn gen_value() -> Int { let g = gen.int; 0 }: a generator as a value
    let mut f = FnBuilder::new("gen_value", Ty::Int);
    let g = f.local("g", Ty::Gen(Box::new(Ty::Int)), BindingKind::Let, false);
    f.body(h.block(
        vec![Stmt::Let {
            local: g,
            value: h.gen(GenExpr::Int, Ty::Int),
        }],
        Some(h.int(0)),
    ));
    b.add_fn(m, f.public());
    // fn unsound(n: Int) -> Positive { Refine(n) }: a proof that does not hold at run time
    let positive = b.add_int_refinement(m, "Positive", BinaryOp::Gt, 0);
    let pos_ty = b.refined(positive);
    let mut f = FnBuilder::new("unsound", pos_ty.clone());
    let n = f.param("n", Ty::Int);
    f.body(h.value(h.refine(h.local(n, Ty::Int), positive, pos_ty)));
    b.add_fn(m, f.public());
    // fn no_arm(n: Int) -> Int { match n { 1 => 1 } }: no arm matches 2
    let mut f = FnBuilder::new("no_arm", Ty::Int);
    let n = f.param("n", Ty::Int);
    let arm = h.arm(
        h.pattern(PatternKind::Int(1.into()), Ty::Int),
        h.value(h.int(1)),
    );
    f.body(h.value(h.match_(h.local(n, Ty::Int), vec![arm], Ty::Int)));
    b.add_fn(m, f.public());
    let p = b.build();
    // the synthetic project of toolchain 1.13
    let tree = VirtualTree {
        files: [(
            "cpl.toml".to_string(),
            b"[project]\nname = \"built\"\nedition = \"0.1\"\n".to_vec(),
        )]
        .into_iter()
        .collect(),
    };
    let mut project =
        load_project_from_tree(&tree, FileOrigin::Virtual, &LoadOptions::default()).unwrap();
    project.root = None;
    project.source_map = p.source_map.clone();
    let sm = p.source_map.clone();
    let plan = cpl_sema::interop::InteropPlan::empty(project.manifest.c_prefix());
    let cfg = EvalConfig::default();
    let mut session =
        cpl_cabi::NativeSession::start(&p, &project, &plan, &cfg, &driver_config(gate.sanitize))
            .unwrap_or_else(|e| {
                panic!(
                    "built: the native engine does not start: {:?}",
                    e.into_diagnostics()
                )
            });
    session.set_request_timeout(Some(std::time::Duration::from_secs(60)));
    for (function, args) in [
        ("built.cases.gen_value", vec![]),
        ("built.cases.unsound", vec![serde_json::json!(3)]),
        ("built.cases.unsound", vec![serde_json::json!(0)]),
        ("built.cases.no_arm", vec![serde_json::json!(1)]),
        ("built.cases.no_arm", vec![serde_json::json!(2)]),
    ] {
        let path = SymbolPath::parse(function).unwrap();
        let args: Vec<RunArg> = args.into_iter().map(RunArg::Json).collect();
        let eval = run_function(&p, &path, &args, &cfg).expect("the evaluator runs the case");
        let prepared = prepare_run(&p, &path, &args, &|_| false).expect("prepare_run");
        let native = session
            .call(prepared.function, &prepared.arguments, &[], true)
            .unwrap_or_else(|e| {
                panic!("{function}: the driver failed: {:?}", e.into_diagnostics())
            });
        gate.compare(
            &session,
            &p,
            &sm,
            &eval.return_ty,
            &eval.value,
            &native,
            format!("D4 built {function}"),
        );
    }
    gate.absorb(&mut session);
    assert_clean_shutdown(session, "built");
}
