mod common;
use common::check_source;

#[test]
fn parallel_tasks_require_transitive_purity_and_local_control_flow() {
    for (prefix, body) in [
        ("mutate fn write(n: Int) -> Int { n }", "write(x)"),
        (
            "mutate fn write(n: Int) -> Int { n }\nmutate fn helper(n: Int) -> Int { write(n) }",
            "helper(x)",
        ),
        ("", "if true { a = x; a } else { 0 }"),
        ("", "if true { return [x]; 0 } else { 0 }"),
    ] {
        let source = format!("module m\n{prefix}\nmutate fn f(xs: List<Int>) -> List<Int> {{ var a = 0; parallel for x in xs: {body} }}\n");
        let checked = check_source(&source.replace(';', "\n"));
        assert!(
            checked
                .diagnostics
                .iter()
                .any(|d| d.code.0 == "parallel_unsafe"),
            "{body}: {:#?}",
            checked.diagnostics
        );
    }
}

#[test]
fn parallel_is_contextual_and_types_flow_through_generic_captures() {
    let checked = check_source("module m\nfn parallel() -> Int { 2 }\npure fn map<T>(xs: List<T>, fallback: T) -> List<T> { parallel for x in xs: if true { x } else { fallback } }\n");
    assert!(!checked.has_errors(), "{:#?}", checked.diagnostics);
}

#[test]
fn expected_result_types_reach_parallel_bodies_and_ranges_are_checked() {
    let ok = check_source(
        "module m\npure fn rows(xs: List<Int>) -> List<List<Int>> { parallel for x in xs: [] }\n",
    );
    assert!(!ok.has_errors(), "{:?}", ok.diagnostics);
    let bad = check_source("module m\npure fn f() -> List<Int> { parallel for x in 2: x }\n");
    assert!(bad
        .diagnostics
        .iter()
        .any(|d| d.code.0 == "parallel_range_not_list"));
}
