//! Helpers shared by the integration tests that run *checked* programs:
//! CPL source is checked by the real checker (`cpl-sema`, a dev-dependency
//! of this crate only, toolchain 1.2 and T50) in a [`VirtualTree`] with a
//! synthetic `cpl.toml`, the example projects are loaded from disk, and
//! functions and tests are run through the public API of `cpl-eval`
//! (`run_function`, `run_tests`).
//!
//! Every checking helper panics with the rendered diagnostics when the
//! check reports an error, so a test only ever runs a program the checker
//! accepted (semantics 7.3, S39: a program with errors does not run).
//! Projects are loaded as `cpl run` and `cpl test` load them: without
//! parsing `work/` files (semantics 1.1; `LoadOptions::default()`).
#![allow(dead_code)]

use cpl_diag::{FileOrigin, Severity, SymbolPath};
use cpl_eval::test::TestReport;
use cpl_eval::{
    run_function, run_tests, EvalConfig, Fault, RunArg, RunError, RunOutcome, TestConfig,
    TestSelector,
};
use cpl_hir::{DefId, Program};
use cpl_sema::project::{load_project, load_project_from_tree, LoadOptions, Project, VirtualTree};
use cpl_sema::{check, CheckResult};
use serde_json::Value as Json;
use std::path::{Path, PathBuf};

// ---- locations --------------------------------------------------------------

/// The workspace root.
pub fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// `examples/<name>` (read-only).
pub fn example_dir(name: &str) -> PathBuf {
    workspace().join("examples").join(name)
}

/// `crates/cpl-eval/tests/corpus` (the interpreter corpus, toolchain 12.5).
pub fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("corpus")
}

// ---- projects from strings ----------------------------------------------------

/// A synthetic manifest (`[project] name`, `edition = "0.1"`, toolchain 11.1).
pub fn manifest(name: &str) -> String {
    format!("[project]\nname = \"{name}\"\nedition = \"0.1\"\n")
}

/// The conventional path of a source file (toolchain 3.7 rule 5, 12.2):
/// `module a.b.c` is `src/a/b/c.cpl`. The header is the first line that
/// starts with `module `; a file without one goes to `src/main.cpl`.
pub fn conventional_path(source: &str) -> String {
    for line in source.lines() {
        if let Some(rest) = line.trim().strip_prefix("module ") {
            let path: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '.')
                .collect();
            return format!("src/{}.cpl", path.replace('.', "/"));
        }
    }
    "src/main.cpl".to_string()
}

/// A tree of text files.
pub fn tree(files: &[(&str, &str)]) -> VirtualTree {
    VirtualTree {
        files: files
            .iter()
            .map(|(path, text)| (path.to_string(), text.as_bytes().to_vec()))
            .collect(),
    }
}

/// Load a tree (origin `Virtual`), panicking on a `LoadError`.
pub fn load_tree(tree: &VirtualTree) -> Project {
    load_project_from_tree(tree, FileOrigin::Virtual, &LoadOptions::default())
        .unwrap_or_else(|e| panic!("the tree does not load: {e}"))
}

/// Check a set of files in a project named `t`; a synthetic `cpl.toml` is
/// added when the set has none. The result is returned whatever it holds.
pub fn check_files_result(files: &[(&str, &str)]) -> CheckResult {
    let manifest = manifest("t");
    let mut all: Vec<(&str, &str)> = files.to_vec();
    if !all.iter().any(|(path, _)| *path == "cpl.toml") {
        all.push(("cpl.toml", manifest.as_str()));
    }
    check(&load_tree(&tree(&all)))
}

/// Check one module, placed at the conventional path of its header.
pub fn check_source_result(source: &str) -> CheckResult {
    let path = conventional_path(source);
    check_files_result(&[(path.as_str(), source)])
}

/// The rendered diagnostics of a result, for assertion messages.
pub fn render(result: &CheckResult) -> String {
    cpl_diag::render_all(&result.diagnostics, result.program.source_map())
}

/// The program of a check that reported no error; panics with the rendered
/// diagnostics otherwise.
pub fn checked(result: CheckResult) -> Program {
    if result.has_errors() {
        panic!("the program does not check:\n{}", render(&result));
    }
    result.program
}

/// Check one module (at its conventional path) and return the program;
/// panics with the rendered diagnostics on any error.
pub fn check_source(source: &str) -> Program {
    checked(check_source_result(source))
}

/// Check several files (a synthetic `cpl.toml` is added when absent) and
/// return the program; panics with the rendered diagnostics on any error.
pub fn check_files(files: &[(&str, &str)]) -> Program {
    checked(check_files_result(files))
}

/// The diagnostics of a result with the given severity, as codes.
pub fn codes_of(result: &CheckResult, severity: Severity) -> Vec<String> {
    result
        .diagnostics
        .iter()
        .filter(|d| d.severity == severity)
        .map(|d| d.code.0.to_string())
        .collect()
}

// ---- the example projects -------------------------------------------------------

/// `examples/<name>` loaded from disk as `cpl run` and `cpl test` load it
/// (work files are not parsed) and checked in development mode.
pub fn check_example(name: &str) -> CheckResult {
    let dir = example_dir(name);
    let project = load_project(&dir, &LoadOptions::default())
        .unwrap_or_else(|e| panic!("{} does not load: {e}", dir.display()));
    check(&project)
}

/// `examples/<name>` checked; panics with the diagnostics on any error.
pub fn example(name: &str) -> Program {
    checked(check_example(name))
}

/// `examples/reader`, checked.
pub fn reader() -> Program {
    example("reader")
}

/// `examples/holes`, checked.
pub fn holes() -> Program {
    example("holes")
}

// ---- names ------------------------------------------------------------------------

/// A qualified name (semantics 1.4), panicking when it does not parse.
pub fn path(qualified: &str) -> SymbolPath {
    SymbolPath::parse(qualified).unwrap_or_else(|e| panic!("{qualified}: {e}"))
}

/// The definition a qualified name names; panics when there is none.
pub fn def(p: &Program, qualified: &str) -> DefId {
    p.lookup(&path(qualified))
        .unwrap_or_else(|| panic!("the program has no {qualified}"))
}

// ---- running functions -------------------------------------------------------------

/// Value-JSON arguments (`--arg JSON`, toolchain 7.3).
pub fn json_args(args: &[Json]) -> Vec<RunArg> {
    args.iter().cloned().map(RunArg::Json).collect()
}

/// Constant-expression arguments (the positional arguments of `cpl run`,
/// toolchain 7.2).
pub fn const_args(args: &[&str]) -> Vec<RunArg> {
    args.iter()
        .map(|a| RunArg::ConstExpr(a.to_string()))
        .collect()
}

/// `run_function` with the default [`EvalConfig`] (checked mode).
pub fn try_run(p: &Program, function: &str, args: &[RunArg]) -> Result<RunOutcome, RunError> {
    run_function(p, &path(function), args, &EvalConfig::default())
}

/// `run_function` with a configuration.
pub fn try_run_with(
    p: &Program,
    function: &str,
    args: &[RunArg],
    config: &EvalConfig,
) -> Result<RunOutcome, RunError> {
    run_function(p, &path(function), args, config)
}

/// [`try_run`], panicking when the run is refused (a [`RunError`]).
pub fn run(p: &Program, function: &str, args: &[RunArg]) -> RunOutcome {
    try_run(p, function, args).unwrap_or_else(|e| panic!("{function}: the run was refused: {e}"))
}

/// Run with value-JSON arguments.
pub fn run_json(p: &Program, function: &str, args: &[Json]) -> RunOutcome {
    run(p, function, &json_args(args))
}

/// Run with constant-expression arguments.
pub fn run_const(p: &Program, function: &str, args: &[&str]) -> RunOutcome {
    run(p, function, &const_args(args))
}

/// The value JSON of a run's result at the declared return type
/// (toolchain 7.3); panics when the run faulted.
pub fn value(p: &Program, outcome: &RunOutcome) -> Json {
    match &outcome.value {
        Ok(v) => cpl_eval::value_to_json(p, v, &outcome.return_ty),
        Err(fault) => panic!("expected a value, got the fault {}", fault_json(p, fault)),
    }
}

/// The fault a run raised; panics when it returned a value.
pub fn fault_of<'a>(p: &Program, outcome: &'a RunOutcome) -> &'a Fault {
    match &outcome.value {
        Err(fault) => fault,
        Ok(v) => panic!(
            "expected a fault, got the value {}",
            cpl_eval::value_to_json(p, v, &outcome.return_ty)
        ),
    }
}

/// A fault as its diagnostic object (semantics 9.2, toolchain 1.7.3).
pub fn fault_json(p: &Program, fault: &Fault) -> Json {
    serde_json::to_value(fault.to_diagnostic(p).to_json(p.source_map()))
        .expect("a diagnostic serializes")
}

/// The diagnostic object of the fault a run raised; panics on a value.
pub fn fault(p: &Program, outcome: &RunOutcome) -> Json {
    fault_json(p, fault_of(p, outcome))
}

/// The outcome of a run as JSON: `{"value": <value JSON>}` or
/// `{"fault": <diagnostic object>}`.
pub fn outcome_json(p: &Program, outcome: &RunOutcome) -> Json {
    match &outcome.value {
        Ok(v) => serde_json::json!({"value": cpl_eval::value_to_json(p, v, &outcome.return_ty)}),
        Err(fault) => serde_json::json!({"fault": fault_json(p, fault)}),
    }
}

// ---- source locations ----------------------------------------------------------

/// The JSON location (semantics S49: 1-based line and column, `end_col`
/// exclusive) of the only occurrence of the single-line `needle` outside
/// comment lines (lines starting with `//`) in the file `file` whose text is
/// `source`; panics unless it occurs exactly once.
pub fn location_of(source: &str, file: &str, needle: &str) -> Json {
    let mut found = None;
    for (i, line) in source.lines().enumerate() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        let mut from = 0;
        while let Some(at) = line[from..].find(needle) {
            assert!(
                found.is_none(),
                "`{needle}` occurs more than once in {file}"
            );
            let col = line[..from + at].chars().count() + 1;
            found = Some((i + 1, col));
            from += at + needle.len();
        }
    }
    let (line, col) = found.unwrap_or_else(|| panic!("`{needle}` does not occur in {file}"));
    serde_json::json!({
        "file": file,
        "line": line,
        "col": col,
        "end_line": line,
        "end_col": col + needle.chars().count(),
    })
}

/// The JSON location of the first occurrence of the single-line `needle`
/// at or after the first line that contains `anchor` (both outside comment
/// lines), for a needle that occurs more than once in the file
/// (`location_after(src, file, "fn div(", "a / b")`).
pub fn location_after(source: &str, file: &str, anchor: &str, needle: &str) -> Json {
    let lines: Vec<&str> = source.lines().collect();
    let code = |l: &&str| !l.trim_start().starts_with("//");
    let start = lines
        .iter()
        .position(|l| code(l) && l.contains(anchor))
        .unwrap_or_else(|| panic!("`{anchor}` does not occur in {file}"));
    for (i, line) in lines.iter().enumerate().skip(start) {
        if !code(line) {
            continue;
        }
        if let Some(at) = line.find(needle) {
            let col = line[..at].chars().count() + 1;
            return serde_json::json!({
                "file": file,
                "line": i + 1,
                "col": col,
                "end_line": i + 1,
                "end_col": col + needle.chars().count(),
            });
        }
    }
    panic!("`{needle}` does not occur after `{anchor}` in {file}");
}

// ---- running tests ---------------------------------------------------------------

/// Every test and property with run seed `seed` and the other defaults of
/// [`TestConfig::with_seed`] (100 cases, 1000 shrinks).
pub fn run_all_tests(p: &Program, seed: u64) -> TestReport {
    run_tests(p, &TestSelector::all(), &TestConfig::with_seed(seed))
}

/// The selection with a configuration.
pub fn run_selected(p: &Program, selector: &TestSelector, config: &TestConfig) -> TestReport {
    run_tests(p, selector, config)
}

/// A test report as JSON (its wire form, toolchain 7.2).
pub fn report_json(report: &TestReport) -> Json {
    serde_json::to_value(report).expect("a test report serializes")
}
