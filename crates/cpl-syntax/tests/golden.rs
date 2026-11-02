//! Golden parse/format tests (`spec/toolchain.md` Section 12, row "golden
//! parse/format"; the dump format of Section 12.1): for every input `X`,
//! the AST dump of `X` matches its `.ast` golden, and the formatted text
//! matches its `.fmt` golden when there is one, otherwise `format(X) == X`.
//!
//! | Inputs | Goldens |
//! | - | - |
//! | `examples/**/*.{cpl,work,patch}`, the acceptance corpus | `tests/golden/examples/<path>.ast`, where `<path>` is the input's path below `examples/` with its whole file name (`reader/work/pagination.work.ast`, since `pagination.work` and `pagination.patch` share a stem); none has a `.fmt`, since every example is canonical (T51) |
//! | `tests/corpus/**/*.{cpl,work,patch}`, this crate's syntax corpus | next to the input, the extension replaced (`items.ast`, `layout/messy_source.fmt`) |
//! | `tests/golden/E*.{cpl,work,patch}`, the design-document examples E1..E12 of grammar Section 6 as standalone inputs | next to the input, as Section 12.1 places `E1.ast` |
//!
//! Reading of the row, which names `examples/**` and `tests/corpus/**` and
//! writes the golden of input `X` as `X.ast`: `examples/` is verbatim
//! design text and read-only, so its goldens mirror its paths under
//! `tests/golden/examples/` instead of lying next to the inputs, and the
//! design-document examples keep the `tests/golden/` home Section 12.1
//! gives them.
//!
//! The syntax corpus covers the productions of grammar Sections 3 and 4 in
//! canonical layout: `items.cpl` every declaration form, `expressions.cpl`
//! every expression and pattern, `statements.cpl` every statement,
//! `comments.cpl` comment attachment (toolchain 2.3), `changes.work` and
//! `patches.patch` the other two file forms. The inputs under `layout/`
//! are deliberately not canonical; their `.fmt` goldens pin the layout
//! rules of toolchain 2.4 (clause order, escapes, digit separators, blank
//! lines, `{` placement, line breaking, `.work` item order). A `.fmt`
//! golden is canonical itself: formatting it changes nothing.
//!
//! `UPDATE_GOLDEN=1` rewrites every `.ast` golden, and the `.fmt` golden of
//! a corpus or design-document input exactly when formatting changes the
//! input (writing it when it is missing); an example that is not canonical
//! fails even then.

use cpl_diag::{FileId, FileOrigin, SourceMap};
use cpl_syntax::{
    dump_module, dump_patch, dump_work, format_source, parse_module, parse_patch, parse_work,
    FileKind,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn crate_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn examples_dir() -> PathBuf {
    crate_dir().join("../../examples")
}

fn golden_dir() -> PathBuf {
    crate_dir().join("tests").join("golden")
}

fn corpus_dir() -> PathBuf {
    crate_dir().join("tests").join("corpus")
}

fn update() -> bool {
    std::env::var("UPDATE_GOLDEN").is_ok_and(|v| v == "1")
}

/// Every file below `dir`, in path order.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
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

/// Which of the three input sets an input belongs to.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Set {
    Examples,
    Corpus,
    Design,
}

/// One input and the paths of its goldens.
struct Input {
    set: Set,
    path: PathBuf,
    kind: FileKind,
    ast: PathBuf,
    fmt: PathBuf,
}

impl Input {
    fn label(&self) -> String {
        self.path.display().to_string()
    }
}

/// `path` with `suffix` appended to its whole file name.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().expect("a file name").to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// Every input of the three sets, examples first, then the corpus, then
/// the design-document examples, each in path order.
fn inputs() -> Vec<Input> {
    let mut out = Vec::new();
    let mut files = Vec::new();
    walk(&examples_dir(), &mut files);
    for path in files {
        let Some(kind) = FileKind::from_path(&path) else {
            continue;
        };
        let relative = path
            .strip_prefix(examples_dir())
            .expect("below examples/")
            .to_path_buf();
        let base = golden_dir().join("examples").join(relative);
        out.push(Input {
            set: Set::Examples,
            ast: with_suffix(&base, ".ast"),
            fmt: with_suffix(&base, ".fmt"),
            path,
            kind,
        });
    }
    let mut files = Vec::new();
    walk(&corpus_dir(), &mut files);
    for path in files {
        if let Some(kind) = FileKind::from_path(&path) {
            out.push(Input {
                set: Set::Corpus,
                ast: path.with_extension("ast"),
                fmt: path.with_extension("fmt"),
                path,
                kind,
            });
        }
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(golden_dir())
        .expect("tests/golden exists")
        .map(|e| e.expect("directory entry").path())
        .filter(|p| p.is_file())
        .collect();
    files.sort();
    for path in files {
        if let Some(kind) = FileKind::from_path(&path) {
            out.push(Input {
                set: Set::Design,
                ast: path.with_extension("ast"),
                fmt: path.with_extension("fmt"),
                path,
                kind,
            });
        }
    }
    out
}

/// A source map holding `text` under `label`, for rendering diagnostics.
fn source_map(label: &str, text: &str) -> (SourceMap, FileId) {
    let mut map = SourceMap::new();
    let file = map.add(label.to_string(), FileOrigin::Virtual, text.to_string());
    (map, file)
}

/// The stable AST dump of a clean parse (Section 12.1), or the rendered
/// diagnostics.
fn dump(label: &str, text: &str, kind: FileKind) -> Result<String, String> {
    let (map, file) = source_map(label, text);
    let (dump, diagnostics) = match kind {
        FileKind::Cpl => {
            let r = parse_module(text, file);
            (r.value.map(|m| dump_module(&m)), r.diagnostics)
        }
        FileKind::Work => {
            let r = parse_work(text, file);
            (r.value.map(|w| dump_work(&w)), r.diagnostics)
        }
        FileKind::Patch => {
            let r = parse_patch(text, file);
            (r.value.map(|p| dump_patch(&p)), r.diagnostics)
        }
    };
    if !diagnostics.is_empty() {
        return Err(format!(
            "{label} does not parse cleanly:\n{}",
            cpl_diag::render_all(&diagnostics, &map)
        ));
    }
    dump.ok_or_else(|| format!("{label}: no tree"))
}

/// The canonical text, or the rendered diagnostics.
fn format(label: &str, text: &str, kind: FileKind) -> Result<String, String> {
    let (map, file) = source_map(label, text);
    format_source(text, kind, file).map_err(|d| {
        format!(
            "{label} does not format:\n{}",
            cpl_diag::render_all(&d, &map)
        )
    })
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn write(path: &Path, text: &str) {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).expect("create the golden directory");
    }
    std::fs::write(path, text).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

/// A mismatch report: the first differing line and both texts.
fn differs(what: &str, golden: &Path, expected: &str, actual: &str) -> String {
    let line = expected
        .lines()
        .zip(actual.lines())
        .position(|(e, a)| e != a)
        .unwrap_or_else(|| expected.lines().count().min(actual.lines().count()));
    format!(
        "{what} differs from {} (first difference at line {}; run with UPDATE_GOLDEN=1 to accept)\n--- golden\n{expected}\n--- actual\n{actual}",
        golden.display(),
        line + 1
    )
}

#[test]
fn every_input_matches_its_ast_golden() {
    let mut failures = Vec::new();
    for input in inputs() {
        let label = input.label();
        let actual = match dump(&label, &read(&input.path), input.kind) {
            Ok(dump) => dump,
            Err(e) => {
                failures.push(e);
                continue;
            }
        };
        if update() {
            write(&input.ast, &actual);
            continue;
        }
        match std::fs::read_to_string(&input.ast) {
            Ok(expected) if expected == actual => {}
            Ok(expected) => failures.push(differs(&label, &input.ast, &expected, &actual)),
            Err(_) => failures.push(format!(
                "{label} has no golden {} (run with UPDATE_GOLDEN=1 to create it)",
                input.ast.display()
            )),
        }
    }
    assert!(
        failures.is_empty(),
        "{} golden dump mismatches:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

#[test]
fn every_input_matches_its_fmt_golden_or_is_canonical() {
    let mut failures = Vec::new();
    for input in inputs() {
        let label = input.label();
        let text = read(&input.path);
        let formatted = match format(&label, &text, input.kind) {
            Ok(formatted) => formatted,
            Err(e) => {
                failures.push(e);
                continue;
            }
        };
        let golden = std::fs::read_to_string(&input.fmt).ok();
        if input.set == Set::Examples {
            // Verbatim design text is canonical (toolchain 12, T51); a
            // difference is a formatter defect, never a reason for a golden.
            if golden.is_some() {
                failures.push(format!(
                    "{}: an example has no .fmt golden",
                    input.fmt.display()
                ));
            }
            if formatted != text {
                failures.push(differs(
                    &format!("{label} (an example, which must be canonical)"),
                    &input.path,
                    &text,
                    &formatted,
                ));
            }
            continue;
        }
        if update() && (golden.is_some() || formatted != text) {
            write(&input.fmt, &formatted);
            continue;
        }
        match golden {
            Some(expected) => {
                if expected != formatted {
                    failures.push(differs(&label, &input.fmt, &expected, &formatted));
                }
                // The golden is canonical itself: a second formatting
                // leaves it alone (idempotency, Section 12).
                match format(&input.fmt.display().to_string(), &expected, input.kind) {
                    Ok(again) if again == expected => {}
                    Ok(again) => failures.push(differs(
                        "formatting the golden",
                        &input.fmt,
                        &expected,
                        &again,
                    )),
                    Err(e) => failures.push(e),
                }
            }
            None if formatted == text => {}
            None => failures.push(differs(
                &format!("{label} is not canonical and has no .fmt golden; formatting it"),
                &input.path,
                &text,
                &formatted,
            )),
        }
    }
    assert!(
        failures.is_empty(),
        "{} golden format mismatches:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

#[test]
fn the_inputs_cover_every_set_and_every_golden_has_its_input() {
    let inputs = inputs();
    let mut count: BTreeMap<Set, usize> = BTreeMap::new();
    for input in &inputs {
        *count.entry(input.set).or_default() += 1;
    }
    let count = |set: Set| count.get(&set).copied().unwrap_or(0);
    // examples/reader has five such files, examples/holes one.
    assert!(
        count(Set::Examples) >= 6,
        "examples: {}",
        count(Set::Examples)
    );
    assert!(
        count(Set::Corpus) >= 9,
        "syntax corpus: {}",
        count(Set::Corpus)
    );
    assert!(count(Set::Design) >= 12, "E1..E12: {}", count(Set::Design));
    // Every form appears in every set but the design examples' (E10 is the
    // only patch block and E9 the only work block of the design).
    for set in [Set::Examples, Set::Corpus, Set::Design] {
        for kind in [FileKind::Cpl, FileKind::Work, FileKind::Patch] {
            assert!(
                inputs.iter().any(|i| i.set == set && i.kind == kind),
                "{set:?} has no {kind:?} input"
            );
        }
    }
    // No two inputs share a golden.
    let mut goldens = BTreeSet::new();
    for input in &inputs {
        assert!(
            goldens.insert(input.ast.clone()),
            "{} is the golden of two inputs",
            input.ast.display()
        );
        assert!(
            goldens.insert(input.fmt.clone()),
            "{} is the golden of two inputs",
            input.fmt.display()
        );
    }
    // No golden is stale: every .ast and .fmt file belongs to an input.
    let mut files = Vec::new();
    walk(&golden_dir(), &mut files);
    walk(&corpus_dir(), &mut files);
    for file in files {
        let golden = matches!(
            file.extension().and_then(|e| e.to_str()),
            Some("ast" | "fmt")
        );
        assert!(
            !golden || goldens.contains(&file),
            "{} belongs to no input",
            file.display()
        );
    }
}

#[test]
fn dumps_are_deterministic_and_span_free() {
    let text = read(&golden_dir().join("E5.cpl"));
    let a = dump_module(&parse_module(&text, FileId(0)).value.expect("parses"));
    let b = dump_module(&parse_module(&text, FileId(7)).value.expect("parses"));
    assert_eq!(a, b, "the dump must not depend on the file id or spans");
    assert!(!a.contains("span"), "the dump must not render spans");
}
