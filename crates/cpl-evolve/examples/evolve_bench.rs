//! Timing of the evolution operations on a generated project:
//! `cargo run --release -p cpl-evolve --example evolve_bench [modules] [functions]`.
use cpl_diag::Facet;
use cpl_evolve::{
    change::{prepare, Acknowledgement, Change, ChangeContract, Edit, Operation},
    differential::Differential,
    explore::{explore, ExploreConfig},
    merge::merge,
    query::query,
    verify::{verify, VerifyConfig},
    Snapshot,
};
use cpl_project::{LoadOptions, VirtualTree};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

fn module(m: usize, functions: usize, body: &dyn Fn(usize, usize) -> String) -> String {
    let mut s = format!("module m{m}\n");
    if m > 0 {
        s.push_str(&format!("\nuse m{}.f{}_0\n", m - 1, m - 1));
    }
    for f in 0..functions {
        s.push_str(&format!(
            "\n@id(\"fn_{m}_{f}\")\npub pure fn f{m}_{f}(n: Int) -> Int\n    requires n >= 0\n{{\n{}\n}}\n",
            body(m, f)
        ));
        if f % 5 == 0 {
            s.push_str(&format!(
                "\n@id(\"test_{m}_{f}\")\ntest \"f{m}_{f}\" {{\n    assert f{m}_{f}(3) >= 0\n}}\n"
            ));
        }
    }
    s.push_str(&format!(
        "\n@id(\"prop_{m}\")\nproperty \"m{m} is monotone\"\n    for n in gen.range(0, 50)\n{{\n    assert f{m}_0(n) <= f{m}_0(n + 1)\n}}\n"
    ));
    s
}

fn body(m: usize, f: usize) -> String {
    match (m, f) {
        (_, 0) if m > 0 => format!("    f{}_0(n) + 1", m - 1),
        (_, 0) => "    n + 1".into(),
        _ => format!("    f{m}_{}(n) + {f}", f - 1),
    }
}

fn tree(modules: usize, functions: usize, body: &dyn Fn(usize, usize) -> String) -> VirtualTree {
    let mut files = BTreeMap::from([(
        "cpl.toml".to_string(),
        b"[project]\nname = \"bench\"\nedition = \"0.1\"\n".to_vec(),
    )]);
    for m in 0..modules {
        files.insert(
            format!("src/m{m}.cpl"),
            module(m, functions, body).into_bytes(),
        );
    }
    files.insert("src/machine.cpl".into(), MACHINE.as_bytes().to_vec());
    VirtualTree { files }
}

const MACHINE: &str = r#"module machine

program Grid {
    x: Int
    y: Int
}
    where self.x >= 0 and self.x < 60 and self.y >= 0 and self.y < 60

enum Move {
    Right
    Up
    Back
}

initial fn start() -> Grid {
    Grid { x: 0, y: 0 }
}

machine step(state: Grid, event: Move) -> Grid {
    match event {
        Right => Grid { ..state, x: (state.x + 1) % 60 }
        Up => Grid { ..state, y: (state.y + 1) % 60 }
        Back => Grid { x: (state.x + 59) % 60, y: (state.y + 59) % 60 }
    }
}

pure fn corner(state: Grid) -> Bool {
    state.x == 59 and state.y == 59
}

pure fn bounded(state: Grid) -> Bool {
    state.x + state.y < 200
}
"#;

fn time<T>(label: &str, runs: u32, mut f: impl FnMut() -> T) -> T {
    let mut best = Duration::MAX;
    let mut out = None;
    for _ in 0..runs {
        let start = Instant::now();
        out = Some(f());
        best = best.min(start.elapsed());
    }
    println!("{label:<44} {:>10.2} ms", best.as_secs_f64() * 1000.0);
    out.unwrap()
}

fn main() {
    let args: Vec<usize> = std::env::args()
        .skip(1)
        .filter_map(|a| a.parse().ok())
        .collect();
    let (modules, functions) = (
        args.first().copied().unwrap_or(20),
        args.get(1).copied().unwrap_or(50),
    );
    let options = LoadOptions::default();
    println!("project: {modules} modules x {functions} functions");
    if std::env::var("BENCH_ONLY").as_deref() == Ok("check") {
        let t = tree(modules, functions, &body);
        for _ in 0..20 {
            let project =
                cpl_project::load_project_from_tree(&t, cpl_diag::FileOrigin::Virtual, &options)
                    .unwrap();
            std::hint::black_box(cpl_sema::check(&project));
        }
        return;
    }
    let base = time("snapshot (load + check)", 3, || {
        Snapshot::new(tree(modules, functions, &body), &options).unwrap()
    });
    profile_index(&base);
    time("index", 5, || base.index().facet_map().entries.len());
    time("digest", 5, || base.digest());
    time("query symbols", 3, || {
        query(&base, "symbols", None).unwrap()
    });
    time("query callers of fn_0_0", 5, || {
        query(&base, "callers of fn_0_0", None).unwrap()
    });
    let config = VerifyConfig {
        cases: 50,
        ..VerifyConfig::default()
    };
    time("verify (all tests)", 1, || {
        verify(&base, None, &config).unwrap()
    });
    let target = format!("fn_{}_{}", modules / 2, functions - 1);
    let id = base.resolve(&target).unwrap();
    let hash = base.index().version(id).facets[&Facet::Body].to_string();
    let change = |text: &str, differential: bool| Change {
        name: "bench".into(),
        actor: "bench".into(),
        edits: vec![Edit {
            symbol: target.clone(),
            expect: BTreeMap::from([(Facet::Body, hash.clone())]),
            operation: Operation::ReplaceBody { body: text.into() },
        }],
        reads: vec![],
        contract: ChangeContract {
            differential: if differential {
                vec![Differential {
                    symbol: target.clone(),
                    cases: Some(500),
                    filter: None,
                }]
            } else {
                vec![]
            },
            acknowledge: Vec::<Acknowledgement>::new(),
            ..ChangeContract::default()
        },
        verification: config.clone(),
    };
    let rewritten = format!(
        "{{\n    {} + f{}_{}(n)\n}}",
        functions - 1,
        modules / 2,
        functions - 2
    );
    let candidate = time("prepare (body edit, affected tests)", 1, || {
        prepare(&base, &change(&rewritten, false), None, &options).unwrap()
    });
    assert!(candidate.report.accepted);
    time("prepare + 500 differential cases", 1, || {
        let c = prepare(&base, &change(&rewritten, true), None, &options).unwrap();
        assert!(c.report.accepted, "{:#?}", c.report.behavior);
    });
    let left = Snapshot::new(
        tree(modules, functions, &|m, f| {
            if m == 1 && f == 3 {
                format!("    f{m}_{}(n) + {f} + 0", f - 1)
            } else {
                body(m, f)
            }
        }),
        &options,
    )
    .unwrap();
    let right = Snapshot::new(
        tree(modules, functions, &|m, f| {
            if m == modules - 1 && f == 2 {
                format!("    {f} + f{m}_{}(n)", f - 1)
            } else {
                body(m, f)
            }
        }),
        &options,
    )
    .unwrap();
    let merged = time("merge (two independent edits)", 1, || {
        merge(&base, &left, &right, &config, &options).unwrap()
    });
    assert!(merged.report.accepted, "{:#?}", merged.report.status);
    let explore_config = ExploreConfig {
        depth: 200,
        ..ExploreConfig::default()
    };
    let result = time("explore 3600-state machine", 1, || {
        explore(&base, "machine.step", None, None, &explore_config).unwrap()
    });
    assert_eq!(result.result, "exhaustive", "{}", result.result);
    let with_predicates = ExploreConfig {
        invariants: vec!["machine.bounded".into()],
        goal: Some("machine.corner".into()),
        ..explore_config
    };
    let result = time("explore with invariant and goal", 1, || {
        explore(&base, "machine.step", None, None, &with_predicates).unwrap()
    });
    assert_eq!(result.result, "witness");
}

#[allow(dead_code)]
fn profile_index(s: &Snapshot) {
    time("  IndexData::from_check", 5, || {
        cpl_index::IndexData::from_check(&s.checked, &s.project, s.revision.clone())
    });
    time("  facet_map", 5, || {
        cpl_index::facet_map(&s.project, s.revision.clone())
    });
    let p = &s.checked.program;
    time("  versions (std)", 5, || {
        p.defs()
            .filter(|d| p.module(d.module).origin == cpl_diag::FileOrigin::Std)
            .map(|d| cpl_index::symbol_version(&p.module(d.module).path, p.ast(d.id)))
            .collect::<Vec<_>>()
    });
    time("  module hashes", 5, || {
        p.modules()
            .filter(|m| m.origin != cpl_diag::FileOrigin::Std)
            .map(|m| cpl_index::module_content_hash(&s.project, &m.files))
            .collect::<Vec<_>>()
    });
    time("  receipt inputs", 5, || {
        cpl_index::receipt_inputs(&s.project, s.revision.clone())
    });
    time("  source_digest", 5, || {
        cpl_index::source_digest(&s.project)
    });
    time("  check only", 3, || cpl_sema::check(&s.project));
    time("  load only", 3, || {
        cpl_project::load_project_from_tree(
            &s.tree,
            cpl_diag::FileOrigin::Virtual,
            &LoadOptions::default(),
        )
        .unwrap()
    });
}
