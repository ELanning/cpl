//! Affine protocol states and role implementations (`spec/evolution.md`, Protocols).
mod common;
use common::{check_source, codes};

const CONNECTION: &str = r#"module m
@protocol("connection", "closed", "initial_terminal")
type Closed(Int)
@protocol("connection", "open", "state")
type Open(Int)
@transition("connection", "closed", "open")
@message("Client", "Server")
pure fn connect(state: Closed) -> Open {
    match state {
        Closed(n) => Open(n)
    }
}
@transition("connection", "open", "open")
@message("Client", "Server")
pure fn send(state: Open, data: Int) -> Open {
    match state {
        Open(n) => Open(n + data)
    }
}
@transition("connection", "open", "closed")
@message("Server", "Client")
pure fn close(state: Open) -> Closed {
    match state {
        Open(n) => Closed(n)
    }
}
pure fn twice<T>(x: T) -> List<T> {
    [x, x]
}
pure fn same<T>(x: T) -> T {
    x
}
"#;

fn check(body: &str) -> Vec<String> {
    codes(&check_source(&format!("{CONNECTION}{body}\n")))
}

#[test]
fn threading_states_through_transitions_is_accepted() {
    let ok = r#"pure fn session(c: Closed) -> Closed {
    let open = connect(c)
    let more = send(open, 1)
    close(same(more))
}
pure fn looped(c: Closed, n: Int) -> Closed {
    var state = connect(c)
    var i = 0
    while i < n {
        state = send(state, i)
        i = i + 1
    }
    close(state)
}
pure fn chosen(c: Closed, flag: Bool) -> Closed {
    let open = connect(c)
    if flag {
        close(send(open, 1))
    } else {
        close(open)
    }
}
test "a session" {
    assert close(connect(Closed(7))) == Closed(7)
}"#;
    assert_eq!(check(ok), Vec::<String>::new());
}

#[test]
fn an_old_state_cannot_be_used_after_its_transition() {
    let reuse = r#"pure fn bad(c: Closed) -> Open {
    let open = connect(c)
    let closed = close(open)
    send(open, 1)
}"#;
    let checked = check_source(&format!("{CONNECTION}{reuse}\n"));
    let d = checked
        .diagnostics
        .iter()
        .find(|d| d.code.0 == "protocol_state_reused")
        .expect("reuse is reported");
    assert!(d.notes[0].message.contains("consumed here"));
    // The closed state, reused after reconnecting, is caught too.
    assert!(check(
        "pure fn bad(c: Closed) -> Open {\n    let a = connect(c)\n    let b = connect(c)\n    a\n}"
    )
    .contains(&"protocol_state_reused".to_string()));
}

#[test]
fn branches_loops_lists_and_duplicating_generics_cannot_copy_states() {
    let branch = "pure fn bad(c: Closed, flag: Bool) -> Open {\n    let open = connect(c)\n    if flag {\n        let x = close(open)\n    } else {\n        let y = 0\n    }\n    open\n}";
    assert!(check(branch).contains(&"protocol_state_reused".to_string()));
    let looped = "pure fn bad(c: Closed, n: Int) -> Int {\n    let open = connect(c)\n    var i = 0\n    while i < n {\n        let closed = close(open)\n        i = i + 1\n    }\n    i\n}";
    assert!(check(looped).contains(&"protocol_state_reused".to_string()));
    let listed = "pure fn bad(c: Closed) -> Int {\n    let xs = [connect(c)]\n    xs.len\n}";
    assert!(check(listed).contains(&"protocol_state_in_list".to_string()));
    let duplicated = "pure fn bad(c: Closed) -> Int {\n    twice(connect(c)).len\n}";
    let codes = check(duplicated);
    assert!(
        codes.contains(&"protocol_state_duplicated".to_string()),
        "{codes:?}"
    );
}

#[test]
fn roles_perform_exactly_their_own_sends() {
    let roles = r#"@role("connection", "Client")
pure fn client(c: Closed) -> Open {
    send(connect(c), 1)
}
@role("connection", "Server")
pure fn server(o: Open) -> Closed {
    close(o)
}"#;
    assert_eq!(check(roles), Vec::<String>::new());
    // A client that closes performs the server's send.
    let violation = r#"@role("connection", "Client")
pure fn client(c: Closed) -> Closed {
    close(send(connect(c), 1))
}"#;
    assert!(check(violation).contains(&"protocol_role_violation".to_string()));
    // A client that never sends data is incomplete.
    let incomplete = r#"@role("connection", "Client")
pure fn client(c: Closed) -> Open {
    connect(c)
}"#;
    assert!(check(incomplete).contains(&"protocol_role_incomplete".to_string()));
    let unknown = "@role(\"connection\", \"Bystander\")\npure fn watcher() -> Int { 1 }";
    assert!(check(unknown).contains(&"protocol_role_unknown".to_string()));
}
