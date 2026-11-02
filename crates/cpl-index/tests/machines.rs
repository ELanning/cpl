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
    for (name, status) in [
        ("m.make", ObligationStatus::Unknown),
        ("m.fixed", ObligationStatus::Proved),
    ] {
        let obligations = index.obligations(id(name));
        let invariant = obligations
            .iter()
            .find(|o| o.kind == "constructor_invariant")
            .unwrap();
        assert_eq!(invariant.status, status);
    }
}
