//! Robustness fuzzing of the front end, the verifier, and the evaluator on mutated sources:
//! `cargo run --release -p cpl-cli --example robust_fuzz [cases] [first-seed]`.
//!
//! Every `.cpl` file of the repository (and the random programs of
//! `cpl_eval::testing::programs`) is mutated a few tokens or lines at a time, then:
//!
//! * parsing, formatting, checking, verifying, and evaluating never panic;
//! * a parsed file formats to text that parses, and formatting that text changes nothing;
//! * a file that checks formats to a program that checks and behaves the same on drawn inputs;
//! * evaluation never raises `internal_error`;
//! * a file that checks compiles to the source of every host (`cpl_emit_host::emit`).
//!
//! A violation is printed with the mutated source and its seed; the process exits 1 when there
//! was one.

use cpl_diag::{FileId, FileOrigin};
use cpl_eval::testing::programs::{generate, Rng};
use cpl_eval::testing::sweep::Sweep;
use cpl_eval::{run_function, value_to_json, EvalConfig, FaultKind, RunArg};
use cpl_project::{load_project_from_tree, LoadOptions, VirtualTree};
use cpl_syntax::formatter::{format_source, FileKind};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

fn corpus() -> Vec<(PathBuf, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut out = Vec::new();
    let mut stack = vec![root.join("crates"), root.join("examples")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            if path.is_dir() {
                if !path.ends_with("target") {
                    stack.push(path);
                }
            } else if path.extension().is_some_and(|x| x == "cpl") {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    out.push((path, text));
                }
            }
        }
    }
    out.sort();
    out
}

#[derive(Clone, Copy, PartialEq)]
enum Class {
    Ident,
    Number,
    Text,
    Op,
    Space,
}

fn tokens(src: &str) -> Vec<(Class, String)> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let start = i;
        let class = if c.is_alphabetic() || c == '_' {
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            Class::Ident
        } else if c.is_ascii_digit() {
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            Class::Number
        } else if c == '"' {
            i += 1;
            while i < chars.len() && chars[i] != '"' && chars[i] != '\n' {
                if chars[i] == '\\' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(chars.len());
            Class::Text
        } else if c.is_whitespace() {
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            Class::Space
        } else {
            i += 1;
            // two-character operators
            if i < chars.len() {
                let two: String = [c, chars[i]].iter().collect();
                if ["==", "!=", "<=", ">=", "=>", "->", "++", "..", "::"].contains(&two.as_str()) {
                    i += 1;
                }
            }
            Class::Op
        };
        out.push((class, chars[start..i].iter().collect()));
    }
    out
}

const VOCABULARY: &[&str] = &[
    "fn",
    "let",
    "var",
    "if",
    "else",
    "match",
    "while",
    "return",
    "requires",
    "ensures",
    "decreases",
    "invariant",
    "forall",
    "in",
    "and",
    "or",
    "not",
    "true",
    "false",
    "struct",
    "enum",
    "type",
    "where",
    "self",
    "result",
    "assert",
    "concurrent",
    "parallel",
    "for",
    "using",
    "effects",
    "check<Positive>",
    "Some",
    "None",
    "Ok",
    "Err",
    "[]",
    "{",
    "}",
    "(",
    ")",
    "[",
    "]",
    ",",
    ":",
    "=",
    "==",
    "+",
    "-",
    "*",
    "/",
    "%",
    "<",
    ">",
    "<=",
    "?",
    ".",
    "\n",
    "0",
    "-1",
    "99999999999999999999999",
    "\"t\"",
    "_",
    "Int",
    "Bool",
    "Text",
    "List<Int>",
    "Option<Int>",
    "?hole",
];

fn mutate(rng: &mut Rng, src: &str) -> String {
    let mut toks = tokens(src);
    let edits = 1 + rng.below(3);
    for _ in 0..edits {
        if toks.is_empty() {
            break;
        }
        let pick = |rng: &mut Rng, toks: &[(Class, String)], class: Class| -> Option<usize> {
            let idx: Vec<usize> = (0..toks.len()).filter(|&i| toks[i].0 == class).collect();
            (!idx.is_empty()).then(|| idx[rng.below(idx.len() as u64) as usize])
        };
        match rng.below(12) {
            0..=2 => {
                // an identifier replaced by another of the file
                if let (Some(a), Some(b)) = (
                    pick(rng, &toks, Class::Ident),
                    pick(rng, &toks, Class::Ident),
                ) {
                    toks[a].1 = toks[b].1.clone();
                }
            }
            3 => {
                if let Some(a) = pick(rng, &toks, Class::Number) {
                    toks[a].1 = [
                        "0",
                        "1",
                        "2",
                        "7",
                        "100",
                        "4294967296",
                        "18446744073709551616",
                    ][rng.below(7) as usize]
                        .to_string();
                }
            }
            4 | 5 => {
                if let Some(a) = pick(rng, &toks, Class::Op) {
                    let groups: &[&[&str]] = &[
                        &["+", "-", "*", "/", "%"],
                        &["<", ">", "<=", ">=", "==", "!="],
                        &["(", ")", "[", "]", "{", "}"],
                        &[",", ":", ".", "=", "?"],
                    ];
                    if let Some(g) = groups.iter().find(|g| g.contains(&toks[a].1.as_str())) {
                        toks[a].1 = g[rng.below(g.len() as u64) as usize].to_string();
                    }
                }
            }
            6 => {
                // delete a token
                let a = rng.below(toks.len() as u64) as usize;
                toks.remove(a);
            }
            7 => {
                let a = rng.below(toks.len() as u64 + 1) as usize;
                let v = VOCABULARY[rng.below(VOCABULARY.len() as u64) as usize];
                toks.insert(a, (Class::Op, format!(" {v} ")));
            }
            8..=10 => {
                // line edits
                let text: String = toks.iter().map(|t| t.1.as_str()).collect();
                let mut lines: Vec<&str> = text.lines().collect();
                if lines.len() > 2 {
                    let a = 1 + rng.below(lines.len() as u64 - 1) as usize;
                    let b = 1 + rng.below(lines.len() as u64 - 1) as usize;
                    match rng.below(3) {
                        0 => {
                            lines.remove(a);
                        }
                        1 => {
                            let l = lines[a];
                            lines.insert(b, l);
                        }
                        _ => lines.swap(a, b),
                    }
                }
                let joined = lines.join("\n") + "\n";
                toks = tokens(&joined);
            }
            _ => {
                let a = rng.below(toks.len() as u64) as usize;
                toks.truncate(a.max(1));
            }
        }
    }
    toks.iter().map(|t| t.1.as_str()).collect()
}

fn module_path(src: &str) -> String {
    src.lines()
        .find_map(|l| l.trim().strip_prefix("module "))
        .map(|m| m.trim().to_string())
        .filter(|m| {
            !m.is_empty()
                && m.split('.').all(|part| {
                    !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                })
        })
        .unwrap_or_else(|| "m".to_string())
}

fn project(src: &str) -> Option<cpl_project::Project> {
    let module = module_path(src);
    let name = module.split('.').next().unwrap_or("m").to_string();
    let file = format!("src/{}.cpl", module.replace('.', "/"));
    let tree = VirtualTree {
        files: [
            (
                "cpl.toml".to_string(),
                format!("[project]\nname = \"{name}\"\nedition = \"0.1\"\n").into_bytes(),
            ),
            (file, src.as_bytes().to_vec()),
        ]
        .into_iter()
        .collect(),
    };
    load_project_from_tree(&tree, FileOrigin::Virtual, &LoadOptions::default()).ok()
}

/// The outcomes of running every runnable function of the project's own modules on drawn inputs:
/// (function, rendering) pairs. `Err` on an internal error.
/// Compile a host package with its toolchain (`JAVAC` names `javac`).
fn host_build(
    target: cpl_emit_host::Target,
    package: &cpl_emit_host::Package,
) -> Result<(), String> {
    use std::process::Command;
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/robust-host");
    let dir = root.join(target.name());
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    package.write(&dir).map_err(|e| e.to_string())?;
    let out = match target {
        cpl_emit_host::Target::Rust => Command::new("cargo")
            .args(["check", "--quiet", "--offline", "--manifest-path"])
            .arg(dir.join("Cargo.toml"))
            .env("CARGO_TARGET_DIR", root.join("rust-target"))
            .output(),
        // compile errors, and code after a terminating statement (the generator's; other
        // analyzers would report what the CPL source itself says, `x && x`)
        cpl_emit_host::Target::Go => Command::new("go")
            .args(["vet", "-unreachable", "./..."])
            .current_dir(&dir)
            .output(),
        cpl_emit_host::Target::Java => {
            let mut sources = Vec::new();
            let mut stack = vec![dir.join("src")];
            while let Some(d) = stack.pop() {
                for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                    let path = e.path();
                    if path.is_dir() {
                        stack.push(path);
                    } else {
                        sources.push(path);
                    }
                }
            }
            Command::new(std::env::var("JAVAC").unwrap_or_else(|_| "javac".into()))
                .args(["-nowarn", "-d"])
                .arg(dir.join("classes"))
                .args(sources)
                .output()
        }
    }
    .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stdout).into_owned()
            + &String::from_utf8_lossy(&out.stderr))
    }
}

fn behavior(p: &cpl_hir::Program, seed: u64) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    let cfg = EvalConfig {
        max_steps: Some(20_000),
        time_budget: Some(std::time::Duration::from_millis(200)),
        ..EvalConfig::default()
    };
    for d in p.defs() {
        if p.module(d.module).path.first().is_some_and(|m| m == "std") {
            continue;
        }
        let Some(f) = d.kind.as_fn() else { continue };
        if !matches!(f.body, cpl_hir::FnBody::Block(_))
            || !f.generics.is_empty()
            || !f.using.is_empty()
            || d.status == cpl_hir::DefStatus::HasErrors
        {
            continue;
        }
        let mut key = [0u8; 32];
        key[..8].copy_from_slice(&seed.to_le_bytes());
        let name = p.display_path(d.id);
        for (i, b) in name.bytes().enumerate() {
            key[8 + i % 24] ^= b;
        }
        let mut sweep = Sweep::new(key);
        for size in [0u32, 1, 3, 10] {
            let mut args = Vec::new();
            for param in &f.params {
                match sweep.draw(p, &param.ty, size) {
                    Ok(v) => args.push(RunArg::Json(value_to_json(p, &v, &param.ty))),
                    Err(_) => break,
                }
            }
            if args.len() != f.params.len() {
                break;
            }
            let Ok(outcome) = run_function(p, &p.symbol_path(d.id), &args, &cfg) else {
                break;
            };
            let rendered = match &outcome.value {
                Ok(v) => value_to_json(p, v, &outcome.return_ty).to_string(),
                Err(fault) => {
                    if fault.kind == FaultKind::Internal {
                        return Err(format!("{name}: {} {:?}", fault.message, fault.data));
                    }
                    if matches!(fault.kind, FaultKind::ResourceExhaustion(_)) {
                        "exhausted".to_string()
                    } else {
                        format!("fault {:?}", fault.kind)
                    }
                }
            };
            out.push((name.clone(), rendered));
        }
    }
    Ok(out)
}

fn main() {
    // Like `cpl` itself, everything runs on the evaluation thread's large stack.
    cpl_eval::on_eval_stack(fuzz);
}

fn fuzz() {
    let mut args = std::env::args().skip(1);
    let cases: u64 = args.next().and_then(|n| n.parse().ok()).unwrap_or(500);
    let first: u64 = args.next().and_then(|n| n.parse().ok()).unwrap_or(1);
    let files = corpus();
    let quiet = std::env::var("FUZZ_QUIET").is_ok();
    std::panic::set_hook(Box::new(|_| {}));
    let (mut violations, mut parsed, mut checked, mut outcomes) = (0usize, 0usize, 0usize, 0usize);
    let start = std::time::Instant::now();
    let trace = std::env::var("FUZZ_TRACE").is_ok();
    for seed in first..first + cases {
        if trace {
            eprintln!("seed {seed}");
        }
        let mut rng = Rng::new(seed);
        let original = if rng.chance(25) {
            generate(seed, &[]).source
        } else {
            files[rng.below(files.len() as u64) as usize].1.clone()
        };
        let src = match std::env::var("FUZZ_FILE") {
            Ok(path) => std::fs::read_to_string(path).expect("FUZZ_FILE"),
            Err(_) => mutate(&mut rng, &original),
        };
        if std::env::var("FUZZ_PRINT").is_ok() {
            eprintln!("{src}");
        }
        let stage = |what: &str| {
            if trace {
                eprintln!("  {what}");
            }
        };
        stage("parse");
        let mut fail = |what: String, src: &str| {
            violations += 1;
            if !quiet {
                println!("=== VIOLATION (seed {seed}): {what}\n{src}\n");
            } else {
                println!("=== VIOLATION (seed {seed}): {what}");
            }
        };
        // parse and format
        let file = FileId(0);
        let formatted = match catch_unwind(|| format_source(&src, FileKind::Cpl, file)) {
            Err(_) => {
                fail("the parser or formatter panicked".into(), &src);
                continue;
            }
            Ok(Err(_)) => None,
            Ok(Ok(text)) => Some(text),
        };
        if let Some(text) = &formatted {
            parsed += 1;
            match catch_unwind(|| format_source(text, FileKind::Cpl, file)) {
                Err(_) => fail("formatting the formatted text panicked".into(), &src),
                Ok(Err(d)) => fail(
                    format!(
                        "the formatted text does not parse: {:?}\n--- formatted\n{text}",
                        d.iter().map(|d| d.message.clone()).collect::<Vec<_>>()
                    ),
                    &src,
                ),
                Ok(Ok(again)) if again != *text => fail(
                    format!("formatting is not idempotent\n--- once\n{text}\n--- twice\n{again}"),
                    &src,
                ),
                Ok(Ok(_)) => {}
            }
        }
        // check, verify, run
        stage("check");
        let Some(proj) = project(&src) else { continue };
        let result = catch_unwind(AssertUnwindSafe(|| cpl_sema::check(&proj)));
        let Ok(c) = result else {
            fail("the checker panicked".into(), &src);
            continue;
        };
        let p = &c.program;
        stage("verify");
        for d in p.defs() {
            if p.module(d.module).path.first().is_some_and(|m| m == "std") {
                continue;
            }
            if d.kind.as_fn().is_some()
                && catch_unwind(AssertUnwindSafe(|| cpl_prove::verify(p, d.id))).is_err()
            {
                fail(
                    format!("the verifier panicked on {}", p.display_path(d.id)),
                    &src,
                );
            }
        }
        stage("run");
        if c.has_errors() {
            // like `cpl run`, nothing runs in a program with errors
            continue;
        }
        stage("emit");
        // A program that checks compiles to every host's source (spec/hosts.md).
        for target in [
            cpl_emit_host::Target::Rust,
            cpl_emit_host::Target::Go,
            cpl_emit_host::Target::Java,
        ] {
            let options = cpl_emit_host::Options {
                name: "fuzz".into(),
                driver: true,
            };
            match catch_unwind(AssertUnwindSafe(|| {
                cpl_emit_host::emit(p, target, &options)
            })) {
                Err(_) => fail(format!("emitting {} panicked", target.name()), &src),
                Ok(Err(e)) => fail(format!("emitting {} failed: {e}", target.name()), &src),
                Ok(Ok(package)) => {
                    // FUZZ_HOST_BUILD=rust|go|java (every FUZZ_HOST_EVERY-th program): the
                    // host's own compiler accepts the package.
                    let every: u64 = std::env::var("FUZZ_HOST_EVERY")
                        .ok()
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(1);
                    if std::env::var("FUZZ_HOST_BUILD").ok().as_deref() == Some(target.name())
                        && seed % every == 0
                    {
                        if let Err(e) = host_build(target, &package) {
                            fail(
                                format!("the {} compiler refused the package:\n{e}", target.name()),
                                &src,
                            );
                        }
                    }
                }
            }
        }
        checked += 1;
        let ours = match catch_unwind(AssertUnwindSafe(|| behavior(p, seed))) {
            Err(_) => {
                fail("the evaluator panicked".into(), &src);
                continue;
            }
            Ok(Err(e)) => {
                fail(format!("internal error: {e}"), &src);
                continue;
            }
            Ok(Ok(b)) => b,
        };
        outcomes += ours.len();
        // the formatted program behaves the same
        if let Some(text) = &formatted {
            let Some(fproj) = project(text) else {
                fail("the formatted program does not load".into(), &src);
                continue;
            };
            let fc = cpl_sema::check(&fproj);
            if fc.has_errors() {
                fail(
                    format!(
                        "the formatted program has errors: {:?}\n--- formatted\n{text}",
                        fc.diagnostics
                            .iter()
                            .filter(|d| d.is_error())
                            .map(|d| d.message.clone())
                            .collect::<Vec<_>>()
                    ),
                    &src,
                );
                continue;
            }
            // An exhausted budget is a resource outcome, not a schedule-independent result
            // (spec/parallelism.md): with parallel tasks it may stand where another run
            // reports the fault of a task that got further, so such outcomes are not compared.
            let differs = |theirs: &Vec<(String, String)>| {
                theirs.len() != ours.len()
                    || theirs.iter().zip(&ours).any(|(a, b)| {
                        a.0 != b.0 || (a.1 != b.1 && a.1 != "exhausted" && b.1 != "exhausted")
                    })
            };
            match behavior(&fc.program, seed) {
                Ok(theirs) if differs(&theirs) => fail(
                    format!(
                        "the formatted program behaves differently\n{ours:?}\n{theirs:?}\n--- formatted\n{text}"
                    ),
                    &src,
                ),
                Err(e) => fail(format!("internal error in the formatted program: {e}"), &src),
                _ => {}
            }
        }
    }
    println!(
        "cases {cases} parsed {parsed} checked {checked} outcomes {outcomes} violations {violations} in {:.1}s",
        start.elapsed().as_secs_f64()
    );
    if violations > 0 {
        std::process::exit(1);
    }
}
