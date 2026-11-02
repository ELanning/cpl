//! The include set of `cpl inspect --include` (`spec/toolchain.md` 6.2)
//! and query targets (6.1).

use cpl_diag::{StableId, SymbolPath};
use cpl_index::{Include, IncludeSet, Target};

#[test]
fn include_lists_parse_into_sets() {
    let set = IncludeSet::parse("signature,contract,effects,callers").expect("parses");
    assert_eq!(
        set.items.iter().copied().collect::<Vec<_>>(),
        [
            Include::Signature,
            Include::Contract,
            Include::Effects,
            Include::Callers
        ]
    );
    assert!(set.contains(Include::Callers));
    assert!(!set.contains(Include::InterfaceDependencies));
    let all = IncludeSet::parse(
        "changes,tests,obligations,dependencies,callers,docs,implementation,effects,contract,signature,docs",
    )
    .expect("parses");
    assert_eq!(all.items.len(), 10);
}

#[test]
fn include_lists_reject_unknown_and_empty_items() {
    for bad in [
        "",
        "signature,",
        ",signature",
        "signature,,docs",
        "bogus",
        "interface_dependencies",
        "signature, docs",
        "Signature",
        "definition",
    ] {
        let error = IncludeSet::parse(bad).expect_err(bad);
        assert!(
            error.contains("signature, contract, effects"),
            "{bad}: {error}"
        );
    }
    assert_eq!(
        IncludeSet::parse("bogus").expect_err("unknown"),
        "unknown include item `bogus`; expected one of signature, contract, effects, implementation, docs, callers, dependencies, obligations, tests, changes"
    );
}

#[test]
fn the_default_set_implies_interface_dependencies() {
    let set = IncludeSet::default_set();
    assert_eq!(
        set.items.iter().copied().collect::<Vec<_>>(),
        [
            Include::Signature,
            Include::Contract,
            Include::Effects,
            Include::Docs,
            Include::InterfaceDependencies
        ]
    );
    // Naming the four explicitly yields no dependency list.
    let explicit = IncludeSet::parse("signature,contract,effects,docs").expect("parses");
    assert_ne!(explicit, set);
}

#[test]
fn targets_name_ids_and_paths() {
    let by_id = Target::Id(StableId::parse("pgn_7c2d").expect("id"));
    let by_path = Target::Path(SymbolPath::parse("reader.pages.paginate").expect("path"));
    assert_ne!(by_id, by_path);
    assert_eq!(
        by_path,
        Target::Path(SymbolPath::parse("reader.pages.paginate").expect("path"))
    );
}
