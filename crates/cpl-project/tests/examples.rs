//! The acceptance corpus `examples/reader` and `examples/holes` load with
//! the exact file list and `FileId` order of T15 and no diagnostics.

mod common;

use common::*;
use cpl_diag::{FileId, FileOrigin};
use cpl_project::{check_bindings, ParsedFile, Project};
use cpl_syntax::ast::DeclKind;
use cpl_syntax::FileKind;
use std::collections::BTreeSet;
use std::fs;

const STD: [&str; 5] = [
    "std/bytes.cpl",
    "std/foreign.cpl",
    "std/gen.cpl",
    "std/prelude.cpl",
    "std/text.cpl",
];

fn blake3_hex(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn assert_ids_in_order(project: &Project) {
    assert_eq!(project.files.len(), project.source_map.len());
    assert_eq!(project.parsed.len(), project.files.len());
    for (i, entry) in project.files.iter().enumerate() {
        assert_eq!(entry.file, FileId(i as u32));
        assert_eq!(project.source_map.file(entry.file).id, entry.file);
    }
}

/// Every `pub capability` of the parsed modules, by qualified name.
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

#[test]
fn reader_loads_with_the_exact_file_list_and_file_ids() {
    let root = example("reader");
    let project = load_dir(&root, true);
    let mut expected: Vec<&str> = STD.to_vec();
    expected.extend([
        "src/reader/images.cpl",
        "src/reader/pages.cpl",
        "src/tests/pagination.cpl",
        "work/pagination.patch",
        "work/pagination.work",
    ]);
    assert_eq!(paths(&project), expected);
    assert_ids_in_order(&project);
    assert!(
        project.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        project.diagnostics
    );

    let kinds: Vec<FileKind> = project.files.iter().map(|e| e.kind).collect();
    let mut expected_kinds = vec![FileKind::Cpl; 8];
    expected_kinds.extend([FileKind::Patch, FileKind::Work]);
    assert_eq!(kinds, expected_kinds);
    assert_eq!(
        parsed_kinds(&project),
        [
            "module", "module", "module", "module", "module", "module", "module", "module",
            "patch", "work"
        ]
    );
    for (i, entry) in project.files.iter().enumerate() {
        let want = if i < 5 {
            FileOrigin::Std
        } else {
            FileOrigin::Project
        };
        assert_eq!(entry.origin, want);
        assert_eq!(project.source_map.file(entry.file).origin, want);
    }

    // Digests: blake3 of the bytes, for every entry and for cpl.toml.
    let manifest_bytes = fs::read(root.join("cpl.toml")).unwrap();
    assert_eq!(project.manifest_digest.0, blake3_hex(&manifest_bytes));
    for entry in &project.files[5..] {
        let path = &project.source_map.file(entry.file).path;
        assert_eq!(
            entry.digest.0,
            blake3_hex(&fs::read(root.join(path)).unwrap()),
            "{path}"
        );
    }

    assert_eq!(project.root.as_deref(), Some(root.as_path()));
    assert!(project.lock.is_none());
    assert_eq!(project.manifest.project.name, "reader");
    assert_eq!(project.manifest.project.edition, "0.1");
    assert_eq!(project.manifest.build.sources, ["src"]);
    assert_eq!(project.manifest.interop.targets, ["python", "node", "c"]);
    let binding = &project.manifest.bindings["image_codec"];
    assert_eq!(binding.capability, "reader.images.ImageCodec");
    assert_eq!(binding.backend, "python");
    assert_eq!(binding.adapter, "reader_adapters.images");
    assert_eq!(binding.isolation.as_deref(), Some("worker"));

    // The parsed modules carry the headers the corpus declares.
    let headers: Vec<String> = project.parsed[5..8]
        .iter()
        .map(|p| match p {
            ParsedFile::Module(m) => m.header.path.dotted(),
            other => panic!("expected a module, found {other:?}"),
        })
        .collect();
    assert_eq!(
        headers,
        ["reader.images", "reader.pages", "tests.pagination"]
    );
}

#[test]
fn reader_without_work_files_skips_them_and_keeps_the_file_ids() {
    let with = load_dir(&example("reader"), true);
    let without = load_dir(&example("reader"), false);
    assert_eq!(paths(&with), paths(&without));
    assert_eq!(with.files, without.files);
    assert_eq!(&parsed_kinds(&without)[8..], ["skipped", "skipped"]);
    assert_eq!(with.parsed[..8], without.parsed[..8]);
    assert!(without.diagnostics.is_empty());
}

#[test]
fn holes_loads_with_the_exact_file_list_and_file_ids() {
    let root = example("holes");
    for include_work_files in [false, true] {
        let project = load_dir(&root, include_work_files);
        let mut expected: Vec<&str> = STD.to_vec();
        expected.push("src/holes/pages.cpl");
        assert_eq!(paths(&project), expected);
        assert_ids_in_order(&project);
        assert_eq!(parsed_kinds(&project), ["module"; 6]);
        assert!(
            project.diagnostics.is_empty(),
            "unexpected diagnostics: {:?}",
            project.diagnostics
        );
        assert_eq!(project.manifest.project.name, "holes");
        assert!(project.manifest.bindings.is_empty());
    }
}

#[test]
fn reader_binding_names_a_pub_capability() {
    let project = load_dir(&example("reader"), false);
    let capabilities = pub_capabilities(&project);
    assert!(capabilities.contains("reader.images.ImageCodec"));
    let diagnostics = check_bindings(&project.manifest, |path| capabilities.contains(path));
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}
