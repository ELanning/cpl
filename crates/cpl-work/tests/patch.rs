//! Semantic patches (`spec/toolchain.md` 10.5, 5.5, and the "work and
//! patch" row of Section 12): `examples/reader/work/pagination.patch`,
//! resolved with `resolve_patches` against the checked copy of
//! `examples/reader` whose `paginate` body is `{ ?build_pages }` (same
//! module path, so its selectors resolve through `pagination.work`'s `use
//! reader.pages.{paginate, Positive}` to the body of `pgn_7c2d`) and
//! applied to it, reproduces the original file byte for byte; the patch is
//! `patch_stale` against a copy whose body changed in any other way and
//! when its baseline differs; selector resolution (scopes, fully qualified
//! selectors, unknown symbols, namespaces and whole declarations);
//! out-of-scope, preserved-facet, wrong-facet, unknown-symbol, and
//! missing-target cases; line ends, indentation, test bodies; and
//! `patch_syntax` for a submitted file that does not parse.

mod common;

use common::*;
use cpl_diag::{Category, Facet, FileId, StableId};
use cpl_index::{facet_hash, FacetMap};
use cpl_project::VirtualTree;
use cpl_sema::SelectorScope;
use cpl_syntax::ast::DeclKind;
use cpl_work::{
    apply_patch, parse_submitted_patch, resolve_patches, validate_patch, Grant, ResolvedPatch,
};
use serde_json::json;

const PAGES: &str = "src/reader/pages.cpl";
const PATCH: &str = "work/pagination.patch";
const WORK: &str = "work/pagination.work";

fn id(text: &str) -> StableId {
    StableId::parse(text).expect("a stable id")
}

fn pgn() -> StableId {
    id("pgn_7c2d")
}

/// The byte range of `paginate`'s body block in `pages.cpl`.
fn paginate_body(tree: &VirtualTree) -> (usize, usize) {
    let project = load(tree);
    let DeclKind::Fn(f) = &decl(module(&project, PAGES), "paginate").kind else {
        panic!("paginate is a fn");
    };
    let body = f.body.as_ref().expect("paginate has a body");
    (body.span.start as usize, body.span.end as usize)
}

/// `tree` with `paginate`'s body replaced by `block`.
fn with_paginate_body(tree: &VirtualTree, block: &str) -> VirtualTree {
    let (start, end) = paginate_body(tree);
    let text = text(tree, PAGES);
    with_file(
        tree,
        PAGES,
        &format!("{}{block}{}", &text[..start], &text[end..]),
    )
}

/// The copy of `examples/reader` of Section 12: `paginate { ?build_pages }`.
fn hole_tree() -> VirtualTree {
    with_paginate_body(&reader_tree(), "{ ?build_pages }")
}

/// The one patch form of `work/pagination.patch`, resolved with
/// `resolve_patches` against the checked tree in the scope of
/// `pagination.work` (its `use` lines): both selectors are `paginate.body`,
/// the body of `pgn_7c2d`.
fn pagination_patch(tree: &VirtualTree) -> ResolvedPatch {
    let (project, program) = checked(tree);
    let work = work_file(&project, WORK);
    let file = patch_file(&project, PATCH);
    let scope = SelectorScope {
        uses: &work.uses,
        module: None,
    };
    let mut patches = resolve_patches(file, &program, &scope).expect("the patch resolves");
    assert_eq!(patches.len(), 1);
    let patch = patches.remove(0);
    assert_eq!(patch.target, (pgn(), Facet::Body));
    assert_eq!(patch.expect, (pgn(), Facet::Body));
    assert_eq!(patch.body, file.forms[0].body);
    assert_eq!(patch.span, file.forms[0].span);
    patch
}

fn grant(write: &[(StableId, Facet)], preserve: &[(StableId, Facet)]) -> Grant {
    Grant {
        write: write.iter().cloned().collect(),
        preserve: preserve.iter().cloned().collect(),
        ..Grant::default()
    }
}

/// The grant of `pagination.work`'s `implementation` task.
fn implementation_grant() -> Grant {
    grant(
        &[(pgn(), Facet::Body)],
        &[
            (pgn(), Facet::Signature),
            (pgn(), Facet::Contract),
            (pgn(), Facet::Effects),
            (pgn(), Facet::VerificationPolicy),
        ],
    )
}

fn body_hash(map: &FacetMap, target: &StableId) -> cpl_diag::json::FacetHash {
    map.get(target)
        .and_then(|e| e.facet(Facet::Body))
        .expect("a body facet")
        .clone()
}

#[test]
fn pagination_patch_restores_the_reader_byte_for_byte() {
    let original = reader_tree();
    let hole = hole_tree();
    assert!(text(&hole, PAGES).contains(
        "    ensures forall page in result.drop_last(): page.len == size\n{ ?build_pages }\n\npub fn paginate_ints("
    ));
    let project = load(&hole);
    let patch = pagination_patch(&hole);
    let patched = apply_patch(&hole, &project, &patch).expect("the patch applies");
    assert_eq!(text(&patched, PAGES), text(&original, PAGES));
    assert_eq!(patched, original, "every other file is byte-identical");
    // The patch body type-checks in the target's scope (10.5): the patched
    // tree checks without errors, and the hole `?build_pages` is gone.
    let before = cpl_sema::check(&project);
    assert_eq!(before.holes.len(), 1);
    assert_eq!(before.holes[0].name, "build_pages");
    let after = cpl_sema::check(&load(&patched));
    assert!(!after.has_errors(), "{:?}", after.diagnostics);
    assert!(after.holes.is_empty(), "{:?}", after.holes);
}

#[test]
fn pagination_patch_validates_against_its_baseline() {
    let hole = hole_tree();
    let baseline = map_of(&hole);
    let patch = pagination_patch(&hole);
    // At task-level validation the tree being patched is the baseline (5.5).
    let implementation = implementation_grant();
    assert_eq!(
        validate_patch(&patch, &baseline, &baseline, Some(&implementation)),
        vec![]
    );
    assert_eq!(validate_patch(&patch, &baseline, &baseline, None), vec![]);
}

#[test]
fn patched_body_hashes_as_the_patch_body() {
    let hole = hole_tree();
    let baseline = map_of(&hole);
    let patch = pagination_patch(&hole);
    let patched = apply_patch(&hole, &load(&hole), &patch).expect("the patch applies");
    let after = map_of(&patched);
    assert_eq!(body_hash(&after, &pgn()), patch.body_hash());
    assert_eq!(
        body_hash(&after, &pgn()),
        body_hash(&map_of(&reader_tree()), &pgn())
    );
    // `{ ?build_pages }` is a multi-line block in canonical text (5.2).
    assert_eq!(
        body_hash(&baseline, &pgn()),
        facet_hash("{\n    ?build_pages\n}")
    );
    let summary = patch.summary(&baseline).expect("the baseline has the body");
    assert_eq!(summary.target, (pgn(), Facet::Body));
    assert_eq!(summary.expected, body_hash(&baseline, &pgn()));
    assert_eq!(summary.applied, patch.body_hash());
    // Only the body changed: every other facet of paginate is the baseline's.
    let before = baseline.get(&pgn()).expect("paginate");
    let now = after.get(&pgn()).expect("paginate");
    for facet in Facet::ALL {
        if facet != Facet::Body {
            assert_eq!(before.facet(facet), now.facet(facet), "{facet}");
        }
    }
}

#[test]
fn stale_against_a_copy_whose_body_changed_otherwise() {
    let hole = hole_tree();
    let baseline = map_of(&hole);
    let patch = pagination_patch(&hole);
    for other in [
        "{\n    []\n}",
        "{ ?other_pages }",
        "{\n    // different\n    [items]\n}",
    ] {
        let current = map_of(&with_paginate_body(&reader_tree(), other));
        let diagnostics =
            validate_patch(&patch, &baseline, &current, Some(&implementation_grant()));
        assert_eq!(codes(&diagnostics), ["patch_stale"], "{other}");
        let d = &diagnostics[0];
        assert_eq!(d.category, Category::Patch);
        assert_eq!(d.data["expected"], json!(body_hash(&baseline, &pgn())));
        assert_eq!(d.data["actual"], json!(body_hash(&current, &pgn())));
        assert_eq!(d.symbol, Some(pgn()));
        assert_eq!(d.qualified_name.as_deref(), Some("reader.pages.paginate"));
        assert_eq!(d.facet, Some(Facet::Body));
        assert_eq!(d.span, Some(patch.span));
    }
}

#[test]
fn stale_when_the_baseline_hash_differs() {
    // A patch authored against another snapshot (the complete reader) is
    // refused on the hole copy, and the other way round.
    let patch = pagination_patch(&hole_tree());
    let complete = map_of(&reader_tree());
    let hole = map_of(&hole_tree());
    assert_eq!(
        codes(&validate_patch(&patch, &complete, &hole, None)),
        ["patch_stale"]
    );
    assert_eq!(
        codes(&validate_patch(&patch, &hole, &complete, None)),
        ["patch_stale"]
    );
}

#[test]
fn out_of_scope_without_write_authority_over_the_body() {
    let hole = hole_tree();
    let baseline = map_of(&hole);
    let patch = pagination_patch(&hole);
    for grant in [
        Grant::default(),
        grant(&[(pgn(), Facet::Contract)], &[]),
        grant(&[(id("reader.pages.clean"), Facet::Body)], &[]),
    ] {
        let diagnostics = validate_patch(&patch, &baseline, &baseline, Some(&grant));
        assert_eq!(codes(&diagnostics), ["patch_out_of_scope"]);
        assert_eq!(diagnostics[0].symbol, Some(pgn()));
        assert_eq!(diagnostics[0].facet, Some(Facet::Body));
    }
}

#[test]
fn preserved_body_refuses_a_changed_body() {
    let hole = hole_tree();
    let baseline = map_of(&hole);
    let patch = pagination_patch(&hole);
    let preserving = grant(&[(pgn(), Facet::Body)], &[(pgn(), Facet::Body)]);
    let diagnostics = validate_patch(&patch, &baseline, &baseline, Some(&preserving));
    assert_eq!(codes(&diagnostics), ["patch_preserved_facet"]);
    assert_eq!(
        diagnostics[0].data["expected"],
        json!(body_hash(&baseline, &pgn()))
    );
    assert_eq!(diagnostics[0].data["actual"], json!(patch.body_hash()));
    // Preserved and not writable: both facts are reported.
    let preserve_only = grant(&[], &[(pgn(), Facet::Body)]);
    assert_eq!(
        codes(&validate_patch(
            &patch,
            &baseline,
            &baseline,
            Some(&preserve_only)
        )),
        ["patch_out_of_scope", "patch_preserved_facet"]
    );
    // A patch whose body equals the baseline's changes nothing preserved.
    let complete = map_of(&reader_tree());
    assert_eq!(
        validate_patch(&patch, &complete, &complete, Some(&preserving)),
        vec![]
    );
}

#[test]
fn only_body_facets_can_be_patched() {
    let hole = hole_tree();
    let project = load(&hole);
    let baseline = map_of(&hole);
    let patch = pagination_patch(&hole);
    let contract = ResolvedPatch {
        target: (pgn(), Facet::Contract),
        expect: (pgn(), Facet::Contract),
        ..patch.clone()
    };
    let diagnostics = validate_patch(&contract, &baseline, &baseline, None);
    assert_eq!(codes(&diagnostics), ["patch_facet_unsupported"]);
    assert_eq!(diagnostics[0].facet, Some(Facet::Contract));
    let mismatched_facet = ResolvedPatch {
        expect: (pgn(), Facet::Contract),
        ..patch.clone()
    };
    let mismatched_symbol = ResolvedPatch {
        expect: (id("reader.pages.clean"), Facet::Body),
        ..patch.clone()
    };
    let positive = id("reader.pages.Positive");
    let type_body = ResolvedPatch {
        target: (positive.clone(), Facet::Body),
        expect: (positive, Facet::Body),
        ..patch.clone()
    };
    for wrong in [&mismatched_facet, &mismatched_symbol, &type_body] {
        assert_eq!(
            codes(&validate_patch(
                wrong,
                &baseline,
                &baseline,
                Some(&Grant::default())
            )),
            ["patch_facet_unsupported"]
        );
    }
    for wrong in [&contract, &type_body] {
        let error = apply_patch(&hole, &project, wrong).expect_err("not a body");
        assert_eq!(error.code, "patch_facet_unsupported");
        assert_eq!(error.span, Some(patch.span));
    }
}

#[test]
fn unknown_symbol_and_missing_target() {
    let hole = hole_tree();
    let baseline = map_of(&hole);
    let patch = pagination_patch(&hole);
    let unknown = id("fn_aaaaaaaaaaaaaaaa");
    let elsewhere = ResolvedPatch {
        target: (unknown.clone(), Facet::Body),
        expect: (unknown, Facet::Body),
        ..patch.clone()
    };
    let diagnostics = validate_patch(&elsewhere, &baseline, &baseline, None);
    assert_eq!(codes(&diagnostics), ["patch_unknown_symbol"]);
    assert_eq!(diagnostics[0].data["target"], json!("fn_aaaaaaaaaaaaaaaa"));
    let error = apply_patch(&hole, &load(&hole), &elsewhere).expect_err("not declared");
    assert_eq!(error.code, "patch_target_missing");

    // The tree being patched no longer declares pgn_7c2d.
    let renumbered = with_file(
        &hole,
        PAGES,
        &text(&hole, PAGES).replace("@id(\"pgn_7c2d\")", "@id(\"pgn_9z9z\")"),
    );
    let diagnostics = validate_patch(&patch, &baseline, &map_of(&renumbered), None);
    assert_eq!(codes(&diagnostics), ["patch_target_missing"]);
    let error = apply_patch(&renumbered, &load(&renumbered), &patch).expect_err("missing");
    assert_eq!(error.code, "patch_target_missing");
}

#[test]
fn resolution_against_the_complete_reader_gives_the_same_patch() {
    // The reader and its hole-bodied copy declare `paginate` alike: the
    // patch's selectors name the same facet in both.
    let hole = pagination_patch(&hole_tree());
    let complete = pagination_patch(&reader_tree());
    assert_eq!(hole, complete);
}

/// The checked hole copy, its work file's scope, and a patch file parsed
/// from `text` (its spans in no file of the program).
fn resolve_text(text: &str, scoped: bool) -> Result<Vec<ResolvedPatch>, Vec<cpl_diag::Diagnostic>> {
    let (project, program) = checked(&hole_tree());
    let work = work_file(&project, WORK);
    let file = parse_submitted_patch(text, FileId(9999)).expect("the patch parses");
    let uses: &[cpl_syntax::ast::UseDecl] = if scoped { &work.uses } else { &[] };
    resolve_patches(&file, &program, &SelectorScope { uses, module: None })
}

#[test]
fn selectors_resolve_through_the_work_file_or_fully_qualified() {
    let (project, program) = checked(&hole_tree());
    let file = patch_file(&project, PATCH);
    // Without the work file's imports, `paginate.body` names nothing.
    let bare = SelectorScope {
        uses: &[],
        module: None,
    };
    let errors = resolve_patches(file, &program, &bare).expect_err("unresolved");
    assert_eq!(
        codes(&errors),
        ["patch_unknown_symbol", "patch_unknown_symbol"]
    );
    assert_eq!(errors[0].category, Category::Patch);
    assert_eq!(errors[0].span, Some(file.forms[0].target.span));
    assert_eq!(errors[1].span, Some(file.forms[0].expect.base.span));
    assert_eq!(errors[0].data["selector"], json!("paginate.body"));
    assert_eq!(errors[0].notes.len(), 1, "the resolver's explanation");

    // A patch submitted outside any task names its target fully qualified.
    let patches = resolve_text(
        "patch reader.pages.paginate.body\n    expect base(reader.pages.paginate.body)\n{\n    []\n}\n",
        false,
    )
    .expect("resolves");
    assert_eq!(patches.len(), 1);
    assert_eq!(patches[0].target, (pgn(), Facet::Body));
    assert_eq!(patches[0].expect, (pgn(), Facet::Body));

    // Several forms resolve in file order.
    let patches = resolve_text(
        "patch paginate.body\n    expect base(paginate.body)\n{\n    []\n}\n\npatch reader.pages.clean.body\n    expect base(reader.pages.clean.body)\n{\n    page\n}\n",
        true,
    )
    .expect("resolves");
    let targets: Vec<(StableId, Facet)> = patches.iter().map(|p| p.target.clone()).collect();
    assert_eq!(
        targets,
        [
            (pgn(), Facet::Body),
            (id("reader.pages.clean"), Facet::Body)
        ]
    );
}

#[test]
fn selectors_name_one_facet_of_one_declaration() {
    for (text, expected) in [
        (
            "patch paginate\n    expect base(paginate.body)\n{\n    []\n}\n",
            vec!["patch_facet_unsupported"],
        ),
        (
            "patch paginate.definition\n    expect base(paginate.definition)\n{\n    []\n}\n",
            vec!["patch_facet_unsupported", "patch_facet_unsupported"],
        ),
        (
            "patch tests.pagination.*\n    expect base(paginate.body)\n{\n    []\n}\n",
            vec!["patch_facet_unsupported"],
        ),
        (
            "patch paginate.body\n    expect base(nope.body)\n{\n    []\n}\n",
            vec!["patch_unknown_symbol"],
        ),
        (
            "patch reader.pages.nope.body\n    expect base(Positive)\n{\n    []\n}\n",
            vec!["patch_unknown_symbol", "patch_facet_unsupported"],
        ),
    ] {
        let errors = resolve_text(text, true).expect_err(text);
        assert_eq!(codes(&errors), expected, "{text}");
    }
    let whole = resolve_text(
        "patch paginate\n    expect base(paginate.body)\n{\n    []\n}\n",
        true,
    )
    .expect_err("a whole declaration");
    assert_eq!(whole[0].symbol, Some(pgn()));
    assert_eq!(whole[0].data["selector"], json!("paginate"));

    // A facet other than `body` resolves, and `validate_patch` refuses it
    // (D39), as it refuses a declaration that has no body.
    let baseline = map_of(&hole_tree());
    for (text, target) in [
        (
            "patch paginate.contract\n    expect base(paginate.contract)\n{\n    []\n}\n",
            (pgn(), Facet::Contract),
        ),
        (
            "patch Positive.body\n    expect base(Positive.body)\n{\n    []\n}\n",
            (id("reader.pages.Positive"), Facet::Body),
        ),
        (
            "patch paginate.body\n    expect base(paginate.contract)\n{\n    []\n}\n",
            (pgn(), Facet::Body),
        ),
    ] {
        let patches = resolve_text(text, true).expect(text);
        assert_eq!(patches[0].target, target);
        assert_eq!(
            codes(&validate_patch(&patches[0], &baseline, &baseline, None)),
            ["patch_facet_unsupported"],
            "{text}"
        );
    }
}

/// The patch forms of a patch file's text.
fn forms(text: &str) -> Vec<cpl_syntax::ast::PatchForm> {
    parse_submitted_patch(text, FileId(0))
        .expect("the patch parses")
        .forms
}

fn body_patch(target: &str, patch_text: &str) -> ResolvedPatch {
    let form = forms(patch_text).remove(0);
    ResolvedPatch {
        target: (id(target), Facet::Body),
        expect: (id(target), Facet::Body),
        body: form.body,
        span: form.span,
    }
}

#[test]
fn apply_keeps_crlf_line_ends_comments_and_the_rest_of_the_file() {
    let source = "module m\r\n\r\n@id(\"fn_aaaaaaaaaaaaaaaa\")\r\nfn f(x: Int) -> Int {\r\n    x\r\n} // after the body\r\n\r\n// the end\r\n";
    let tree = tree(&[("src/m.cpl", source)]);
    let patch = body_patch(
        "fn_aaaaaaaaaaaaaaaa",
        "patch f.body\n    expect base(f.body)\n{\n    // doubled\n    let y = x + x\n\n    y\n}\n",
    );
    let patched = apply_patch(&tree, &load(&tree), &patch).expect("applies");
    assert_eq!(
        text(&patched, "src/m.cpl"),
        "module m\r\n\r\n@id(\"fn_aaaaaaaaaaaaaaaa\")\r\nfn f(x: Int) -> Int {\r\n    // doubled\r\n    let y = x + x\r\n\r\n    y\r\n} // after the body\r\n\r\n// the end\r\n"
    );
    assert_eq!(patched.files.len(), tree.files.len());
    assert_eq!(patched.files["cpl.toml"], tree.files["cpl.toml"]);
    load(&patched);
}

#[test]
fn apply_reindents_to_the_declaration_level() {
    let source = "module m\n\n  fn g() -> Int\n      requires true\n  {\n      1\n  }\n";
    let tree = tree(&[("src/m.cpl", source)]);
    let patch = body_patch(
        "m.g",
        "patch g.body\n    expect base(g.body)\n{\n    if true {\n        2\n    } else {\n        3\n    }\n}\n",
    );
    let patched = apply_patch(&tree, &load(&tree), &patch).expect("applies");
    assert_eq!(
        text(&patched, "src/m.cpl"),
        "module m\n\n  fn g() -> Int\n      requires true\n  {\n      if true {\n          2\n      } else {\n          3\n      }\n  }\n"
    );
    load(&patched);
}

#[test]
fn apply_replaces_test_and_property_bodies() {
    let source = "module tests.t\n\ntest \"adds\" {\n    assert 1 + 1 == 2\n}\n\nproperty \"commutes\"\n    for a in gen.int\n{\n    assert a + 1 == 1 + a\n}\n";
    let tree = tree(&[("src/tests/t.cpl", source)]);
    let test = body_patch(
        "tests.t.\"adds\"",
        "patch t.body\n    expect base(t.body)\n{\n    assert 2 + 2 == 4\n}\n",
    );
    let patched = apply_patch(&tree, &load(&tree), &test).expect("applies");
    let property = body_patch(
        "tests.t.\"commutes\"",
        "patch t.body\n    expect base(t.body)\n{}\n",
    );
    let patched = apply_patch(&patched, &load(&patched), &property).expect("applies");
    assert_eq!(
        text(&patched, "src/tests/t.cpl"),
        "module tests.t\n\ntest \"adds\" {\n    assert 2 + 2 == 4\n}\n\nproperty \"commutes\"\n    for a in gen.int\n{}\n"
    );
}

#[test]
fn bodiless_function_has_no_block_to_replace() {
    let tree = tree(&[("src/m.cpl", "module m\n\nfn h() -> Int\n")]);
    let patch = body_patch(
        "m.h",
        "patch h.body\n    expect base(h.body)\n{\n    1\n}\n",
    );
    let error = apply_patch(&tree, &load(&tree), &patch).expect_err("no block");
    assert_eq!(error.code, "patch_target_missing");
    assert_eq!(error.qualified_name.as_deref(), Some("m.h"));
}

#[test]
fn apply_needs_the_project_of_the_tree() {
    let hole = hole_tree();
    let patch = pagination_patch(&hole);
    let error = apply_patch(&hole, &load(&reader_tree()), &patch).expect_err("other project");
    assert_eq!(error.code, "internal_error");
    assert_eq!(error.category, Category::Internal);
}

#[test]
fn unparsable_patch_file_is_patch_syntax() {
    let one_line = "patch paginate.body expect base(paginate.body) {\n    pages\n}\n";
    let error = parse_submitted_patch(one_line, FileId(7)).expect_err("a syntax error");
    assert_eq!(error.code, "patch_syntax");
    assert_eq!(error.category, Category::Patch);
    assert_eq!(error.notes.len(), 1);
    assert!(
        error.notes[0].message.starts_with("syntax_error: "),
        "{}",
        error.notes[0].message
    );
    let span = error.span.expect("the parse error's span");
    assert_eq!(span.file, FileId(7));
    assert_eq!(error.notes[0].span, Some(span));
    // The corpus patch parses.
    assert_eq!(forms(&text(&reader_tree(), PATCH)).len(), 1);
}
