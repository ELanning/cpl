//! `cpl run`'s semantics for capability-using functions on both engines (toolchain 12.6): every such function of the
//! binding fixtures, the evaluator through `CAdapter` (`manifest_hosts`) and the native engine through the driver's
//! in-process grant (`bind`), compared after the S75 normalization. The fixture `cboundary` reaches the reviewed
//! wrapper over `imgtest` (`Ok`, `InvalidResult` of a refinement with a `ForeignError` arm, the error map) and
//! `cshapes` the declarative vocabulary (a refined result without an arm: `refinement_violation`; a `bool` result that
//! is no Bool: `binding_error_unrepresentable`).

use cpl_cabi::NativeSession;
use cpl_diag::SymbolPath;
use cpl_eval::{prepare_run, run_function_with_hosts, EvalConfig, RunArg};
use cpl_hir::Ty;
use cpl_project::{load_project, LoadOptions};
use serde_json::{json, Value as Json};
use std::path::{Path, PathBuf};
use std::process::Command;

fn tests_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let target = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &target);
        } else {
            std::fs::copy(e.path(), &target).unwrap();
        }
    }
}

/// A copy of a fixture with its vendored files (toolchain 12.7).
fn fixture(name: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("run_bindings")
        .join(name);
    let _ = std::fs::remove_dir_all(&root);
    copy_dir(&tests_dir().join("projects").join(name), &root);
    if name == "cboundary" {
        copy_dir(
            &tests_dir().join("clib/imgtest"),
            &root.join("vendor/imgtest"),
        );
        let extra = root.join("vendor/libextra");
        std::fs::create_dir_all(&extra).unwrap();
        let cc = cpl_rt::testing::find_cc().unwrap();
        let o = extra.join("extra.o");
        assert!(Command::new(&cc)
            .args(["-fPIC", "-c"])
            .arg(tests_dir().join("clib/libextra/extra.c"))
            .arg("-o")
            .arg(&o)
            .status()
            .unwrap()
            .success());
        assert!(Command::new("ar")
            .arg("rcs")
            .arg(extra.join("libextra.a"))
            .arg(&o)
            .status()
            .unwrap()
            .success());
    }
    root
}

fn render(
    p: &cpl_hir::Program,
    sm: &cpl_diag::SourceMap,
    ty: &Ty,
    r: &Result<cpl_eval::Value, cpl_eval::Fault>,
) -> Json {
    match r {
        Ok(v) => {
            json!({"value": cpl_eval::value_to_json(p, v, ty), "text": cpl_eval::value_text(p, v, ty)})
        }
        Err(f) => json!({"fault": serde_json::to_value(f.to_diagnostic(p).to_json(sm)).unwrap()}),
    }
}

/// Every case of one fixture: (function, value-JSON arguments).
fn run_fixture(name: &str, cases: &[(&str, Vec<Json>)]) -> Vec<String> {
    let root = fixture(name);
    let project = load_project(&root, &LoadOptions::default()).unwrap();
    let checked = cpl_sema::check(&project);
    assert!(!checked.has_errors(), "{name}: {:?}", checked.diagnostics);
    let p = &checked.program;
    let sm = project.source_map.clone();
    let plan = checked.interop.clone().unwrap();
    let dcfg = cpl_emit_c::package::DriverConfig {
        cache_dir: Path::new(env!("CARGO_TARGET_TMPDIR")).join("run_bindings-cache"),
        profile: cpl_emit_c::cc::Profile::Debug,
        sanitize: vec![],
        coverage: false,
        cc: None,
        runtime: cpl_rt::RuntimeSource::Embedded,
        werror: true,
    };
    let cfg = EvalConfig::default();
    let mut native = NativeSession::start(p, &project, &plan, &cfg, &dcfg).unwrap();
    native.set_request_timeout(Some(std::time::Duration::from_secs(60)));
    let mut out = Vec::new();
    for (function, args) in cases {
        let path = SymbolPath::parse(function).unwrap();
        let run_args: Vec<RunArg> = args.iter().cloned().map(RunArg::Json).collect();
        // the evaluator: cpl run's hosts, a CAdapter per using parameter
        let mut hosts = cpl_cabi::adapter::manifest_hosts(p, &project, &plan, &dcfg);
        let eval = run_function_with_hosts(p, &path, &run_args, &cfg, &mut hosts)
            .expect("the evaluator runs the case");
        // the native engine: the driver's grants
        let prepared = prepare_run(p, &path, &run_args, &|_| true).expect("prepare_run");
        let fd = p.def(prepared.function).kind.as_fn().unwrap();
        let mut handles = Vec::new();
        for param in &fd.using {
            let Ty::Capability(cap) = param.ty else {
                panic!("a using parameter")
            };
            let binding = plan
                .bindings
                .iter()
                .find(|b| b.capability == cap)
                .expect("a binding of the capability");
            handles.push(native.bind(&binding.name).expect("the grant"));
        }
        let outcome = native
            .call(prepared.function, &prepared.arguments, &handles, true)
            .expect("the driver answers");
        let (mut a, mut b) = (
            render(p, &sm, &eval.return_ty, &eval.value),
            render(p, &sm, &eval.return_ty, &outcome),
        );
        cpl_cabi::compare::normalize(&mut a);
        cpl_cabi::compare::normalize(&mut b);
        if a != b {
            out.push(format!(
                "{name} {function} {args:?}:\n  eval:   {a}\n  native: {b}"
            ));
        }
    }
    let report = native.shutdown().unwrap();
    if (report.live_handles, report.live_objects) != (0, 0) {
        out.push(format!(
            "{name}: the driver is not clean at shutdown: {report:?}"
        ));
    }
    out
}

#[test]
fn capability_using_functions_agree_on_both_engines() {
    if cpl_rt::testing::find_cc().is_none() {
        if std::env::var("CPL_REQUIRE_CC").is_ok_and(|v| v == "1") {
            panic!("CPL_REQUIRE_CC=1, but there is no C compiler");
        }
        println!("skipped: no C compiler (set CPL_REQUIRE_CC=1 to fail)");
        return;
    }
    let bytes = |hex: &str| json!({"$type": "Bytes", "hex": hex});
    let mut divergences = run_fixture(
        "cboundary",
        &[
            (
                "reader.images.image_width",
                vec![bytes("494d475480020000e0010000")],
            ),
            ("reader.images.image_width", vec![bytes("4e454757")]),
            ("reader.images.image_width", vec![bytes("7878")]),
            (
                "reader.images.image_width",
                vec![bytes("494d47580100000001000000")],
            ),
            (
                "reader.images.image_width",
                vec![bytes("494d4754214e000001000000")],
            ),
        ],
    );
    let size = json!({"$type": "app.shapes.Size", "w": 3, "h": 4, "flag": true});
    divergences.extend(run_fixture(
        "cshapes",
        &[
            (
                "app.shapes.scale_of",
                vec![size.clone(), json!("ok"), json!(true)],
            ),
            (
                "app.shapes.scale_of",
                vec![size.clone(), json!("flag2"), json!(false)],
            ),
            (
                "app.shapes.scale_of",
                vec![size, json!("bad"), json!(false)],
            ),
            ("app.shapes.upper_of", vec![json!("hello")]),
            ("app.shapes.upper_of", vec![json!("badutf8")]),
            ("app.shapes.is_even_of", vec![json!(4)]),
            ("app.shapes.is_even_of", vec![json!(42)]),
            ("app.shapes.parity_of", vec![json!(4)]),
            ("app.shapes.parity_of", vec![json!(3)]),
            ("app.shapes.touched", vec![json!(2)]),
            ("app.shapes.check_of", vec![json!(1500)]),
            ("app.shapes.check_of", vec![json!(70000)]),
        ],
    ));
    assert!(
        divergences.is_empty(),
        "{} divergences:\n{}",
        divergences.len(),
        divergences.join("\n")
    );
}
