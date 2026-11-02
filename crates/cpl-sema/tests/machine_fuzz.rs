mod common;
use cpl_diag::FileId;
use cpl_syntax::{format_module, parse_module};

const SEED: &str = r#"module m
struct Child { value: Int } where self.value >= 0
program App { child: Child }
enum Event {
    Set(Int)
    Reset
}
initial fn start() -> App { App { child: Child { value: 0 } } }
pure fn calculate(n: Int) -> Int { n + 1 }
machine reduce(state: App, event: Event) -> App {
    match event {
        Set(n) => App { child: Child { value: calculate(n) } }
        Reset => start()
    }
}
mutate fn host() -> Int { calculate(1) }
"#;

#[test]
fn seeded_mutation_fuzzes_parsing_formatting_and_semantic_recovery() {
    let inserts = [
        "pure ",
        "mutate ",
        "machine ",
        "program ",
        "initial ",
        "where self.value > 0",
        "?hole",
        "\n",
        "{",
        "}",
        "/*",
        "//",
        "\0",
        "λ",
        "fn ",
        "self",
        "host()",
        "<T>",
        "List<App>",
        "interface ",
    ];
    let mut seed = 0x6ae7_df1d_89e4_83e8_u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed as usize
    };
    let mut checked_count = 0;
    for case in 0..5_000 {
        let mut source = SEED.to_string();
        for _ in 0..1 + next() % 4 {
            let boundaries = source
                .char_indices()
                .map(|(i, _)| i)
                .chain([source.len()])
                .collect::<Vec<_>>();
            let at = 9 + next() % (boundaries.len() - 9);
            let start = boundaries[at];
            if next() % 3 == 0 && at + 1 < boundaries.len() {
                let end = boundaries[(at + 1 + next() % 8).min(boundaries.len() - 1)];
                source.replace_range(start..end, "");
            } else {
                source.insert_str(start, inserts[next() % inserts.len()]);
            }
        }
        let run = std::panic::catch_unwind(|| {
            if let Some(module) = parse_module(&source, FileId(0)).value {
                let formatted = format_module(&module);
                let reparsed = parse_module(&formatted, FileId(0));
                assert!(
                    reparsed.diagnostics.is_empty(),
                    "formatter produced invalid source: {formatted}"
                );
                assert_eq!(format_module(&reparsed.value.unwrap()), formatted);
                common::check_source(&source);
                true
            } else {
                false
            }
        });
        match run {
            Ok(true) => checked_count += 1,
            Ok(false) => {}
            Err(panic) => {
                eprintln!("reproduce case {case}, fixed seed 0x6ae7df1d89e483e8:\n{source}");
                std::panic::resume_unwind(panic);
            }
        }
    }
    assert!(
        checked_count > 100,
        "only {checked_count} mutations reached semantic checking"
    );
    eprintln!("5,000 seeded mutations; {checked_count} reached semantic checking");
}

#[test]
fn generated_recursive_call_graphs_cannot_hide_mutation() {
    for size in 1..=80 {
        let mut source = String::from("module m\nmutate fn effect() -> Int { 0 }\n");
        for i in 0..size {
            source.push_str(&format!(
                "pure fn f{i}(n: Int) -> Int {{ if n == 0 {{ {} }} else {{ f{}(n - 1) }} }}\n",
                if i + 1 == size {
                    "effect()".to_string()
                } else {
                    "0".to_string()
                },
                (i + 1) % size
            ));
        }
        let checked = common::check_source(&source);
        let errors = checked
            .diagnostics
            .iter()
            .filter(|d| d.code.0 == "pure_calls_mutate")
            .count();
        assert!(
            errors >= size,
            "mutation escaped through a cycle of {size} functions: {:#?}",
            checked.diagnostics
        );
        for d in checked
            .diagnostics
            .iter()
            .filter(|d| d.code.0 == "pure_calls_mutate")
        {
            let path = d.data["call_chain"].as_array().unwrap();
            assert!(path.len() <= 67, "unbounded diagnostic path");
            assert_eq!(path.last().unwrap(), "m.effect");
            if path.len() == 67 {
                assert_eq!(d.data["call_chain_truncated"], true);
            }
        }
    }
}
