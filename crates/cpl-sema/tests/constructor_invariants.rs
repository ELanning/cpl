mod common;

use common::check_source;
use cpl_hir::{ExprKind, FnBody};

#[test]
fn constant_constructors_are_proved_or_rejected_by_the_checker() {
    for (value, fails) in [("3", false), ("-1", true), ("2 + 4", true)] {
        let source = format!("module m\nstruct Count {{ n: Int }} where self.n >= 0 and self.n <= 5\npure fn make() -> Count {{ Count {{ n: {value} }} }}\n");
        let checked = check_source(&source);
        assert_eq!(checked.has_errors(), fails, "{:#?}", checked.diagnostics);
        if fails {
            assert!(checked
                .diagnostics
                .iter()
                .any(|d| d.code.0 == "constructor_invariant_false"));
        } else {
            let f = checked
                .program
                .defs()
                .find(|d| d.name.display() == "make")
                .unwrap()
                .kind
                .as_fn()
                .unwrap();
            let FnBody::Block(body) = &f.body else {
                panic!("body")
            };
            assert!(matches!(
                body.tail.as_ref().unwrap().kind,
                ExprKind::StructLit {
                    invariant_proven: true,
                    ..
                }
            ));
        }
    }
}

#[test]
fn unknown_proofs_keep_runtime_checks_and_known_faults_are_compile_errors() {
    let source = "module m\nstruct Count { n: Int } where self.n > 0\npure fn make(n: Int) -> Count { Count { n: n } }\n";
    let checked = check_source(source);
    assert!(!checked.has_errors(), "{:#?}", checked.diagnostics);
    let f = checked
        .program
        .defs()
        .find(|d| d.name.display() == "make")
        .unwrap()
        .kind
        .as_fn()
        .unwrap();
    let FnBody::Block(body) = &f.body else {
        panic!("body")
    };
    assert!(matches!(
        body.tail.as_ref().unwrap().kind,
        ExprKind::StructLit {
            invariant_proven: false,
            ..
        }
    ));
    let faulty = check_source("module m\nstruct Count { n: Int } where 10 / self.n > 0\nfn make() -> Count { Count { n: 0 } }\n");
    assert!(
        faulty
            .diagnostics
            .iter()
            .any(|d| d.code.0 == "constant_evaluation_fault"),
        "{:#?}",
        faulty.diagnostics
    );
}

#[test]
fn constructors_cannot_execute_mutation_or_non_boolean_predicates() {
    for source in [
        "module m\nmutate fn yes() -> Bool { true }\nstruct Count { n: Int } where yes()\n",
        "module m\nstruct Count { n: Int } where self.n\n",
        "module m\nstruct Count { n: Int } where missing(self.n)\n",
    ] {
        let checked = check_source(source);
        assert!(checked.has_errors(), "accepted {source}");
    }
}

#[test]
fn generic_nested_and_recursive_predicates_finish_checking() {
    for source in [
        "module m\nstruct Bag<T> { xs: List<T> } where self.xs.len > 0\nfn make() -> Bag<Int> { Bag { xs: [1] } }\n",
        "module m\nstruct Boxed { n: Int } where valid(self)\npure fn valid(x: Boxed) -> Bool { x.n >= 0 }\nfn make(n: Int) -> Boxed { Boxed { n: n } }\n",
    ] {
        let checked = check_source(source);
        assert!(!checked.has_errors(), "{:#?}", checked.diagnostics);
    }
}

#[test]
fn exponential_compile_time_work_is_bounded_and_never_mistaken_for_a_proof() {
    let mut predicate = String::from("if true { let x0 = \"0123456789\"\n");
    for i in 1..40 {
        predicate.push_str(&format!("let x{i} = x{} ++ x{}\n", i - 1, i - 1));
    }
    predicate.push_str("x39.len > 0 } else { false }");
    let source = format!("module m\nstruct Count {{ n: Int }} where {predicate}\nfn make() -> Count {{ Count {{ n: 1 }} }}\n");
    let checked = check_source(&source);
    assert!(!checked.has_errors(), "{:#?}", checked.diagnostics);
    let f = checked
        .program
        .defs()
        .find(|d| d.name.display() == "make")
        .unwrap()
        .kind
        .as_fn()
        .unwrap();
    let FnBody::Block(body) = &f.body else {
        panic!("body")
    };
    assert!(matches!(
        body.tail.as_ref().unwrap().kind,
        ExprKind::StructLit {
            invariant_proven: false,
            ..
        }
    ));
    // Legacy refinements use the same resource limit; they require proof and therefore report unknown.
    let source =
        format!("module m\ntype Count = Int where {predicate}\nfn make() -> Count {{ 1 }}\n");
    let checked = check_source(&source);
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|d| d.code.0 == "cannot_prove_refinement"),
        "{:#?}",
        checked.diagnostics
    );
}
