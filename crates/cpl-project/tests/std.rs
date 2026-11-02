//! The embedded standard library (`spec/toolchain.md` 3.6): the generated
//! `STD_FILES` table, parsing, `std_digest`, and the `StdSource::Dir` override.

mod common;

use common::*;
use cpl_diag::{FileId, FileOrigin};
use cpl_project::{
    load_project, load_project_from_tree, std_digest, LoadError, LoadOptions, ParsedFile,
    StdSource, STD_FILES,
};
use std::fs;

#[test]
fn std_files_are_the_workspace_stubs_in_sorted_name_order() {
    let names: Vec<&str> = STD_FILES.iter().map(|(name, _)| *name).collect();
    assert_eq!(
        names,
        [
            "bytes.cpl",
            "foreign.cpl",
            "gen.cpl",
            "prelude.cpl",
            "text.cpl"
        ]
    );
    for (name, text) in STD_FILES {
        let on_disk = fs::read_to_string(workspace().join("std").join(name)).unwrap();
        assert_eq!(*text, on_disk, "{name} is embedded verbatim");
    }
}

#[test]
fn each_std_file_parses_with_zero_diagnostics() {
    for (name, text) in STD_FILES {
        let result = cpl_syntax::parse_module(text, FileId(0));
        assert!(
            result.diagnostics.is_empty(),
            "std/{name}: {:?}",
            result.diagnostics
        );
        let module = result
            .value
            .unwrap_or_else(|| panic!("std/{name} does not parse"));
        let stem = name.strip_suffix(".cpl").unwrap();
        assert_eq!(module.header.path.dotted(), format!("std.{stem}"));
    }
}

#[test]
fn std_files_get_the_lowest_file_ids_and_parse_in_every_project() {
    let project = load_tree(
        &tree(&[("cpl.toml", MANIFEST), ("src/a.cpl", "module a\n")]),
        true,
    );
    for (i, (name, text)) in STD_FILES.iter().enumerate() {
        let entry = &project.files[i];
        assert_eq!(entry.file, FileId(i as u32));
        assert_eq!(entry.origin, FileOrigin::Std);
        let file = project.source_map.file(entry.file);
        assert_eq!(file.path, format!("std/{name}"));
        assert_eq!(file.origin, FileOrigin::Std);
        assert_eq!(file.text(), *text);
        assert!(matches!(project.parsed[i], ParsedFile::Module(_)));
        assert_eq!(
            entry.digest.0,
            blake3::hash(text.as_bytes()).to_hex().to_string()
        );
    }
    assert!(project
        .diagnostics
        .iter()
        .all(|d| d.span.is_none_or(|s| s.file.0 as usize >= STD_FILES.len())));
}

#[test]
fn std_digest_is_blake3_of_name_lf_content_lf() {
    let mut concatenation = Vec::new();
    for (name, text) in STD_FILES {
        concatenation.extend_from_slice(name.as_bytes());
        concatenation.push(b'\n');
        concatenation.extend_from_slice(text.as_bytes());
        concatenation.push(b'\n');
    }
    let expected = blake3::hash(&concatenation).to_hex().to_string();
    assert_eq!(std_digest(STD_FILES).0, expected);
    assert_eq!(std_digest(STD_FILES).0.len(), 64);
    assert_eq!(
        std_digest(&[("a.cpl", "x")]).0,
        blake3::hash(b"a.cpl\nx\n").to_hex().to_string()
    );

    let project = load_dir(&example("holes"), false);
    assert_eq!(project.std_digest(), std_digest(STD_FILES));
}

#[test]
fn std_dir_override_of_the_workspace_std_equals_the_embedded_library() {
    let embedded = load_dir(&example("holes"), false);
    let opts = LoadOptions {
        std: StdSource::Dir(workspace().join("std")),
        include_work_files: false,
    };
    let overridden = load_project(&example("holes"), &opts).unwrap();
    assert_same_project(&embedded, &overridden, true);
    assert_eq!(overridden.std_digest(), embedded.std_digest());
}

#[test]
fn std_dir_override_loads_exactly_the_directory() {
    let dir = temp_dir("std-dir-override");
    write(&dir, "zeta.cpl", "module std.zeta\n");
    write(&dir, "alpha.cpl", "module std.alpha\n");
    write(&dir, "notes.txt", "not a stub\n");
    write(&dir, "nested/deeper.cpl", "module std.deeper\n");
    let opts = LoadOptions {
        std: StdSource::Dir(dir.clone()),
        include_work_files: false,
    };
    let project = load_project_from_tree(
        &tree(&[("cpl.toml", MANIFEST), ("src/a.cpl", "module a\n")]),
        FileOrigin::Virtual,
        &opts,
    )
    .unwrap();
    assert_eq!(
        paths(&project),
        ["std/alpha.cpl", "std/zeta.cpl", "src/a.cpl"]
    );
    assert_eq!(
        project.std_digest(),
        std_digest(&[
            ("alpha.cpl", "module std.alpha\n"),
            ("zeta.cpl", "module std.zeta\n")
        ])
    );
    assert!(project.diagnostics.is_empty(), "{:?}", project.diagnostics);
}

#[test]
fn std_dir_parse_errors_are_ordinary_parse_diagnostics() {
    let dir = temp_dir("std-dir-broken");
    write(&dir, "prelude.cpl", "module std.prelude\nfn ;\n");
    let opts = LoadOptions {
        std: StdSource::Dir(dir.clone()),
        include_work_files: false,
    };
    let project = load_project_from_tree(
        &tree(&[("cpl.toml", MANIFEST), ("src/a.cpl", "module a\n")]),
        FileOrigin::Virtual,
        &opts,
    )
    .unwrap();
    assert_eq!(parsed_kinds(&project), ["failed", "module"]);
    assert_eq!(project.diagnostics.len(), 1);
    assert_eq!(project.diagnostics[0].span.unwrap().file, FileId(0));
}

#[test]
fn missing_std_dir_is_load_error_std() {
    let missing = temp_dir("std-dir-missing").join("nope");
    let opts = LoadOptions {
        std: StdSource::Dir(missing),
        include_work_files: false,
    };
    let err = load_project_from_tree(&tree(&[("cpl.toml", MANIFEST)]), FileOrigin::Virtual, &opts)
        .unwrap_err();
    assert!(matches!(err, LoadError::Std(_)), "{err:?}");
    assert!(
        err.to_string()
            .starts_with("standard library unavailable: cannot read "),
        "{err}"
    );
}

#[test]
fn std_dir_without_stubs_is_load_error_std() {
    let dir = temp_dir("std-dir-empty");
    write(&dir, "README", "no stubs here\n");
    let opts = LoadOptions {
        std: StdSource::Dir(dir),
        include_work_files: false,
    };
    let err = load_project_from_tree(&tree(&[("cpl.toml", MANIFEST)]), FileOrigin::Virtual, &opts)
        .unwrap_err();
    match err {
        LoadError::Std(message) => {
            assert!(message.ends_with("contains no .cpl files"), "{message}")
        }
        other => panic!("expected LoadError::Std, found {other:?}"),
    }
}
