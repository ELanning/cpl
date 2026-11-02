//! Suite D6 (toolchain 12.6): harvested programs. Every CPL module source that the tests of `cpl-eval` embed as a
//! string literal and every module of `crates/cpl-sema/tests/corpus/ok/**`, extracted once into
//! `differential/harvested/` (regenerated with `UPDATE_HARVEST=1`), each run with all its tests and properties on both
//! engines and with D5's sweeps; a module that does not check on its own is counted and skipped.

use crate::{assert_clean_shutdown, driver_config, sweep_module, Gate};
use cpl_cabi::NativeSession;
use cpl_eval::EvalConfig;
use std::path::{Path, PathBuf};

fn harvested_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/differential/harvested")
}

/// The contents of every string literal of a Rust source (raw strings with any number of `#`, and ordinary strings
/// with their escapes) that starts with `module `.
fn module_literals(rs: &str) -> Vec<String> {
    let b = rs.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        // a line comment
        if b[i] == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // a raw string: r#*"
        if b[i] == b'r' && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')) {
            let mut j = i + 1;
            let mut hashes = 0;
            while j < b.len() && b[j] == b'#' {
                hashes += 1;
                j += 1;
            }
            if j < b.len() && b[j] == b'"' {
                let close = format!("\"{}", "#".repeat(hashes));
                if let Some(end) = rs[j + 1..].find(&close) {
                    let text = &rs[j + 1..j + 1 + end];
                    if text.trim_start().starts_with("module ") {
                        out.push(text.to_string());
                    }
                    i = j + 1 + end + close.len();
                    continue;
                }
            }
        }
        // a character literal that is a quote: '"'
        if b[i] == b'\'' && b.get(i + 1) == Some(&b'"') && b.get(i + 2) == Some(&b'\'') {
            i += 3;
            continue;
        }
        if b[i] == b'"' {
            let mut text = String::new();
            let mut j = i + 1;
            while j < b.len() && b[j] != b'"' {
                if b[j] == b'\\' && j + 1 < b.len() {
                    match b[j + 1] {
                        b'n' => text.push('\n'),
                        b't' => text.push('\t'),
                        b'r' => text.push('\r'),
                        b'0' => text.push('\0'),
                        b'\\' => text.push('\\'),
                        b'"' => text.push('"'),
                        b'\'' => text.push('\''),
                        b'\n' => {
                            // a line continuation: the newline and the next line's leading whitespace
                            j += 2;
                            while j < b.len() && (b[j] == b' ' || b[j] == b'\t' || b[j] == b'\n') {
                                j += 1;
                            }
                            continue;
                        }
                        b'u' => {
                            let end = rs[j..].find('}').map(|e| j + e).unwrap_or(j + 2);
                            let hex = &rs[j + 3..end];
                            if let Some(c) =
                                u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
                            {
                                text.push(c);
                            }
                            j = end + 1;
                            continue;
                        }
                        other => text.push(other as char),
                    }
                    j += 2;
                    continue;
                }
                let ch = rs[j..].chars().next().unwrap();
                text.push(ch);
                j += ch.len_utf8();
            }
            if text.trim_start().starts_with("module ") {
                out.push(text);
            }
            i = j + 1;
            continue;
        }
        i += 1;
    }
    out
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

/// Extracts the harvested programs into `differential/harvested/`.
fn harvest() {
    let dir = harvested_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files: Vec<PathBuf> = std::fs::read_dir(crates.join("cpl-eval/tests"))
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "rs"))
        .collect();
    files.sort();
    for f in files {
        let stem = f.file_stem().unwrap().to_string_lossy().to_string();
        for (n, text) in module_literals(&std::fs::read_to_string(&f).unwrap())
            .into_iter()
            .enumerate()
        {
            std::fs::write(dir.join(format!("eval_{stem}_{n:03}.cpl")), text).unwrap();
        }
    }
    let ok = crates.join("cpl-sema/tests/corpus/ok");
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&ok)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    for e in entries {
        let name = e.file_name().unwrap().to_string_lossy().to_string();
        if e.is_dir() {
            copy_dir(&e, &dir.join(format!("sema_{name}")));
        } else if name.ends_with(".cpl") {
            std::fs::copy(&e, dir.join(format!("sema_{name}"))).unwrap();
        }
    }
}

/// D6: every harvested program, on worker threads (each with its own drivers); the number of programs run and of
/// those that do not check on their own.
pub(crate) fn d6(gate: &mut Gate) -> (usize, usize) {
    if std::env::var("UPDATE_HARVEST").is_ok_and(|v| v == "1") || !harvested_dir().is_dir() {
        harvest();
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(harvested_dir())
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    let workers = std::thread::available_parallelism()
        .map_or(1, |n| n.get() / 3)
        .clamp(1, 4);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let sanitize = gate.sanitize;
    let results: Vec<(Gate, usize, usize)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                let (entries, next) = (&entries, &next);
                scope.spawn(move || {
                    let mut g = Gate {
                        sanitize,
                        ..Gate::default()
                    };
                    let (mut ran, mut unchecked) = (0, 0);
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(e) = entries.get(i) else { break };
                        if run_program(&mut g, e) {
                            ran += 1;
                        } else {
                            unchecked += 1;
                        }
                    }
                    (g, ran, unchecked)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("a D6 worker"))
            .collect()
    });
    let (mut ran, mut unchecked) = (0, 0);
    for (g, r, u) in results {
        gate.merge(g);
        ran += r;
        unchecked += u;
    }
    (ran, unchecked)
}

/// One harvested program: false when it does not check on its own.
fn run_program(gate: &mut Gate, e: &Path) -> bool {
    let name = e.file_name().unwrap().to_string_lossy().to_string();
    let started = std::time::Instant::now();
    if std::env::var("CPL_DIFF_VERBOSE").is_ok() {
        println!("  D6 {name} ...");
    }
    let project = if e.is_dir() {
        // a multi-file project of the sema corpus: its own source tree under a generated manifest
        let mut files = std::collections::BTreeMap::new();
        files.insert(
            "cpl.toml".to_string(),
            b"[project]\nname = \"harvested\"\nedition = \"0.1\"\n".to_vec(),
        );
        let mut stack = vec![e.to_path_buf()];
        while let Some(d) = stack.pop() {
            for f in std::fs::read_dir(&d).unwrap().filter_map(|x| x.ok()) {
                let p = f.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "cpl") {
                    let rel = p.strip_prefix(e).unwrap().to_string_lossy().to_string();
                    files.insert(rel, std::fs::read(&p).unwrap());
                }
            }
        }
        cpl_project::load_project_from_tree(
            &cpl_project::VirtualTree { files },
            cpl_diag::FileOrigin::Virtual,
            &cpl_project::LoadOptions::default(),
        )
    } else {
        let source = std::fs::read_to_string(e).unwrap();
        let module = source
            .trim_start()
            .lines()
            .next()
            .and_then(|l| l.strip_prefix("module "))
            .unwrap_or("harvested")
            .trim()
            .to_string();
        Ok(crate::project_of("harvested", &module, &source))
    };
    let Ok(project) = project else {
        return false;
    };
    let checked = cpl_sema::check(&project);
    if checked.has_errors() || !project.diagnostics.is_empty() {
        return false;
    }
    let p = &checked.program;
    let sm = project.source_map.clone();
    let plan = cpl_sema::interop::InteropPlan::empty(project.manifest.c_prefix());
    let mut session = NativeSession::start(
        p,
        &project,
        &plan,
        &EvalConfig::default(),
        &driver_config(gate.sanitize),
    )
    .unwrap_or_else(|e| {
        panic!(
            "D6 {name}: the native engine does not start: {:?}",
            e.into_diagnostics()
        )
    });
    session.set_request_timeout(Some(std::time::Duration::from_secs(60)));
    // a budget for each top-level evaluation: a test that never ends is a time fault, compared by rule 3
    let mut config = cpl_eval::TestConfig::with_seed(1);
    config.eval.time_budget = Some(std::time::Duration::from_millis(200));
    config.max_shrinks = 50;
    session.reconfigure(&config.eval).expect("reconfigure");
    for test in cpl_eval::select_tests(p, &cpl_eval::TestSelector::default()) {
        let a = cpl_eval::run_test(p, test, &config);
        let b = cpl_eval::run_test_with(p, test, &config, &mut session.clone());
        if let Some(e) = session.take_error() {
            panic!("D6 {name}: the driver failed: {:?}", e.into_diagnostics());
        }
        gate.compared += 1;
        // a test whose evaluation met a time or guard fault is compared by rule 3 only
        if let Some(d) = cpl_cabi::compare::test_records(&a, &b, crate::sanctioned(&a, &b)) {
            gate.divergences.push(format!(
                "D6 {name} test {}:\n  eval:   {}\n  native: {}",
                a.qualified_name, d.eval, d.native
            ));
        }
    }
    sweep_module(gate, p, &sm, &mut session);
    gate.absorb(&mut session);
    assert_clean_shutdown(session, &name);
    if std::env::var("CPL_DIFF_VERBOSE").is_ok() {
        println!("  D6 {name}: {:?}", started.elapsed());
    }
    true
}
