//! The sema corpus (`crates/cpl-sema/tests/corpus`, read-only) as far as
//! loading decides it: for every case, the markers (`spec/toolchain.md`
//! 12.2, T38) whose codes loading raises must match the loading diagnostics
//! exactly, as a multiset of (file, line, severity, code); the binding
//! markers must match [`check_bindings`] run with a resolver over the parsed
//! modules. Every other marker belongs to the checker.

mod common;

use common::*;
use cpl_diag::{Diagnostic, Severity};
use cpl_project::{check_bindings, codes, ParsedFile, Project, VirtualTree};
use cpl_syntax::ast::DeclKind;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// (file, line, severity, code); file and line are `None` for a `(global)` marker.
type Expect = (Option<String>, Option<u32>, Severity, String);

/// The codes loading raises.
fn loading_codes() -> BTreeSet<&'static str> {
    let mut set: BTreeSet<&'static str> = [
        codes::PROJECT_MANIFEST_UNKNOWN_KEY,
        codes::PROJECT_SOURCE_DIR_MISSING,
        codes::PROJECT_NO_SOURCES,
        codes::PROJECT_DUPLICATE_FILE_MODULE,
        codes::PROJECT_LOCK_TOOLCHAIN_MISMATCH,
        codes::UNSUPPORTED_EDITION,
        codes::RESERVED_MODULE_PATH,
        codes::MODULE_FILE_MISPLACED,
    ]
    .iter()
    .map(|c| c.0)
    .collect();
    set.extend(cpl_syntax::codes::ALL.iter().map(|c| c.0));
    set
}

/// The codes of the binding checks.
fn binding_codes() -> BTreeSet<&'static str> {
    [
        codes::PROJECT_BINDING_UNKNOWN_CAPABILITY,
        codes::PROJECT_BINDING_TARGET_NOT_ENABLED,
        codes::PROJECT_BINDING_BAD_BACKEND,
        codes::PROJECT_BINDING_BAD_ISOLATION,
    ]
    .iter()
    .map(|c| c.0)
    .collect()
}

/// The markers of one file (`path` is its project-relative path).
fn markers(path: &str, text: &str) -> Vec<Expect> {
    let mut out = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let number = index as u32 + 1;
        let mut rest = line;
        while let Some(at) = rest.find("// expect-") {
            rest = &rest[at + "// expect-".len()..];
            let (severity, tail) = if let Some(t) = rest.strip_prefix("error") {
                (Severity::Error, t)
            } else if let Some(t) = rest.strip_prefix("warning") {
                (Severity::Warning, t)
            } else if let Some(t) = rest.strip_prefix("info") {
                (Severity::Info, t)
            } else {
                continue;
            };
            let (place, tail) = if let Some(t) = tail.strip_prefix("(global)") {
                (None, t)
            } else if let Some(t) = tail.strip_prefix("@+") {
                let digits: String = t.chars().take_while(char::is_ascii_digit).collect();
                (
                    Some(number + digits.parse::<u32>().expect("@+N")),
                    &t[digits.len()..],
                )
            } else {
                (Some(number), tail)
            };
            let Some(list) = tail.strip_prefix(':') else {
                continue;
            };
            for item in list.split(',') {
                let code: String = item
                    .trim_start()
                    .chars()
                    .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_')
                    .collect();
                if !code.is_empty() {
                    let file = place.map(|_| path.to_string());
                    out.push((file, place, severity, code));
                }
            }
        }
    }
    out
}

fn expect_of(project: &Project, d: &Diagnostic) -> Expect {
    match d.span {
        None => (None, None, d.severity, d.code.0.to_string()),
        Some(span) => {
            let file = project.source_map.file(span.file);
            let line = project.source_map.line_col(span.file, span.start).line;
            (
                Some(file.path.clone()),
                Some(line),
                d.severity,
                d.code.0.to_string(),
            )
        }
    }
}

/// Every `pub capability` of the parsed modules, by qualified name: the
/// resolution the binding checks need (the checker supplies its own).
fn pub_capabilities(project: &Project) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for parsed in &project.parsed {
        if let ParsedFile::Module(module) = parsed {
            for decl in &module.declarations {
                if let DeclKind::Capability(cap) = &decl.kind {
                    if cap.is_pub {
                        out.insert(format!("{}.{}", module.header.path.dotted(), cap.name.text));
                    }
                }
            }
        }
    }
    out
}

/// Compare the loading and binding diagnostics of `project` with the
/// markers of `files` (project-relative path, text).
fn check_case(name: &str, project: &Project, files: &[(String, String)]) {
    let decided: BTreeSet<&str> = loading_codes().union(&binding_codes()).copied().collect();
    let mut expected: Vec<Expect> = files
        .iter()
        .flat_map(|(path, text)| markers(path, text))
        .filter(|(_, _, _, code)| decided.contains(code.as_str()))
        .collect();
    let capabilities = pub_capabilities(project);
    let bindings = check_bindings(&project.manifest, |path| capabilities.contains(path));
    let mut actual: Vec<Expect> = project
        .diagnostics
        .iter()
        .chain(&bindings)
        .map(|d| expect_of(project, d))
        .collect();
    expected.sort();
    actual.sort();
    assert_eq!(
        actual, expected,
        "case {name}: loading diagnostics differ from the markers"
    );
}

/// Every `.cpl` file under `dir`, with its path relative to `dir`.
fn cpl_files(dir: &Path) -> Vec<(String, String)> {
    let tree = tree_of_dir(dir);
    tree.files
        .into_iter()
        .filter(|(path, _)| path.ends_with(".cpl"))
        .map(|(path, bytes)| (path, String::from_utf8(bytes).expect("UTF-8 corpus file")))
        .collect()
}

fn directory_cases(group: &str) -> Vec<PathBuf> {
    let dir = sema_corpus().join(group);
    let mut cases: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.join("cpl.toml").is_file())
        .collect();
    cases.sort();
    cases
}

fn check_directory_case(dir: &Path) -> Project {
    let project = load_dir(dir, true);
    let name = dir.file_name().unwrap().to_string_lossy().into_owned();
    check_case(&name, &project, &cpl_files(dir));
    project
}

/// One of the four `errors/project/` cases loading alone decides.
fn project_case(name: &str) -> Project {
    check_directory_case(&sema_corpus().join("errors/project").join(name))
}

#[test]
fn errors_project_module_file_misplaced() {
    let project = project_case("module_file_misplaced");
    assert_eq!(codes(&project.diagnostics), ["module_file_misplaced"]);
    assert_eq!(project.diagnostics[0].data["expected"], "src/cases/lib.cpl");
}

#[test]
fn errors_project_project_manifest_unknown_key() {
    let project = project_case("project_manifest_unknown_key");
    assert_eq!(
        codes(&project.diagnostics),
        ["project_manifest_unknown_key"]
    );
    assert_eq!(project.diagnostics[0].data["key"], "project.colour");
}

#[test]
fn errors_project_project_source_dir_missing() {
    let project = project_case("project_source_dir_missing");
    assert_eq!(codes(&project.diagnostics), ["project_source_dir_missing"]);
    assert_eq!(project.diagnostics[0].data["path"], "lib");
}

#[test]
fn errors_project_unsupported_edition() {
    let project = project_case("unsupported_edition");
    assert_eq!(codes(&project.diagnostics), ["unsupported_edition"]);
    assert_eq!(project.diagnostics[0].data["found"], "0.2");
}

#[test]
fn errors_project_binding_cases_are_decided_by_the_binding_checks() {
    for (case, code) in [
        ("project_binding_bad_backend", "project_binding_bad_backend"),
        (
            "project_binding_bad_isolation",
            "project_binding_bad_isolation",
        ),
        (
            "project_binding_target_not_enabled",
            "project_binding_target_not_enabled",
        ),
        (
            "project_binding_unknown_capability",
            "project_binding_unknown_capability",
        ),
    ] {
        let project = project_case(case);
        // Loading itself reports nothing: binding checks run after name resolution.
        assert!(
            project.diagnostics.is_empty(),
            "{case}: {:?}",
            project.diagnostics
        );
        let capabilities = pub_capabilities(&project);
        let bindings = check_bindings(&project.manifest, |path| capabilities.contains(path));
        assert_eq!(codes(&bindings), [code], "{case}");
    }
}

#[test]
fn errors_project_cases_the_checker_decides_load_cleanly() {
    for case in [
        "duplicate_across_files",
        "duplicate_test_across_files",
        "private_item",
    ] {
        let project = project_case(case);
        assert!(
            project.diagnostics.is_empty(),
            "{case}: {:?}",
            project.diagnostics
        );
    }
}

#[test]
fn every_directory_case_matches_its_loading_markers() {
    let mut count = 0;
    for group in ["ok", "errors/project", "release"] {
        for dir in directory_cases(group) {
            check_directory_case(&dir);
            count += 1;
        }
    }
    assert_eq!(count, 2 + 11 + 3);
}

/// Wrap a single-file case as toolchain 12.2 describes: a synthetic
/// manifest and the file at the conventional path of its `module` header.
fn single_file_project(file: &Path, name: &str) -> (Project, String, String) {
    let text = fs::read_to_string(file).unwrap();
    let parsed = cpl_syntax::parse_module(&text, cpl_diag::FileId(0));
    let module = parsed.value.unwrap_or_else(|| {
        panic!(
            "{} does not parse: {:?}",
            file.display(),
            parsed.diagnostics
        )
    });
    let segments: Vec<&str> = module
        .header
        .path
        .segments
        .iter()
        .map(|s| s.text.as_str())
        .collect();
    let path = format!("src/{}.cpl", segments.join("/"));
    let tree = VirtualTree {
        files: [
            (
                "cpl.toml".to_string(),
                format!("[project]\nname = \"{name}\"\nedition = \"0.1\"\n").into_bytes(),
            ),
            (path.clone(), text.clone().into_bytes()),
        ]
        .into_iter()
        .collect(),
    };
    (load_tree(&tree, true), path, text)
}

#[test]
fn every_single_file_case_matches_its_loading_markers() {
    let mut count = 0;
    let mut groups: Vec<(PathBuf, &str)> = vec![(sema_corpus().join("ok"), "ok")];
    let mut categories: Vec<PathBuf> = fs::read_dir(sema_corpus().join("errors"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir() && !p.ends_with("project"))
        .collect();
    categories.sort();
    groups.extend(categories.into_iter().map(|p| (p, "errors")));
    for (dir, name) in groups {
        let mut files: Vec<PathBuf> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "cpl"))
            .collect();
        files.sort();
        for file in files {
            let (project, path, text) = single_file_project(&file, name);
            check_case(&file.display().to_string(), &project, &[(path, text)]);
            count += 1;
        }
    }
    assert!(count > 100, "only {count} single-file cases");
}

#[test]
fn the_reserved_module_path_case_is_decided_by_loading() {
    let file = sema_corpus().join("errors/resolve/reserved_module_path.cpl");
    let (project, path, _) = single_file_project(&file, "errors");
    assert_eq!(path, "src/gen/extra.cpl");
    assert_eq!(codes(&project.diagnostics), ["reserved_module_path"]);
    let d = &project.diagnostics[0];
    assert_eq!(d.data["module"], "gen.extra");
    assert_eq!(project.source_map.text(d.span.unwrap()), "gen.extra");
}
