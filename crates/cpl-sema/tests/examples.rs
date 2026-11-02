//! The acceptance examples against `spec/toolchain.md` 12.3's `cpl check`
//! rows and `spec/semantics.md` Section 11.
//!
//! The `*_declarations_*` tests check the declaration-level checks
//! (collection, resolution, signatures, the resolution walk) and the body
//! typer: no error and no warning on either project. The `*_row` tests are
//! the full rows of every stage (`verification_result` comes from the
//! contract stage, `verification_unsatisfied` from the policy stage, the
//! `hole` and `hole_in_release` diagnostics from the typer), in development
//! and release mode.

mod common;

use common::*;
use cpl_diag::Severity;
use cpl_hir::{DefKind, Ty};
use cpl_sema::{check_with, CheckConfig, CheckResult};

fn check_example(name: &str, release: bool) -> CheckResult {
    let project = load_dir(&example(name));
    check_with(
        &project,
        &CheckConfig {
            release,
            ..CheckConfig::default()
        },
    )
}

/// (severity, code) of every diagnostic, sorted.
fn kinds(result: &CheckResult) -> Vec<(Severity, String)> {
    let mut v: Vec<(Severity, String)> = result
        .diagnostics
        .iter()
        .map(|d| (d.severity, d.code.0.to_string()))
        .collect();
    v.sort();
    v
}

/// The codes the declaration half may leave for the later stages.
const LATER: &[&str] = &[
    "verification_result",
    "verification_unsatisfied",
    "hole",
    "hole_in_release",
];

#[test]
fn reader_declarations_check_clean() {
    let result = check_example("reader", false);
    let early: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| !LATER.contains(&d.code.0))
        .collect();
    assert!(early.is_empty(), "{}", render(&result));
    assert!(!result.has_errors());
    // Every module and declaration is collected.
    let paths: Vec<String> = result
        .program
        .modules()
        .filter(|m| m.path[0] != "std")
        .map(|m| m.path.join("."))
        .collect();
    assert_eq!(paths, ["reader.images", "reader.pages", "tests.pagination"]);
    let paginate = result
        .program
        .defs()
        .find(|d| result.program.display_path(d.id) == "reader.pages.paginate")
        .expect("paginate");
    assert_eq!(paginate.stable_id.as_str(), "pgn_7c2d");
    assert!(!paginate.provisional);
    assert!(paginate.policy.prove);
    let f = paginate.kind.as_fn().expect("a fn");
    assert_eq!(f.generics, ["T"]);
    assert_eq!(
        result.program.display_ty_in(&f.params[0].ty, &f.generics),
        "List<T>"
    );
    assert_eq!(
        result.program.display_ty_in(&f.ret, &f.generics),
        "List<List<T>>"
    );
    assert!(matches!(f.params[1].ty, Ty::Refined(..)));
    assert_eq!(f.ensures.len(), 3);
    assert_eq!(f.ensures[0].text, "flatten(result) == items");
    assert_eq!(
        f.locals[f.result_local.expect("result").0 as usize].name,
        "result"
    );
}

#[test]
fn reader_positive_is_the_generator_positive() {
    // S55, T39: `reader.pages.Positive` and `std.gen.Positive` are one class.
    let result = check_example("reader", false);
    let p = &result.program;
    let find = |path: &str| {
        p.defs()
            .find(|d| p.display_path(d.id) == path)
            .unwrap_or_else(|| panic!("{path}"))
            .id
    };
    let user = find("reader.pages.Positive");
    let gen = find("std.gen.Positive");
    let class = |d| match p.def(d).kind {
        DefKind::Refinement(_) => p
            .refinement_classes
            .iter()
            .position(|c| c.members.contains(&d))
            .expect("a class"),
        _ => panic!("not a refinement"),
    };
    assert_eq!(class(user), class(gen));
    assert_eq!(p.std_defs().positive, gen);
    // gen.positive_int's element type is that class.
    let positive_int = find("std.gen.positive_int");
    let ret = &p.def(positive_int).kind.as_fn().expect("a fn").ret;
    match ret {
        Ty::Gen(inner) => match **inner {
            Ty::Refined(c, named) => {
                assert_eq!(c.0 as usize, class(user));
                assert_eq!(named, gen, "the declaration named at the use site");
            }
            ref other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    }
}

#[test]
fn holes_declarations_check_clean() {
    let result = check_example("holes", false);
    let early: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| !LATER.contains(&d.code.0))
        .collect();
    assert!(early.is_empty(), "{}", render(&result));
    assert!(!result.has_errors());
}

#[test]
fn reader_declarations_release_lock() {
    // The corpus projects carry no cpl.lock: --release warns (T37).
    let result = check_example("reader", true);
    let early: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| !LATER.contains(&d.code.0))
        .map(|d| (d.severity, d.code.0))
        .collect();
    assert_eq!(early, [(Severity::Warning, "project_lock_missing")]);
}

/// Toolchain 12.3: `cpl check` on `examples/reader` exits 0 with
/// `verification_result` (info, unknown) for `paginate`.
#[test]
fn reader_check_row() {
    let result = check_example("reader", false);
    assert!(!result.has_errors(), "{}", render(&result));
    assert_eq!(
        kinds(&result),
        [(Severity::Info, "verification_result".to_string())],
        "{}",
        render(&result)
    );
    assert_eq!(result.verification.len(), 1);
}

/// Toolchain 12.3: `cpl check` on `examples/holes` exits 0 with
/// `verification_result` for `paginate` and two `hole` reports.
#[test]
fn holes_check_row() {
    let result = check_example("holes", false);
    assert!(!result.has_errors(), "{}", render(&result));
    assert_eq!(
        kinds(&result),
        [
            (Severity::Info, "hole".to_string()),
            (Severity::Info, "hole".to_string()),
            (Severity::Info, "verification_result".to_string()),
        ],
        "{}",
        render(&result)
    );
    let names: Vec<&str> = result.holes.iter().map(|h| h.name.as_str()).collect();
    assert_eq!(names, ["build_pages", "summary"]);
}

/// Toolchain 12.3: `cpl check --release` on `examples/reader` exits 1 with
/// exactly one `verification_unsatisfied` error and the warning
/// `project_lock_missing`.
#[test]
fn reader_release_row() {
    let result = check_example("reader", true);
    assert!(result.has_errors());
    let errors_and_warnings: Vec<_> = kinds(&result)
        .into_iter()
        .filter(|(s, _)| *s != Severity::Info)
        .collect();
    assert_eq!(
        errors_and_warnings,
        [
            (Severity::Error, "verification_unsatisfied".to_string()),
            (Severity::Warning, "project_lock_missing".to_string()),
        ],
        "{}",
        render(&result)
    );
    let unsatisfied = result
        .diagnostics
        .iter()
        .find(|d| d.code.0 == "verification_unsatisfied")
        .expect("verification_unsatisfied");
    assert_eq!(
        unsatisfied.qualified_name.as_deref(),
        Some("reader.pages.paginate")
    );
}

/// Toolchain 12.3: `cpl check --release` on `examples/holes` exits 1 with
/// exactly two `hole_in_release` and one `verification_unsatisfied`
/// errors, plus the warning `project_lock_missing`.
#[test]
fn holes_release_row() {
    let result = check_example("holes", true);
    assert!(result.has_errors());
    let errors_and_warnings: Vec<_> = kinds(&result)
        .into_iter()
        .filter(|(s, _)| *s != Severity::Info)
        .collect();
    assert_eq!(
        errors_and_warnings,
        [
            (Severity::Error, "hole_in_release".to_string()),
            (Severity::Error, "hole_in_release".to_string()),
            (Severity::Error, "verification_unsatisfied".to_string()),
            (Severity::Warning, "project_lock_missing".to_string()),
        ],
        "{}",
        render(&result)
    );
}
