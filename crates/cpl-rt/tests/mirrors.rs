//! The `mirrors` comments of the runtime (spec/cabi.md 2.1, 2.7, 2.9): every function of `crates/cpl-rt/c/` that
//! transcribes a function of the evaluator carries `/* mirrors cpl_eval::<path> ... */` directly above its definition,
//! and `crates/cpl-rt/c/README.md` indexes them. This test holds that
//!
//! - every comment names a function that exists in `crates/cpl-eval/src` (read as text: cpl-rt depends on no
//!   evaluator crate, T71);
//! - every callable builtin's entry `CPLRT(bi_<id>)` carries a comment naming a function of `cpl_eval::builtins`
//!   (cabi.md 2.7);
//! - the README's index lists exactly the comments, in file and line order (`UPDATE_MIRRORS=1` rewrites it).

use std::path::{Path, PathBuf};

fn c_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("c")
}

fn eval_src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../cpl-eval/src")
}

/// One `mirrors` comment: the C file, the line of the comment, the C function below it, and the Rust paths it names.
#[derive(Debug)]
struct Mirror {
    file: String,
    line: usize,
    function: String,
    paths: Vec<String>,
}

/// The name a definition line defines: `CPLRT(name)` for a runtime symbol, else the identifier before `(`.
fn defined_name(line: &str) -> Option<String> {
    if line.starts_with(' ')
        || line.starts_with('\t')
        || line.starts_with('#')
        || line.starts_with("/*")
    {
        return None;
    }
    let open = line.find('(')?;
    let head = &line[..open];
    if let Some(at) = head.rfind("CPLRT") {
        if head[at..].trim() == "CPLRT" {
            let rest = &line[open + 1..];
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            return Some(format!("CPLRT({name})"));
        }
    }
    let name: String = head
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    (!name.is_empty() && head.trim() != name).then_some(name)
}

fn mirrors() -> Vec<Mirror> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(c_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "c"))
        .collect();
    files.sort();
    let mut out = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let mut i = 0;
        while i < lines.len() {
            if !lines[i].starts_with("/* mirrors cpl_eval::") {
                i += 1;
                continue;
            }
            // the whole comment, then the definition below it
            let start = i;
            let mut comment = String::new();
            while i < lines.len() {
                comment.push_str(lines[i]);
                comment.push(' ');
                if lines[i].contains("*/") {
                    break;
                }
                i += 1;
            }
            let function = lines[i + 1..]
                .iter()
                .find_map(|l| defined_name(l))
                .unwrap_or_else(|| {
                    panic!(
                        "{}:{}: no definition below the comment",
                        path.display(),
                        start + 1
                    )
                });
            let paths = comment
                .match_indices("cpl_eval::")
                .map(|(at, _)| {
                    comment[at..]
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == ':')
                        .collect::<String>()
                        .trim_end_matches(':')
                        .to_string()
                })
                .collect();
            out.push(Mirror {
                file: path.file_name().unwrap().to_string_lossy().to_string(),
                line: start + 1,
                function,
                paths,
            });
            i += 1;
        }
    }
    out
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Whether `cpl_eval::<path>` names a function: the longest module prefix that has a file, and a `fn <name>` in that
/// file or in the files below its directory (an `impl` may sit in a submodule, as `interp/eval.rs` does).
fn resolves(path: &str) -> bool {
    let segments: Vec<&str> = path
        .strip_prefix("cpl_eval::")
        .unwrap_or(path)
        .split("::")
        .collect();
    let name = segments[segments.len() - 1];
    for k in (0..segments.len()).rev() {
        let module = segments[..k].join("/");
        let (file, dir) = if k == 0 {
            (eval_src().join("lib.rs"), None)
        } else if eval_src().join(format!("{module}.rs")).is_file() {
            (
                eval_src().join(format!("{module}.rs")),
                Some(eval_src().join(&module)),
            )
        } else if eval_src().join(&module).join("mod.rs").is_file() {
            (
                eval_src().join(&module).join("mod.rs"),
                Some(eval_src().join(&module)),
            )
        } else {
            continue;
        };
        let mut files = vec![file];
        if let Some(d) = dir.filter(|d| d.is_dir()) {
            rust_files(&d, &mut files);
        }
        let needle = format!("fn {name}");
        return files.iter().any(|f| {
            let text = std::fs::read_to_string(f).unwrap_or_default();
            text.match_indices(&needle).any(|(at, _)| {
                let after = text[at + needle.len()..].chars().next();
                after.is_some_and(|c| c == '(' || c == '<')
            })
        });
    }
    false
}

#[test]
fn every_mirrors_comment_names_an_evaluator_function() {
    let all = mirrors();
    assert!(all.len() >= 80, "{} mirrors comments", all.len());
    let unresolved: Vec<String> = all
        .iter()
        .flat_map(|m| {
            m.paths
                .iter()
                .filter(|p| !resolves(p))
                .map(move |p| format!("{}:{} {p}", m.file, m.line))
        })
        .collect();
    assert!(
        unresolved.is_empty(),
        "paths that name no function of cpl-eval:\n{}",
        unresolved.join("\n")
    );
}

#[test]
fn every_callable_builtin_mirrors_its_evaluator_function() {
    let all = mirrors();
    let mut missing = Vec::new();
    for b in cpl_rtspec::builtins::BUILTINS.iter().filter(|b| b.callable) {
        let entry = format!("CPLRT(bi_{})", b.id);
        let found = all.iter().any(|m| {
            m.function == entry
                && m.paths
                    .iter()
                    .any(|p| p.starts_with("cpl_eval::builtins::"))
        });
        if !found {
            missing.push(entry);
        }
    }
    assert_eq!(
        cpl_rtspec::builtins::BUILTINS
            .iter()
            .filter(|b| b.callable)
            .count(),
        46,
        "the 46 callable ids"
    );
    assert!(
        missing.is_empty(),
        "callable builtins without a mirrors comment: {missing:?}"
    );
}

/// The index of the README: one row per comment.
fn index(all: &[Mirror]) -> String {
    let mut s = String::from("| File | C function | Mirrors |\n| - | - | - |\n");
    for m in all {
        let paths: Vec<String> = m.paths.iter().map(|p| format!("`{p}`")).collect();
        s.push_str(&format!(
            "| `{}` | `{}` | {} |\n",
            m.file,
            m.function,
            paths.join(", ")
        ));
    }
    s
}

const BEGIN: &str =
    "<!-- mirrors index: generated by crates/cpl-rt/tests/mirrors.rs (UPDATE_MIRRORS=1) -->\n";
const END: &str = "<!-- end of the mirrors index -->\n";

#[test]
fn the_readme_indexes_every_mirrors_comment() {
    let all = mirrors();
    let readme_path = c_dir().join("README.md");
    let readme = std::fs::read_to_string(&readme_path).unwrap_or_default();
    let table = index(&all);
    let (Some(b), Some(e)) = (readme.find(BEGIN), readme.find(END)) else {
        panic!("crates/cpl-rt/c/README.md lacks the index markers");
    };
    let current = &readme[b + BEGIN.len()..e];
    if current != table {
        if std::env::var("UPDATE_MIRRORS").is_ok_and(|v| v == "1") {
            let updated = format!("{}{BEGIN}{table}{}", &readme[..b], &readme[e..]);
            std::fs::write(&readme_path, updated).unwrap();
            return;
        }
        panic!("crates/cpl-rt/c/README.md's index is out of date (run with UPDATE_MIRRORS=1):\n{table}");
    }
}
