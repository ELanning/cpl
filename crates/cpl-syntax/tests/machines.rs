use cpl_diag::FileId;
use cpl_syntax::{format_module, parse_module};

#[test]
fn roles_programs_and_interfaces_format_without_losing_their_meaning() {
    let source = "module m.app\n\n/// root\npub program App {\ncount: Int\n}\npub interface Clock {\nfn now() -> Int\n}\ninitial fn start() -> App { App { count: 0 } }\npure fn value(x: Int) -> Int { x }\nmutate fn execute(x: Int) -> Int { value(x) }\nmachine step(state: App, event: Int) -> App { state }\nfn pure() -> Int { 1 }\n";
    let parsed = parse_module(source, FileId(0));
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let formatted = format_module(&parsed.value.unwrap());
    for spelling in [
        "pub program App",
        "pub interface Clock",
        "initial fn start",
        "pure fn value",
        "mutate fn execute",
        "machine step",
        "fn pure()",
    ] {
        assert!(formatted.contains(spelling), "{formatted}");
    }
    let again = parse_module(&formatted, FileId(0));
    assert!(again.diagnostics.is_empty(), "{:#?}", again.diagnostics);
    assert_eq!(format_module(&again.value.unwrap()), formatted);
}

#[test]
fn malformed_roles_do_not_fall_back_to_ordinary_functions() {
    for declaration in [
        "pure work() -> Int { 1 }",
        "mutate pure fn work() -> Int { 1 }",
        "initial machine work() -> Int { 1 }",
        "program app {}",
        "machine fn work() -> Int { 1 }",
    ] {
        let parsed = parse_module(&format!("module m.p\n{declaration}\n"), FileId(0));
        assert!(parsed.value.is_none(), "{declaration}");
    }
}

#[test]
fn constructor_contracts_keep_comments_and_round_trip() {
    let source = "module m\n/// counter\npub struct Count {\n    n: Int // field\n}\n    // check\n    where self.n >= 0 // bound\n\npure fn value(c: Count) -> Int { c.n }\n";
    let parsed = parse_module(source, FileId(0));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let formatted = format_module(&parsed.value.unwrap());
    for text in [
        "/// counter",
        "// field",
        "// check",
        "// bound",
        "where self.n >= 0",
    ] {
        assert!(formatted.contains(text), "lost {text}: {formatted}");
    }
    assert_eq!(
        format_module(&parse_module(&formatted, FileId(0)).value.unwrap()),
        formatted
    );
}
