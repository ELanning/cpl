//! Collection (`spec/toolchain.md` 3.1, 3.6, 3.7, 4.3-4.5;
//! `spec/semantics.md` 1.2-1.4, 1.8 annotations, S2, S3, S6; stdlib 10.2,
//! 10.3): the module graph, the definition table in T15 order, names and
//! identities, the annotation vocabulary, and the standard library.

mod common;

use common::*;
use cpl_diag::{DeclName, Facet, FileId, FileOrigin, Severity, StableId, SymbolPath};
use cpl_hir::{DefKind, DefStatus, FnBody, ModuleId, Ty};
use cpl_sema::project::{load_project_from_tree, LoadOptions, StdSource};
use cpl_sema::{check, check_with, CheckConfig};
use std::fs;
use std::path::PathBuf;

#[test]
fn std_collects_without_diagnostics() {
    let result = check_source("module m.empty\n");
    assert!(result.diagnostics.is_empty(), "{}", render(&result));
    let p = &result.program;
    // The five std modules come first, in FileId order (T15).
    let std_paths: Vec<String> = p
        .modules()
        .filter(|m| m.origin == FileOrigin::Std)
        .map(|m| m.path.join("."))
        .collect();
    assert_eq!(
        std_paths,
        [
            "std.bytes",
            "std.foreign",
            "std.gen",
            "std.prelude",
            "std.text"
        ]
    );
    assert!(p.defs().all(|d| d.status == DefStatus::Ok));
    // Every std function is a @builtin stub with its registry id.
    for d in p
        .defs()
        .filter(|d| p.module(d.module).origin == FileOrigin::Std)
    {
        if let DefKind::Fn(f) = &d.kind {
            assert!(
                matches!(f.body, FnBody::Builtin(_)),
                "{}",
                p.display_path(d.id)
            );
        }
        assert!(d.provisional, "std identities are provisional (stdlib S43)");
        assert_eq!(d.stable_id.as_str(), p.display_path(d.id));
    }
}

#[test]
fn defs_are_in_t15_order_and_modules_group_files() {
    let result = check_files(&[
        ("src/a/b.cpl", "/// First file docs.\nmodule a.b\n\nfn one() -> Int {\n    1\n}\n\nfn two() -> Int {\n    2\n}\n"),
        ("src/a/b/more.cpl", "/// Second file docs.\nmodule a.b\n\nfn three() -> Int {\n    one()\n}\n"),
        ("src/c.cpl", "module c\n\npub type Id(Text)\n"),
    ]);
    assert!(result.diagnostics.is_empty(), "{}", render(&result));
    let p = &result.program;
    for (i, d) in p.defs().enumerate() {
        assert_eq!(d.id.0 as usize, i);
    }
    let order: Vec<(FileId, usize)> = p.defs().map(|d| (d.file, d.decl_index)).collect();
    let mut sorted = order.clone();
    sorted.sort();
    assert_eq!(order, sorted, "DefIds follow (FileId, declaration) order");
    let ab = p
        .module_by_path(&["a".to_string(), "b".to_string()])
        .expect("module a.b");
    let m = p.module(ab);
    assert_eq!(m.files.len(), 2);
    let names: Vec<String> = m.defs.iter().map(|d| p.def(*d).name.display()).collect();
    assert_eq!(names, ["one", "two", "three"]);
    assert_eq!(
        m.docs.as_deref(),
        Some("First file docs.\n\nSecond file docs.")
    );
    assert_eq!(p.display_path(m.defs[2]), "a.b.three");
    assert!(m.id > ModuleId(4), "project modules follow std");
}

#[test]
fn names_identities_and_member_stubs() {
    let result = check_source(
        "module m.ids\n\n@id(\"fn_abcdefghijklmnop\")\npub fn f() -> Int {\n    1\n}\n\ntest \"a test\" {\n    assert true\n}\n",
    );
    assert!(result.diagnostics.is_empty(), "{}", render(&result));
    let p = &result.program;
    let f = p
        .by_stable_id(&StableId::parse("fn_abcdefghijklmnop").expect("an id"))
        .expect("f");
    assert!(!p.def(f).provisional);
    assert_eq!(p.display_path(f), "m.ids.f");
    let t = p
        .lookup(&SymbolPath::new(
            vec!["m".into(), "ids".into()],
            DeclName::Text("a test".into()),
        ))
        .expect("the test");
    assert_eq!(p.def(t).stable_id.as_str(), "m.ids.\"a test\"");
    // std member stubs: `std.prelude.List.take` (stdlib S37).
    let take = p
        .lookup(&SymbolPath::parse("std.prelude.List.take").expect("a path"))
        .expect("List.take");
    assert_eq!(
        p.def(take).name,
        DeclName::Member {
            receiver: "List".into(),
            name: "take".into()
        }
    );
    assert!(
        matches!(&p.def(take).kind, DefKind::Fn(f) if f.body == FnBody::Builtin("list_take".into()))
    );
    // Builtin types and StdDefs.
    let int = p
        .lookup(&SymbolPath::parse("std.prelude.Int").expect("a path"))
        .expect("Int");
    assert!(matches!(&p.def(int).kind, DefKind::BuiltinType(b) if b.builtin == "int"));
    let std = p.std_defs();
    assert_eq!(p.display_path(std.unit), "std.prelude.Unit");
    assert_eq!(p.display_path(std.option), "std.prelude.Option");
    assert_eq!(p.display_path(std.result), "std.prelude.Result");
    assert_eq!(
        p.display_path(std.constraint_error),
        "std.prelude.ConstraintError"
    );
    assert_eq!(
        p.display_path(std.foreign_error),
        "std.foreign.ForeignError"
    );
    assert_eq!(p.display_path(std.positive), "std.gen.Positive");
}

#[test]
fn verification_policy_and_docs() {
    let result =
        check_source("module m.p\n\n/// Docs of f.\n@prove\nfn f(n: Int) -> Int {\n    n\n}\n");
    let p = &result.program;
    let f = p
        .lookup(&SymbolPath::parse("m.p.f").expect("a path"))
        .expect("f");
    assert!(p.def(f).policy.prove);
    assert_eq!(p.def(f).docs.as_deref(), Some("Docs of f."));
}

/// The repeated `@id` is one `duplicate_identity` at the later
/// declaration's name token (semantics S63 (3) lists the code among the
/// declaration-level diagnostics; (4) picks the later occurrence), line 9
/// here, with a note at the earlier declaration.
#[test]
fn duplicate_identity_keeps_the_later_definition_provisional() {
    let result = check_source(
        "module m.d\n\n@id(\"same_id\")\nfn a() -> Int {\n    1\n}\n\n@id(\"same_id\")\nfn b() -> Int {\n    2\n}\n",
    );
    assert_eq!(
        project_triples(&result),
        [(9, Severity::Error, "duplicate_identity".to_string())]
    );
    let d = &result.diagnostics[0];
    assert_eq!(d.symbol.as_ref().map(StableId::as_str), Some("same_id"));
    assert_eq!(d.qualified_name.as_deref(), Some("m.d.b"));
    assert_eq!(d.facet, Some(Facet::Identity));
    assert_eq!(d.notes.len(), 1);
    let p = &result.program;
    let a = p
        .lookup(&SymbolPath::parse("m.d.a").expect("a path"))
        .expect("a");
    let b = p
        .lookup(&SymbolPath::parse("m.d.b").expect("a path"))
        .expect("b");
    assert_eq!(p.def(a).stable_id.as_str(), "same_id");
    assert_eq!(p.def(b).stable_id.as_str(), "m.d.b");
    assert!(p.def(b).provisional);
    assert_eq!(p.def(b).status, DefStatus::HasErrors);
    assert_eq!(
        p.by_stable_id(&StableId::parse("same_id").expect("an id")),
        Some(a)
    );
}

#[test]
fn id_kind_mismatch_and_id_missing() {
    let source = "module m.k\n\n@id(\"ty_abcdefghijklmnop\")\npub fn f() -> Int {\n    1\n}\n\npub fn g() -> Int {\n    2\n}\n";
    let result = check_source(source);
    assert_eq!(
        project_triples(&result),
        [(3, Severity::Warning, "id_kind_mismatch".to_string())]
    );
    let lint = CheckConfig {
        lints: ["id_missing".to_string()].into_iter().collect(),
        ..CheckConfig::default()
    };
    let result = check_source_with(source, &lint);
    assert_eq!(
        project_triples(&result),
        [
            (3, Severity::Warning, "id_kind_mismatch".to_string()),
            (8, Severity::Warning, "id_missing".to_string()),
        ]
    );
}

#[test]
fn annotation_rules_in_project_files() {
    let result = check_source(
        "module m.a\n\n@prove()\nfn f() -> Int {\n    1\n}\n\n@property\nfn g(xs: List<Int>) -> Int {\n    xs.len\n}\n\n@prove(1)\nfn h() -> Int {\n    1\n}\n",
    );
    // `f`'s `@prove()` is a valid `@prove`, so `f` has a verification
    // status (S36); `h`'s invalid `@prove(1)` contributes nothing.
    assert_eq!(
        project_triples(&result),
        [
            (4, Severity::Info, "verification_result".to_string()),
            (8, Severity::Error, "unknown_annotation".to_string()),
            (
                13,
                Severity::Error,
                "invalid_annotation_argument".to_string()
            ),
        ]
    );
    let f = result
        .program
        .lookup(&SymbolPath::parse("m.a.f").expect("a path"))
        .expect("f");
    assert!(
        result.program.def(f).policy.prove,
        "`@prove()` takes no argument"
    );
}

/// A copy of the embedded std with `extra` appended to `prelude.cpl`, under
/// a unique temporary directory.
fn std_with(name: &str, extra: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("cpl-sema-std")
        .join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).expect("remove an old directory");
    }
    fs::create_dir_all(&dir).expect("create a directory");
    for (file, text) in cpl_sema::project::STD_FILES {
        let text = if *file == "prelude.cpl" {
            format!("{text}\n{extra}")
        } else {
            text.to_string()
        };
        fs::write(dir.join(file), text).expect("write a std file");
    }
    dir
}

#[test]
fn std_annotation_rules() {
    let extra = "\
@method
@builtin(\"list_len\")
pub fn no_receiver() -> Int

@property
@builtin(\"list_len\")
pub fn two(a: Int, b: Int) -> Int

@method
@builtin(\"list_len\")
pub fn on_unit(u: Unit) -> Int

@method
@property
@builtin(\"list_len\")
pub fn both(xs: List<Int>) -> Int

@builtin(\"nope\")
pub fn unknown(n: Int) -> Int

@builtin(\"int_abs\")
pub fn bodied(n: Int) -> Int {
    n
}

pub fn bodiless(n: Int) -> Int
";
    let dir = std_with("annotations", extra);
    let tree = tree(&[("cpl.toml", &manifest("t")), ("src/m.cpl", "module m\n")]);
    let project = load_project_from_tree(
        &tree,
        FileOrigin::Virtual,
        &LoadOptions {
            std: StdSource::Dir(dir),
            include_work_files: true,
        },
    )
    .expect("loads");
    let result = check(&project);
    let std_lines = result
        .program
        .source_map()
        .file(FileId(3))
        .text()
        .lines()
        .count() as u32;
    let base = std_lines - extra.lines().count() as u32;
    let got: Vec<(u32, String)> = result
        .diagnostics
        .iter()
        .map(|d| {
            let span = d.span.expect("a span");
            assert_eq!(
                result.program.source_map().file(span.file).path,
                "std/prelude.cpl"
            );
            (
                result
                    .program
                    .source_map()
                    .line_col(span.file, span.start)
                    .line
                    - base,
                d.code.0.to_string(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            (1, "annotation_misplaced".to_string()),
            (5, "annotation_misplaced".to_string()),
            (9, "annotation_misplaced".to_string()),
            (14, "annotation_misplaced".to_string()),
            (18, "std_builtin_missing".to_string()),
            (21, "annotation_misplaced".to_string()),
            (26, "missing_body".to_string()),
        ],
        "{}",
        render(&result)
    );
}

#[test]
fn reserved_module_files_are_not_collected() {
    let result = check_files(&[(
        "src/gen/extra.cpl",
        "module gen.extra\n\nfn one() -> Int {\n    undefined\n}\n",
    )]);
    assert_eq!(codes(&result), ["reserved_module_path"]);
    assert!(result.program.modules().all(|m| m.path != ["gen", "extra"]));
}

#[test]
fn parse_failure_stops_before_resolution() {
    // S52: the broken unit's one parse diagnostic; nothing is resolved, not
    // even the unresolved name of the other file.
    let result = check_files(&[
        ("src/a.cpl", "module a\n\nfn f() -> Int {\n    nowhere\n}\n"),
        ("src/b.cpl", "module b\n\nfn g( -> Int {\n    1\n}\n"),
    ]);
    assert_eq!(codes(&result), ["syntax_error"]);
    assert!(result.program.defs.is_empty());
    assert!(result.has_errors());
    // A broken .work file is reported but does not stop the check (S53).
    let result = check_files(&[
        ("src/a.cpl", "module a\n\nfn f() -> Int {\n    nowhere\n}\n"),
        ("work/broken.work", "change {\n"),
    ]);
    let mut got = codes(&result);
    got.sort();
    assert_eq!(got, ["syntax_error", "unresolved_name"]);
    assert!(!result.program.defs.is_empty());
}

#[test]
fn std_types_resolve_to_intrinsic_forms() {
    let result = check_source(
        "module m.t\n\nfn f(a: Int, b: Bool, c: Text, d: Bytes, e: List<Int>, u: Unit) -> Option<Int> {\n    None\n}\n",
    );
    assert!(result.diagnostics.is_empty(), "{}", render(&result));
    let p = &result.program;
    let f = p
        .lookup(&SymbolPath::parse("m.t.f").expect("a path"))
        .expect("f");
    let fd = p.def(f).kind.as_fn().expect("a fn");
    let tys: Vec<&Ty> = fd.params.iter().map(|x| &x.ty).collect();
    assert_eq!(
        tys,
        [
            &Ty::Int,
            &Ty::Bool,
            &Ty::Text,
            &Ty::Bytes,
            &Ty::list(Ty::Int),
            &Ty::unit(p.std_defs())
        ]
    );
    assert_eq!(fd.ret, Ty::option(p.std_defs(), Ty::Int));
}

#[test]
fn check_with_default_config_equals_check() {
    let project = load_tree(&tree(&[
        ("cpl.toml", &manifest("t")),
        ("src/m.cpl", "module m\n\nfn f() -> Int {\n    nope\n}\n"),
    ]));
    let a = check(&project);
    let b = check_with(&project, &CheckConfig::default());
    assert_eq!(a.diagnostics, b.diagnostics);
}

/// Toolchain 4.1: the example id in the `@id` messages is of the generated
/// format (`fn_k7w3qz2mb6ptx4ve`; the earlier `fn_k7w3qz2mb9ptx4ve` holds
/// a `9`, outside the base32 alphabet): the checker's message for a
/// malformed `@id` and `cpl-diag`'s `IdError` message for a bad value.
#[test]
fn id_messages_cite_a_generated_format_example() {
    let result = check_source(
        "module m.ids\n\n@id(7)\nfn a() -> Int {\n    1\n}\n\n@id(\"Not_An_Id\")\nfn b() -> Int {\n    2\n}\n",
    );
    let messages: Vec<String> = json_values(&result)
        .into_iter()
        .filter(|d| d["code"] == "invalid_annotation_argument")
        .map(|d| d["message"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(messages.len(), 2, "{}", render(&result));
    for m in &messages {
        assert!(m.contains("fn_k7w3qz2mb6ptx4ve"), "{m}");
        assert!(!m.contains("fn_k7w3qz2mb9ptx4ve"), "{m}");
    }
    let generated = |s: &str| {
        let (kind, rest) = s.split_once('_').expect("a kind prefix");
        ["fn", "ty", "cap", "test", "prop"].contains(&kind)
            && rest.len() == 16
            && rest
                .bytes()
                .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
    };
    assert!(generated("fn_k7w3qz2mb6ptx4ve"));
    assert!(!generated("fn_k7w3qz2mb9ptx4ve"));
}
