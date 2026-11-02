//! The check driver (`spec/toolchain.md` 1.6, 3.1, 7.2, 11.1, 11.2, T37,
//! T46) and selector resolution (1.6, T42, T43).

mod common;

use common::*;
use cpl_diag::{FacetSelector, FileOrigin, Severity, Span};
use cpl_hir::DefStatus;
use cpl_sema::project::{render_lock, Lock, LockToolchain};
use cpl_sema::{check_with, resolve_selector, CheckConfig, SelectorScope};
use cpl_syntax::ast;
use std::collections::BTreeMap;

fn release() -> CheckConfig {
    CheckConfig {
        release: true,
        ..CheckConfig::default()
    }
}

const SOURCE: &str = "module m.l\n\npub fn f() -> Int {\n    1\n}\n";

/// A lock recording exactly the current tree.
fn fresh_lock(files: &[(&str, &str)]) -> String {
    let project = load_tree(&tree(files));
    let mut sources = BTreeMap::new();
    sources.insert("cpl.toml".to_string(), project.manifest_digest.clone());
    for entry in &project.files {
        if entry.origin != FileOrigin::Std {
            sources.insert(
                project.source_map.file(entry.file).path.clone(),
                entry.digest.clone(),
            );
        }
    }
    render_lock(&Lock {
        version: 1,
        toolchain: LockToolchain {
            cpl: cpl_diag::version::TOOLCHAIN.to_string(),
            edition: cpl_diag::version::EDITION.to_string(),
            std: project.std_digest(),
        },
        sources,
        verification: project.manifest.verification.clone(),
        foreign: Default::default(),
        adapters: Default::default(),
        targets: Default::default(),
    })
}

#[test]
fn release_lock_check() {
    let manifest = manifest("t");
    let files = [("cpl.toml", manifest.as_str()), ("src/m/l.cpl", SOURCE)];
    // Missing: a span-less warning, only under --release (T37).
    let project = load_tree(&tree(&files));
    assert!(check_with(&project, &CheckConfig::default())
        .diagnostics
        .is_empty());
    let result = check_with(&project, &release());
    assert_eq!(
        triples(&result),
        [(0, Severity::Warning, "project_lock_missing".to_string())]
    );
    assert!(!result.has_errors());
    // Fresh: nothing.
    let lock = fresh_lock(&files);
    let locked = [files[0], files[1], ("cpl.lock", lock.as_str())];
    let result = check_with(&load_tree(&tree(&locked)), &release());
    assert!(result.diagnostics.is_empty(), "{}", render(&result));
    // Stale: an edited source.
    let edited = "module m.l\n\npub fn f() -> Int {\n    2\n}\n";
    let stale = [
        files[0],
        ("src/m/l.cpl", edited),
        ("cpl.lock", lock.as_str()),
    ];
    let result = check_with(&load_tree(&tree(&stale)), &release());
    assert_eq!(
        triples(&result),
        [(0, Severity::Error, "project_lock_stale".to_string())]
    );
    let j = json_values(&result);
    assert_eq!(j[0]["differences"], serde_json::json!(["sources"]));
    assert_eq!(j[0]["paths"], serde_json::json!(["src/m/l.cpl"]));
}

#[test]
fn bindings_need_a_public_capability() {
    let manifest = "[project]\nname = \"t\"\nedition = \"0.1\"\n\n[interop]\ntargets = [\"python\"]\n\n[bindings.a]\ncapability = \"m.b.Open\"\nbackend = \"python\"\nadapter = \"x.a\"\n\n[bindings.b]\ncapability = \"m.b.Closed\"\nbackend = \"python\"\nadapter = \"x.b\"\n";
    let source = "module m.b\n\npub capability Open {\n    fn get(k: Text) -> Int\n}\n\ncapability Closed {\n    fn get(k: Text) -> Int\n}\n";
    let result = check_files(&[("cpl.toml", manifest), ("src/m/b.cpl", source)]);
    assert_eq!(
        triples(&result),
        [(
            0,
            Severity::Error,
            "project_binding_unknown_capability".to_string()
        )]
    );
    let j = json_values(&result);
    assert_eq!(j[0]["binding"], serde_json::json!("b"));
}

#[test]
fn deny_warnings_and_sorted_output() {
    let source = "module m.w\n\nuse std.text.{trim}\n\nfn f() -> Int {\n    nope\n}\n\nfn g() -> Int {\n    let unused = 1\n    2\n}\n";
    let result = check_source(source);
    // T46: by file, then span start, then severity, then code.
    assert_eq!(
        triples(&result),
        [
            (3, Severity::Warning, "unused_import".to_string()),
            (6, Severity::Error, "unresolved_name".to_string()),
            (10, Severity::Warning, "unused_binding".to_string()),
        ]
    );
    let f = result
        .program
        .defs()
        .find(|d| result.program.display_path(d.id) == "m.w.f")
        .expect("f");
    assert_eq!(f.status, DefStatus::HasErrors);
    let warnings_only = check_source("module m.w\n\nuse std.text.{trim}\n");
    assert!(!warnings_only.has_errors());
    let denied = check_source_with(
        "module m.w\n\nuse std.text.{trim}\n",
        &CheckConfig {
            warnings_as_errors: true,
            ..CheckConfig::default()
        },
    );
    assert!(denied.has_errors());
    assert_eq!(
        denied.diagnostics[0].severity,
        Severity::Warning,
        "severities are kept"
    );
}

fn selector(segments: &[&str], kind: ast::SelectorKind) -> ast::Selector {
    let span = Span::point(cpl_diag::FileId(0), 0);
    ast::Selector {
        span,
        trivia: Default::default(),
        path: ast::SelectorPath {
            span,
            segments: segments.iter().map(|s| ast::Name::new(*s, span)).collect(),
        },
        kind,
    }
}

#[test]
fn selectors_resolve_through_use_lists_and_namespaces() {
    let reader = check_with(&load_dir(&example("reader")), &CheckConfig::default());
    let p = &reader.program;
    let work = std::fs::read_to_string(example("reader").join("work/pagination.work"))
        .expect("the work file");
    let parsed = cpl_syntax::parse_work(&work, cpl_diag::FileId(99));
    let file = parsed.value.expect("the work file parses");
    let scope = SelectorScope {
        uses: &file.uses,
        module: None,
    };
    let facet = |name: &str| {
        ast::SelectorKind::Facet(ast::Facet {
            span: Span::point(cpl_diag::FileId(0), 0),
            kind: ast::FacetKind::parse(name).expect("a facet"),
        })
    };
    // `paginate.body` through `use reader.pages.{paginate, Positive}`.
    let got =
        resolve_selector(p, &selector(&["paginate"], facet("body")), &scope).expect("resolves");
    assert_eq!(got.len(), 1);
    assert_eq!(p.display_path(got[0].def), "reader.pages.paginate");
    assert_eq!(got[0].facet, FacetSelector::Facet(cpl_diag::Facet::Body));
    let got = resolve_selector(p, &selector(&["Positive"], facet("definition")), &scope)
        .expect("resolves");
    assert_eq!(got[0].facet, FacetSelector::Definition);
    assert_eq!(p.display_path(got[0].def), "reader.pages.Positive");
    // A namespace wildcard covers the module and those below it (T43).
    let got = resolve_selector(
        p,
        &selector(&["tests"], ast::SelectorKind::Namespace),
        &scope,
    )
    .expect("resolves");
    let names: Vec<String> = got.iter().map(|s| p.display_path(s.def)).collect();
    assert_eq!(names.len(), 3, "{names:?}");
    assert!(names.iter().all(|n| n.starts_with("tests.pagination.\"")));
    // Fully qualified, and std members.
    let got = resolve_selector(
        p,
        &selector(
            &["std", "prelude", "List", "take"],
            ast::SelectorKind::Declaration,
        ),
        &scope,
    )
    .expect("resolves");
    assert_eq!(p.display_path(got[0].def), "std.prelude.List.take");
    // An empty namespace is empty, not an error (an `add` may create it).
    let got = resolve_selector(
        p,
        &selector(&["brand", "new"], ast::SelectorKind::Namespace),
        &scope,
    )
    .expect("resolves");
    assert!(got.is_empty());
    let err = resolve_selector(
        p,
        &selector(&["reader", "pages", "nope"], ast::SelectorKind::Declaration),
        &scope,
    )
    .expect_err("does not resolve");
    assert_eq!(err.code, "unresolved_name");
    // The module scope.
    let pages = p
        .module_by_path(&["reader".to_string(), "pages".to_string()])
        .expect("reader.pages");
    let in_module = SelectorScope {
        uses: &[],
        module: Some(pages),
    };
    let got = resolve_selector(
        p,
        &selector(&["clean"], ast::SelectorKind::Declaration),
        &in_module,
    )
    .expect("resolves");
    assert_eq!(p.display_path(got[0].def), "reader.pages.clean");
}

/// Toolchain 11.1: a `.cpl` file reachable from two overlapping `[build]
/// sources` directories is `project_duplicate_file_module` (span-less) and
/// is loaded once, so `check` sees each of its declarations once: no
/// `duplicate_declaration` and no second module (the corpus README's
/// claim that it would duplicate every declaration was wrong).
#[test]
fn a_doubly_reachable_file_is_checked_once() {
    let files = [
        (
            "cpl.toml",
            "[project]\nname = \"t\"\nedition = \"0.1\"\n\n[build]\nsources = [\"src\", \"src/a\"]\n",
        ),
        ("src/a/b.cpl", "module a.b\n\npub fn one() -> Int {\n    1\n}\n"),
    ];
    let result = check_files(&files);
    assert_eq!(
        triples(&result),
        [(
            0,
            Severity::Error,
            "project_duplicate_file_module".to_string()
        )],
        "{}",
        render(&result)
    );
    let p = &result.program;
    let ones: Vec<String> = p
        .defs()
        .filter(|d| d.name.display() == "one")
        .map(|d| p.display_path(d.id))
        .collect();
    assert_eq!(ones, ["a.b.one"]);
}
