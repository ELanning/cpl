//! The differential harness of the host backends (`spec/hosts.md` 6), shared by the tests and
//! the `host_diff` example.
//!
//! A case is a CPL program compiled to a target with its JSON driver, built with the target's
//! toolchain, and run on arguments beside the reference evaluator. Values must be equal as
//! value JSON; faults must have the same code, clause kind, and blamed symbol; an argument the
//! evaluator refuses at the boundary must be refused. Resource exhaustion is not compared, and
//! only calls the evaluator finishes within its step budget reach the driver.

#![allow(dead_code)]

use cpl_diag::FileOrigin;
use cpl_emit_host::{emit, Options, Target};
use cpl_eval::testing::programs::{inputs, Rng};
use cpl_eval::{ClauseKind, EvalConfig, FaultKind, RunArg, RunError};
use cpl_hir::{DefId, Program};
use cpl_project::{load_project_from_tree, LoadOptions, VirtualTree};
use serde_json::{json, Value as Json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Check one source file as a project of its own.
pub fn check(source: &str) -> Result<Program, Vec<String>> {
    let module = source
        .lines()
        .find_map(|l| l.trim().strip_prefix("module "))
        .unwrap_or("m")
        .trim()
        .to_string();
    let tree = VirtualTree {
        files: [
            (
                "cpl.toml".to_string(),
                b"[project]\nname = \"m\"\nedition = \"0.1\"\n".to_vec(),
            ),
            (
                format!("src/{}.cpl", module.replace('.', "/")),
                source.as_bytes().to_vec(),
            ),
        ]
        .into_iter()
        .collect(),
    };
    let project = load_project_from_tree(&tree, FileOrigin::Virtual, &LoadOptions::default())
        .map_err(|e| vec![format!("{e:?}")])?;
    let checked = cpl_sema::check(&project);
    let errors: Vec<String> = checked
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .map(|d| format!("{}: {}", d.code.0, d.message))
        .collect();
    if errors.is_empty() {
        Ok(checked.program)
    } else {
        Err(errors)
    }
}

/// The evaluator's outcome in the driver's answer shape; `None` when it is not compared
/// (resource exhaustion, the step budget, an internal error).
pub fn evaluate(p: &Program, f: DefId, args: &[Json], steps: u64) -> Option<Json> {
    let config = EvalConfig {
        max_steps: Some(steps),
        ..EvalConfig::default()
    };
    let call = cpl_eval::run_function(
        p,
        &p.symbol_path(f),
        &args.iter().cloned().map(RunArg::Json).collect::<Vec<_>>(),
        &config,
    );
    let outcome = match call {
        Ok(o) => o,
        Err(RunError::Arg { .. }) => return Some(json!({"outside": "decode"})),
        Err(e) => panic!("not runnable: {e}"),
    };
    match outcome.value {
        Ok(v) => Some(json!({"value": cpl_eval::value_to_json(p, &v, &outcome.return_ty)})),
        Err(fault) => match fault.kind {
            FaultKind::RefinementViolation => Some(json!({"outside": "refinement"})),
            FaultKind::ResourceExhaustion(_) | FaultKind::Internal => None,
            kind => {
                let code = fault.to_diagnostic(p).code.as_str().to_string();
                let symbol = fault
                    .symbol
                    .as_ref()
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_default();
                let mut f = json!({"code": code, "symbol": symbol});
                if let FaultKind::ContractViolation(k) = kind {
                    f["clause_kind"] = json!(match k {
                        ClauseKind::Requires => "requires",
                        ClauseKind::Ensures => "ensures",
                        ClauseKind::Invariant => "invariant",
                        ClauseKind::Decreases => "decreases",
                    });
                }
                Some(json!({ "fault": f }))
            }
        },
    }
}

/// Whether the target's toolchain is on this machine (`JAVAC` and `JAVA` name the JDK's).
pub fn available(target: Target) -> bool {
    let probe = |cmd: &str, arg: &str| {
        Command::new(cmd)
            .arg(arg)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    match target {
        Target::Rust => probe("cargo", "--version"),
        Target::Go => probe("go", "version"),
        Target::Java => probe(&javac(), "-version") && probe(&java(), "-version"),
    }
}

pub fn javac() -> String {
    std::env::var("JAVAC").unwrap_or_else(|_| "javac".into())
}

pub fn java() -> String {
    std::env::var("JAVA").unwrap_or_else(|_| "java".into())
}

/// Every `.java` file under `dir`.
pub fn java_sources(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let path = e.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|x| x == "java") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Every `.cpl` file under `dir`, sorted.
pub fn cpl_sources(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let path = e.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|x| x == "cpl") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// The command that runs a built driver.
pub struct Built {
    pub driver: PathBuf,
}

/// Compile `p` to `target` with its driver under `dir` and build it (`shared` is the Cargo
/// target directory the Rust cases share).
pub fn build(
    target: Target,
    dir: &Path,
    shared: &Path,
    name: &str,
    p: &Program,
) -> Result<Built, String> {
    let package = emit(
        p,
        target,
        &Options {
            name: name.to_string(),
            driver: true,
        },
    )?;
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    package.write(dir).map_err(|e| e.to_string())?;
    let failed = |out: std::process::Output| {
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr)
    };
    match target {
        Target::Rust => {
            let out = Command::new("cargo")
                .args([
                    "build",
                    "--release",
                    "--quiet",
                    "--offline",
                    "--manifest-path",
                ])
                .arg(dir.join("Cargo.toml"))
                .env("CARGO_TARGET_DIR", shared)
                .env("RUSTFLAGS", "-Awarnings")
                .output()
                .map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err(failed(out));
            }
            let exe = if cfg!(windows) {
                "cpl-driver.exe"
            } else {
                "cpl-driver"
            };
            Ok(Built {
                driver: shared.join("release").join(exe),
            })
        }
        Target::Go => {
            let bin = dir.join(if cfg!(windows) {
                "cpl-driver.exe"
            } else {
                "cpl-driver"
            });
            // `go vet` holds the generated code to what Go code is held to: nothing after a
            // terminating statement (the analyzers on what the CPL source says, `x && x`, are off)
            let out = Command::new("go")
                .args(["vet", "-unreachable", "./..."])
                .current_dir(dir)
                .output()
                .map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err(failed(out));
            }
            let out = Command::new("go")
                .args(["build", "-o"])
                .arg(&bin)
                .arg("./cmd/cpl-driver")
                .current_dir(dir)
                .output()
                .map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err(failed(out));
            }
            Ok(Built { driver: bin })
        }
        Target::Java => {
            let classes = dir.join("classes");
            let out = Command::new(javac())
                .args(["-nowarn", "-encoding", "UTF-8", "-d"])
                .arg(&classes)
                .args(java_sources(&dir.join("src")))
                .output()
                .map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err(failed(out));
            }
            let script = dir.join("cpl-driver.sh");
            std::fs::write(
                &script,
                format!(
                    "#!/bin/sh\nexec {} -Xss64m -cp {} {name}.Driver\n",
                    java(),
                    classes.display()
                ),
            )
            .map_err(|e| e.to_string())?;
            let _ = Command::new("chmod").arg("+x").arg(&script).status();
            Ok(Built { driver: script })
        }
    }
}

/// The outcome of one case.
pub struct Case {
    pub compared: usize,
    /// The first disagreement, described with the program.
    pub divergence: Option<String>,
}

/// The arguments of a case: random inputs of the generator's shapes, or (`swept`) the input
/// sweeps of the differential suites, which reach every type.
fn requests(p: &Program, seed: u64, swept: bool) -> Vec<(DefId, Vec<Json>)> {
    // HOST_DIFF_REQUESTS=path: the requests of a file instead, one per line, in the driver's
    // form `{"function": "<qualified name>", "args": [...]}` (hand-picked edge cases).
    if let Ok(path) = std::env::var("HOST_DIFF_REQUESTS") {
        let text = std::fs::read_to_string(&path).expect("HOST_DIFF_REQUESTS");
        return parse_requests(p, &text);
    }
    let functions: Vec<DefId> = p
        .defs()
        .filter(|d| p.module(d.module).path.first().is_some_and(|s| s != "std"))
        .filter(|d| {
            d.kind.as_fn().is_some_and(|f| {
                matches!(f.body, cpl_hir::FnBody::Block(_))
                    && f.generics.is_empty()
                    && f.using.is_empty()
            })
        })
        .map(|d| d.id)
        .collect();
    let mut rng = Rng::new(seed ^ 0x5EED);
    let mut out = Vec::new();
    for f in &functions {
        if swept {
            let fd = p.def(*f).kind.as_fn().expect("a function");
            let mut key = [0u8; 32];
            key[..8].copy_from_slice(&seed.to_le_bytes());
            for (i, b) in p.display_path(*f).bytes().enumerate() {
                key[8 + i % 24] ^= b;
            }
            let mut sweep = cpl_eval::testing::sweep::Sweep::new(key);
            'cases: for size in 0..30u32 {
                let mut args = Vec::new();
                for q in &fd.params {
                    match sweep.draw(p, &q.ty, size.min(12)) {
                        Ok(v) => args.push(cpl_eval::value_to_json(p, &v, &q.ty)),
                        Err(_) => break 'cases,
                    }
                }
                out.push((*f, args));
            }
            continue;
        }
        for k in 0..30 {
            out.push((*f, inputs(p, *f, &mut rng, k % 4 == 0)));
        }
    }
    out
}

/// Run one case: `Err` is a build failure, with the toolchain's output.
/// Requests in the driver's form, one JSON object per line.
fn parse_requests(p: &Program, text: &str) -> Vec<(DefId, Vec<Json>)> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let r: Json = serde_json::from_str(l).expect("a request");
            let name = r["function"].as_str().expect("a function");
            let f = p
                .defs()
                .find(|d| p.display_path(d.id) == name)
                .unwrap_or_else(|| panic!("no function {name}"))
                .id;
            (f, r["args"].as_array().cloned().unwrap_or_default())
        })
        .collect()
}

/// Run one case on the requests of `text` (the driver's form, one per line).
pub fn run_requests(
    target: Target,
    root: &Path,
    seed: u64,
    source: &str,
    text: &str,
) -> Result<Case, String> {
    run_with(target, root, seed, source, true, Some(text))
}

pub fn run_case(
    target: Target,
    root: &Path,
    seed: u64,
    source: &str,
    swept: bool,
) -> Result<Case, String> {
    run_with(target, root, seed, source, swept, None)
}

fn run_with(
    target: Target,
    root: &Path,
    seed: u64,
    source: &str,
    swept: bool,
    explicit: Option<&str>,
) -> Result<Case, String> {
    let p = match check(source) {
        Ok(p) => p,
        Err(_) => {
            return Ok(Case {
                compared: 0,
                divergence: None,
            })
        }
    };
    let name = format!("p{seed}");
    let built = build(target, &root.join(&name), &root.join("target"), &name, &p)?;
    // Steps are activations and loop iterations; swept inputs reach loops whose memory is
    // quadratic in their iterations, so they get a smaller budget. HOST_DIFF_STEPS overrides.
    let steps = std::env::var("HOST_DIFF_STEPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(if swept { 5_000 } else { 200_000 });
    let mut asked = Vec::new();
    let mut expected = Vec::new();
    let wanted = match explicit {
        Some(text) => parse_requests(&p, text),
        None => requests(&p, seed, swept),
    };
    for (f, args) in wanted {
        if let Some(e) = evaluate(&p, f, &args, steps) {
            asked.push((f, args));
            expected.push(e);
        }
    }
    let mut child = Command::new(&built.driver)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("the driver does not start: {e}"))?;
    let lines: Vec<String> = asked
        .iter()
        .map(|(f, args)| json!({"function": p.display_path(*f), "args": args}).to_string())
        .collect();
    let mut stdin = child.stdin.take().expect("a pipe");
    let writer = std::thread::spawn(move || {
        for l in lines {
            if writeln!(stdin, "{l}").is_err() {
                break;
            }
        }
    });
    let answers: Vec<Json> = BufReader::new(child.stdout.take().expect("a pipe"))
        .lines()
        .map(|l| {
            serde_json::from_str(&l.unwrap_or_default()).unwrap_or(json!({"unreadable": true}))
        })
        .collect();
    let _ = writer.join();
    let _ = child.wait();
    let mut compared = 0;
    for (i, (f, args)) in asked.iter().enumerate() {
        let Some(actual) = answers.get(i) else {
            return Ok(Case {
                compared,
                divergence: Some(format!(
                    "no answer for {} {}\n--- program\n{source}",
                    p.display_path(*f),
                    json!(args)
                )),
            });
        };
        compared += 1;
        if *actual != expected[i] {
            return Ok(Case {
                compared,
                divergence: Some(format!(
                    "{} {}\n  evaluator: {}\n  {}: {actual}\n--- program\n{source}",
                    p.display_path(*f),
                    json!(args),
                    expected[i],
                    target.name()
                )),
            });
        }
    }
    Ok(Case {
        compared,
        divergence: None,
    })
}
