//! The value encodings between the engines (toolchain 12.6, beside the suites): for generated values of every kind, a
//! nest 10^6 deep, and DAGs with sharing, the node-list encoding and decoding, and the C value-JSON writer and reader
//! (the driver's `render` and `from_json` requests, spec/cabi.md 3.8), give structurally equal values with identical
//! renderings.

use cpl_cabi::NativeSession;
use cpl_diag::FileOrigin;
use cpl_eval::testing::sweep::Sweep;
use cpl_eval::{EvalConfig, Value};
use cpl_hir::{Program, Ty};
use cpl_project::{load_project_from_tree, LoadOptions, VirtualTree};
use std::path::Path;

const KINDS: &str = r#"module proto.kinds

pub type Name(Text)
pub type Positive = Int where self > 0

pub struct Point {
    x: Int
    y: Int
}

pub enum Shape {
    Dot
    Line(Point, Point)
    Named(Name)
}

pub struct Bag {
    ints: List<Int>
    texts: List<Text>
    bytes: Bytes
    flag: Bool
    shape: Shape
    maybe: Option<Positive>
    outcome: Result<Unit, Text>
    nested: List<List<Option<Name>>>
}

pub fn keep(b: Bag) -> Bag {
    b
}

pub fn keep_shapes(s: List<Shape>) -> List<Shape> {
    s
}

// every type of the round trips is a parameter type, so a driver's type table holds it (spec/cabi.md 5.3)
pub fn every(a: Int, b: Bool, c: Text, d: Bytes, e: Unit, f: List<Int>, g: Option<Text>, h: Result<Int, Text>,
             i: Name, j: Point, k: Shape, l: Bag, m: List<List<Text>>) -> Int {
    0
}
"#;

fn require_cc() -> bool {
    if cpl_rt::testing::find_cc().is_some() {
        return true;
    }
    if std::env::var("CPL_REQUIRE_CC").is_ok_and(|v| v == "1") {
        panic!("CPL_REQUIRE_CC=1, but there is no C compiler");
    }
    println!("skipped: no C compiler (set CPL_REQUIRE_CC=1 to fail)");
    false
}

/// The types of the round trips: every kind of value.
fn types(p: &Program) -> Vec<Ty> {
    let named = |name: &str| {
        p.lookup(&cpl_diag::SymbolPath::parse(name).unwrap())
            .unwrap()
    };
    let option = |t: Ty| Ty::Adt(p.std_defs.option, vec![t]);
    vec![
        Ty::Int,
        Ty::Bool,
        Ty::Text,
        Ty::Bytes,
        Ty::Adt(p.std_defs.unit, vec![]),
        Ty::list(Ty::Int),
        option(Ty::Text),
        Ty::Adt(p.std_defs.result, vec![Ty::Int, Ty::Text]),
        Ty::Wrapper(named("proto.kinds.Name")),
        Ty::Adt(named("proto.kinds.Point"), vec![]),
        Ty::Adt(named("proto.kinds.Shape"), vec![]),
        Ty::Adt(named("proto.kinds.Bag"), vec![]),
        Ty::list(Ty::list(Ty::Text)),
    ]
}

/// A JSON value as compact text, as `serde_json::to_string` writes it, but iteratively: serde's serializer recurses
/// once per level, which a value nested 10^6 deep does not survive.
fn compact(v: &serde_json::Value) -> String {
    use serde_json::Value as J;
    enum Step<'a> {
        Value(&'a J),
        Text(&'static str),
        Key(&'a str),
    }
    fn string(out: &mut String, s: &str) {
        out.push('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\u{8}' => out.push_str("\\b"),
                '\u{c}' => out.push_str("\\f"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        }
        out.push('"');
    }
    let mut out = String::new();
    let mut work = vec![Step::Value(v)];
    while let Some(step) = work.pop() {
        match step {
            Step::Text(t) => out.push_str(t),
            Step::Key(k) => {
                string(&mut out, k);
                out.push(':');
            }
            Step::Value(J::Array(items)) => {
                out.push('[');
                work.push(Step::Text("]"));
                for (i, x) in items.iter().enumerate().rev() {
                    work.push(Step::Value(x));
                    if i > 0 {
                        work.push(Step::Text(","));
                    }
                }
            }
            Step::Value(J::Object(map)) => {
                out.push('{');
                work.push(Step::Text("}"));
                let entries: Vec<(&String, &J)> = map.iter().collect();
                for (i, (k, x)) in entries.into_iter().enumerate().rev() {
                    work.push(Step::Value(x));
                    work.push(Step::Key(k));
                    if i > 0 {
                        work.push(Step::Text(","));
                    }
                }
            }
            Step::Value(J::String(t)) => string(&mut out, t),
            Step::Value(other) => out.push_str(&other.to_string()),
        }
    }
    out
}

/// Drops a JSON value without recursing (serde's drop recurses once per level).
fn dismantle(v: serde_json::Value) {
    let mut work = vec![v];
    while let Some(x) = work.pop() {
        match x {
            serde_json::Value::Array(items) => work.extend(items),
            serde_json::Value::Object(map) => work.extend(map.into_iter().map(|(_, x)| x)),
            _ => {}
        }
    }
}

/// The violations of one value's round trips.
fn round_trips(
    session: &mut NativeSession<'_>,
    p: &Program,
    v: &Value,
    ty: &Ty,
    typed: bool,
) -> Vec<String> {
    let mut out = Vec::new();
    let tree = cpl_eval::json::value_to_json_untyped(p, v);
    let json = compact(&tree);
    dismantle(tree);
    let text = cpl_eval::json::value_text_untyped(p, v);
    // the node list
    let nodes = cpl_cabi::wire::to_nodes(v);
    match cpl_cabi::wire::from_nodes(p, &nodes) {
        Ok(back) if back.structurally_equal(v) => {}
        Ok(_) => out.push(format!("the node list changes {}", short(&text))),
        Err(e) => out.push(format!(
            "the node list of {} does not decode: {}",
            short(&text),
            e.message
        )),
    }
    // the C writer
    match session.render(v) {
        Ok((back, r)) => {
            if !back.structurally_equal(v) {
                out.push(format!("render changes {}", short(&text)));
            }
            if r.json.as_deref() != Some(json.as_str()) || r.text.as_deref() != Some(text.as_str())
            {
                out.push(format!(
                    "the C renderings of {} differ: json {:?}, text {:?}",
                    short(&text),
                    r.json.as_deref().map(short),
                    r.text.as_deref().map(short)
                ));
            }
        }
        Err(e) => out.push(format!(
            "render of {}: {:?}",
            short(&text),
            e.into_diagnostics()
        )),
    }
    // the C reader, at the value's type
    if typed {
        let type_text = cpl_sema::interop::type_text(p, ty);
        let tree = cpl_eval::value_to_json(p, v, ty);
        let typed_json = compact(&tree);
        dismantle(tree);
        match session.from_json(&type_text, &typed_json) {
            Ok(Ok((back, r))) => {
                if !back.structurally_equal(v) {
                    out.push(format!("from_json at {type_text} changes {}", short(&text)));
                }
                if r.text.as_deref() != Some(text.as_str()) {
                    out.push(format!(
                        "from_json at {type_text}: the C text of {} differs",
                        short(&text)
                    ));
                }
            }
            Ok(Err(d)) => out.push(format!(
                "from_json at {type_text} refuses {}: {}",
                short(&typed_json),
                d.message
            )),
            Err(e) => out.push(format!("from_json: {:?}", e.into_diagnostics())),
        }
    }
    out
}

fn short(s: &str) -> String {
    if s.len() > 120 {
        format!(
            "{}...",
            &s[..s.char_indices().nth(117).map_or(s.len(), |(i, _)| i)]
        )
    } else {
        s.to_string()
    }
}

#[test]
fn values_of_every_kind_round_trip_through_both_encodings() {
    if !require_cc() {
        return;
    }
    // the deep values are built, rendered, compared, and dropped on the evaluator's thread of 1 GiB of stack, as
    // cpl run does its work (the renderers of the evaluator recurse over a value's depth)
    let (count, violations) = cpl_eval::on_eval_stack(run);
    println!("protocol: {count} values round-tripped");
    assert!(
        violations.is_empty(),
        "{} violations:\n{}",
        violations.len(),
        violations.join("\n")
    );
}

fn run() -> (usize, Vec<String>) {
    let tree = VirtualTree {
        files: [
            (
                "cpl.toml".to_string(),
                b"[project]\nname = \"proto\"\nedition = \"0.1\"\n".to_vec(),
            ),
            ("src/proto/kinds.cpl".to_string(), KINDS.as_bytes().to_vec()),
        ]
        .into_iter()
        .collect(),
    };
    let project =
        load_project_from_tree(&tree, FileOrigin::Virtual, &LoadOptions::default()).unwrap();
    let checked = cpl_sema::check(&project);
    assert!(!checked.has_errors(), "{:?}", checked.diagnostics);
    let p = &checked.program;
    let plan = cpl_sema::interop::InteropPlan::empty(project.manifest.c_prefix());
    let dcfg = cpl_emit_c::package::DriverConfig {
        cache_dir: Path::new(env!("CARGO_TARGET_TMPDIR")).join("protocol-cache"),
        profile: cpl_emit_c::cc::Profile::Debug,
        sanitize: vec![],
        coverage: false,
        cc: None,
        runtime: cpl_rt::RuntimeSource::Embedded,
        werror: true,
    };
    let mut session =
        NativeSession::start(p, &project, &plan, &EvalConfig::default(), &dcfg).unwrap();
    session.set_request_timeout(Some(std::time::Duration::from_secs(120)));
    let mut violations = Vec::new();
    let mut count = 0;
    // generated values of every kind
    let mut sweep = Sweep::new(*blake3::hash(b"protocol").as_bytes());
    for ty in types(p) {
        for i in 0..150u32 {
            let v = sweep
                .draw(p, &ty, i.min(100))
                .expect("every kind is drawable");
            violations.extend(round_trips(&mut session, p, &v, &ty, true));
            count += 1;
        }
    }
    // a nest 10^6 deep (no type text spells it, so the reader is not asked)
    let mut deep = Value::Int(1.into());
    for _ in 0..1_000_000 {
        deep = Value::some(p, deep);
    }
    let deep_ty = Ty::Int; // unused: the untyped renderings
    violations.extend(round_trips(&mut session, p, &deep, &deep_ty, false));
    // DAGs with sharing: one buffer referenced many times, at several depths
    let leaf = Value::list((0..1000).map(|n| Value::Int(n.into())).collect());
    let row = Value::list(vec![leaf.clone(); 200]);
    let dag = Value::list(vec![row.clone(), row.clone(), Value::list(vec![row, leaf])]);
    let dag_nodes = cpl_cabi::wire::to_nodes(&dag);
    assert!(
        dag_nodes.nodes.len() < 2000,
        "a shared buffer is sent once: {} nodes",
        dag_nodes.nodes.len()
    );
    violations.extend(round_trips(&mut session, p, &dag, &Ty::Int, false));
    count += 2;
    drop(deep);
    let report = session.shutdown().unwrap();
    if (report.live_objects, report.live_handles) != (0, 0) {
        violations.push(format!("the driver is not clean at shutdown: {report:?}"));
    }
    (count, violations)
}
