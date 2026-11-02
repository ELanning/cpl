//! Every file under `examples/` is the acceptance corpus of `spec/grammar.md`
//! and must parse with zero diagnostics.

use cpl_diag::{FileId, FileOrigin, SourceMap};
use cpl_syntax::{parse_module, parse_patch, parse_work};
use std::path::{Path, PathBuf};

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .map(|e| e.expect("directory entry").path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

#[test]
fn every_example_file_parses_cleanly() {
    let mut files = Vec::new();
    walk(&examples_dir(), &mut files);
    let mut parsed = 0;
    let mut failures = Vec::new();
    for path in &files {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !matches!(ext, "cpl" | "work" | "patch") {
            continue;
        }
        let text = std::fs::read_to_string(path).expect("read example");
        let mut map = SourceMap::new();
        let file = map.add(
            path.display().to_string(),
            FileOrigin::Virtual,
            text.clone(),
        );
        let (ok, diagnostics) = match ext {
            "cpl" => {
                let r = parse_module(&text, file);
                (r.value.is_some(), r.diagnostics)
            }
            "work" => {
                let r = parse_work(&text, file);
                (r.value.is_some(), r.diagnostics)
            }
            _ => {
                let r = parse_patch(&text, file);
                (r.value.is_some(), r.diagnostics)
            }
        };
        parsed += 1;
        if !ok || !diagnostics.is_empty() {
            let rendered = cpl_diag::render_all(&diagnostics, &map);
            println!("FAILED: {}\n{rendered}", path.display());
            failures.push(path.display().to_string());
        }
    }
    assert!(
        parsed >= 6,
        "expected to find the example corpus, parsed {parsed} files"
    );
    assert!(
        failures.is_empty(),
        "corpus files with diagnostics: {failures:?}"
    );
}

#[test]
fn corpus_files_lex_without_errors() {
    let mut files = Vec::new();
    walk(&examples_dir(), &mut files);
    for path in files {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !matches!(ext, "cpl" | "work" | "patch") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read example");
        let (tokens, diagnostics) = cpl_syntax::lex(&text, FileId(0));
        assert!(
            diagnostics.is_empty(),
            "{}: {diagnostics:?}",
            path.display()
        );
        assert_eq!(
            tokens.last().map(|t| t.kind),
            Some(cpl_syntax::TokenKind::Eof)
        );
    }
}
