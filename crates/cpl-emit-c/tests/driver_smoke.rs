//! A smoke test of the driver build (toolchain 1.13 `package::driver`) and cpl-driver protocol 1 (cabi.md 3.8).

use cpl_project::{load_project, LoadOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};

#[test]
fn reader_driver_answers_requests() {
    if cpl_rt::testing::find_cc().is_none() {
        println!("skipped: no C compiler");
        return;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/reader");
    let project = load_project(&root, &LoadOptions::default()).expect("loads");
    let checked = cpl_sema::check(&project);
    let plan = checked
        .interop
        .clone()
        .unwrap_or_else(|| cpl_sema::interop::InteropPlan::empty(project.manifest.c_prefix()));
    let cfg = cpl_emit_c::package::DriverConfig {
        cache_dir: Path::new(env!("CARGO_TARGET_TMPDIR")).join("driver_smoke_cache"),
        profile: cpl_emit_c::cc::Profile::Debug,
        sanitize: vec![],
        coverage: false,
        cc: None,
        runtime: cpl_rt::RuntimeSource::Embedded,
        werror: true,
    };
    let built = cpl_emit_c::package::driver(&project, &checked.program, &plan, &cfg)
        .expect("the driver builds");
    let p = &checked.program;
    let f = p
        .defs
        .iter()
        .find(|d| p.display_path(d.id) == "reader.pages.divide_exactly")
        .unwrap()
        .id
        .0;
    let pi = p
        .defs
        .iter()
        .find(|d| p.display_path(d.id) == "reader.pages.paginate_ints")
        .unwrap()
        .id
        .0;
    let mut child = Command::new(&built.path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("the driver starts");
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    println!("banner: {line}");
    let requests = [
        format!(
            r#"{{"op":"call","function":{f},"args":[{{"nodes":[["i","12"]]}},{{"nodes":[["i","4"]]}}],"using":[],"check_arguments":true}}"#
        ),
        format!(
            r#"{{"op":"call","function":{f},"args":[{{"nodes":[["i","12"]]}},{{"nodes":[["i","5"]]}}],"using":[],"check_arguments":true}}"#
        ),
        format!(
            r#"{{"op":"call","function":{pi},"args":[{{"nodes":[["i","1"],["i","2"],["i","3"],["l",0,1,2]]}},{{"nodes":[["i","2"]]}}],"using":[],"check_arguments":true}}"#
        ),
        r#"{"op":"stats"}"#.to_string(),
        r#"{"op":"shutdown"}"#.to_string(),
    ];
    let mut answers = Vec::new();
    for r in &requests {
        writeln!(stdin, "{r}").unwrap();
        stdin.flush().unwrap();
        let mut a = String::new();
        out.read_line(&mut a).unwrap();
        println!("> {r}\n< {a}");
        answers.push(a);
    }
    assert!(child.wait().unwrap().success());
    assert!(answers[0].contains(r#""json":"3""#), "{}", answers[0]);
    assert!(answers[1].contains("contract_violation"), "{}", answers[1]);
    assert!(answers[2].contains(r#"[[1, 2], [3]]"#), "{}", answers[2]);
}
