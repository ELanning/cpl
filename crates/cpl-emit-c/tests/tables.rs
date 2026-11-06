//! The table-fidelity test (toolchain 12, 12.9 item 14; spec/cabi.md Section 3): for every `DefId`, site, clause, hole,
//! and refinement of every corpus program, the emitted model (`EmittedPackage::tables`) gives the `Program`'s own
//! answers, the compiled tables give the same through the driver's `tables` request (spoken here directly: this crate
//! does not reach `cpl-cabi`), and `cpl_sema::interop::type_text` names every declaration as
//! `cpl_eval::types::type_name` does.

use cpl_diag::FileOrigin;
use cpl_hir::{DefId, DefKind, Program, Ty};
use cpl_project::{load_project, load_project_from_tree, LoadOptions, Project, VirtualTree};
use serde_json::{json, Value as Json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A driver child spoken to by cpl-driver protocol 1 (spec/cabi.md 3.8).
struct Driver {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
}

impl Driver {
    fn start(path: &Path) -> Driver {
        let mut child = Command::new(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut banner = String::new();
        stdout.read_line(&mut banner).unwrap();
        let mut d = Driver {
            child,
            stdin,
            stdout,
        };
        let r = d.ask(&json!({"op": "configure", "max_call_depth": 10000, "time_budget_ms": null, "check_contracts": true, "max_heap_bytes": 0}));
        assert!(r.get("ok").is_some(), "configure: {r}");
        d
    }

    fn ask(&mut self, req: &Json) -> Json {
        writeln!(self.stdin, "{req}").unwrap();
        self.stdin.flush().unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"))
    }

    /// Every entry of a table.
    fn table(&mut self, table: &str) -> Vec<Json> {
        let n = self.ask(&json!({"op": "tables", "table": table}))["ok"]
            .as_u64()
            .unwrap();
        (0..n)
            .map(|i| self.ask(&json!({"op": "tables", "table": table, "index": i}))["ok"].clone())
            .collect()
    }

    fn shutdown(mut self) {
        let _ = self.ask(&json!({"op": "shutdown"}));
        let _ = self.child.wait();
    }
}

/// The type of a declaration that names a type.
fn ty_of(p: &Program, d: DefId) -> Option<Ty> {
    Some(match &p.def(d).kind {
        DefKind::Struct(_) | DefKind::Enum(_) => Ty::Adt(d, vec![]),
        DefKind::Wrapper(_) => Ty::Wrapper(d),
        DefKind::Capability(_) => Ty::Capability(d),
        DefKind::Refinement(_) => {
            let class = p
                .refinement_classes
                .iter()
                .position(|c| c.members.contains(&d))?;
            Ty::Refined(cpl_hir::RefinementClass(class as u32), d)
        }
        _ => return None,
    })
}

/// The violations of one program.
fn check_program(what: &str, project: &Project, cc: bool) -> Vec<String> {
    let mut out = Vec::new();
    let checked = cpl_sema::check(project);
    if checked.has_errors() {
        return out;
    }
    let p = &checked.program;
    let sm = &project.source_map;
    let plan = cpl_sema::interop::InteropPlan::empty(project.manifest.c_prefix());
    // type_text names every declaration as type_name does
    for d in p.defs() {
        if let Some(ty) = ty_of(p, d.id) {
            let (a, b) = (
                cpl_sema::interop::type_text(p, &ty),
                cpl_eval::types::type_name(p, d.id),
            );
            if a != b {
                out.push(format!("{what}: type_text {a:?} but type_name {b:?}"));
            }
        }
    }
    let cfg = cpl_emit_c::EmitConfig {
        with_tests: true,
        ..Default::default()
    };
    let model = cpl_emit_c::emit(p, project, &plan, &cfg)
        .expect("emits")
        .tables;
    // the model against the Program: a clause's site lies in its owner's declaration; a hole's site reads ?<name>;
    // a refinement row names a checked predicate
    for (i, (text, _, site, owner, _, _)) in model.clause_rows.iter().enumerate() {
        let owner_span = p.def(DefId(*owner)).span;
        let span = model.site_spans[*site as usize];
        if span.file != owner_span.file
            || span.start < owner_span.start
            || span.end > owner_span.end
        {
            out.push(format!(
                "{what}: clause {i} ({text}) lies outside its owner {}",
                p.display_path(DefId(*owner))
            ));
        }
    }
    for (id, name, _, site) in &model.hole_rows {
        let span = model.site_spans[*site as usize];
        let source = sm.text(span);
        if source != format!("?{name}") {
            out.push(format!("{what}: hole {id} {name} is at {source:?}"));
        }
    }
    // a refinement row names a refinement, or a struct with an invariant (`checked_predicate`: both are checked
    // predicates of the CPL_R table)
    for (def, _, declared) in &model.ref_rows {
        match &p.def(DefId(*def)).kind {
            DefKind::Refinement(_) if p.def(DefId(*def)).name.display() == *declared => {}
            DefKind::Struct(s) if s.invariant.is_some() && p.def(DefId(*def)).name.display() == *declared => {}
            _ => out.push(format!(
                "{what}: refinement row {def} names no refinement {declared}"
            )),
        }
    }
    if !cc {
        return out;
    }
    // the compiled tables through the driver, against the model and the Program
    let dcfg = cpl_emit_c::package::DriverConfig {
        cache_dir: Path::new(env!("CARGO_TARGET_TMPDIR")).join("tables-cache"),
        profile: cpl_emit_c::cc::Profile::Debug,
        sanitize: vec![],
        coverage: false,
        cc: None,
        runtime: cpl_rt::RuntimeSource::Embedded,
        werror: true,
    };
    let build = cpl_emit_c::package::driver(project, p, &plan, &dcfg)
        .unwrap_or_else(|e| panic!("{what}: {e:?}"));
    let mut d = Driver::start(&build.path);
    let defs = d.table("defs");
    if defs.len() != p.defs.len() {
        out.push(format!(
            "{what}: {} compiled defs for {} DefIds",
            defs.len(),
            p.defs.len()
        ));
    }
    for (i, row) in defs.iter().enumerate().take(p.defs.len()) {
        let def = p.def(DefId(i as u32));
        if row["stable_id"] != def.stable_id.as_str()
            || row["qualified_name"] != p.display_path(def.id)
        {
            out.push(format!(
                "{what}: def {i}: {row} for {} {}",
                def.stable_id.as_str(),
                p.display_path(def.id)
            ));
        }
    }
    let files = d.table("files");
    let sites = d.table("sites");
    if sites.len() != model.site_spans.len() {
        out.push(format!(
            "{what}: {} compiled sites for {} in the model",
            sites.len(),
            model.site_spans.len()
        ));
    }
    for (i, (row, span)) in sites.iter().zip(&model.site_spans).enumerate() {
        let loc = sm.location(*span);
        let file = row["file"]
            .as_u64()
            .and_then(|f| files.get(f as usize))
            .cloned()
            .unwrap_or(Json::Null);
        let same = file == loc.file
            && row["line"] == loc.line
            && row["col"] == loc.col
            && row["end_line"] == loc.end_line
            && row["end_col"] == loc.end_col;
        if !same {
            out.push(format!(
                "{what}: site {i}: compiled {row} ({file}), source map {loc:?}"
            ));
        }
    }
    let clauses = d.table("clauses");
    for (i, (row, m)) in clauses.iter().zip(&model.clause_rows).enumerate() {
        let (text, kind, site, owner, op, _) = m;
        let same = row["text"] == *text
            && row["kind"] == *kind
            && row["site"] == *site
            && row["owner"] == *owner
            && row["op"] == op.map_or(Json::Null, Json::from);
        if !same {
            out.push(format!("{what}: clause {i}: compiled {row}, model {m:?}"));
        }
    }
    let holes = d.table("holes");
    for (id, name, expected, site) in &model.hole_rows {
        let row = holes.get(*id as usize).cloned().unwrap_or(Json::Null);
        if row["name"] != *name || row["expected_type"] != *expected || row["site"] != *site {
            out.push(format!(
                "{what}: hole {id}: compiled {row}, model {name} {expected} {site}"
            ));
        }
    }
    let refs = d.table("refinements");
    for (i, (row, (def, type_name, declared))) in refs.iter().zip(&model.ref_rows).enumerate() {
        if row["def"] != *def || row["type_name"] != *type_name || row["declared_name"] != *declared
        {
            out.push(format!(
                "{what}: refinement {i}: compiled {row}, model {def} {type_name} {declared}"
            ));
        }
    }
    d.shutdown();
    out
}

fn single(source: &str) -> Project {
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
                b"[project]\nname = \"fidelity\"\nedition = \"0.1\"\n".to_vec(),
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

#[test]
fn the_tables_give_the_programs_own_answers() {
    let cc = match cpl_rt::testing::find_cc() {
        Some(_) => true,
        None if std::env::var("CPL_REQUIRE_CC").is_ok_and(|v| v == "1") => {
            panic!("CPL_REQUIRE_CC=1, but there is no C compiler")
        }
        None => {
            println!("skipped the compiled tables: no C compiler (set CPL_REQUIRE_CC=1 to fail)");
            false
        }
    };
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
        let mut files: Vec<PathBuf> = std::fs::read_dir(ws.join(dir))
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "cpl"))
            .collect();
        files.sort();
        for f in files {
            programs.push((
                f.display().to_string(),
                single(&std::fs::read_to_string(&f).unwrap()),
            ));
        }
    }
    let mut violations = Vec::new();
    for (what, project) in &programs {
        violations.extend(check_program(what, project, cc));
    }
    println!("tables: {} programs", programs.len());
    assert!(
        violations.is_empty(),
        "{} table-fidelity violations:\n{}",
        violations.len(),
        violations.join("\n")
    );
}
