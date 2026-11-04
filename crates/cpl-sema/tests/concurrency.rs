//! `concurrent` blocks (`spec/concurrency.md`): task scoping, safety, and effect-footprint
//! independence, with `@read_only` operations commuting.
mod common;
use common::{check_source, codes};

const STORE: &str = r#"module m
@read_only("get", "list")
interface Store {
    fn get(key: Text) -> Int
    fn list() -> List<Int>
    fn put(key: Text, value: Int) -> Unit
}
mutate fn save(key: Text, using store: Store) -> Unit
    effects { store.put }
{
    store.put(key, 1)
}
"#;

fn check(body: &str, effects: &str) -> Vec<String> {
    let source = format!(
        "{STORE}mutate fn f(using a: Store, using b: Store) -> Int\n    effects {{ {effects} }}\n{{\n{body}\n}}\n"
    );
    let checked = check_source(&source);
    codes(&checked)
}

fn errors(body: &str, effects: &str) -> Vec<String> {
    check(body, effects)
        .into_iter()
        .filter(|c| c != "unused_binding" && c != "unused_capability")
        .collect()
}

#[test]
fn distinct_handles_and_read_only_operations_run_as_independent_tasks() {
    // Exclusive operations on distinct handles do not conflict.
    let ok = "    concurrent {\n        a.put(\"k\", 1)\n        b.put(\"k\", 2)\n    }\n    0";
    assert_eq!(errors(ok, "a.put, b.put"), Vec::<String>::new());
    // Two read-only operations of one handle commute, including the same operation twice.
    let reads = "    concurrent {\n        let x = a.get(\"k\")\n        let y = a.list()\n        let z = a.get(\"j\")\n    }\n    x + y.len + z";
    assert_eq!(errors(reads, "a.get, a.list"), Vec::<String>::new());
}

#[test]
fn an_exclusive_operation_shared_with_a_sibling_conflicts_directly_or_through_calls() {
    let direct =
        "    concurrent {\n        let x = a.get(\"k\")\n        a.put(\"k\", 1)\n    }\n    x";
    assert_eq!(errors(direct, "a.get, a.put"), vec!["concurrent_conflict"]);
    // Two writes conflict even though they are the same operation.
    let writes = "    concurrent {\n        a.put(\"j\", 1)\n        a.put(\"k\", 2)\n    }\n    0";
    assert_eq!(errors(writes, "a.put"), vec!["concurrent_conflict"]);
    // A forwarded handle carries the callee's declared bound into the task's footprint.
    let forwarded = "    concurrent {\n        let x = a.get(\"k\")\n        save(\"k\", using a)\n    }\n    x";
    assert_eq!(
        errors(forwarded, "a.get, a.put"),
        vec!["concurrent_conflict"]
    );
    let elsewhere = "    concurrent {\n        let x = a.get(\"k\")\n        save(\"k\", using b)\n    }\n    x";
    assert_eq!(errors(elsewhere, "a.get, b.put"), Vec::<String>::new());
}

#[test]
fn tasks_cannot_depend_on_their_order() {
    let assign = "    var n = 0\n    concurrent {\n        let x = if true {\n            n = 1\n            a.get(\"k\")\n        } else {\n            0\n        }\n    }\n    x + n";
    assert_eq!(errors(assign, "a.get"), vec!["concurrent_unsafe"]);
    // Assigning a variable declared inside the task is local to it.
    let local = "    concurrent {\n        let x = if true {\n            var m = 0\n            m = a.get(\"k\")\n            m\n        } else {\n            0\n        }\n    }\n    x";
    assert_eq!(errors(local, "a.get"), Vec::<String>::new());
    let ret = "    concurrent {\n        let x = if true {\n            return 1\n        } else {\n            a.get(\"k\")\n        }\n    }\n    x";
    assert!(errors(ret, "a.get").contains(&"concurrent_unsafe".to_string()));
}

#[test]
fn sibling_bindings_are_invisible_and_visible_after_the_block() {
    let sibling =
        "    concurrent {\n        let x = a.get(\"k\")\n        let y = x + 1\n    }\n    y";
    let source = format!(
        "{STORE}mutate fn f(using a: Store, using b: Store) -> Int\n    effects {{ a.get }}\n{{\n{sibling}\n}}\n"
    );
    let checked = check_source(&source);
    let unresolved = checked
        .diagnostics
        .iter()
        .find(|d| d.code.0 == "unresolved_name")
        .expect("a sibling binding is out of scope");
    assert!(unresolved.notes[0].message.contains("another task"));
    let twice = "    concurrent {\n        let x = a.get(\"k\")\n        let x = b.get(\"k\")\n    }\n    x";
    assert!(errors(twice, "a.get, b.get").contains(&"shadowed_binding".to_string()));
    let invalid = "    concurrent {\n        var x = a.get(\"k\")\n    }\n    x";
    assert_eq!(errors(invalid, "a.get"), vec!["concurrent_task_invalid"]);
}

#[test]
fn read_only_annotations_name_operations_of_a_capability() {
    let unknown = check_source(
        "module m\n@read_only(\"fetch\")\ninterface Store {\n    fn get() -> Int\n}\n",
    );
    assert_eq!(codes(&unknown), vec!["read_only_unknown_operation"]);
    let misplaced = check_source("module m\n@read_only(\"get\")\npure fn get() -> Int { 1 }\n");
    assert_eq!(codes(&misplaced), vec!["annotation_misplaced"]);
    let empty = check_source("module m\n@read_only()\ninterface Store {\n    fn get() -> Int\n}\n");
    assert_eq!(codes(&empty), vec!["invalid_annotation_argument"]);
}

#[test]
fn concurrent_is_contextual_and_pure_tasks_need_no_effects() {
    let checked = check_source(
        "module m\npure fn concurrent(n: Int) -> Int { n }\npure fn f(n: Int) -> Int {\n    let concurrent = concurrent(n)\n    concurrent {\n        let a = n + 1\n        let b = concurrent * 2\n    }\n    a + b\n}\n",
    );
    assert!(!checked.has_errors(), "{:#?}", checked.diagnostics);
}
