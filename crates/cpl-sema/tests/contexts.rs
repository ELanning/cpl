//! One state machine per call stack (spec/state-machines.md): the `context` statement, `Context<S>` handles passed
//! as `using` parameters, `dispatch` and `state`, and the checker's rules about where state machines live.

mod common;

use common::check_source;

const APP: &str = r#"
module m.app

program App {
    count: Int
}

enum Event {
    Add(Int)
    Reset
}

enum Command {
    Saved(Int)
}

struct Step<S, C> {
    state: S
    commands: List<C>
}

initial fn start() -> App {
    App { count: 0 }
}

machine reduce(state: App, event: Event) -> Step<App, Command> {
    match event {
        Add(n) => Step { state: App { count: state.count + n }, commands: [Saved(state.count + n)] }
        Reset => Step { state: App { count: 0 }, commands: [] }
    }
}
"#;

fn codes(source: &str) -> Vec<String> {
    check_source(source)
        .diagnostics
        .iter()
        .map(|d| d.code.0.to_string())
        .collect()
}

fn errors(source: &str) -> Vec<String> {
    check_source(source)
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .map(|d| d.code.0.to_string())
        .collect()
}

#[test]
fn a_context_lives_in_one_function_and_is_passed_down() {
    let source = format!(
        r#"{APP}
mutate fn add_all(xs: List<Int>, using app: Context<App>) -> Int
    effects {{ app.dispatch, app.state }}
{{
    var sent = 0
    var i = 0
    while i < xs.len {{
        sent = sent + app.dispatch(Add(xs.get(i).unwrap_or(0))).len
        i = i + 1
    }}
    sent * 1000 + app.state().count
}}

pub mutate fn handle(xs: List<Int>) -> Int {{
    context app = start()
    let total = add_all(xs, using app)
    let _ = app.dispatch(Reset)
    total + app.state().count
}}

pub mutate fn twice(xs: List<Int>) -> Int {{
    handle(xs) + handle(xs)
}}
"#
    );
    let checked = check_source(&source);
    assert!(
        checked.diagnostics.iter().all(|d| !d.is_error()),
        "{:#?}",
        checked.diagnostics
    );
    // a context-only mutator adds no level to `mutation_depth`
    assert!(!codes(&source).contains(&"mutation_depth".into()));
}

#[test]
fn a_second_state_machine_on_one_call_stack_is_an_error() {
    // a callee of a function holding the context starts another, through a helper
    let source = format!(
        r#"{APP}
mutate fn inner() -> Int {{
    context other = start()
    other.state().count
}}
mutate fn middle() -> Int {{ inner() }}
pub mutate fn outer() -> Int {{
    context app = start()
    middle() + app.state().count
}}
"#
    );
    let checked = check_source(&source);
    let nested: Vec<_> = checked
        .diagnostics
        .iter()
        .filter(|d| d.code.0 == "context_nested")
        .collect();
    assert_eq!(nested.len(), 1, "{:#?}", checked.diagnostics);
    assert_eq!(
        nested[0].data.get("call_chain"),
        Some(&serde_json::json!(["m.app.outer", "m.app.middle", "m.app.inner"]))
    );
    // two contexts in one function, a context while holding a caller's, recursion through the creator
    for body in [
        "context a = start()\n    context b = start()\n    a.state().count + b.state().count",
        "again()",
    ] {
        let source = format!(
            "{APP}\npub mutate fn again() -> Int {{\n    context a = start()\n    let n = a.state().count\n    {body}\n}}\n"
        );
        assert!(errors(&source).contains(&"context_nested".into()), "{body}");
    }
    let source = format!(
        "{APP}\nmutate fn take(using app: Context<App>) -> Int\n    effects {{ app.state }}\n{{\n    context b = start()\n    app.state().count + b.state().count\n}}\n"
    );
    assert!(errors(&source).contains(&"context_nested".into()));
}

#[test]
fn reducers_start_no_context_and_only_machines_call_machines() {
    let source = format!(
        "{APP}\ninitial fn bad() -> App {{\n    context app = start()\n    app.state()\n}}\n"
    );
    assert!(errors(&source).contains(&"context_in_reducer".into()));
    // calling the root machine from ordinary code is a scattered state machine
    let source = format!(
        "{APP}\npub fn bypass() -> Int {{\n    reduce(start(), Add(1)).state.count\n}}\n"
    );
    assert!(errors(&source).contains(&"machine_call_outside_machine".into()));
    // a test may call a machine directly
    let source = format!(
        "{APP}\ntest \"a transition\" {{\n    assert reduce(start(), Add(2)).state.count == 2\n}}\n"
    );
    assert!(errors(&source).is_empty(), "{:?}", errors(&source));
}

#[test]
fn a_child_machine_belongs_to_its_root() {
    let child = r#"
struct Counter {
    n: Int
}
machine tick(state: Counter, event: Event) -> Counter {
    match event {
        Add(k) => Counter { n: state.n + k }
        Reset => Counter { n: 0 }
    }
}
"#;
    // reached by no root machine: a warning
    let source = format!("{APP}{child}");
    assert!(codes(&source).contains(&"machine_unrooted".into()));
    assert!(!errors(&source).contains(&"machine_unrooted".into()));
}

#[test]
fn context_handles_are_handles_not_values() {
    // `Context<S>` names a program
    let source = format!(
        "{APP}\nmutate fn f(using c: Context<Int>) -> Int {{ 1 }}\n"
    );
    assert!(errors(&source).contains(&"context_not_program".into()));
    let source = format!("{APP}\npub mutate fn f() -> Int {{\n    context c = 5\n    1\n}}\n");
    assert!(errors(&source).contains(&"context_not_program".into()));
    // stored, compared, returned: S27
    for body in ["let copy = app\n    1", "if app == app { 1 } else { 2 }"] {
        let source = format!("{APP}\npub mutate fn f() -> Int {{\n    context app = start()\n    {body}\n}}\n");
        let found = errors(&source);
        assert!(
            found.contains(&"capability_as_value".into()) || found.contains(&"capability_equality".into()),
            "{body}: {found:?}"
        );
    }
    let source = format!("{APP}\nstruct Holder {{\n    app: Context<App>\n}}\n");
    assert!(errors(&source).contains(&"capability_in_value_position".into()));
    // a received handle's operations are declared effects
    let source = format!(
        "{APP}\nmutate fn peek(using app: Context<App>) -> Int {{ app.state().count }}\n"
    );
    assert!(errors(&source).contains(&"undeclared_effect".into()));
}

#[test]
fn concurrent_tasks_cannot_touch_the_state_machine() {
    let source = format!(
        "{APP}\npub mutate fn f() -> Int {{\n    context app = start()\n    concurrent {{\n        let a = app.state().count\n        let b = 2\n    }}\n    a + b\n}}\n"
    );
    assert!(errors(&source).contains(&"context_in_concurrent".into()));
    // a parallel body is a pure task: it cannot capture the handle
    for body in ["item + app.state().count", "app.dispatch(Add(item)).len"] {
        let source = format!(
            "{APP}\npub fn f(items: List<Int>) -> List<Int> {{\n    context app = start()\n    parallel for item in items: {body}\n}}\n"
        );
        assert!(errors(&source).contains(&"parallel_unsafe".into()), "{body}");
    }
}
