mod common;
use cpl_diag::FileId;
use cpl_syntax::{format_module, parse_module};

const SEED: &str = r#"module m
pure fn calc(n: Int) -> Int { n * n }
pure fn map(xs: List<Int>, bias: Int) -> List<Int> {
    parallel for x in xs: calc(x) + bias
}
pure fn nested(xs: List<Int>) -> List<List<Int>> {
    parallel for x in xs: (parallel for y in [1, 2]: x + y)
}
"#;

#[test]
fn seeded_mutation_fuzzes_parsing_formatting_and_semantic_recovery() {
    let inserts = [
        "parallel for ",
        " in ",
        ": ",
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
        checked_count > 30,
        "only {checked_count} mutations reached semantic checking"
    );
    eprintln!("5,000 seeded mutations; {checked_count} reached semantic checking");
}
