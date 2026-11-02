//! The type helpers: `strip` and `chain` (semantics 3.3), generic
//! instantiation, and the one naming rule of `$type` and `type_name`
//! (toolchain T30, stdlib S44).

use crate::support::{reader, refined};
use cpl_eval::builtins::stub_of;
use cpl_eval::types::{
    bare_name, chain, instantiate, is_prelude, refinement_base, strip, type_name,
};
use cpl_hir::{DefId, Ty};

#[test]
fn strip_and_chain_follow_the_declared_bases() {
    let r = reader();
    let p = &r.p;
    let positive = refined(p, r.positive);
    let small = refined(p, r.small);
    assert_eq!(strip(p, &positive), Ty::Int);
    assert_eq!(strip(p, &small), Ty::Int);
    assert_eq!(
        strip(p, &Ty::list(positive.clone())),
        Ty::list(positive.clone()),
        "top level only"
    );
    assert_eq!(strip(p, &Ty::Text), Ty::Text);
    assert_eq!(chain(p, r.small), vec![r.small, r.positive]);
    assert_eq!(chain(p, r.positive), vec![r.positive]);
    assert_eq!(chain(p, r.page), Vec::<DefId>::new());
    assert_eq!(refinement_base(p, r.small), Some(&positive));
    assert_eq!(refinement_base(p, r.page), None);
    // A refinement id that names nothing strips to an error, never a loop.
    assert_eq!(
        strip(
            p,
            &Ty::Refined(cpl_hir::RefinementClass(0), DefId(u32::MAX))
        ),
        Ty::Error
    );
}

#[test]
fn instantiate_substitutes_parameters_everywhere() {
    let r = reader();
    let std = r.p.std_defs;
    let generic = Ty::result(&std, Ty::list(Ty::Param(0)), Ty::option(&std, Ty::Param(1)));
    assert_eq!(
        instantiate(&generic, &[Ty::Int, Ty::Text]),
        Ty::result(&std, Ty::list(Ty::Int), Ty::option(&std, Ty::Text))
    );
    assert_eq!(instantiate(&Ty::Param(2), &[Ty::Int]), Ty::Param(2));
    assert_eq!(
        instantiate(&Ty::Gen(Box::new(Ty::Param(0))), &[Ty::Bool]),
        Ty::Gen(Box::new(Ty::Bool))
    );
    assert_eq!(instantiate(&Ty::Param(0), &[]), Ty::Param(0));
}

#[test]
fn prelude_names_are_bare_everything_else_qualified() {
    let r = reader();
    let p = &r.p;
    let s = p.std_defs;
    for (def, name) in [
        (s.unit, "Unit"),
        (s.option, "Option"),
        (s.result, "Result"),
        (s.constraint_error, "ConstraintError"),
        (stub_of(p, "int").unwrap(), "Int"),
        (stub_of(p, "list").unwrap(), "List"),
    ] {
        assert!(is_prelude(p, def));
        assert_eq!(type_name(p, def), name);
    }
    for (def, name) in [
        (s.foreign_error, "std.foreign.ForeignError"),
        (s.positive, "std.gen.Positive"),
        (r.page_id, "reader.pages.PageId"),
        (r.small, "m.Small"),
    ] {
        assert!(!is_prelude(p, def));
        assert_eq!(type_name(p, def), name);
    }
    assert_eq!(bare_name(p, r.page_id), "PageId");
    assert_eq!(bare_name(p, s.foreign_error), "ForeignError");
    assert_eq!(
        type_name(p, DefId(u32::MAX)),
        format!("{{def {}}}", u32::MAX)
    );
}
