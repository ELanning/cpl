//! The C host suites (toolchain 12.7): every host program of `tests/c/` against the fixtures its first line names,
//! built through `cpl_emit_c::package::build`, compiled with `-std=c11 -Wall -Wextra -Werror` in two configurations
//! (`-O2` against an optimized library; ASan and UBSan against a sanitized one), linked against the shared and,
//! separately, the static library, and run. Each host checks its own expectations and ends with zero live handles,
//! capabilities, and objects and a successful `cpl_runtime_close`; every call line it prints (`tests/c/host_common.h`)
//! is compared with the evaluator's outcome of the same call after the S75 normalization, and the exit report of
//! `imgtest` (`IMGTEST_REPORT`) must show no live allocation and no double free.

use cpl_cabi::protocol::{Script, ScriptAnswer, ScriptResponse};
use cpl_cabi::scripted::ScriptHost;
use cpl_diag::{Diagnostic, SymbolPath};
use cpl_emit_c::cc::{Compiler, Profile, Sanitizer};
use cpl_emit_c::package::{ArtifactKind, BuildConfig, DriverConfig};
use cpl_eval::run::RunHosts;
use cpl_eval::{BindOptions, CapabilityImpl, EvalConfig, RunArg};
use cpl_hir::{ParamDef, Program, Ty};
use cpl_project::{load_project, LoadOptions, Project};
use cpl_sema::interop::InteropPlan;
use serde_json::Value as Json;
use std::collections::HashMap;
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

/// Whether a fixture vendors `imgtest` (and reports its counters at exit).
fn has_imgtest(name: &str) -> bool {
    name == "cboundary" || name == "cthumbs"
}

/// A fresh copy of a fixture with its vendored files provided (toolchain 12.7).
fn fixture(name: &str, dir: &Path) -> PathBuf {
    let root = dir.join(name);
    let _ = std::fs::remove_dir_all(&root);
    let src = if name == "reader" {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/reader")
    } else {
        tests_dir().join("projects").join(name)
    };
    copy_dir(&src, &root);
    if has_imgtest(name) {
        copy_dir(
            &tests_dir().join("clib/imgtest"),
            &root.join("vendor/imgtest"),
        );
    }
    if name == "cboundary" {
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

/// A checked fixture: what the evaluator's side of a comparison needs.
struct Checked {
    project: Project,
    checked: cpl_sema::CheckResult,
    plan: InteropPlan,
}

fn check(root: &Path) -> Checked {
    let project = load_project(root, &LoadOptions::default()).expect("the fixture loads");
    let checked = cpl_sema::check(&project);
    assert!(
        !checked.has_errors(),
        "{}: {:?}",
        root.display(),
        checked.diagnostics
    );
    let plan = checked.interop.clone().expect("a C plan");
    Checked {
        project,
        checked,
        plan,
    }
}

fn build(root: &Path, c: &Checked, sanitize: &[Sanitizer]) -> PathBuf {
    let out = root.join(".cpl/build/c");
    let cfg = BuildConfig {
        out_dir: out.clone(),
        cache_dir: Path::new(env!("CARGO_TARGET_TMPDIR")).join("c_hosts_cache"),
        profile: Profile::Optimized,
        sanitize: sanitize.to_vec(),
        artifacts: vec![ArtifactKind::Shared, ArtifactKind::Static],
        release: false,
        lock: false,
        cc: None,
        jobs: 4,
        runtime: cpl_rt::RuntimeSource::Embedded,
        werror: true,
    };
    if let Err(ds) = cpl_emit_c::package::build(&c.project, &c.checked.program, &c.plan, &cfg) {
        panic!("{} does not build: {ds:#?}", root.display());
    }
    out
}

fn driver_config() -> DriverConfig {
    DriverConfig {
        cache_dir: Path::new(env!("CARGO_TARGET_TMPDIR")).join("c_hosts_cache"),
        profile: Profile::Debug,
        sanitize: vec![],
        coverage: false,
        cc: None,
        runtime: cpl_rt::RuntimeSource::Embedded,
        werror: true,
    }
}

/// One `using` capability of a call line: a grant, or callbacks of the host with fixed answers.
enum Using {
    Grant,
    Answers(String, Json),
}

/// A type named in an answer (`"type"`, an ill-typed result on purpose): a builtin type or a declaration's
/// qualified name.
fn type_of_text(p: &Program, text: &str) -> Ty {
    match text {
        "Int" => Ty::Int,
        "Bool" => Ty::Bool,
        "Text" => Ty::Text,
        "Bytes" => Ty::Bytes,
        "Unit" => Ty::Adt(p.std_defs.unit, vec![]),
        _ => {
            let d = p
                .lookup(&SymbolPath::parse(text).unwrap())
                .unwrap_or_else(|| panic!("no type {text}"));
            match &p.def(d).kind {
                cpl_hir::DefKind::Wrapper(_) => Ty::Wrapper(d),
                _ => Ty::Adt(d, vec![]),
            }
        }
    }
}

/// The script of a host's answers (value JSON) for capability `cap`: each argument decoded with its parameter's type,
/// each result with the operation's return type or the answer's `type`.
fn script_of(p: &Program, cap: cpl_hir::DefId, answers: &Json) -> Script {
    let ops = &p.def(cap).kind.as_capability().expect("a capability").ops;
    let decode = |j: &Json, ty: &Ty| {
        let v = cpl_eval::value_from_json(p, j, ty)
            .unwrap_or_else(|e| panic!("an answer does not decode: {j}: {e:?}"));
        cpl_cabi::wire::to_nodes(&v)
    };
    Script {
        responses: answers
            .as_array()
            .expect("answers")
            .iter()
            .map(|a| {
                let op = a["op"].as_str().expect("an op");
                let od = ops
                    .iter()
                    .find(|o| o.name == op)
                    .unwrap_or_else(|| panic!("no operation {op}"));
                let args = a["args"]
                    .as_array()
                    .expect("args")
                    .iter()
                    .zip(&od.params)
                    .map(|(j, prm)| decode(j, &prm.ty))
                    .collect();
                let answer = match a.get("foreign_error") {
                    Some(f) => ScriptAnswer::ForeignError(
                        serde_json::from_value(f.clone()).expect("a foreign failure"),
                    ),
                    None => {
                        let ty = a
                            .get("type")
                            .and_then(Json::as_str)
                            .map(|t| type_of_text(p, t))
                            .unwrap_or_else(|| od.ret.clone());
                        ScriptAnswer::Result(decode(&a["result"], &ty))
                    }
                };
                ScriptResponse {
                    op: op.to_string(),
                    args,
                    answer,
                }
            })
            .collect(),
    }
}

/// The evaluator's hosts of one call line, in `using`-parameter order.
struct LineHosts<'a> {
    p: &'a Program,
    using: Vec<Using>,
    next: usize,
    manifest: cpl_cabi::adapter::ManifestHosts<'a>,
}

impl RunHosts for LineHosts<'_> {
    fn available(&self, _param: &ParamDef) -> bool {
        true
    }

    fn create(
        &mut self,
        param: &ParamDef,
    ) -> Result<(Box<dyn CapabilityImpl>, BindOptions), Vec<Diagnostic>> {
        let i = self.next;
        self.next += 1;
        match &self.using[i] {
            Using::Grant => self.manifest.create(param),
            Using::Answers(label, answers) => {
                let Ty::Capability(cap) = param.ty else {
                    panic!("a using parameter")
                };
                let script = script_of(self.p, cap, answers);
                Ok((
                    Box::new(ScriptHost::new(cap, label, script)),
                    BindOptions {
                        label: label.clone(),
                        validate_results: true,
                    },
                ))
            }
        }
    }
}

/// The evaluator's outcome of a call line, rendered as the line renders it: (key, JSON).
fn evaluate(c: &Checked, line: &Json) -> (String, Json) {
    let p: &Program = &c.checked.program;
    let sm = &c.project.source_map;
    let function = line["call"].as_str().unwrap();
    let path = SymbolPath::parse(function).unwrap();
    let args: Vec<RunArg> = line["args"]
        .as_array()
        .unwrap()
        .iter()
        .cloned()
        .map(RunArg::Json)
        .collect();
    // the runtime options and call options the host used (toolchain 12.7: one depth and one budget on both sides)
    let config = line.get("config");
    let cfg = EvalConfig {
        max_call_depth: config
            .and_then(|c| c.get("max_call_depth"))
            .and_then(Json::as_u64)
            .map(|d| d as u32)
            .unwrap_or(c.project.manifest.run.max_call_depth),
        time_budget: config
            .and_then(|c| c.get("time_budget_ms"))
            .and_then(Json::as_u64)
            .map(std::time::Duration::from_millis),
        ..EvalConfig::default()
    };
    let using: Vec<Using> = line
        .get("using")
        .and_then(Json::as_array)
        .map(|a| {
            a.iter()
                .map(|u| match (u.get("grant"), u.get("answers")) {
                    (Some(_), _) => Using::Grant,
                    (None, Some(a)) => {
                        Using::Answers(u["label"].as_str().unwrap_or("host").to_string(), a.clone())
                    }
                    _ => panic!("a capability is a grant or a host's answers: {u}"),
                })
                .collect()
        })
        .unwrap_or_default();
    let driver = driver_config();
    let outcome = if using.is_empty() {
        cpl_eval::run_function(p, &path, &args, &cfg)
    } else {
        let mut hosts = LineHosts {
            p,
            using,
            next: 0,
            manifest: cpl_cabi::adapter::manifest_hosts(p, &c.project, &c.plan, &driver),
        };
        cpl_eval::run_function_with_hosts(p, &path, &args, &cfg, &mut hosts)
    }
    .unwrap_or_else(|e| panic!("the evaluator refuses {line}: {e:?}"));
    let key = ["value", "ok", "err", "fault"]
        .into_iter()
        .find(|k| line.get(*k).is_some())
        .unwrap();
    match (&outcome.value, key) {
        (Err(f), _) => (
            "fault".into(),
            serde_json::to_value(f.to_diagnostic(p).to_json(sm)).unwrap(),
        ),
        (Ok(v), "value") => (
            "value".into(),
            cpl_eval::value_to_json(p, v, &outcome.return_ty),
        ),
        (Ok(v), _) => {
            // a split Result: its payload under its own type
            let Ty::Adt(_, targs) = &outcome.return_ty else {
                panic!("{function} does not return a Result")
            };
            let cpl_eval::Value::Enum {
                variant, payload, ..
            } = v
            else {
                panic!("{function}: no Result value")
            };
            let variant = *variant as usize;
            let k = if variant == 0 { "ok" } else { "err" };
            (
                k.into(),
                cpl_eval::value_to_json(p, &payload[0], &targs[variant]),
            )
        }
    }
}

/// Every call line of a run compared with the evaluator: the number of lines and the divergences.
fn compare_lines(
    fixtures: &HashMap<String, Checked>,
    first: &str,
    stdout: &str,
    cache: &mut HashMap<String, (String, Json)>,
) -> (usize, Vec<String>) {
    let mut out = Vec::new();
    let mut n = 0;
    for text in stdout
        .lines()
        .filter(|l| l.starts_with("{\"call\"") || l.starts_with("{\"package\""))
    {
        n += 1;
        let line: Json =
            serde_json::from_str(text).unwrap_or_else(|e| panic!("a malformed line ({e}): {text}"));
        if let Some(f) = line.get("protocol") {
            // a C callback's binding_protocol: the fields of semantics S80, which no scripted host mirrors
            let ok = f["code"] == "internal_error"
                && f["kind"] == "binding_protocol"
                && f["operation"].is_string()
                && f["binding"] == "host"
                && f["detail"].is_string();
            if !ok {
                out.push(format!("{text}\n  expected a binding_protocol fault with operation, binding, and detail"));
            }
            continue;
        }
        if let Some(err) = line.get("error") {
            // a CPL_E_* status: its fields (cabi.md 4.9)
            let expect = line["expect"].as_str().unwrap_or_default();
            if err["code"] != expect || err["category"] != "interop" || err["severity"] != "error" {
                out.push(format!("{text}\n  expected the interop error {expect}"));
            }
            continue;
        }
        let package = line.get("package").and_then(Json::as_str).unwrap_or(first);
        let c = &fixtures[package];
        let mut key_of = line.clone();
        for k in ["value", "ok", "err", "fault"] {
            key_of.as_object_mut().unwrap().remove(k);
        }
        let key = format!("{package} {key_of}");
        let (k, eval) = cache
            .entry(key)
            .or_insert_with(|| evaluate(c, &line))
            .clone();
        let native = line.get(k.as_str());
        // a guard fault (stack, with depth) or a time fault: the host suites run those cases under one depth and one
        // budget on both sides (12.7), so both sides must fault alike before rule 2 reduces them
        let class = |f: &Json| match (
            f.get("depth").is_some(),
            f.get("resource").and_then(Json::as_str),
        ) {
            (true, Some("stack")) => 1,
            (_, Some("time")) => 2,
            _ => 0,
        };
        let diverges = match native {
            None => true,
            Some(n) if k == "fault" => {
                class(&eval) != class(n) || cpl_cabi::compare::faults(&eval, n).is_some()
            }
            Some(n) => n != &eval,
        };
        if diverges {
            out.push(format!("{text}\n  evaluator: {{\"{k}\": {eval}}}"));
        }
    }
    (n, out)
}

fn sanitizers_available() -> bool {
    let ok = Compiler::discover(None, None)
        .is_ok_and(|c| c.supports(&[Sanitizer::Address, Sanitizer::Undefined]));
    if !ok {
        if std::env::var("CPL_REQUIRE_SANITIZERS").is_ok_and(|v| v == "1") {
            panic!("CPL_REQUIRE_SANITIZERS=1, but the compiler cannot link address,undefined");
        }
        println!("skipped: the compiler cannot link the sanitizers address,undefined (set CPL_REQUIRE_SANITIZERS=1 to fail)");
    }
    ok
}

fn run_host(host: &str) {
    let Some(cc) = cpl_rt::testing::find_cc() else {
        if std::env::var("CPL_REQUIRE_CC").is_ok_and(|v| v == "1") {
            panic!("CPL_REQUIRE_CC=1, but there is no C compiler");
        }
        println!("skipped: no C compiler (set CPL_REQUIRE_CC=1 to fail)");
        return;
    };
    let source = tests_dir().join("c").join(host);
    let text = std::fs::read_to_string(&source).unwrap();
    let first = text.lines().next().unwrap_or_default();
    let names: Vec<&str> = first
        .trim()
        .strip_prefix("/* fixture:")
        .and_then(|s| s.strip_suffix("*/"))
        .unwrap_or_else(|| panic!("{host}: the first line names its fixtures"))
        .split_whitespace()
        .collect();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("c_hosts")
        .join(host.trim_end_matches(".c"));
    let mut configs = vec![("optimized", vec![], vec!["-O2"])];
    if sanitizers_available() {
        configs.push((
            "sanitize",
            vec![Sanitizer::Address, Sanitizer::Undefined],
            vec![
                "-O1",
                "-g",
                "-fno-omit-frame-pointer",
                "-fsanitize=address,undefined",
                "-fno-sanitize-recover=all",
            ],
        ));
    }
    let mut cache = HashMap::new();
    let mut divergences = Vec::new();
    for (config, sanitize, cflags) in configs {
        let cdir = dir.join(config);
        let mut includes = vec![format!("-I{}", tests_dir().join("c").display())];
        let mut libs = Vec::new();
        let mut fixtures = HashMap::new();
        for f in &names {
            let root = fixture(f, &cdir);
            let c = check(&root);
            let out = build(&root, &c, &sanitize);
            includes.push(format!("-I{}", out.join("include").display()));
            if has_imgtest(f) {
                includes.push(format!("-I{}", root.join("vendor/imgtest").display()));
            }
            libs.push((
                out.join("lib"),
                cpl_emit_c::unit_prefix(&c.plan),
                root.clone(),
            ));
            fixtures.insert(c.project.manifest.c_prefix().to_string(), c);
        }
        let first_prefix = libs[0].1.clone();
        for link in ["shared", "static"] {
            let exe = cdir.join(format!("{}-{link}", host.trim_end_matches(".c")));
            let mut cmd = Command::new(&cc);
            cmd.args(["-std=c11", "-Wall", "-Wextra", "-Werror"])
                .args(&cflags)
                .args(&includes)
                .arg(&source);
            for (lib, prefix, root) in &libs {
                if link == "shared" {
                    cmd.arg(format!("-L{}", lib.display()))
                        .arg(format!("-l{prefix}"))
                        .arg(format!("-Wl,-rpath,{}", lib.display()));
                } else {
                    cmd.arg(lib.join(format!("lib{prefix}.a")));
                    if root.join("vendor/libextra").is_dir() {
                        cmd.arg(format!("-L{}", root.join("vendor/libextra").display()))
                            .arg("-lextra");
                    }
                }
            }
            cmd.arg("-lpthread").arg("-o").arg(&exe);
            let built = cmd.output().unwrap();
            assert!(
                built.status.success(),
                "{host} ({config}, {link}) does not compile:\n{}",
                String::from_utf8_lossy(&built.stderr)
            );
            let report = cdir.join(format!("{}-{link}.imgtest", host.trim_end_matches(".c")));
            let _ = std::fs::remove_file(&report);
            let run = Command::new(&exe)
                .env("ASAN_OPTIONS", "detect_leaks=1:halt_on_error=1")
                .env("UBSAN_OPTIONS", "halt_on_error=1:print_stacktrace=1")
                .env("IMGTEST_REPORT", &report)
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&run.stdout).to_string();
            println!("{host} ({config}, {link}):\n{stdout}");
            assert!(
                run.status.success() && run.stderr.is_empty(),
                "{host} ({config}, {link}) failed ({}):\n{stdout}{}",
                run.status,
                String::from_utf8_lossy(&run.stderr)
            );
            // imgtest's exact counters at exit (toolchain 12.7): no live allocation, no double free. imgtest reports
            // once it allocated, which a grant of its binding does; a host without one may leave no report.
            if names.iter().any(|f| has_imgtest(f)) {
                let counts = std::fs::read_to_string(&report).unwrap_or_default();
                assert!(
                    !counts.is_empty() || !stdout.contains("\"grant\""),
                    "{host} ({config}, {link}): imgtest wrote no report"
                );
                for l in counts.lines() {
                    assert_eq!(
                        l, "live 0 double_frees 0",
                        "{host} ({config}, {link}): imgtest at exit"
                    );
                }
            }
            let (n, ds) = compare_lines(&fixtures, &first_prefix, &stdout, &mut cache);
            println!("{host} ({config}, {link}): {n} call lines compared with the evaluator");
            assert!(n > 0, "{host} ({config}, {link}): no call line");
            for d in ds {
                divergences.push(format!("({config}, {link}) {d}"));
            }
        }
    }
    assert!(
        divergences.is_empty(),
        "{host}: {} lines differ from the evaluator:\n{}",
        divergences.len(),
        divergences.join("\n")
    );
}

#[test]
fn host_reader() {
    run_host("host_reader.c");
}

#[test]
fn host_imports() {
    run_host("host_imports.c");
}

#[test]
fn host_declarative() {
    run_host("host_declarative.c");
}

#[test]
fn host_basic() {
    run_host("host_basic.c");
}

#[test]
fn host_version() {
    run_host("host_version.c");
}

/// The compile-time half of versioning (toolchain 12.9 item 6): a generated header refuses a `cpl.h` of another API
/// version with `#error` (a compile-must-fail check), and every generated header passes `g++ -std=c++17
/// -fsyntax-only`.
#[test]
fn headers_refuse_another_api_and_parse_as_cpp() {
    let Some(cc) = cpl_rt::testing::find_cc() else {
        if std::env::var("CPL_REQUIRE_CC").is_ok_and(|v| v == "1") {
            panic!("CPL_REQUIRE_CC=1, but there is no C compiler");
        }
        println!("skipped: no C compiler (set CPL_REQUIRE_CC=1 to fail)");
        return;
    };
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("c_hosts")
        .join("headers");
    let root = fixture("cboundary", &dir);
    let c = check(&root);
    let include = build(&root, &c, &[]).join("include");
    // a cpl.h of API 1.x beside the package header: the package header's guard stops the compile
    for (name, from, to) in [
        (
            "major",
            "#define CPL_API_VERSION_MAJOR 0u",
            "#define CPL_API_VERSION_MAJOR 1u",
        ),
        (
            "minor",
            "#define CPL_API_VERSION_MINOR 1u",
            "#define CPL_API_VERSION_MINOR 0u",
        ),
    ] {
        let bad = dir.join(format!("bad-{name}"));
        std::fs::create_dir_all(&bad).unwrap();
        let cpl_h = std::fs::read_to_string(include.join("cpl.h")).unwrap();
        assert!(cpl_h.contains(from), "cpl.h defines {from}");
        std::fs::write(bad.join("cpl.h"), cpl_h.replace(from, to)).unwrap();
        std::fs::copy(include.join("cboundary.h"), bad.join("cboundary.h")).unwrap();
        let src = bad.join("use.c");
        std::fs::write(
            &src,
            "#include \"cboundary.h\"\nint main(void) { return 0; }\n",
        )
        .unwrap();
        let o = Command::new(&cc)
            .args(["-std=c11", "-fsyntax-only"])
            .arg(format!("-I{}", bad.display()))
            .arg(&src)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&o.stderr);
        assert!(!o.status.success(), "{name}: a mismatched cpl.h compiles");
        assert!(
            stderr.contains("cboundary.h was generated for CPL C API 0.1"),
            "{name}: {stderr}"
        );
    }
    // C++17
    let cxx = ["g++", "c++", "clang++"].into_iter().find(|c| {
        Command::new(c)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    });
    let Some(cxx) = cxx else {
        println!("skipped: no C++ compiler for the -std=c++17 check");
        return;
    };
    for h in [
        "cpl.h",
        "cpl_adapter.h",
        "cboundary.h",
        "cboundary_adapter_image_codec.h",
    ] {
        let o = Command::new(cxx)
            .args([
                "-std=c++17",
                "-fsyntax-only",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-x",
                "c++",
            ])
            .arg(format!("-I{}", include.display()))
            .arg(include.join(h))
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "{h} is no C++17: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
}

#[test]
fn host_ownership() {
    run_host("host_ownership.c");
}

#[test]
fn host_caps() {
    run_host("host_caps.c");
}

#[test]
fn host_failures() {
    run_host("host_failures.c");
}

#[test]
fn host_threads() {
    run_host("host_threads.c");
}

#[test]
fn host_two_packages() {
    run_host("host_two_packages.c");
}

/// Two static copies of one package (toolchain 12.7, cabi.md 1.6): built into two output directories (optimized and
/// debug, so that the copies tell themselves apart), linked with `-Wl,--whole-archive` they fail with a
/// multiple-definition error, and linked plainly the host holds one copy, the first archive's.
#[test]
fn two_static_copies_of_one_package() {
    let Some(cc) = cpl_rt::testing::find_cc() else {
        if std::env::var("CPL_REQUIRE_CC").is_ok_and(|v| v == "1") {
            panic!("CPL_REQUIRE_CC=1, but there is no C compiler");
        }
        println!("skipped: no C compiler (set CPL_REQUIRE_CC=1 to fail)");
        return;
    };
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("c_hosts")
        .join("two_static_copies");
    let root = fixture("cboundary", &dir);
    let c = check(&root);
    let mut archives = Vec::new();
    for (name, profile) in [("first", Profile::Optimized), ("second", Profile::Debug)] {
        let out = dir.join(format!("out-{name}"));
        let cfg = BuildConfig {
            out_dir: out.clone(),
            cache_dir: Path::new(env!("CARGO_TARGET_TMPDIR")).join("c_hosts_cache"),
            profile,
            sanitize: vec![],
            artifacts: vec![ArtifactKind::Static],
            release: false,
            lock: false,
            cc: None,
            jobs: 4,
            runtime: cpl_rt::RuntimeSource::Embedded,
            werror: true,
        };
        if let Err(ds) = cpl_emit_c::package::build(&c.project, &c.checked.program, &c.plan, &cfg) {
            panic!("cboundary does not build into {name}: {ds:#?}");
        }
        archives.push(out.join("lib/libcboundary.a"));
    }
    let src = dir.join("one_copy.c");
    std::fs::write(
        &src,
        "#include <stdio.h>\n#include \"cboundary.h\"\nint main(void) { printf(\"%s\\n\", cboundary_library_info()->profile); return 0; }\n",
    )
    .unwrap();
    let include = format!("-I{}", dir.join("out-first/include").display());
    let extra = format!("-L{}", root.join("vendor/libextra").display());
    let whole = Command::new(&cc)
        .args(["-std=c11", &include])
        .arg(&src)
        .arg("-Wl,--whole-archive")
        .args(&archives)
        .arg("-Wl,--no-whole-archive")
        .args([extra.as_str(), "-lextra", "-lpthread", "-o"])
        .arg(dir.join("whole"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&whole.stderr);
    assert!(
        !whole.status.success(),
        "two whole archives of one package link"
    );
    assert!(stderr.contains("multiple definition"), "{stderr}");
    let plain = Command::new(&cc)
        .args(["-std=c11", &include])
        .arg(&src)
        .args(&archives)
        .args([extra.as_str(), "-lextra", "-lpthread", "-o"])
        .arg(dir.join("plain"))
        .output()
        .unwrap();
    assert!(
        plain.status.success(),
        "{}",
        String::from_utf8_lossy(&plain.stderr)
    );
    let run = Command::new(dir.join("plain")).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "optimized\n",
        "the first archive's copy"
    );
}

#[test]
fn host_oom() {
    run_host("host_oom.c");
}
