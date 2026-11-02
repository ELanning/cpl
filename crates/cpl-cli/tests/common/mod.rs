//! Shared helpers of the `cpl` integration tests (`spec/toolchain.md`
//! 12.4), for this crate's test binaries and the ones later commands add.
//!
//! * [`Fixture`]: `examples/reader` and `examples/holes` copied into a fresh
//!   temporary directory under `CARGO_TARGET_TMPDIR`, optionally made a git
//!   repository (`git init -b main`, `git add -A`, `git commit`) with the
//!   identity passed through `GIT_AUTHOR_*` and `GIT_COMMITTER_*`; tests
//!   that need git skip themselves, with a message, when it is absent
//!   ([`Fixture::committed`] returns `None`).
//! * [`cpl`]: run the built binary (`CARGO_BIN_EXE_cpl`) in a directory.
//! * [`Run`]: the exit code and outputs, with [`Run::json`], which asserts
//!   that stdout is exactly one JSON object and stderr is empty.
//! * [`normalize`] and [`check_golden`]: the volatile fields of 12.4
//!   (`duration_ms`, `produced_at`, `revision.commit`, `revision.digest`,
//!   OS-chosen seeds, absolute paths) replaced by placeholders, then a
//!   byte-for-byte comparison with `tests/golden/<name>`, rewritten under
//!   `UPDATE_GOLDEN=1`. The ids `cpl id assign` generates are OS randomness
//!   too (Section 4.1), which the 12.4 list does not name: they are replaced
//!   by `<kind>_<generated>` ([`Volatile::with_generated_ids`]).
#![allow(dead_code)] // each test binary uses a different subset

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

/// The workspace root.
pub fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The built `cpl` binary.
pub fn cpl_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cpl"))
}

/// A fresh, empty directory under `CARGO_TARGET_TMPDIR`, unique per call.
pub fn temp_dir(name: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, Ordering::SeqCst);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("cpl-cli")
        .join(format!("{name}-{}-{n}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clear the temporary directory");
    }
    std::fs::create_dir_all(&dir).expect("create the temporary directory");
    dir
}

/// Copy a directory tree.
pub fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create directory");
    let mut entries: Vec<_> = std::fs::read_dir(from)
        .expect("read directory")
        .map(|e| e.expect("entry"))
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let target = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy file");
        }
    }
}

/// True when `git` runs; prints the skip message once per test otherwise.
pub fn git_available() -> bool {
    static FOUND: OnceLock<bool> = OnceLock::new();
    let found = *FOUND.get_or_init(|| {
        Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    });
    if !found {
        eprintln!("skipped: git is not installed");
    }
    found
}

/// The environment of every child process: no user or system git
/// configuration (so hooks, signing, templates, and line-ending conversion
/// of the user's setup never apply), a fixed identity, the C locale, and no
/// standard-library override.
fn isolate(command: &mut Command, scratch: &Path) {
    let empty = scratch.join(".gitconfig-empty");
    if !empty.exists() {
        let _ = std::fs::write(&empty, "");
    }
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", &empty)
        .env("GIT_AUTHOR_NAME", "cpl test")
        .env("GIT_AUTHOR_EMAIL", "cpl-test@example.invalid")
        .env("GIT_COMMITTER_NAME", "cpl test")
        .env("GIT_COMMITTER_EMAIL", "cpl-test@example.invalid")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .env_remove("CPL_STD_DIR")
        .env_remove("UPDATE_GOLDEN");
}

/// The examples copied into a temporary directory: `<root>/reader` and
/// `<root>/holes`, optionally committed to a repository at `<root>`.
pub struct Fixture {
    pub root: PathBuf,
    pub reader: PathBuf,
    pub holes: PathBuf,
    /// HEAD after the initial commit, for a committed fixture.
    pub commit: Option<String>,
}

impl Fixture {
    /// The copies, without a repository.
    pub fn new(name: &str) -> Fixture {
        let root = temp_dir(name);
        let reader = root.join("reader");
        let holes = root.join("holes");
        copy_dir(&workspace().join("examples/reader"), &reader);
        copy_dir(&workspace().join("examples/holes"), &holes);
        Fixture {
            root,
            reader,
            holes,
            commit: None,
        }
    }

    /// The copies committed to a fresh repository at the fixture's root
    /// (`git init -b main`, so that the corpus's `git("main")` resolves
    /// whatever `init.defaultBranch` says); `None`, with a message, when
    /// git is not installed.
    pub fn committed(name: &str) -> Option<Fixture> {
        if !git_available() {
            return None;
        }
        let mut fixture = Fixture::new(name);
        fixture.git(&["init", "-q", "-b", "main"]);
        fixture.commit = Some(fixture.commit_all("examples"));
        Some(fixture)
    }

    /// Run git at the fixture's root; its stdout, trimmed.
    pub fn git(&self, args: &[&str]) -> String {
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(&self.root)
            .args(["-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"])
            .args(args);
        isolate(&mut command, &scratch());
        let output = command.output().expect("run git");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("utf-8")
            .trim()
            .to_string()
    }

    /// Commit every change; the new HEAD.
    pub fn commit_all(&self, message: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&[
            "commit",
            "-q",
            "--no-verify",
            "--allow-empty",
            "-m",
            message,
        ]);
        self.git(&["rev-parse", "HEAD"])
    }

    /// The text of a file of the fixture.
    pub fn read(&self, path: &str) -> String {
        std::fs::read_to_string(self.root.join(path)).unwrap_or_else(|e| panic!("read {path}: {e}"))
    }

    /// Replace a file of the fixture.
    pub fn write(&self, path: &str, text: &str) {
        let full = self.root.join(path);
        if let Some(dir) = full.parent() {
            std::fs::create_dir_all(dir).expect("create directory");
        }
        std::fs::write(full, text).unwrap_or_else(|e| panic!("write {path}: {e}"));
    }
}

/// The outcome of one `cpl` invocation.
#[derive(Debug)]
pub struct Run {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Run {
    fn from_output(output: Output) -> Run {
        Run {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8(output.stdout).expect("stdout is UTF-8"),
            stderr: String::from_utf8(output.stderr).expect("stderr is UTF-8"),
        }
    }

    /// Stdout as exactly one JSON object (Section 7.1: `--json` prints one
    /// envelope and nothing else; stderr is for crash traces only).
    #[track_caller]
    pub fn json(&self) -> Value {
        assert!(
            self.stderr.is_empty(),
            "--json wrote to stderr:\n{}",
            self.stderr
        );
        let mut stream = serde_json::Deserializer::from_str(&self.stdout).into_iter::<Value>();
        let value = match stream.next() {
            Some(Ok(value)) => value,
            other => panic!("stdout is not a JSON value ({other:?}):\n{}", self.stdout),
        };
        assert!(
            stream.next().is_none(),
            "stdout holds more than one JSON value:\n{}",
            self.stdout
        );
        assert!(value.is_object(), "the envelope is not an object: {value}");
        assert!(self.stdout.ends_with("}\n"), "one trailing newline");
        value
    }

    /// Stdout as JSON lines (`--jsonl`): every line one object.
    #[track_caller]
    pub fn json_lines(&self) -> Vec<Value> {
        assert!(
            self.stderr.is_empty(),
            "--jsonl wrote to stderr:\n{}",
            self.stderr
        );
        self.stdout
            .lines()
            .map(|line| {
                let value: Value = serde_json::from_str(line)
                    .unwrap_or_else(|e| panic!("not one JSON object per line ({e}): {line}"));
                assert!(value.is_object(), "{line}");
                value
            })
            .collect()
    }

    /// Assert the exit code, showing both streams when it differs.
    #[track_caller]
    pub fn expect_code(&self, code: i32) -> &Run {
        assert_eq!(
            self.code, code,
            "exit code\n--- stdout\n{}\n--- stderr\n{}",
            self.stdout, self.stderr
        );
        self
    }
}

/// Run `cpl <args>` in `dir`.
pub fn cpl(dir: &Path, args: &[&str]) -> Run {
    cpl_with_stdin(dir, args, None)
}

/// Run `cpl <args>` in `dir` with `input` on stdin.
pub fn cpl_with_stdin(dir: &Path, args: &[&str], input: Option<&str>) -> Run {
    cpl_full(dir, args, input, &[])
}

/// Run `cpl <args>` in `dir` with extra environment variables.
pub fn cpl_env(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Run {
    cpl_full(dir, args, None, env)
}

fn cpl_full(dir: &Path, args: &[&str], input: Option<&str>, env: &[(&str, &str)]) -> Run {
    use std::io::Write as _;
    let mut command = Command::new(cpl_bin());
    command.current_dir(dir).args(args);
    isolate(&mut command, &scratch());
    for (key, value) in env {
        command.env(key, value);
    }
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("run cpl");
    if let Some(input) = input {
        let mut stdin = child.stdin.take().expect("stdin");
        stdin.write_all(input.as_bytes()).expect("write stdin");
    }
    Run::from_output(child.wait_with_output().expect("wait for cpl"))
}

/// Where the empty git configuration of the children lives.
fn scratch() -> PathBuf {
    let base = Path::new(env!("CARGO_TARGET_TMPDIR")).join("cpl-cli");
    std::fs::create_dir_all(&base).expect("create the temporary base directory");
    base
}

/// The diagnostics of an envelope as sorted `(severity, code)` pairs: the
/// multiset a row of Section 12.3 states.
pub fn kinds(envelope: &Value) -> Vec<(String, String)> {
    let mut kinds: Vec<(String, String)> = envelope["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .map(|d| {
            (
                d["severity"].as_str().expect("severity").to_string(),
                d["code"].as_str().expect("code").to_string(),
            )
        })
        .collect();
    kinds.sort();
    kinds
}

/// `(severity, code)` pairs, sorted, from string pairs.
pub fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = expected
        .iter()
        .map(|(s, c)| (s.to_string(), c.to_string()))
        .collect();
    v.sort();
    v
}

/// The diagnostics of an envelope with a given code.
pub fn with_code<'a>(envelope: &'a Value, code: &str) -> Vec<&'a Value> {
    envelope["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .filter(|d| d["code"] == code)
        .collect()
}

// ---------------------------------------------------------------------------
// 12.4 normalization and golden files
// ---------------------------------------------------------------------------

/// The volatile values of one output (12.4).
#[derive(Default, Debug)]
pub struct Volatile {
    /// Commit ids (`revision.commit`, and the `source_revision` that
    /// restates them).
    pub commits: Vec<String>,
    /// `revision.digest` values.
    pub digests: Vec<String>,
    /// Absolute paths (temporary directories), replaced by `<tmp>`.
    pub paths: Vec<PathBuf>,
    /// An OS-chosen run seed (`cpl test` without `--seed`), with every
    /// `property_seed` derived from it.
    pub seed: Option<u64>,
    /// The ids `cpl id assign` drew from OS randomness (Section 4.1).
    pub generated_ids: Vec<String>,
}

impl Volatile {
    /// The commits and digests of every `revision` object in `json`, the
    /// fixture's temporary root, and, when `os_seed`, the run seed and the
    /// property seeds.
    pub fn of(json: &Value, fixture_root: Option<&Path>, os_seed: bool) -> Volatile {
        let mut v = Volatile::default();
        collect_revisions(json, &mut v);
        if let Some(root) = fixture_root {
            v.paths.push(root.to_path_buf());
            if let Ok(real) = root.canonicalize() {
                v.paths.push(real);
            }
        }
        if os_seed {
            v.seed = json["seed"].as_u64();
        }
        v
    }

    /// Also the `id` of every entry of a `cpl id assign` output's
    /// `assignments` (Section 7.2).
    pub fn with_generated_ids(mut self, json: &Value) -> Volatile {
        for assignment in json["assignments"].as_array().into_iter().flatten() {
            if let Some(id) = assignment["id"].as_str() {
                self.generated_ids.push(id.to_string());
            }
        }
        self
    }
}

/// Every revision object (Section 6.3: `{ commit, state, digest }`) under
/// any key: an envelope's or a receipt's `revision`, a diff's `old` and
/// `new`, an inspect response's `changes.since`; and the `base_commit` of
/// `cpl work check`, which restates a commit.
fn collect_revisions(json: &Value, v: &mut Volatile) {
    fn commit(commit: &str, v: &mut Volatile) {
        if !v.commits.iter().any(|c| c == commit) {
            v.commits.push(commit.to_string());
        }
    }
    match json {
        Value::Object(map) => {
            let is_revision = ["commit", "state", "digest"]
                .iter()
                .all(|key| map.contains_key(*key));
            if is_revision {
                if let Some(c) = map["commit"].as_str() {
                    commit(c, v);
                }
                if let Some(digest) = map["digest"].as_str() {
                    if !v.digests.iter().any(|d| d == digest) {
                        v.digests.push(digest.to_string());
                    }
                }
            }
            if let Some(c) = map.get("base_commit").and_then(Value::as_str) {
                commit(c, v);
            }
            for value in map.values() {
                collect_revisions(value, v);
            }
        }
        Value::Array(items) => items.iter().for_each(|i| collect_revisions(i, v)),
        _ => {}
    }
}

/// Replace the volatile fields of 12.4 in an output's text: `duration_ms`
/// values by `0`, `produced_at` values by `"<produced_at>"`, every commit
/// id by `<commit>`, every revision digest by `<digest>`, the temporary
/// directory by `<tmp>`, an OS-chosen seed by `<seed>` (its property
/// seeds by `<property_seed>`), and a generated id by its kind prefix and
/// `_<generated>` (`fn_<generated>`). Nothing else changes.
pub fn normalize(text: &str, v: &Volatile) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        out.push_str(&normalize_line(line, v));
    }
    for id in &v.generated_ids {
        let kind = id.split('_').next().unwrap_or("id");
        out = out.replace(id.as_str(), &format!("{kind}_<generated>"));
    }
    for commit in &v.commits {
        out = out.replace(commit.as_str(), "<commit>");
    }
    for digest in &v.digests {
        out = out.replace(digest.as_str(), "<digest>");
    }
    for path in &v.paths {
        out = out.replace(path.to_string_lossy().as_ref(), "<tmp>");
    }
    out
}

fn normalize_line(line: &str, v: &Volatile) -> String {
    let mut out = replace_number(line, "\"duration_ms\":", "0");
    out = replace_string(&out, "\"produced_at\":", "<produced_at>");
    if let Some(seed) = v.seed {
        out = out.replace(&format!("\"seed\": {seed}"), "\"seed\": \"<seed>\"");
        out = out.replace(&format!("\"seed\":{seed}"), "\"seed\":\"<seed>\"");
        out = replace_string(&out, "\"property_seed\":", "<property_seed>");
    }
    out
}

/// Every `key <digits>` (pretty or compact) with its digits replaced.
fn replace_number(line: &str, key: &str, by: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(at) = rest.find(key) {
        let (head, tail) = rest.split_at(at + key.len());
        out.push_str(head);
        let spaces = tail.len() - tail.trim_start_matches(' ').len();
        out.push_str(&tail[..spaces]);
        let tail = &tail[spaces..];
        let digits = tail.len() - tail.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits > 0 {
            out.push_str(by);
        }
        rest = &tail[digits..];
    }
    out.push_str(rest);
    out
}

/// Every `key "<text>"` with its string replaced by `"<by>"`.
fn replace_string(line: &str, key: &str, by: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(at) = rest.find(key) {
        let (head, tail) = rest.split_at(at + key.len());
        out.push_str(head);
        let spaces = tail.len() - tail.trim_start_matches(' ').len();
        out.push_str(&tail[..spaces]);
        let tail = &tail[spaces..];
        match tail
            .strip_prefix('"')
            .and_then(|t| t.find('"').map(|end| (t, end)))
        {
            Some((t, end)) => {
                out.push('"');
                out.push_str(by);
                out.push('"');
                rest = &t[end + 1..];
            }
            None => rest = tail,
        }
    }
    out.push_str(rest);
    out
}

/// True when golden files are to be rewritten (`UPDATE_GOLDEN=1`).
pub fn update_golden() -> bool {
    std::env::var("UPDATE_GOLDEN").is_ok_and(|v| v == "1")
}

/// Compare `actual` with `tests/golden/<name>` byte for byte, or rewrite
/// the file under `UPDATE_GOLDEN=1`.
#[track_caller]
pub fn check_golden(name: &str, actual: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if update_golden() {
        std::fs::create_dir_all(path.parent().expect("golden dir")).expect("create golden dir");
        std::fs::write(&path, actual).expect("write the golden file");
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e} (run with UPDATE_GOLDEN=1 to create it)",
            path.display()
        )
    });
    if expected != actual {
        let first = expected
            .lines()
            .zip(actual.lines())
            .position(|(e, a)| e != a)
            .unwrap_or_else(|| expected.lines().count().min(actual.lines().count()));
        panic!(
            "{} differs (first difference at line {}):\n--- golden\n{}\n--- actual\n{}\n(run with UPDATE_GOLDEN=1 to accept)",
            path.display(),
            first + 1,
            expected.lines().nth(first).unwrap_or("<end>"),
            actual.lines().nth(first).unwrap_or("<end>"),
        );
    }
}

/// Normalize a `--json` run and compare it with its golden file.
#[track_caller]
pub fn check_json_golden(name: &str, run: &Run, fixture: &Fixture, os_seed: bool) {
    let json = run.json();
    let volatile = Volatile::of(&json, Some(&fixture.root), os_seed);
    check_golden(name, &normalize(&run.stdout, &volatile));
}

/// [`check_json_golden`] for a `cpl id assign --json` run, whose ids are
/// normalized too.
#[track_caller]
pub fn check_id_assign_golden(name: &str, run: &Run, fixture: &Fixture) {
    let json = run.json();
    let volatile = Volatile::of(&json, Some(&fixture.root), false).with_generated_ids(&json);
    check_golden(name, &normalize(&run.stdout, &volatile));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn revisions_are_found_under_any_key() {
        let revision = |c: &str, d: &str| json!({ "commit": c, "state": "clean", "digest": d });
        let json = json!({
            "revision": revision("c1", "d1"),
            "old": revision("c2", "d2"),
            "changes": { "since": revision("c3", "d1") },
            "unversioned": { "commit": null, "state": "worktree", "digest": "d4" },
            "changes_list": [{ "base_commit": "c4" }],
            "commit": "not-a-revision",
        });
        let mut v = Volatile::of(&json, None, false);
        v.commits.sort();
        v.digests.sort();
        assert_eq!(v.commits, ["c1", "c2", "c3", "c4"]);
        assert_eq!(v.digests, ["d1", "d2", "d4"]);
    }

    #[test]
    fn normalization_touches_exactly_the_volatile_fields() {
        let v = Volatile {
            commits: vec!["c".repeat(40)],
            digests: vec!["d".repeat(64)],
            paths: vec![PathBuf::from("/tmp/x")],
            seed: Some(77),
            generated_ids: Vec::new(),
        };
        let text = format!(
            "\"duration_ms\": 12,\n\"duration_ms\":3,\"produced_at\":\"2026-01-01T00:00:00Z\"\n\"commit\": \"{}\"\n\"digest\": \"{}\"\n/tmp/x/a\n\"seed\": 77,\n\"property_seed\": \"ab\"\n\"cases\": 100\n",
            "c".repeat(40),
            "d".repeat(64)
        );
        assert_eq!(
            normalize(&text, &v),
            "\"duration_ms\": 0,\n\"duration_ms\":0,\"produced_at\":\"<produced_at>\"\n\"commit\": \"<commit>\"\n\"digest\": \"<digest>\"\n<tmp>/a\n\"seed\": \"<seed>\",\n\"property_seed\": \"<property_seed>\"\n\"cases\": 100\n"
        );
    }

    #[test]
    fn generated_ids_keep_their_kind() {
        let json = json!({ "assignments": [
            { "id": "fn_k7w3qz2mb6ptx4ve" }, { "id": "ty_aaaaaaaaaaaaaaaa" }
        ] });
        let v = Volatile::default().with_generated_ids(&json);
        assert_eq!(
            normalize(
                "\"id\": \"fn_k7w3qz2mb6ptx4ve\"\n\"id\": \"ty_aaaaaaaaaaaaaaaa\"\n\"id\": \"pgn_7c2d\"\n",
                &v
            ),
            "\"id\": \"fn_<generated>\"\n\"id\": \"ty_<generated>\"\n\"id\": \"pgn_7c2d\"\n"
        );
    }
}
