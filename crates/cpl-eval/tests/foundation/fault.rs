//! Faults as diagnostics: toolchain 1.7.3; semantics 6.7, 9.2, S34, S57.

use crate::support::{json, reader};
use cpl_diag::json::BindingKind;
use cpl_diag::{Category, Facet, Severity, Span, StableId};
use cpl_eval::fault::{binding_json, codes};
use cpl_eval::{AttributedTo, ClauseKind, Fault, FaultKind, Frame, Resource};

#[test]
fn every_kind_has_its_code_and_category() {
    let table = [
        (
            FaultKind::ContractViolation(ClauseKind::Requires),
            "contract_violation",
            Category::Contract,
        ),
        (
            FaultKind::ContractViolation(ClauseKind::Decreases),
            "contract_violation",
            Category::Contract,
        ),
        (
            FaultKind::AssertionFailed,
            "assertion_failed",
            Category::Test,
        ),
        (FaultKind::UnresolvedHole, "unresolved_hole", Category::Hole),
        (
            FaultKind::DivisionByZero,
            "division_by_zero",
            Category::Runtime,
        ),
        (
            FaultKind::RefinementViolation,
            "refinement_violation",
            Category::Runtime,
        ),
        (
            FaultKind::ResourceExhaustion(Resource::Stack),
            "resource_exhaustion",
            Category::Internal,
        ),
        (FaultKind::Internal, "internal_error", Category::Internal),
    ];
    let r = reader();
    for (kind, code, category) in table {
        assert_eq!(kind.code(), code);
        assert_eq!(kind.category(), category);
        let d = Fault::new(kind, "m").to_diagnostic(&r.p);
        assert_eq!(d.code, code);
        assert_eq!(d.category, category);
        assert_eq!(d.severity, Severity::Error);
        assert_eq!(d.message, "m");
    }
    assert_eq!(codes::CONTRACT_VIOLATION, "contract_violation");
}

#[test]
fn clause_kind_resource_and_attribution_spellings() {
    assert_eq!(
        serde_json::to_value(ClauseKind::Requires).unwrap(),
        "requires"
    );
    assert_eq!(
        serde_json::to_value(ClauseKind::Ensures).unwrap(),
        "ensures"
    );
    assert_eq!(
        serde_json::to_value(ClauseKind::Invariant).unwrap(),
        "invariant"
    );
    assert_eq!(
        serde_json::to_value(ClauseKind::Decreases).unwrap(),
        "decreases"
    );
    assert_eq!(serde_json::to_value(Resource::Stack).unwrap(), "stack");
    assert_eq!(serde_json::to_value(Resource::Memory).unwrap(), "memory");
    assert_eq!(serde_json::to_value(Resource::Time).unwrap(), "time");
    assert_eq!(serde_json::to_value(Resource::Steps).unwrap(), "steps");
    assert_eq!(AttributedTo::Binding.as_str(), "binding");
    assert_eq!(
        ClauseKind::Requires.default_attribution(),
        AttributedTo::Caller
    );
    assert_eq!(
        ClauseKind::Ensures.default_attribution(),
        AttributedTo::Body
    );
    assert_eq!(
        ClauseKind::Invariant.default_attribution(),
        AttributedTo::Body
    );
}

#[test]
fn contract_violation_with_an_id_symbol() {
    // An `@id` declaration: symbol is the id, qualified_name its path (S2).
    let r = reader();
    let p = &r.p;
    let clause_span = Span::new(cpl_diag::FileId(5), 3, 9);
    let f = Fault::contract_violation(
        ClauseKind::Ensures,
        AttributedTo::Body,
        "flatten(result) == items",
    )
    .blamed_on(p, r.paginate)
    .with_span(clause_span)
    .with_data("input", json(r#"{"items": [0, 0, 0], "size": 2}"#))
    .with_data("actual", json("[[0, 0]]"));
    assert_eq!(f.kind, FaultKind::ContractViolation(ClauseKind::Ensures));
    assert_eq!(f.message, "ensures clause failed: flatten(result) == items");
    assert_eq!(f.clause(), Some("flatten(result) == items"));
    let d = f.to_diagnostic(p);
    assert_eq!(d.symbol, Some(StableId::parse("pgn_7c2d").unwrap()));
    assert_eq!(d.qualified_name.as_deref(), Some("reader.pages.paginate"));
    assert_eq!(d.facet, Some(Facet::Contract));
    assert_eq!(d.span, Some(clause_span));
    assert_eq!(d.data["clause_kind"], "ensures");
    assert_eq!(d.data["attributed_to"], "body");
    assert_eq!(d.data["clause"], "flatten(result) == items");
    assert_eq!(d.data["status"], "failed");
    assert_eq!(d.data["input"], json(r#"{"items": [0, 0, 0], "size": 2}"#));
    assert_eq!(d.data["actual"], json("[[0, 0]]"));
}

#[test]
fn the_kind_decides_its_own_payload_keys() {
    let r = reader();
    let mut f = Fault::new(FaultKind::ContractViolation(ClauseKind::Requires), "m");
    f.data.insert("clause_kind".into(), json(r#""ensures""#));
    f.data.insert("status".into(), json(r#""tested""#));
    let d = f.to_diagnostic(&r.p);
    assert_eq!(d.data["clause_kind"], "requires");
    assert_eq!(d.data["status"], "failed");
    assert_eq!(d.data["attributed_to"], "caller");
    assert_eq!(d.facet, Some(Facet::Contract));
    // The kind decides the facet too: every contract_violation is `contract`.
    let d = Fault::new(FaultKind::ContractViolation(ClauseKind::Invariant), "m")
        .with_facet(Facet::Body)
        .to_diagnostic(&r.p);
    assert_eq!(d.facet, Some(Facet::Contract));
    assert_eq!(d.data["attributed_to"], "body");

    let mut f = Fault::new(FaultKind::ResourceExhaustion(Resource::Time), "m");
    f.data.insert("limit".into(), json("250"));
    let d = f.to_diagnostic(&r.p);
    assert_eq!(d.data["resource"], "time");
    assert_eq!(d.data["limit"], 250);

    // Reserved keys never reach the payload (T5).
    let mut f = Fault::new(FaultKind::Internal, "m");
    f.data.insert("symbol".into(), json(r#""x""#));
    f.data.insert("code".into(), json(r#""y""#));
    let d = f.to_diagnostic(&r.p);
    assert!(!d.data.contains_key("symbol"));
    assert!(!d.data.contains_key("code"));
    assert_eq!(d.data["kind"], "unspecified");
}

#[test]
fn facet_defaults_and_the_s34_override() {
    let r = reader();
    let p = &r.p;
    // An operator fault blamed on its enclosing declaration: facet body.
    let d = Fault::division_by_zero(&1.into(), &0.into())
        .blamed_on(p, r.take_n)
        .to_diagnostic(p);
    assert_eq!(d.facet, Some(Facet::Body));
    assert_eq!(d.qualified_name.as_deref(), Some("reader.pages.take_n"));
    assert_eq!(
        d.symbol.as_ref().map(StableId::as_str),
        Some("reader.pages.take_n")
    );
    assert_eq!(d.data["input"], json(r#"{"a": 1, "b": 0}"#));
    // The same fault while evaluating a clause (S34): facet contract, clause set.
    let d = Fault::division_by_zero(&10.into(), &0.into())
        .blamed_on(p, r.positive)
        .with_facet(Facet::Contract)
        .with_data("clause", json(r#""10 / self > 0""#))
        .to_diagnostic(p);
    assert_eq!(d.facet, Some(Facet::Contract));
    assert_eq!(d.data["clause"], "10 / self > 0");
    assert_eq!(d.code, "division_by_zero");
    // No symbol: no facet (internal_error names no declaration).
    let d = Fault::internal("unbound_capability", "handle 3").to_diagnostic(p);
    assert_eq!(d.facet, None);
    assert_eq!(d.symbol, None);
    assert_eq!(d.qualified_name, None);
    assert_eq!(d.data["kind"], "unbound_capability");
    assert_eq!(d.data["detail"], "handle 3");
}

#[test]
fn symbols_unknown_to_the_program_keep_their_identity_as_name() {
    // A kind that blames a declaration (toolchain 1.7.3): internal_error
    // names none, so its symbol is never rendered (below).
    let r = reader();
    let id = StableId::parse("std.prelude.List.take").unwrap();
    let d = Fault::new(FaultKind::DivisionByZero, "m")
        .with_symbol(id.clone())
        .to_diagnostic(&r.p);
    assert_eq!(d.qualified_name.as_deref(), Some("std.prelude.List.take"));
    let unknown = StableId::parse("zz_unknown").unwrap();
    let d = Fault::new(FaultKind::DivisionByZero, "m")
        .with_symbol(unknown)
        .to_diagnostic(&r.p);
    assert_eq!(d.qualified_name.as_deref(), Some("zz_unknown"));
    // internal_error has no symbol, hence no facet (semantics 6.7, toolchain
    // 1.7.3), even when the fault carries them.
    let d = Fault::new(FaultKind::Internal, "m")
        .with_symbol(id)
        .with_facet(Facet::Contract)
        .to_diagnostic(&r.p);
    assert_eq!(d.symbol, None);
    assert_eq!(d.qualified_name, None);
    assert_eq!(d.facet, None);
}

#[test]
fn backtrace_frames_render_as_qualified_names() {
    let r = reader();
    let p = &r.p;
    let mut f = Fault::division_by_zero(&1.into(), &0.into()).blamed_on(p, r.take_n);
    let take = p
        .by_stable_id(&StableId::parse("std.prelude.List.take").unwrap())
        .unwrap();
    f.push_frame(Frame {
        def: r.take_n,
        span: None,
    });
    f.push_frame(Frame {
        def: r.paginate,
        span: Some(Span::point(cpl_diag::FileId(5), 0)),
    });
    f.push_frame(Frame {
        def: take,
        span: None,
    });
    f.push_frame(Frame {
        def: cpl_hir::DefId(99_999),
        span: None,
    });
    let d = f.to_diagnostic(p);
    let names: Vec<&str> = d
        .backtrace
        .iter()
        .map(|b| b.qualified_name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "reader.pages.take_n",
            "reader.pages.paginate",
            "std.prelude.List.take",
            "{def 99999}"
        ]
    );
    // The JSON form: [{qualified_name, span?}], innermost first.
    let j = serde_json::to_value(d.to_json(p.source_map())).unwrap();
    assert_eq!(
        j["backtrace"][0],
        json(r#"{"qualified_name": "reader.pages.take_n"}"#)
    );
    assert_eq!(j["backtrace"][1]["qualified_name"], "reader.pages.paginate");
    assert!(j["backtrace"][1]["span"].is_object());
}

#[test]
fn the_json_object_of_a_requires_violation() {
    let r = reader();
    let p = &r.p;
    let d = Fault::contract_violation(ClauseKind::Requires, AttributedTo::Caller, "b != 0")
        .blamed_on(p, r.take_n)
        .with_data("input", json(r#"{"a": 7, "b": 0}"#))
        .to_diagnostic(p);
    let j = serde_json::to_value(d.to_json(p.source_map())).unwrap();
    assert_eq!(
        j,
        json(
            r#"{"code": "contract_violation", "category": "contract", "severity": "error",
                "message": "requires clause failed: b != 0",
                "symbol": "reader.pages.take_n", "qualified_name": "reader.pages.take_n", "facet": "contract",
                "clause_kind": "requires", "attributed_to": "caller", "clause": "b != 0",
                "input": {"a": 7, "b": 0}, "status": "failed"}"#
        )
    );
}

#[test]
fn payloads_of_the_other_faults() {
    let r = reader();
    let p = &r.p;
    let bindings = vec![
        binding_json("b", "Int", BindingKind::Parameter, false, json("0")),
        binding_json("a", "Int", BindingKind::Parameter, false, json("1")),
    ];
    let d = Fault::assertion_failed("b != 0", bindings)
        .blamed_on(p, r.take_n)
        .to_diagnostic(p);
    assert_eq!(d.code, "assertion_failed");
    assert_eq!(d.category, Category::Test);
    assert_eq!(d.facet, Some(Facet::Body));
    assert_eq!(d.message, "assertion failed: b != 0");
    assert_eq!(d.data["clause"], "b != 0");
    assert_eq!(
        d.data["bindings"],
        json(
            r#"[{"name": "b", "type": "Int", "kind": "parameter", "value": 0},
                 {"name": "a", "type": "Int", "kind": "parameter", "value": 1}]"#
        )
    );

    let page = binding_json(
        "page",
        "Page",
        BindingKind::Parameter,
        false,
        json(r#"{"$type": "reader.pages.Page"}"#),
    );
    let d = Fault::unresolved_hole("summary", "Text", vec![page])
        .blamed_on(p, r.take_n)
        .to_diagnostic(p);
    assert_eq!(d.code, "unresolved_hole");
    assert_eq!(d.category, Category::Hole);
    assert_eq!(d.data["hole"], "summary");
    assert_eq!(d.data["expected_type"], "Text");
    assert_eq!(d.data["bindings"][0]["name"], "page");

    let d = Fault::refinement_violation("Positive", json("0"), "self > 0")
        .blamed_on(p, r.take_n)
        .to_diagnostic(p);
    assert_eq!(d.code, "refinement_violation");
    assert_eq!(d.category, Category::Runtime);
    assert_eq!(d.data["expected_type"], "Positive");
    assert_eq!(d.data["actual"], 0);
    assert_eq!(d.data["clause"], "self > 0");

    let d = Fault::resource_exhaustion(Resource::Stack, 10_000)
        .blamed_on(p, r.take_n)
        .to_diagnostic(p);
    assert_eq!(d.code, "resource_exhaustion");
    assert_eq!(d.category, Category::Internal);
    assert_eq!(d.data["resource"], "stack");
    assert_eq!(d.data["limit"], 10_000);
    assert!(d.message.contains("10000"), "{}", d.message);
}

#[test]
fn binding_objects_mark_mutable_bindings() {
    assert_eq!(
        binding_json("pages", "List<List<T>>", BindingKind::Var, true, json("[]")),
        json(
            r#"{"name": "pages", "type": "List<List<T>>", "kind": "var", "mutable": true, "value": []}"#
        )
    );
    assert_eq!(
        binding_json("self", "Int", BindingKind::SelfValue, false, json("1")),
        json(r#"{"name": "self", "type": "Int", "kind": "self", "value": 1}"#)
    );
}

#[test]
fn display_and_error() {
    let f = Fault::division_by_zero(&1.into(), &0.into());
    assert_eq!(f.to_string(), "division_by_zero: division by zero");
    let e: &dyn std::error::Error = &f;
    assert!(e.source().is_none());
}
