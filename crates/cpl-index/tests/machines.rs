mod common;
use common::{load_tree, tree, unversioned};
use cpl_diag::{json::ObligationStatus, Facet};
use cpl_index::{Index, Target};

#[test]
fn roles_and_constructor_contracts_are_visible_in_facets_and_proof_obligations() {
    let source = "module m\nstruct Count { n: Int } where valid(self.n)\npure fn valid(n: Int) -> Bool { n >= 0 }\npure fn make(n: Int) -> Count { Count { n: n } }\nstruct Constant { n: Int } where self.n >= 0\npure fn fixed() -> Constant { Constant { n: 1 } }\n";
    let project = load_tree(&tree(&[("src/m.cpl", source)]));
    let checked = cpl_sema::check(&project);
    assert!(!checked.has_errors(), "{:?}", checked.diagnostics);
    let index = Index::build(&checked.program, &project, unversioned(&project));
    let id = |name: &str| index.resolve(&Target::parse(name)).unwrap();
    let module = common::module(&project, "src/m.cpl");
    let path = common::module_path(module);
    let decl = common::decl(module, "Count");
    assert_eq!(
        cpl_index::facet::facet_text(&path, decl, Facet::Contract).as_deref(),
        Some("where valid(self.n)")
    );
    let effect =
        cpl_index::facet::facet_text(&path, common::decl(module, "make"), Facet::Effects).unwrap();
    assert_eq!(effect, "pure fn");
    assert!(index
        .dependencies(id("m.Count"))
        .iter()
        .any(|d| d.def == id("m.valid")));
    // `make(-1)` breaks the invariant; `fixed` is established by evaluation.
    for (name, status) in [
        ("m.make", ObligationStatus::Failed),
        ("m.fixed", ObligationStatus::Proved),
    ] {
        let obligations = index.obligations(id(name));
        let invariant = obligations
            .iter()
            .find(|o| o.kind == "constructor_invariant")
            .unwrap();
        assert_eq!(invariant.status, status);
    }
    let make = index.obligations(id("m.make"));
    let n = &make[0].evidence.as_ref().unwrap()["model"]["n"];
    assert!(n.as_i64().unwrap() < 0, "{n}");
}

#[test]
fn check_and_prove_name_a_failed_obligation_alike() {
    // `next(4)` and `next(5)` are 0, which breaks the postcondition.
    let source = "module m\n@prove\npure fn next(n: Int) -> Int\n    requires n > 0\n    ensures result > 0\n{\n    if n > 3 { n / 2 - 2 } else { n }\n}\n";
    let project = load_tree(&tree(&[("src/m.cpl", source)]));
    let checked = cpl_sema::check(&project);
    assert!(!checked.has_errors(), "{:?}", checked.diagnostics);
    let index = Index::build(&checked.program, &project, unversioned(&project));
    let from_check = checked
        .diagnostics
        .iter()
        .find(|d| d.code.0 == "verification_result")
        .expect("next is @prove");
    let proof = index.prove(&Target::parse("m.next")).unwrap();
    assert_eq!(
        proof.diagnostic.message,
        "next: failed (ensures obligation)"
    );
    assert_eq!(from_check.message, proof.diagnostic.message);
    assert_eq!(from_check.data["obligation"], "m.next#ensures/1");
    assert_eq!(
        proof.diagnostic.data["obligation"],
        from_check.data["obligation"]
    );
    assert_eq!(proof.diagnostic.data["model"], from_check.data["model"]);
    let n = from_check.data["model"]["n"].as_i64().unwrap();
    assert!(n == 4 || n == 5, "{n}");
}
