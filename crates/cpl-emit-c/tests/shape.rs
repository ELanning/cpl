//! The emitted-shape test (toolchain 12, 12.9 item 14; spec/cabi.md 1.3): the slot discipline, checked mechanically on
//! every `cpl_f`, `cpl_p`, `cpl_o`, `cpl_t`, and `cpl_b` function of the emitted program unit of every corpus program
//! (the interpreter corpus, the examples, the checker's `ok` corpus, the differential corpus), each function read
//! alone: (1) every slot is declared at the top and initialized to 0; (2) only `CPLRT_SET`, `CPLRT_COPY`, and
//! `CPLRT_MOVE` write a slot; (3) the only labels are `exit:`, `fail:`, and `done:`; (4) no `return` before `done:`
//! but the refusal after `cplrt_enter_fn` or `cplrt_enter_act`; (5) no `setjmp`/`longjmp` and no mutable global state.

use cpl_diag::FileOrigin;
use cpl_project::{load_project, load_project_from_tree, LoadOptions, Project, VirtualTree};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A slot name of rule 1: `l<k>_<name>`, `t<k>`, `c<k>`, `m<k>`, or `rv`.
fn is_slot(word: &str) -> bool {
    if word == "rv" {
        return true;
    }
    let b = word.as_bytes();
    if b.len() < 2 || !b[1].is_ascii_digit() {
        return false;
    }
    match b[0] {
        b't' | b'c' | b'm' => b[1..].iter().all(u8::is_ascii_digit),
        b'l' => {
            let digits = b[1..].iter().take_while(|c| c.is_ascii_digit()).count();
            b.get(1 + digits) == Some(&b'_')
        }
        _ => false,
    }
}

/// The identifiers of a line of C (outside string literals).
fn words(line: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let b = line.as_bytes();
    let mut i = 0;
    let mut in_str = false;
    while i < b.len() {
        let c = b[i];
        if in_str {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == b'"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        if c == b'"' {
            in_str = true;
            i += 1;
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            out.push((start, &line[start..i]));
            continue;
        }
        i += 1;
    }
    out
}

/// The violations of the slot discipline in one emitted function (its lines, header first).
fn check_function(name: &str, lines: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    // rule 1: the declarations at the top: `cplrt_w <slot> = 0, ...;`
    let mut declared: BTreeSet<String> = BTreeSet::new();
    let mut body_start = 2;
    for (i, l) in lines.iter().enumerate().skip(2) {
        let t = l.trim();
        if t.starts_with("int ") || t.starts_with("uint64_t ") {
            continue;
        }
        if let Some(rest) = t.strip_prefix("cplrt_w ") {
            if rest.contains('[') {
                break; // an array of borrowed words: not a slot declaration
            }
            for item in rest.trim_end_matches(';').split(',') {
                let item = item.trim();
                match item.split_once('=') {
                    Some((n, v)) if v.trim() == "0" => {
                        declared.insert(n.trim().to_string());
                    }
                    _ => out.push(format!("{name}: a slot declared without = 0: {item}")),
                }
            }
            body_start = i + 1;
            continue;
        }
        break;
    }
    let done_at = lines
        .iter()
        .position(|l| l.trim() == "done:")
        .unwrap_or(lines.len());
    for (i, l) in lines.iter().enumerate().skip(body_start) {
        let t = l.trim();
        // rule 3: the labels
        if !t.contains(' ')
            && t.ends_with(':')
            && !t.starts_with("case")
            && !t.starts_with("default")
        {
            let label = t.trim_end_matches(':');
            if !["exit", "fail", "done"].contains(&label) {
                out.push(format!(
                    "{name}: a label {label}: other than exit, fail, done"
                ));
            }
            continue;
        }
        // rule 4: no return before done: but the refusal after entering the frame
        if i < done_at && words(t).iter().any(|(_, w)| *w == "return") {
            let prev = lines[..i]
                .iter()
                .rev()
                .find(|l| !l.trim().is_empty())
                .copied()
                .unwrap_or("");
            if !(prev.contains("cplrt_enter_fn") || prev.contains("cplrt_enter_act")) {
                out.push(format!("{name}: a return before done: {t}"));
            }
        }
        // rules 1 and 2: every slot used is declared, and written only by the three macros
        for (at, w) in words(l) {
            if !is_slot(w) {
                continue;
            }
            if !declared.contains(w) {
                out.push(format!(
                    "{name}: the slot {w} is not declared at the top: {t}"
                ));
            }
            let after = l[at + w.len()..].trim_start();
            let assigns = after.starts_with('=') && !after.starts_with("==");
            let compound = [
                "+=", "-=", "*=", "/=", "|=", "&=", "^=", "<<=", ">>=", "%=", "++", "--",
            ]
            .iter()
            .any(|op| after.starts_with(op));
            if assigns || compound {
                out.push(format!(
                    "{name}: the slot {w} is written outside CPLRT_SET/COPY/MOVE: {t}"
                ));
            }
        }
        // rule 5
        if t.contains("setjmp") || t.contains("longjmp") {
            out.push(format!("{name}: setjmp/longjmp: {t}"));
        }
    }
    out
}

/// The violations of one program unit.
fn check_unit(what: &str, text: &str) -> (usize, Vec<String>) {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut checked = 0;
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        let is_fn = (l.starts_with("static int cpl_") || l.starts_with("int cpl_"))
            && !l.trim_end().ends_with(';')
            && lines.get(i + 1).is_some_and(|n| n.trim() == "{")
            && {
                let name = l
                    .split(['(', ' '])
                    .find(|w| w.starts_with("cpl_"))
                    .unwrap_or("");
                let kind = name.as_bytes().get(4).copied().unwrap_or(0);
                matches!(kind, b'f' | b'p' | b'o' | b't' | b'b')
            };
        if !is_fn {
            // rule 5: no mutable state at file scope (only const tables and functions)
            let t = l.trim_start();
            if !l.starts_with(' ')
                && !l.starts_with('}')
                && !l.starts_with('#')
                && !l.starts_with("/*")
                && !t.is_empty()
            {
                let statement = l.trim_end().ends_with(';') && !l.contains('(');
                // an immortal constant (CPLRT_RC_IMMORTAL) is never written: its header is not counted (cabi.md 2.4)
                let immortal = l.contains("CPLRT_RC_IMMORTAL");
                if statement
                    && !immortal
                    && !(l.contains("const ")
                        || l.starts_with("extern ")
                        || l.starts_with("typedef "))
                {
                    out.push(format!("{what}: mutable state at file scope: {l}"));
                }
            }
            i += 1;
            continue;
        }
        let end = lines[i..]
            .iter()
            .position(|x| *x == "}")
            .map(|e| i + e)
            .unwrap_or(lines.len() - 1);
        let name = l
            .split(['(', ' '])
            .find(|w| w.starts_with("cpl_"))
            .unwrap_or("?")
            .to_string();
        out.extend(check_function(&format!("{what} {name}"), &lines[i..=end]));
        checked += 1;
        i = end + 1;
    }
    (checked, out)
}

fn emit_prog(project: &Project, with_tests: bool) -> Option<String> {
    let checked = cpl_sema::check(project);
    if checked.has_errors() {
        return None;
    }
    let plan = checked
        .interop
        .clone()
        .unwrap_or_else(|| cpl_sema::interop::InteropPlan::empty(project.manifest.c_prefix()));
    let cfg = cpl_emit_c::EmitConfig {
        with_tests,
        ..Default::default()
    };
    let pkg = cpl_emit_c::emit(&checked.program, project, &plan, &cfg).ok()?;
    let prefix = cpl_emit_c::unit_prefix(&plan);
    pkg.files
        .into_iter()
        .find(|f| f.path == format!("src/{prefix}_prog.c"))
        .map(|f| f.text)
}

fn single(name: &str, source: &str) -> Project {
    let module = source
        .lines()
        .next()
        .and_then(|l| l.strip_prefix("module "))
        .unwrap_or("m")
        .trim()
        .to_string();
    let tree = VirtualTree {
        files: [
            (
                "cpl.toml".to_string(),
                format!("[project]\nname = \"{name}\"\nedition = \"0.1\"\n").into_bytes(),
            ),
            (
                format!("src/{}.cpl", module.replace('.', "/")),
                source.as_bytes().to_vec(),
            ),
        ]
        .into_iter()
        .collect(),
    };
    load_project_from_tree(&tree, FileOrigin::Virtual, &LoadOptions::default()).unwrap()
}

fn cpl_files(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|r| {
            r.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "cpl"))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

#[test]
fn every_corpus_program_keeps_the_slot_discipline() {
    let ws = workspace();
    let mut programs: Vec<(String, Project)> = Vec::new();
    for root in ["examples/reader", "examples/holes"] {
        programs.push((
            root.to_string(),
            load_project(&ws.join(root), &LoadOptions::default()).unwrap(),
        ));
    }
    for dir in [
        "crates/cpl-eval/tests/corpus",
        "crates/cpl-sema/tests/corpus/ok",
        "crates/cpl-cabi/tests/differential/corpus",
    ] {
        for f in cpl_files(&ws.join(dir)) {
            let source = std::fs::read_to_string(&f).unwrap();
            programs.push((f.display().to_string(), single("shape", &source)));
        }
    }
    let mut checked_programs = 0;
    let mut functions = 0;
    let mut violations = Vec::new();
    for (what, project) in &programs {
        for with_tests in [false, true] {
            let Some(text) = emit_prog(project, with_tests) else {
                continue;
            };
            let (n, v) = check_unit(what, &text);
            functions += n;
            violations.extend(v);
        }
        checked_programs += 1;
    }
    println!("shape: {functions} emitted functions of {checked_programs} programs checked");
    assert!(
        functions > 100,
        "too few emitted functions were found ({functions}): the parser of the text is off"
    );
    assert!(
        violations.is_empty(),
        "{} violations of the slot discipline:\n{}",
        violations.len(),
        violations.join("\n")
    );
}
