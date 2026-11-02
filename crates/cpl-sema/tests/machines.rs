mod common;

use common::check_source;
use cpl_hir::FunctionMode;

const APP: &str = r#"
module m.app

program App {
    count: Int
}

enum Event {
    Add(Int)
    Reset
}

initial fn start() -> App {
    App { count: 0 }
}

machine reduce(state: App, event: Event) -> App {
    match event {
        Add(n) => App { count: state.count + n }
        Reset => App { count: 0 }
    }
}
"#;

fn errors(source: &str) -> Vec<String> {
    check_source(source)
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .map(|d| d.code.0.to_string())
        .collect()
}

#[test]
fn a_typed_program_has_one_pure_root_machine() {
    let checked = check_source(APP);
    assert!(
        checked.diagnostics.iter().all(|d| !d.is_error()),
        "{:#?}",
        checked.diagnostics
    );
    let p = &checked.program;
    let machine = p
        .defs()
        .find(|d| p.function_mode(d.id) == FunctionMode::Machine)
        .unwrap();
    let signature = p.machine_signature(machine.id).unwrap();
    assert!(signature.command.is_none());
    assert_eq!(signature.state, machine.kind.as_fn().unwrap().ret);
}

#[test]
fn pure_calls_cannot_hide_mutation_through_helpers_or_recursion() {
    let source = r#"module m.p
mutate fn write() -> Int { 1 }
fn helper() -> Int { write() }
pure fn read() -> Int { helper() }
pure fn cycle(n: Int) -> Int {
    if n == 0 { read() } else { cycle(n - 1) }
}
"#;
    let checked = check_source(source);
    let failures: Vec<_> = checked
        .diagnostics
        .iter()
        .filter(|d| d.code.0 == "pure_calls_mutate")
        .collect();
    assert!(failures.len() >= 3, "{:#?}", checked.diagnostics);
    assert!(failures.iter().any(|d| d.data.get("call_chain")
        == Some(&serde_json::json!(["m.p.read", "m.p.helper", "m.p.write"]))));
}

#[test]
fn contracts_cannot_call_a_mutator_even_without_external_effects() {
    let source = "module m.p\nmutate fn answer() -> Bool { true }\nmutate fn work() -> Int\n    requires answer()\n{ 1 }\n";
    assert!(errors(source).contains(&"mutate_in_contract".into()));
}

#[test]
fn explicit_purity_rejects_capability_effects() {
    let source = "module m.p\ninterface Clock {\n    fn now() -> Int\n}\npure fn read(using clock: Clock) -> Int\n    effects { clock.now }\n{ clock.now() }\n";
    assert!(errors(source).contains(&"pure_declares_effects".into()));
}

#[test]
fn helpers_cannot_construct_or_update_root_state() {
    for body in ["App { count: 9 }", "App { ..state, count: 9 }"] {
        let source = format!("{APP}\npure fn bypass(state: App) -> App {{ {body} }}\n");
        assert!(
            errors(&source).contains(&"state_construction_outside_machine".into()),
            "{body}"
        );
    }
    let source = format!(
        "{APP}\ninitial fn bypass(state: App) -> App {{ App {{ count: state.count + 1 }} }}\n"
    );
    assert!(errors(&source).contains(&"initial_signature".into()));
}

#[test]
fn machine_signatures_and_event_coverage_are_checked() {
    let bad = APP.replace("event: Event", "event: Int");
    assert!(errors(&bad).contains(&"machine_signature".into()));
    let checked = check_source(&bad);
    for d in checked
        .diagnostics
        .iter()
        .filter(|d| matches!(d.code.0, "machine_signature" | "program_machine_count"))
    {
        assert_eq!(d.facet, Some(cpl_diag::Facet::Signature));
    }
    let missing = APP.replace("        Reset => App { count: 0 }\n", "");
    assert!(errors(&missing).iter().any(|c| c.contains("exhaust")));
    let duplicate = format!(
        "{APP}\nmachine second(state: App, event: Event) -> App {{ reduce(state, event) }}\n"
    );
    assert!(errors(&duplicate).contains(&"program_machine_count".into()));
}

#[test]
fn commands_are_values_outside_the_transition() {
    let source = format!(
        r#"{APP}
struct Step<S, C> {{
    commands: List<C>
    state: S
}}
enum Command {{
    Save(Int)
}}
struct Child {{
    count: Int
}}
machine child(state: Child, event: Event) -> Step<Child, Command> {{
    match event {{
        Add(n) => Step {{ state: Child {{ count: state.count + n }}, commands: [Save(n)] }}
        Reset => Step {{ state: Child {{ count: 0 }}, commands: [] }}
    }}
}}
"#
    );
    let checked = check_source(&source);
    assert!(
        checked.diagnostics.iter().all(|d| !d.is_error()),
        "{:#?}",
        checked.diagnostics
    );
    let p = &checked.program;
    let child = p.defs().find(|d| d.name.display() == "child").unwrap();
    let signature = p.machine_signature(child.id).unwrap();
    assert_eq!(signature.state_field, Some(1));
    assert_eq!(signature.commands_field, Some(0));
    assert!(signature.command.is_some());
}

#[test]
fn deep_mutating_chains_get_an_architectural_warning() {
    let source = "module m.p\nmutate fn top() -> Int { middle() }\nmutate fn middle() -> Int { leaf() }\nmutate fn leaf() -> Int { 1 }\n";
    let checked = check_source(source);
    assert!(checked.diagnostics.iter().all(|d| !d.is_error()));
    assert!(checked
        .diagnostics
        .iter()
        .any(|d| d.code.0 == "mutation_depth"));
}

#[test]
fn machine_holes_report_state_event_and_effect_context() {
    let source = APP.replace("App { count: state.count + n }", "?next");
    let checked = check_source(&source);
    assert!(!checked.has_errors(), "{:?}", checked.diagnostics);
    let hole = checked
        .diagnostics
        .iter()
        .find(|d| d.code.0 == "hole")
        .unwrap();
    assert_eq!(hole.data["function_mode"], "machine");
    assert_eq!(hole.data["machine_state"], "App");
    assert_eq!(hole.data["machine_event"], "Event");
}

#[test]
fn initializers_cannot_launder_existing_roots_through_containers() {
    for ty in ["List<App>", "Option<App>", "Envelope<App>", "Hidden"] {
        let source = format!("{APP}\nstruct Envelope<T> {{ value: T }}\ntype Hidden(App)\ninitial fn bypass(existing: {ty}) -> App {{ start() }}\n");
        assert!(
            errors(&source).contains(&"initial_signature".into()),
            "accepted {ty}"
        );
    }
}
