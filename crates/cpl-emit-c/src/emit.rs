//! The orchestration of one emission (`spec/cabi.md` 1.1, 1.6): the program unit `<prefix>_prog.c`, the tables unit
//! `<prefix>_tables.c`, the API unit `<prefix>_api.c`, and the digests.

use crate::lower::{self, Names, N_A, N_CL, N_D, N_H, N_K, N_R, N_S};
use crate::print::{comment_text, prologue};
use crate::tables::{Tables, UnitInfo};
use crate::{EmitConfig, EmittedFile, EmittedPackage, InteropPlan, TablesModel};
use cpl_diag::json::Digest;
use cpl_diag::{Category, DiagCode, Diagnostic};
use cpl_hir::{DefKind, DefStatus, Program};
use cpl_project::Project;
use std::fmt::Write as _;

const ZERO_DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";

fn digest_of(units: &[(&str, &str)]) -> Digest {
    let mut h = blake3::Hasher::new();
    for (path, text) in units {
        h.update(path.as_bytes());
        h.update(b"\n");
        h.update(text.as_bytes());
        h.update(b"\n");
    }
    Digest(h.finalize().to_hex().to_string())
}

pub(crate) fn emit(
    p: &Program,
    project: &Project,
    plan: &InteropPlan,
    cfg: &EmitConfig,
) -> Result<EmittedPackage, Vec<Diagnostic>> {
    if let Some(d) = p.defs.iter().find(|d| d.status == DefStatus::HasErrors) {
        let detail = format!(
            "{} has errors; a program with errors is never emitted",
            p.display_path(d.id)
        );
        return Err(vec![Diagnostic::error(
            DiagCode("internal_error"),
            Category::Internal,
            format!("internal error ({}): {detail}", crate::codes::EMIT_FAILED),
        )
        .with_data("kind", serde_json::json!(crate::codes::EMIT_FAILED))
        .with_data("detail", serde_json::json!(detail))]);
    }
    let prefix = crate::unit_prefix(plan);
    let runtime_digest = cpl_rt::runtime_digest(&cfg.runtime).map_err(|e| {
        vec![Diagnostic::error(
            DiagCode("internal_error"),
            Category::Internal,
            format!(
                "internal error ({}): cannot read the runtime: {e}",
                crate::codes::EMIT_FAILED
            ),
        )
        .with_data("kind", serde_json::json!(crate::codes::EMIT_FAILED))
        .with_data(
            "detail",
            serde_json::json!(format!("cannot read the runtime: {e}")),
        )]
    })?;

    let mut tables = Tables::new(p, cfg.with_tests);
    tables.optimize = cfg.optimize && !cfg.coverage;
    if cfg.coverage {
        tables.cov = Some(Vec::new());
    }
    let mut names = Names::default();

    // ---- the functions of the program unit
    let mut protos = String::new();
    let mut funcs = String::new();
    for d in &p.defs {
        match &d.kind {
            DefKind::Fn(_) if lower::emits_fn(p, d.id) => {
                let _ = writeln!(protos, "{};", lower::fn_prototype(p, d.id));
            }
            kind if kind.checked_predicate().is_some() => {
                let rd = kind.checked_predicate().unwrap();
                let _ = writeln!(protos, "{};", lower::pred_prototype(d.id, rd.self_local));
            }
            DefKind::Capability(cd) => {
                for i in 0..cd.ops.len() {
                    let _ = writeln!(protos, "{};", lower::op_prototype(p, d.id, i));
                }
            }
            _ => {}
        }
    }
    let mut tests = Vec::new();
    let mut bounds = Vec::new();
    let mut holes = 0u32;
    for d in &p.defs {
        match &d.kind {
            DefKind::Fn(_) if lower::emits_fn(p, d.id) => {
                funcs.push_str(&lower::emit_fn(&mut tables, &mut names, d.id));
                funcs.push('\n');
            }
            kind if kind.checked_predicate().is_some() => {
                funcs.push_str(&lower::emit_pred(&mut tables, &mut names, d.id));
                funcs.push('\n');
            }
            DefKind::Capability(cd) => {
                for i in 0..cd.ops.len() {
                    funcs.push_str(&lower::emit_op(&mut tables, &mut names, d.id, i));
                    funcs.push('\n');
                }
            }
            DefKind::Test(_) | DefKind::Property(_) if cfg.with_tests => {
                funcs.push_str(&lower::emit_test(&mut tables, &mut names, d.id));
                funcs.push('\n');
                tests.push(d.id);
                if let DefKind::Property(_) = &d.kind {
                    let (text, entries) = lower::emit_bounds(&mut tables, &mut names, d.id);
                    funcs.push_str(&text);
                    for (binder, hi, name) in entries {
                        bounds.push((d.id.0, binder, hi, name));
                    }
                }
            }
            _ => {}
        }
    }
    holes += tables.holes.len() as u32;
    // the entry trampolines: every function with a block body and every callable stub in a driver; the exports'
    // functions in every build
    let models = crate::api::export_models(p, plan);
    let exported: std::collections::HashSet<_> = models.iter().map(|m| m.function).collect();
    let mut entries = Vec::new();
    for d in &p.defs {
        let exported = exported.contains(&d.id);
        if cfg.with_tests || exported {
            if let Some(text) = lower::emit_entry(p, &mut names, d.id) {
                funcs.push_str(&text);
                entries.push(d.id.0);
            }
        }
    }
    for t in &plan.types {
        tables.add_type(&t.ty);
    }
    for m in &models {
        for prm in &m.params {
            tables.add_type(&prm.ty);
        }
    }

    // ---- the program unit
    let mut prog = String::with_capacity(funcs.len() + 16 * 1024);
    let _ = writeln!(
        prog,
        "/* {prefix}_prog.c: the program of package \"{}\" (spec/cabi.md 1.4). Generated by cpl; do not edit. */",
        comment_text(&project.manifest.project.name)
    );
    prog.push_str(&prologue(&prefix));
    prog.push_str("#include \"cpl_rt.h\"\n\n");
    for (kind, n) in &names.used {
        match *kind {
            N_D => {
                let _ = writeln!(prog, "#define CPL_D{n} {n}u");
            }
            N_S => {
                let _ = writeln!(prog, "#define CPL_S{n} {n}u");
            }
            N_CL => {
                let _ = writeln!(prog, "#define CPL_CL{n} {n}u");
            }
            N_H => {
                let _ = writeln!(prog, "#define CPL_H{n} {n}u");
            }
            N_A => {
                let _ = writeln!(prog, "#define CPL_A{n} {n}u");
            }
            N_K => {
                let _ = writeln!(prog, "#define CPL_K{n} cplrt_word_of(&cpl_k{n}.hdr)");
            }
            N_R => {
                let _ = writeln!(prog, "#define CPL_R{n} {n}u");
            }
            _ => {}
        }
    }
    prog.push('\n');
    for (k, key) in tables.consts.iter().enumerate() {
        prog.push_str(&lower::emit_const(k as u32, key));
    }
    prog.push('\n');
    prog.push_str(&protos);
    prog.push('\n');
    prog.push_str(&funcs);
    // the code table
    let ndefs = p.defs.len();
    let mut preds = Vec::new();
    for d in &p.defs {
        if d.kind.checked_predicate().is_some() {
            preds.push(d.id.0);
        }
    }
    let designated = |items: &[u32], f: &str| -> String {
        if items.is_empty() {
            "0".to_string()
        } else {
            items
                .iter()
                .map(|n| format!("[{n}] = {f}{n}"))
                .collect::<Vec<_>>()
                .join(", ")
        }
    };
    let _ = writeln!(
        prog,
        "\nstatic const cplrt_fn_entry cpl_fns[{}] = {{ {} }};",
        ndefs.max(1),
        designated(&entries, "cpl_e")
    );
    let _ = writeln!(
        prog,
        "static const cplrt_pred_entry cpl_preds[{}] = {{ {} }};",
        ndefs.max(1),
        designated(&preds, "cpl_p")
    );
    let tests_name = if cfg.with_tests {
        let t: Vec<u32> = tests.iter().map(|d| d.0).collect();
        let _ = writeln!(
            prog,
            "static const cplrt_test_entry cpl_tests[{}] = {{ {} }};",
            ndefs.max(1),
            designated(&t, "cpl_t")
        );
        "cpl_tests"
    } else {
        "NULL"
    };
    let bounds_name = if bounds.is_empty() {
        "NULL"
    } else {
        let items: Vec<String> = bounds
            .iter()
            .map(|(prop, binder, hi, name)| {
                format!("{{ {prop}u, {binder}u, {}u, {name} }}", u32::from(*hi))
            })
            .collect();
        let _ = writeln!(
            prog,
            "static const cplrt_bound cpl_bounds[{}] = {{ {} }};",
            items.len(),
            items.join(", ")
        );
        "cpl_bounds"
    };
    let _ = writeln!(
        prog,
        "const cplrt_code CPLPKG(code) = {{ {ndefs}u, cpl_fns, cpl_preds, {tests_name}, {}u, {bounds_name} }};",
        bounds.len()
    );

    // ---- the tables' type table, then the API unit
    let tt = tables.type_table();
    let mut api = String::new();
    let _ = writeln!(
        api,
        "/* {prefix}_api.c: the C API surface of package \"{}\" (spec/cabi.md Section 5). Generated by cpl; do not edit. */",
        comment_text(&project.manifest.project.name)
    );
    api.push_str(&prologue(&prefix));
    api.push_str("#include \"cpl_rt.h\"\n");
    let _ = writeln!(api, "#include \"{prefix}.h\"");
    for b in &plan.bindings {
        if b.form == cpl_project::AdapterForm::Wrapper {
            let _ = writeln!(api, "#include \"{prefix}_adapter_{}.h\"", b.name);
        }
    }
    // a declarative binding has no adapter header: its generated wrapper defines the same contract (6.2, 6.3)
    for b in &plan.bindings {
        if b.form == cpl_project::AdapterForm::Declarative {
            let base = format!("{prefix}_adapter_{}", b.name);
            let _ = writeln!(
                api,
                "/* [bindings.{}]: defined by {} (6.3) */",
                b.name,
                crate::cbind::wrapper_path(&prefix, &b.name)
            );
            let _ = writeln!(
                api,
                "cpl_status {base}_init(cpl_runtime *rt, void **state, cpl_error **error);"
            );
            let _ = writeln!(api, "void {base}_fini(void *state);");
            for (i, n) in crate::api::adapter_op_names(p, b.capability)
                .iter()
                .enumerate()
            {
                let _ = writeln!(
                    api,
                    "cpl_status {base}_{n}({});",
                    crate::cbind::contract_params(p, b.capability, i).join(", ")
                );
            }
        }
    }
    api.push('\n');
    let _ = writeln!(
        api,
        "CPLRT_EXPORT const cpl_library_info *{prefix}_library_info(void);"
    );
    let _ = writeln!(
        api,
        "CPLRT_EXPORT cpl_status {prefix}_runtime_open(const cpl_runtime_options *options, cpl_runtime **out, cpl_error **error);\n"
    );
    let _ = writeln!(api, "CPLRT_EXPORT const cpl_library_info *{prefix}_library_info(void)\n{{\n    return &CPLPKG(program).info;\n}}\n");
    let _ = writeln!(
        api,
        "CPLRT_EXPORT cpl_status {prefix}_runtime_open(const cpl_runtime_options *options, cpl_runtime **out, cpl_error **error)\n{{\n    return cplrt_runtime_open(&CPLPKG(program), options, out, error);\n}}"
    );
    api.push_str(&crate::api::render_api(p, plan, &prefix, &tt, &models));
    api.push_str(&crate::api::render_grants(p, plan, &prefix));

    // ---- the tables unit, the digests
    let exports_c = crate::api::render_export_table(p, &tt, &models);
    let exports_digest = crate::lockgen::exports_digest(p, plan);
    let grants_c = crate::api::render_grant_table(p, plan, &prefix);
    // Milestone 3: the Python worker's source, embedded when a backend-python binding exists (spec/python.md 6.5)
    let python_worker = if plan.python_bindings.is_empty() {
        None
    } else {
        let exports: Vec<cpl_hir::DefId> = plan.exports.iter().map(|e| e.function).collect();
        let model = crate::python::PyModel::new(p, &project.manifest, &exports);
        Some((model.worker_source(), model.digest.clone()))
    };
    let abi = u32::from(plan.abi);
    let max_call_depth = project.manifest.run.max_call_depth;
    let render_tables = |program_digest: &str| -> String {
        tables.render(
            &tt,
            &UnitInfo {
                prefix: &prefix,
                package: &project.manifest.project.name,
                edition: &project.manifest.project.edition,
                abi,
                release: cfg.release,
                holes,
                runtime_digest: runtime_digest.as_str(),
                program_digest,
                exports_digest: exports_digest.as_str(),
                target: &cfg.target,
                compiler: &cfg.compiler,
                profile: &cfg.profile,
                max_call_depth,
                exports_c: &exports_c,
                nexports: models.len(),
                grants_c: &grants_c,
                ngrants: plan.ngrants(),
                python_worker: python_worker
                    .as_ref()
                    .map(|(s, d)| (s.as_str(), d.as_str())),
                coverage: tables.cov.as_deref(),
            },
        )
    };
    let tables_zero = render_tables(ZERO_DIGEST);
    let api_path = format!("src/{prefix}_api.c");
    let prog_path = format!("src/{prefix}_prog.c");
    let tables_path = format!("src/{prefix}_tables.c");
    // the generated wrappers of the declarative bindings (6.3), in path order between the API unit and the program
    let mut wrappers: Vec<(String, String)> = Vec::new();
    for b in &plan.bindings {
        if b.form == cpl_project::AdapterForm::Declarative {
            let spec = b.cbind.as_ref().ok_or_else(|| {
                vec![Diagnostic::error(
                    cpl_diag::DiagCode("internal_error"),
                    cpl_diag::Category::Internal,
                    format!(
                        "internal error: the declarative binding `{}` was not parsed",
                        b.name
                    ),
                )]
            })?;
            wrappers.push((
                crate::cbind::wrapper_path(&prefix, &b.name),
                crate::cbind::render_wrapper(p, &prefix, plan, b, spec),
            ));
        }
    }
    wrappers.sort();
    let mut units: Vec<(&str, &str)> = vec![(&api_path, &api)];
    units.extend(wrappers.iter().map(|(a, b)| (a.as_str(), b.as_str())));
    units.push((&prog_path, &prog));
    units.push((&tables_path, &tables_zero));
    let program_digest = digest_of(&units);
    let tables_text = render_tables(program_digest.as_str());
    let header = crate::header::render(
        p,
        plan,
        &prefix,
        &project.manifest.project.name,
        &project.manifest.project.edition,
        runtime_digest.as_str(),
        program_digest.as_str(),
        exports_digest.as_str(),
        &cfg.target,
        &models,
    );
    let mut files = vec![
        EmittedFile {
            path: format!("include/{prefix}.h"),
            text: header,
        },
        EmittedFile {
            path: api_path,
            text: api,
        },
        EmittedFile {
            path: prog_path,
            text: prog,
        },
        EmittedFile {
            path: tables_path,
            text: tables_text,
        },
    ];
    for b in &plan.bindings {
        if b.form == cpl_project::AdapterForm::Wrapper {
            files.push(EmittedFile {
                path: format!("include/{prefix}_adapter_{}.h", b.name),
                text: crate::api::render_adapter_header(p, &prefix, b),
            });
        }
    }
    for (path, text) in wrappers {
        files.push(EmittedFile { path, text });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(EmittedPackage {
        files,
        program_digest,
        exports_digest,
        holes,
        tables: TablesModel {
            sites: tables.sites.len(),
            clauses: tables.clauses.len(),
            bsites: tables.bsites.len(),
            refinements: tables.refs.len(),
            types: tt.rows.iter().map(|r| r.text.clone()).collect(),
            site_spans: tables.sites.clone(),
            clause_rows: tables
                .clauses
                .iter()
                .map(|c| (c.text.clone(), c.kind, c.site, c.owner, c.op, c.bsite))
                .collect(),
            hole_rows: tables
                .holes
                .iter()
                .map(|(id, h)| (*id, h.name.clone(), h.expected_type.clone(), h.site))
                .collect(),
            ref_rows: tables
                .refs
                .iter()
                .map(|r| (r.def.0, r.type_name.clone(), r.declared_name.clone()))
                .collect(),
        },
    })
}
