//! Adversarial declarations and bodies: the checker terminates without a
//! panic and reports what the rules say, with no cascade of follow-up
//! diagnostics (semantics S62).

mod common;

use common::*;

/// Check `source` and return its codes (the check must not panic).
fn codes_of(source: &str) -> Vec<String> {
    let result = check_source(source);
    let mut c = codes(&result);
    c.sort();
    c
}

#[test]
fn polymorphic_recursion_terminates() {
    let got = codes_of(
        "module m.p\n\npub struct A<T> {\n    a: A<A<T>>\n}\n\npub struct B<T> {\n    c: C<T>\n}\n\npub struct C<T> {\n    b: B<List<T>>\n}\n\npub struct D<T> {\n    d: List<D<D<T>>>\n}\n",
    );
    // A, and the B/C pair, contain themselves; D's `List` breaks containment, but its argument
    // grows (D<D<T>>), so it has no finite set of instantiations (S7)
    assert_eq!(
        got,
        ["infinite_type", "infinite_type", "infinite_type", "polymorphic_recursion"]
    );
}

#[test]
fn refinements_that_mention_themselves() {
    let got = codes_of(
        "module m.r\n\ntype R = List<R> where true\ntype S = Int where check<S>(self).is_ok()\ntype T = Option<U> where true\ntype U = T where true\n",
    );
    // R and the T/U pair are cycles; S only names itself in its predicate.
    assert_eq!(
        got,
        [
            "cyclic_type_definition",
            "cyclic_type_definition",
            "cyclic_type_definition"
        ]
    );
}

#[test]
fn errors_do_not_cascade() {
    let got = codes_of(
        "module m.e\n\nuse nowhere.at.{all}\n\npub struct Bad {\n    x: Missing\n}\n\nfn f(b: Bad, y: Nope<Int>) -> Bad {\n    let z: Missing = all(b)\n    all.thing(z)\n}\n",
    );
    // One diagnostic per mistake: the import, the two unknown types of the
    // signature, and the annotation's; every use of `all` and `z` is
    // error-typed silently.
    assert_eq!(
        got,
        [
            "unresolved_module",
            "unresolved_type",
            "unresolved_type",
            "unresolved_type"
        ]
    );
}

#[test]
fn odd_but_parseable_shapes() {
    let source = "module m.o

pub enum Empty {
}

pub struct Unitlike {}

pub capability Nothing {
}

type Deep = Int where if self > 0 { match self { 1 => true\n _ => { let x = self\n x > 1 } } } else { false }

fn chains() -> Int {
    let a = 1
    let _ = a.b.c.d
    let _ = std.prelude.Option.None
    let _ = m.o.chains
    1
}

fn patterns(x: Option<Option<Int>>) -> Int {
    match x {
        Some(Some(n)) => n
        Some(None) => 0
        Option.None => -1
    }
}

fn holes(n: Int) -> Int {
    let y: Int = ?first
    y + n + ?second
}
";
    let result = check_source(source);
    let got: Vec<String> = codes(&result);
    // `a.b` is a member of an `Int` (unknown), `std.prelude.Option.None`
    // has no expected type (`cannot_infer_type`, as `let _ = None`),
    // `m.o.chains` names a function in value position, and the two holes
    // have expected types (`hole` infos).
    assert_eq!(
        got,
        [
            "unknown_member",
            "cannot_infer_type",
            "not_a_value",
            "hole",
            "hole"
        ],
        "{}",
        render(&result)
    );
}

#[test]
fn std_prelude_and_gen_imports_are_redundant() {
    let result = check_source(
        "module m.g\n\nuse std.gen\nuse std.prelude.{Option}\n\nproperty \"p\"\n    for n in gen.int\n{\n    let o: Option<Int> = Some(n)\n    assert o.is_some()\n}\n",
    );
    assert!(result.diagnostics.is_empty(), "{}", render(&result));
}
