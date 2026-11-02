//! Adversarial tests of `cpl inspect`, `cpl prove`, `cpl diff`, `cpl work
//! check`, and the reserved `cpl work run` / `cpl work integrate`
//! (`spec/toolchain.md` 7.2), run as the built binary against temporary git
//! repositories and read with Sections 6 (the index and revisions), 8
//! (receipts), 9 (semantic diff), and 12.4 (CLI integration tests):
//!
//! * the revision of clean, dirty, unversioned, and commit-less trees, with
//!   the digest of 6.3 recomputed from the files on disk;
//! * projects at the top of a repository, below it, and below directories
//!   whose names are glob patterns (`GIT_LITERAL_PATHSPECS`);
//! * unknown revisions and the `A..B`, `A...B`, and `A` forms of `cpl diff`;
//! * every statement of 9.1 through the command line, with its `facet`, its
//!   `detail`, the order of statements, and the key order of 9.2;
//! * receipts: their inputs, digests, configuration, and key order, every
//!   digest recomputed with the reference BLAKE3 of this file;
//! * inspect targets, include sets, `changes`, and their errors;
//! * prove on `@prove` and plain functions, and the obligations of 8.1;
//! * work check on the corpus work file and on mutated copies;
//! * the reserved commands, and the commands with a `PATH` without `git`.
//!
//! Every expectation is the specification's; where it is silent a comment
//! names the reading taken and why it fits design-doc.md. Tests that need
//! git skip themselves when it is absent.

mod common;

use common::*;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

// ===========================================================================
// Reference digests (Sections 3.6, 5.3, 5.4, 6.3, 8)
// ===========================================================================

mod b3 {
    //! BLAKE3 in its hash mode with one 32-byte output, written out from the
    //! BLAKE3 specification so that every digest the commands print can be
    //! recomputed without the toolchain's own hashing.

    const IV: [u32; 8] = [
        0x6A09_E667,
        0xBB67_AE85,
        0x3C6E_F372,
        0xA54F_F53A,
        0x510E_527F,
        0x9B05_688C,
        0x1F83_D9AB,
        0x5BE0_CD19,
    ];
    const PERMUTATION: [usize; 16] = [2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8];
    const CHUNK_START: u32 = 1;
    const CHUNK_END: u32 = 2;
    const PARENT: u32 = 4;
    const ROOT: u32 = 8;

    fn g(s: &mut [u32; 16], (a, b, c, d): (usize, usize, usize, usize), x: u32, y: u32) {
        s[a] = s[a].wrapping_add(s[b]).wrapping_add(x);
        s[d] = (s[d] ^ s[a]).rotate_right(16);
        s[c] = s[c].wrapping_add(s[d]);
        s[b] = (s[b] ^ s[c]).rotate_right(12);
        s[a] = s[a].wrapping_add(s[b]).wrapping_add(y);
        s[d] = (s[d] ^ s[a]).rotate_right(8);
        s[c] = s[c].wrapping_add(s[d]);
        s[b] = (s[b] ^ s[c]).rotate_right(7);
    }

    fn compress(cv: &[u32; 8], block: &[u32; 16], counter: u64, len: u32, flags: u32) -> [u32; 16] {
        let mut s = [
            cv[0],
            cv[1],
            cv[2],
            cv[3],
            cv[4],
            cv[5],
            cv[6],
            cv[7],
            IV[0],
            IV[1],
            IV[2],
            IV[3],
            counter as u32,
            (counter >> 32) as u32,
            len,
            flags,
        ];
        let mut m = *block;
        for round in 0..7 {
            g(&mut s, (0, 4, 8, 12), m[0], m[1]);
            g(&mut s, (1, 5, 9, 13), m[2], m[3]);
            g(&mut s, (2, 6, 10, 14), m[4], m[5]);
            g(&mut s, (3, 7, 11, 15), m[6], m[7]);
            g(&mut s, (0, 5, 10, 15), m[8], m[9]);
            g(&mut s, (1, 6, 11, 12), m[10], m[11]);
            g(&mut s, (2, 7, 8, 13), m[12], m[13]);
            g(&mut s, (3, 4, 9, 14), m[14], m[15]);
            if round < 6 {
                m = std::array::from_fn(|i| m[PERMUTATION[i]]);
            }
        }
        let (low, high) = s.split_at_mut(8);
        for ((l, h), c) in low.iter_mut().zip(high.iter_mut()).zip(cv) {
            *l ^= *h;
            *h ^= c;
        }
        s
    }

    fn words(block: &[u8]) -> [u32; 16] {
        let mut bytes = [0u8; 64];
        bytes[..block.len()].copy_from_slice(block);
        std::array::from_fn(|i| {
            u32::from_le_bytes([
                bytes[4 * i],
                bytes[4 * i + 1],
                bytes[4 * i + 2],
                bytes[4 * i + 3],
            ])
        })
    }

    fn first8(s: &[u32; 16]) -> [u32; 8] {
        std::array::from_fn(|i| s[i])
    }

    /// A node that is not compressed yet: the root gets the ROOT flag.
    struct Output {
        cv: [u32; 8],
        block: [u32; 16],
        counter: u64,
        len: u32,
        flags: u32,
    }

    impl Output {
        fn chaining_value(&self) -> [u32; 8] {
            first8(&compress(
                &self.cv,
                &self.block,
                self.counter,
                self.len,
                self.flags,
            ))
        }

        fn root_hex(&self) -> String {
            let s = compress(&self.cv, &self.block, 0, self.len, self.flags | ROOT);
            s[..8]
                .iter()
                .flat_map(|w| w.to_le_bytes())
                .map(|b| format!("{b:02x}"))
                .collect()
        }
    }

    fn chunk(bytes: &[u8], counter: u64) -> Output {
        let blocks: Vec<&[u8]> = if bytes.is_empty() {
            vec![bytes]
        } else {
            bytes.chunks(64).collect()
        };
        let mut cv = IV;
        for (i, block) in blocks.iter().enumerate() {
            let start = if i == 0 { CHUNK_START } else { 0 };
            if i + 1 == blocks.len() {
                return Output {
                    cv,
                    block: words(block),
                    counter,
                    len: block.len() as u32,
                    flags: start | CHUNK_END,
                };
            }
            cv = first8(&compress(&cv, &words(block), counter, 64, start));
        }
        unreachable!("a chunk has at least one block")
    }

    fn parent(left: [u32; 8], right: [u32; 8]) -> Output {
        let mut block = [0u32; 16];
        block[..8].copy_from_slice(&left);
        block[8..].copy_from_slice(&right);
        Output {
            cv: IV,
            block,
            counter: 0,
            len: 64,
            flags: PARENT,
        }
    }

    /// blake3 of `input`, as 64 lowercase hex digits.
    pub fn hex(input: &[u8]) -> String {
        let chunks: Vec<&[u8]> = if input.is_empty() {
            vec![input]
        } else {
            input.chunks(1024).collect()
        };
        let mut stack: Vec<[u32; 8]> = Vec::new();
        for (i, bytes) in chunks.iter().enumerate() {
            let out = chunk(bytes, i as u64);
            if i + 1 == chunks.len() {
                let mut out = out;
                while let Some(left) = stack.pop() {
                    out = parent(left, out.chaining_value());
                }
                return out.root_hex();
            }
            let mut cv = out.chaining_value();
            let mut total = i as u64 + 1;
            while total & 1 == 0 {
                cv = parent(stack.pop().expect("a left subtree"), cv).chaining_value();
                total >>= 1;
            }
            stack.push(cv);
        }
        unreachable!("an input has at least one chunk")
    }
}

/// The eight facets in `Facet::ALL` order (README "Facet names").
const FACETS: [&str; 8] = [
    "identity",
    "name",
    "signature",
    "contract",
    "effects",
    "body",
    "docs",
    "verification_policy",
];

/// Section 5.3: the first 32 hex digits of blake3 of a canonical text.
fn facet_hash(text: &str) -> String {
    b3::hex(text.as_bytes())[..32].to_string()
}

/// Section 5.4 step 2: `facet_hash` of one `<facet>=<hash>` line per
/// applicable facet in `Facet::ALL` order, joined with LF.
fn definition_hash(facets: &Value) -> String {
    let lines: Vec<String> = FACETS
        .iter()
        .filter_map(|f| {
            facets
                .get(*f)
                .map(|h| format!("{f}={}", h.as_str().expect("a hash")))
        })
        .collect();
    facet_hash(&lines.join("\n"))
}

/// The source set of the project at `root`, read from disk (Section 0,
/// T54): `cpl.toml`; every `*.cpl` file with a stem under the `sources`
/// directories, recursively, a link to a file read as that file and a link
/// to a directory not followed; every `*.work` and `*.patch` file directly
/// under `work/`. Keyed by project-relative path, `/`-separated.
fn source_files(root: &Path, sources: &[&str]) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    files.insert(
        "cpl.toml".to_string(),
        std::fs::read(root.join("cpl.toml")).expect("read cpl.toml"),
    );
    for dir in sources {
        let base = if *dir == "." {
            root.to_path_buf()
        } else {
            root.join(dir)
        };
        collect_cpl(&base, root, &mut files);
    }
    if let Ok(entries) = std::fs::read_dir(root.join("work")) {
        for entry in entries {
            let entry = entry.expect("a directory entry");
            let name = entry.file_name().to_string_lossy().into_owned();
            let member = [".work", ".patch"]
                .iter()
                .any(|ext| name.len() > ext.len() && name.ends_with(ext));
            if member && entry.path().is_file() {
                files.insert(
                    format!("work/{name}"),
                    std::fs::read(entry.path()).expect("read a work file"),
                );
            }
        }
    }
    files
}

fn collect_cpl(dir: &Path, root: &Path, files: &mut BTreeMap<String, Vec<u8>>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let entry = entry.expect("a directory entry");
        let path = entry.path();
        if entry.file_type().expect("a file type").is_dir() {
            collect_cpl(&path, root, files);
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.len() > ".cpl".len() && name.ends_with(".cpl") && path.is_file() {
            let relative: Vec<String> = path
                .strip_prefix(root)
                .expect("below the project root")
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            files.insert(
                relative.join("/"),
                std::fs::read(&path).expect("read a source file"),
            );
        }
    }
}

/// Section 6.3: blake3 of one `<path> <hex blake3 of bytes>` LF line per
/// file of the source set, in path order.
fn source_digest(files: &BTreeMap<String, Vec<u8>>) -> String {
    let lines: String = files
        .iter()
        .map(|(path, bytes)| format!("{path} {}\n", b3::hex(bytes)))
        .collect();
    b3::hex(lines.as_bytes())
}

/// The digest of a project with the default `[build] sources`.
fn project_digest(root: &Path) -> String {
    source_digest(&source_files(root, &["src"]))
}

/// The `inputs.sources` object of a receipt (Section 8).
fn source_digests(files: &BTreeMap<String, Vec<u8>>) -> BTreeMap<String, String> {
    files
        .iter()
        .map(|(path, bytes)| (path.clone(), b3::hex(bytes)))
        .collect()
}

/// Section 3.6: blake3 of `name LF content LF` for every std file, in
/// sorted file-name order.
fn std_digest() -> String {
    let dir = workspace().join("std");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .expect("read std")
        .map(|e| {
            e.expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|n| n.ends_with(".cpl"))
        .collect();
    names.sort();
    let mut text = Vec::new();
    for name in names {
        text.extend_from_slice(name.as_bytes());
        text.push(b'\n');
        text.extend(std::fs::read(dir.join(&name)).expect("read a std file"));
        text.push(b'\n');
    }
    b3::hex(&text)
}

/// Section 8: every object's keys sorted by their UTF-8 bytes, no
/// whitespace, strings and numbers as serde_json writes them.
fn canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            out.push('{');
            for (i, (key, item)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key).expect("a key"));
                out.push(':');
                canonical(item, out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical(item, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&serde_json::to_string(scalar).expect("a scalar")),
    }
}

/// Section 8: `receipt_digest`, blake3 of the canonical receipt without its
/// top-level `produced_at` and `duration_ms`.
fn receipt_digest(receipt: &Value) -> String {
    let mut top = receipt.as_object().expect("a receipt object").clone();
    top.remove("produced_at");
    top.remove("duration_ms");
    let mut text = String::new();
    canonical(&Value::Object(top), &mut text);
    b3::hex(text.as_bytes())
}

// ===========================================================================
// Key order (Section 7.1: keys in declaration order; 9.2 for `detail`)
// ===========================================================================

/// A JSON value with its object keys in document order (a parsed
/// `serde_json::Value` sorts them).
#[derive(Debug)]
enum Ordered {
    /// The raw text of a string, number, `true`, `false`, or `null`.
    Scalar(String),
    Array(Vec<Ordered>),
    Object(Vec<(String, Ordered)>),
}

impl Ordered {
    #[track_caller]
    fn parse(text: &str) -> Ordered {
        let mut p = OrderedParser {
            s: text.as_bytes(),
            i: 0,
        };
        let value = p.value();
        p.ws();
        assert_eq!(p.i, p.s.len(), "one JSON value");
        value
    }

    /// The keys of an object, in document order.
    #[track_caller]
    fn keys(&self) -> Vec<&str> {
        match self {
            Ordered::Object(entries) => entries.iter().map(|(k, _)| k.as_str()).collect(),
            other => panic!("not an object: {other:?}"),
        }
    }

    #[track_caller]
    fn get(&self, key: &str) -> &Ordered {
        match self {
            Ordered::Object(entries) => entries
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v)
                .unwrap_or_else(|| panic!("no key {key} in {:?}", self.keys())),
            other => panic!("not an object: {other:?}"),
        }
    }

    #[track_caller]
    fn items(&self) -> &[Ordered] {
        match self {
            Ordered::Array(items) => items,
            other => panic!("not an array: {other:?}"),
        }
    }

    /// A string scalar, decoded.
    #[track_caller]
    fn text(&self) -> String {
        match self {
            Ordered::Scalar(raw) => serde_json::from_str(raw).expect("a JSON string"),
            other => panic!("not a scalar: {other:?}"),
        }
    }
}

struct OrderedParser<'a> {
    s: &'a [u8],
    i: usize,
}

impl OrderedParser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn value(&mut self) -> Ordered {
        self.ws();
        match self.s[self.i] {
            b'{' => {
                self.i += 1;
                let mut entries = Vec::new();
                self.ws();
                if self.s[self.i] == b'}' {
                    self.i += 1;
                    return Ordered::Object(entries);
                }
                loop {
                    self.ws();
                    let key = self.string();
                    self.ws();
                    assert_eq!(self.s[self.i], b':', "a colon at {}", self.i);
                    self.i += 1;
                    let value = self.value();
                    entries.push((key, value));
                    self.ws();
                    let c = self.s[self.i];
                    self.i += 1;
                    match c {
                        b',' => {}
                        b'}' => return Ordered::Object(entries),
                        other => panic!("unexpected {} at {}", other as char, self.i),
                    }
                }
            }
            b'[' => {
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.s[self.i] == b']' {
                    self.i += 1;
                    return Ordered::Array(items);
                }
                loop {
                    items.push(self.value());
                    self.ws();
                    let c = self.s[self.i];
                    self.i += 1;
                    match c {
                        b',' => {}
                        b']' => return Ordered::Array(items),
                        other => panic!("unexpected {} at {}", other as char, self.i),
                    }
                }
            }
            b'"' => {
                let start = self.i;
                self.string();
                Ordered::Scalar(String::from_utf8_lossy(&self.s[start..self.i]).into_owned())
            }
            _ => {
                let start = self.i;
                while self.i < self.s.len()
                    && !matches!(self.s[self.i], b',' | b'}' | b']')
                    && !self.s[self.i].is_ascii_whitespace()
                {
                    self.i += 1;
                }
                Ordered::Scalar(String::from_utf8_lossy(&self.s[start..self.i]).into_owned())
            }
        }
    }

    /// A string literal, decoded.
    fn string(&mut self) -> String {
        assert_eq!(self.s[self.i], b'"', "a string at {}", self.i);
        let start = self.i;
        self.i += 1;
        while self.s[self.i] != b'"' {
            self.i += if self.s[self.i] == b'\\' { 2 } else { 1 };
        }
        self.i += 1;
        serde_json::from_slice(&self.s[start..self.i]).expect("a JSON string")
    }
}

/// Assert that `keys` are declared keys, in their declaration order.
#[track_caller]
fn assert_declared_order(keys: &[&str], declared: &[&str]) {
    let ranks: Vec<usize> = keys
        .iter()
        .map(|k| {
            declared
                .iter()
                .position(|d| d == k)
                .unwrap_or_else(|| panic!("undeclared key {k} in {keys:?}"))
        })
        .collect();
    assert!(
        ranks.windows(2).all(|w| w[0] < w[1]),
        "keys {keys:?} are not in the declared order {declared:?}"
    );
}

/// The field order of `SymbolBlock` (Section 6.4).
const BLOCK_KEYS: [&str; 19] = [
    "id",
    "path",
    "provisional",
    "facets",
    "definition",
    "kind",
    "module",
    "public",
    "location",
    "verification_policy",
    "signature",
    "contract",
    "effects",
    "docs",
    "implementation",
    "holes",
    "relation",
    "interface",
    "call_sites",
];

// ===========================================================================
// Helpers
// ===========================================================================

const PAGES: &str = "reader/src/reader/pages.cpl";
const TESTS_CPL: &str = "reader/src/tests/pagination.cpl";
const WORK: &str = "reader/work/pagination.work";

const TESTS: [&str; 3] = [
    "tests.pagination.\"pagination handles empty input\"",
    "tests.pagination.\"pagination preserves a short tail\"",
    "tests.pagination.\"pagination preserves input\"",
];

/// The contract clauses of `paginate` (its `contract` facet, one per line).
const PAGINATE_CONTRACT: [&str; 3] = [
    "ensures flatten(result) == items",
    "ensures forall page in result: page.len > 0 and page.len <= size",
    "ensures forall page in result.drop_last(): page.len == size",
];

/// A module beside `reader.pages`, canonically formatted, whose functions
/// have the obligation kinds of Section 8.1 that `reader.pages` lacks.
const EXTRA: &str = "module reader.extra

use reader.pages.{paginate}

pub fn two_pages(items: List<Int>) -> List<List<Int>> {
    paginate(items, 2)
}

fn checked(n: Int) -> Int {
    assert n == n
    n
}

fn forever(n: Int) -> Int {
    if n <= 0 {
        0
    } else {
        forever(n - 1)
    }
}

fn count(n: Int) -> Int
    requires n >= 0
    decreases n
{
    if n == 0 {
        0
    } else {
        1 + count(n - 1)
    }
}
";

/// Two `@prove` functions whose termination is unknown (semantics 5.4).
const LOOPS: &str = "module reader.loops

@prove
fn spin(n: Int) -> Int {
    if n <= 0 {
        0
    } else {
        spin(n - 1)
    }
}

@prove
fn walk(xs: List<Int>) -> Int {
    var rest = xs
    while not rest.is_empty() {
        rest = rest.drop(1)
    }
    0
}
";

/// A `@prove` function with nested and sequential loops, asserts, and
/// divisions (canonically formatted).
const NEST: &str = "module reader.nest

@prove
fn loops(n: Int) -> Int
    requires n >= 0
{
    var i = 0
    while i < n
        invariant i <= n
        decreases n - i
    {
        var j = 0
        while j < i
            invariant j <= i
            decreases i - j
        {
            j = j + 1
        }
        i = i + 1
    }
    var k = 0
    while k < n
        invariant k <= n
        decreases n - k
    {
        k = k + 1
    }
    assert k == n
    assert i == n
    k / 1 + n % 1
}
";

/// A capability with two operations and a function bounded by both.
const STORE: &str = "module reader.store

pub capability Store {
    fn get(key: Text) -> Int
    fn put(key: Text, value: Int) -> Int
}

pub fn touch(key: Text, using db: Store) -> Int
    effects { db.put, db.get }
{
    db.put(key, db.get(key))
}
";

/// The commit of a committed fixture.
fn head(fx: &Fixture) -> String {
    fx.commit.clone().expect("a committed fixture")
}

/// A fixture whose repository is at `root` (a project at the top level of
/// its repository, or one deeper below it).
fn fixture_at(root: PathBuf) -> Fixture {
    Fixture {
        reader: root.clone(),
        holes: root.clone(),
        root,
        commit: None,
    }
}

/// Replace the first `from` in a fixture file by `to`.
#[track_caller]
fn edit(fx: &Fixture, path: &str, from: &str, to: &str) {
    let before = fx.read(path);
    assert!(before.contains(from), "{from:?} in {path}");
    fx.write(path, &before.replacen(from, to, 1));
}

/// Edit `paginate`'s second `ensures` clause: only its contract changes.
fn edit_contract(fx: &Fixture) {
    edit(
        fx,
        PAGES,
        "page.len > 0 and page.len <= size",
        "page.len >= 1 and page.len <= size",
    );
}

/// `cpl <args> --json` in `dir`: the exit code and the envelope.
#[track_caller]
fn json_of(dir: &Path, args: &[&str]) -> (i32, Value) {
    json_env(dir, args, &[])
}

/// [`json_of`] with extra environment variables.
#[track_caller]
fn json_env(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> (i32, Value) {
    let mut all = args.to_vec();
    all.push("--json");
    let run = cpl_env(dir, &all, env);
    (run.code, run.json())
}

/// A `PATH` with no `git` on it: an empty directory.
fn path_without_git() -> String {
    temp_dir("adv-no-git-path")
        .to_str()
        .expect("a UTF-8 path")
        .to_string()
}

/// The one diagnostic with `code`.
#[track_caller]
fn only<'a>(env: &'a Value, code: &str) -> &'a Value {
    let found = with_code(env, code);
    assert_eq!(found.len(), 1, "exactly one {code} in {env}");
    found[0]
}

/// The codes of the diagnostics with `severity`, sorted.
fn codes_with(env: &Value, severity: &str) -> Vec<String> {
    let mut codes: Vec<String> = env["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .filter(|d| d["severity"] == severity)
        .map(|d| d["code"].as_str().expect("a code").to_string())
        .collect();
    codes.sort();
    codes
}

/// `(statement, path)` of every statement of a diff, in order.
fn statements(env: &Value) -> Vec<(String, String)> {
    env["statements"]
        .as_array()
        .expect("statements")
        .iter()
        .map(|s| {
            (
                s["statement"].as_str().expect("a statement").to_string(),
                s["path"].as_str().expect("a path").to_string(),
            )
        })
        .collect()
}

fn st(statement: &str, path: &str) -> (String, String) {
    (statement.to_string(), path.to_string())
}

/// The `SymbolVersion` part of a symbol block.
fn version_of(block: &Value) -> Value {
    json!({
        "id": block["id"],
        "path": block["path"],
        "provisional": block["provisional"],
        "facets": block["facets"],
        "definition": block["definition"],
    })
}

/// The keys of a JSON object, sorted.
fn keys(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value
        .as_object()
        .expect("an object")
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}

/// The `module` of every entry of a `modules` list.
fn module_names(modules: &Value) -> Vec<String> {
    modules
        .as_array()
        .expect("modules")
        .iter()
        .map(|m| m["module"].as_str().expect("module").to_string())
        .collect()
}

/// The obligations of a `cpl prove` envelope by id.
fn obligations_by_id(env: &Value) -> BTreeMap<String, Value> {
    let list = env["obligations"].as_array().expect("obligations");
    let map: BTreeMap<String, Value> = list
        .iter()
        .map(|o| (o["id"].as_str().expect("an id").to_string(), o.clone()))
        .collect();
    assert_eq!(map.len(), list.len(), "obligation ids are unique: {list:?}");
    map
}

/// The name the third human line of `cpl prove` gives the receipt
/// (`  receipt: <digest>  (blake3 of the receipt)`).
#[track_caller]
fn human_receipt_name(dir: &Path, target: &str) -> String {
    let run = cpl(dir, &["prove", target]);
    run.expect_code(1);
    let line = run.stdout.lines().nth(2).expect("a third line").to_string();
    let rest = line
        .strip_prefix("  receipt: ")
        .unwrap_or_else(|| panic!("the receipt line: {line:?}"));
    assert!(rest.ends_with("(blake3 of the receipt)"), "{line:?}");
    rest.split("  ").next().expect("a name").to_string()
}

// ===========================================================================
// The reference digests agree with the toolchain where both are defined
// ===========================================================================

#[test]
fn the_reference_blake3_agrees_with_the_toolchain() {
    assert_eq!(
        b3::hex(b""),
        "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
    );
    // The test vector of Section 5.3.
    assert_eq!(facet_hash(""), "af1349b9f5f9a1a6a0404dea36dcc949");
    // cpl_project::std_digest is blake3 of `name LF content LF` (3.6): an
    // oracle for every text with a line feed inside and one at its end,
    // across block, chunk, and tree boundaries.
    for total in [
        6usize, 7, 63, 64, 65, 127, 128, 129, 1023, 1024, 1025, 2047, 2048, 2049, 3072, 3073, 4095,
        4096, 4097, 5120, 7169, 8192, 8193, 16384, 16385, 31745, 32768, 40000,
    ] {
        let content: String = (0..total - 6)
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect();
        let text = format!("name\n{content}\n");
        assert_eq!(text.len(), total);
        let oracle = cpl_project::std_digest(&[("name", content.as_str())]);
        assert_eq!(b3::hex(text.as_bytes()), oracle.0, "{total} bytes");
    }
}

// ===========================================================================
// Revisions (Section 6.3)
// ===========================================================================

#[test]
fn a_clean_tree_is_head_and_clean_with_the_digest_of_its_files() {
    let Some(fx) = Fixture::committed("adv-clean") else {
        return;
    };
    let commit = head(&fx);
    assert!(
        commit.len() == 40
            && commit
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "{commit}"
    );
    let digest = project_digest(&fx.reader);
    for args in [
        vec!["inspect", "paginate"],
        vec!["prove", "paginate"],
        vec!["diff", "HEAD"],
        vec!["work", "check", "work/pagination.work"],
    ] {
        let (_, env) = json_of(&fx.reader, &args);
        assert_eq!(
            env["revision"],
            json!({ "commit": commit, "state": "clean", "digest": digest }),
            "{args:?}"
        );
    }
}

#[test]
fn without_a_repository_the_revision_is_unversioned_with_the_same_digest() {
    let fx = Fixture::new("adv-unversioned");
    let digest = project_digest(&fx.reader);
    for args in [vec!["inspect", "paginate"], vec!["prove", "paginate"]] {
        let (_, env) = json_of(&fx.reader, &args);
        assert_eq!(
            env["revision"],
            json!({ "commit": null, "state": "worktree", "digest": digest }),
            "{args:?}"
        );
    }
    let (_, env) = json_of(&fx.reader, &["prove", "paginate"]);
    let core = &env["receipt"]["core"];
    assert_eq!(core["source_revision"], "unversioned");
    assert!(core.get("dirty").is_none(), "{core}");
    assert_eq!(env["receipt"]["revision"], env["revision"]);
    // The digest is the source set's, not git's: a committed copy of the
    // same files has the same one.
    if let Some(twin) = Fixture::committed("adv-unversioned-twin") {
        let (_, env) = json_of(&twin.reader, &["inspect", "paginate"]);
        assert_eq!(env["revision"]["digest"], json!(digest));
    }
}

#[test]
fn editing_a_member_makes_the_tree_worktree_with_a_new_digest() {
    let Some(fx) = Fixture::committed("adv-edit-member") else {
        return;
    };
    let committed = project_digest(&fx.reader);
    for (path, from, to) in [
        (PAGES, "fn clean(", "// tidied\nfn clean("),
        (WORK, "    workers 3\n", "    workers 2\n"),
        (
            "reader/work/pagination.patch",
            "    pages\n}",
            "    pages\n}\n",
        ),
        (
            "reader/cpl.toml",
            "edition = \"0.1\"\n",
            "edition = \"0.1\"\ndescription = \"the corpus\"\n",
        ),
    ] {
        let original = fx.read(path);
        edit(&fx, path, from, to);
        let digest = project_digest(&fx.reader);
        assert_ne!(digest, committed, "{path}");
        for args in [vec!["inspect", "paginate"], vec!["prove", "paginate"]] {
            let (_, env) = json_of(&fx.reader, &args);
            assert_eq!(
                env["revision"],
                json!({ "commit": head(&fx), "state": "worktree", "digest": digest }),
                "{path}: {args:?}"
            );
        }
        fx.write(path, &original);
    }
    let (_, env) = json_of(&fx.reader, &["inspect", "paginate"]);
    assert_eq!(env["revision"]["state"], "clean");
    assert_eq!(env["revision"]["digest"], json!(committed));
}

#[test]
fn staged_and_deleted_members_make_the_tree_worktree() {
    let Some(fx) = Fixture::committed("adv-staged-deleted") else {
        return;
    };
    // A staged edit is still an uncommitted one.
    edit(&fx, PAGES, "fn clean(", "// tidied\nfn clean(");
    fx.git(&["add", "-A"]);
    let (_, env) = json_of(&fx.reader, &["inspect", "paginate"]);
    assert_eq!(
        env["revision"],
        json!({ "commit": head(&fx), "state": "worktree", "digest": project_digest(&fx.reader) })
    );
    fx.git(&["commit", "-q", "--no-verify", "-m", "a staged edit"]);
    let commit = fx.git(&["rev-parse", "HEAD"]);
    // A deleted member leaves the source set and the digest.
    std::fs::remove_file(fx.reader.join("src/reader/images.cpl")).expect("remove images.cpl");
    let files = source_files(&fx.reader, &["src"]);
    assert!(!files.contains_key("src/reader/images.cpl"));
    let (code, env) = json_of(&fx.reader, &["inspect", "paginate"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(
        env["revision"],
        json!({ "commit": commit, "state": "worktree", "digest": source_digest(&files) })
    );
    let (_, env) = json_of(&fx.reader, &["diff", "HEAD"]);
    assert_eq!(
        statements(&env),
        [
            st("identity_removed", "reader.images.Dimensions"),
            st("identity_removed", "reader.images.ImageCodec"),
            st("identity_removed", "reader.images.image_width"),
        ]
    );
}

#[test]
fn a_detached_head_is_still_the_commit_of_the_tree() {
    let Some(fx) = Fixture::committed("adv-detached") else {
        return;
    };
    fx.git(&["checkout", "-q", "--detach"]);
    let (_, env) = json_of(&fx.reader, &["inspect", "paginate"]);
    assert_eq!(env["revision"]["commit"], json!(head(&fx)));
    assert_eq!(env["revision"]["state"], "clean");
    // `main` is still a branch, and still resolves.
    let result = json_of(&fx.reader, &["work", "check", "work/pagination.work"]);
    assert_work(&result, 0, &[PROVISIONAL]);
    assert_eq!(result.1["changes"][0]["base_commit"], json!(head(&fx)));
}

#[test]
fn untracked_members_make_the_tree_worktree_and_enter_the_digest() {
    let Some(fx) = Fixture::committed("adv-untracked-member") else {
        return;
    };
    let committed = project_digest(&fx.reader);
    fx.write(
        "reader/src/reader/extra.cpl",
        "module reader.extra\n\npub fn extra() -> Int {\n    1\n}\n",
    );
    fx.write("reader/work/second.work", &fx.read(WORK));
    let files = source_files(&fx.reader, &["src"]);
    assert!(files.contains_key("src/reader/extra.cpl") && files.contains_key("work/second.work"));
    let digest = source_digest(&files);
    assert_ne!(digest, committed);
    let (code, env) = json_of(&fx.reader, &["inspect", "reader.extra.extra"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(
        env["revision"],
        json!({ "commit": head(&fx), "state": "worktree", "digest": digest })
    );
    // A receipt's sources are exactly the source set, each by its digest.
    let (_, env) = json_of(&fx.reader, &["prove", "reader.extra.extra"]);
    assert_eq!(
        env["receipt"]["inputs"]["sources"],
        json!(source_digests(&files))
    );
}

#[test]
fn untracked_files_under_a_root_that_are_no_members_change_the_state_but_not_the_digest() {
    let Some(fx) = Fixture::committed("adv-untracked-other") else {
        return;
    };
    let digest = project_digest(&fx.reader);
    for (path, text) in [
        ("reader/src/notes.txt", "notes\n"),
        // Section 0: a file named just `.cpl` is not a source file.
        ("reader/src/reader/.cpl", "this is no source file {\n"),
        // Only files directly under work/ are members.
        ("reader/work/deep/x.work", "change {\n"),
        ("reader/work/notes.md", "# notes\n"),
    ] {
        fx.write(path, text);
        let (code, env) = json_of(&fx.reader, &["inspect", "paginate"]);
        assert_eq!(code, 0, "{path}: {env}");
        assert_eq!(env["diagnostics"], json!([]), "{path}");
        assert_eq!(
            env["revision"],
            json!({ "commit": head(&fx), "state": "worktree", "digest": digest }),
            "{path}"
        );
        std::fs::remove_file(fx.root.join(path)).expect("remove the file");
    }
}

#[test]
fn changes_outside_the_source_roots_leave_the_tree_clean() {
    let Some(fx) = Fixture::committed("adv-outside-roots") else {
        return;
    };
    let digest = project_digest(&fx.reader);
    let commit = head(&fx);
    let still_clean = |label: &str| {
        let (code, env) = json_of(&fx.reader, &["inspect", "paginate"]);
        assert_eq!(code, 0, "{label}: {env}");
        assert_eq!(
            env["revision"],
            json!({ "commit": commit, "state": "clean", "digest": digest }),
            "{label}"
        );
    };
    fx.write("reader/README.md", "# reader\n");
    still_clean("a README beside cpl.toml");
    fx.write(
        "reader/notes.cpl",
        "module reader.notes\n\nfn n() -> Int {\n    1\n}\n",
    );
    still_clean("a .cpl file outside the source directories");
    edit(
        &fx,
        "holes/src/holes/pages.cpl",
        "?summary",
        "?summary_text",
    );
    still_clean("an edit in a sibling project");
    cpl(&fx.reader, &["check", "--lock"]).expect_code(0);
    assert!(fx.reader.join("cpl.lock").is_file());
    still_clean("an untracked cpl.lock, which is read but no member");
}

#[test]
fn a_gitignored_member_is_clean_but_part_of_the_digest() {
    let Some(fx) = Fixture::committed("adv-ignored-member") else {
        return;
    };
    fx.write("reader/.gitignore", "src/reader/gen.cpl\n");
    let commit = fx.commit_all("ignore a generated module");
    let committed = project_digest(&fx.reader);
    fx.write(
        "reader/src/reader/gen.cpl",
        "module reader.gen\n\npub fn generated() -> Int {\n    1\n}\n",
    );
    let digest = project_digest(&fx.reader);
    assert_ne!(digest, committed);
    let (code, env) = json_of(&fx.reader, &["inspect", "reader.gen.generated"]);
    assert_eq!(code, 0, "{env}");
    // 6.3: `clean` is git's answer, the digest the loaded tree's.
    assert_eq!(
        env["revision"],
        json!({ "commit": commit, "state": "clean", "digest": digest })
    );
    // The commit's own tree does not hold the file.
    let (_, env) = json_of(&fx.reader, &["diff", "HEAD"]);
    assert_eq!(
        statements(&env),
        [st("identity_added", "reader.gen.generated")]
    );
    assert_eq!(env["old"]["digest"], json!(committed));
    assert_eq!(env["new"]["digest"], json!(digest));
}

#[test]
fn a_checkout_filter_leaves_the_state_clean_and_the_digest_the_loaded_trees() {
    let Some(fx) = Fixture::committed("adv-eol") else {
        return;
    };
    let committed = project_digest(&fx.reader);
    fx.write("reader/.gitattributes", "*.cpl text eol=crlf\n");
    let commit = fx.commit_all("line endings");
    // Check the sources out again, through the filter.
    std::fs::remove_dir_all(fx.reader.join("src")).expect("remove the sources");
    fx.git(&["checkout", "--", "reader/src"]);
    let pages = std::fs::read(fx.reader.join("src/reader/pages.cpl")).expect("read pages.cpl");
    if !pages.windows(2).any(|w| w == b"\r\n") {
        eprintln!("skipped: git wrote no CRLF line ends");
        return;
    }
    let digest = project_digest(&fx.reader);
    assert_ne!(digest, committed);
    // 6.3: `clean` is git's answer, not a comparison of digests, and the
    // digest is always the loaded tree's.
    let (code, env) = json_of(&fx.reader, &["inspect", "paginate"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(
        env["revision"],
        json!({ "commit": commit, "state": "clean", "digest": digest })
    );
    // Facets do not depend on line ends: no statement between the two.
    let (code, env) = json_of(&fx.reader, &["diff", "HEAD"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(env["statements"], json!([]));
    assert_eq!(env["old"]["digest"], json!(committed));
    assert_eq!(env["new"]["digest"], json!(digest));
}

#[test]
fn a_repository_without_commits_has_no_commit_and_resolves_nothing() {
    if !git_available() {
        return;
    }
    let fx = Fixture::new("adv-no-commits");
    fx.git(&["init", "-q", "-b", "main"]);
    let digest = project_digest(&fx.reader);
    let (code, env) = json_of(&fx.reader, &["inspect", "paginate"]);
    assert_eq!(code, 0);
    assert_eq!(
        env["revision"],
        json!({ "commit": null, "state": "worktree", "digest": digest })
    );
    assert_eq!(env["diagnostics"], json!([]));
    // A receipt then names no commit: there is none (semantics S48).
    let (code, env) = json_of(&fx.reader, &["prove", "paginate"]);
    assert_eq!(code, 1);
    assert_eq!(env["receipt"]["core"]["source_revision"], "unversioned");
    assert_eq!(env["receipt"]["revision"]["commit"], Value::Null);
    // HEAD names nothing yet.
    let (code, env) = json_of(&fx.reader, &["diff", "HEAD"]);
    assert_eq!(code, 2);
    assert_eq!(kinds(&env), pairs(&[("error", "git_unknown_revision")]));
    // There is a repository and git runs, so `base` and `target` are
    // resolved (no git_unavailable, 7.2), and `main` names no commit yet.
    let (code, env) = json_of(&fx.reader, &["work", "check", "work/pagination.work"]);
    assert_eq!(code, 1, "{env}");
    assert_eq!(
        kinds(&env),
        pairs(&[
            ("error", "work_unknown_ref"),
            ("error", "work_unknown_ref"),
            ("warning", "work_provisional_identity"),
        ])
    );
    assert!(env["changes"][0].get("base_commit").is_none());
}

#[test]
fn a_project_at_the_top_of_its_repository() {
    if !git_available() {
        return;
    }
    let fx = Fixture::new("adv-top-level");
    let repo = fixture_at(fx.reader.clone());
    repo.git(&["init", "-q", "-b", "main"]);
    let first = repo.commit_all("the project at the top level");
    let digest = project_digest(&fx.reader);
    let (_, env) = json_of(&fx.reader, &["inspect", "paginate"]);
    assert_eq!(
        env["revision"],
        json!({ "commit": first, "state": "clean", "digest": digest })
    );
    edit(
        &repo,
        "src/reader/pages.cpl",
        "page.len > 0 and page.len <= size",
        "page.len >= 1 and page.len <= size",
    );
    let (_, env) = json_of(&fx.reader, &["inspect", "paginate"]);
    assert_eq!(env["revision"]["state"], "worktree");
    let second = repo.commit_all("a contract edit");
    let (code, env) = json_of(&fx.reader, &["diff", "HEAD~1..HEAD"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(env["old"]["commit"], json!(first));
    assert_eq!(env["new"]["commit"], json!(second));
    assert_eq!(env["old"]["digest"], json!(digest));
    assert_eq!(
        statements(&env),
        [st("contract_changed", "reader.pages.paginate")]
    );
    let (code, env) = json_of(&fx.reader, &["work", "check", "work/pagination.work"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(env["changes"][0]["base_commit"], json!(second));
}

#[test]
fn a_project_below_directories_with_spaces_in_their_names() {
    if !git_available() {
        return;
    }
    let root = temp_dir("adv-deep-prefix");
    let project = root.join("nested/with space/reader");
    copy_dir(&workspace().join("examples/reader"), &project);
    let repo = fixture_at(root.clone());
    repo.git(&["init", "-q", "-b", "main"]);
    repo.write("nested/with space/notes.txt", "beside the project\n");
    let first = repo.commit_all("a nested project");
    let digest = project_digest(&project);
    let (_, env) = json_of(&project, &["inspect", "paginate"]);
    assert_eq!(
        env["revision"],
        json!({ "commit": first, "state": "clean", "digest": digest })
    );
    // Beside the project is outside its source set.
    repo.write("nested/with space/notes.txt", "changed\n");
    let (_, env) = json_of(&project, &["inspect", "paginate"]);
    assert_eq!(env["revision"]["state"], "clean");
    edit(
        &repo,
        "nested/with space/reader/src/reader/pages.cpl",
        "page.len > 0 and",
        "page.len >= 1 and",
    );
    let (_, env) = json_of(&project, &["inspect", "paginate"]);
    assert_eq!(env["revision"]["state"], "worktree");
    repo.commit_all("a contract edit below the prefix");
    let (code, env) = json_of(&project, &["diff", "HEAD~1..HEAD"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(
        statements(&env),
        [st("contract_changed", "reader.pages.paginate")]
    );
    // A commit's tree holds the project's files, keyed without the prefix.
    assert_eq!(env["old"]["digest"], json!(digest));
    assert_eq!(env["new"]["digest"], json!(project_digest(&project)));
}

#[test]
fn glob_characters_in_the_project_path_match_only_themselves() {
    if !git_available() {
        return;
    }
    let root = temp_dir("adv-glob-prefix");
    let bracket = root.join("proj[1]");
    let plain = root.join("proj1");
    copy_dir(&workspace().join("examples/reader"), &bracket);
    copy_dir(&workspace().join("examples/reader"), &plain);
    let repo = fixture_at(root.clone());
    repo.git(&["init", "-q", "-b", "main"]);
    let first = repo.commit_all("two projects");
    // As a pattern `proj[1]` matches `proj1`; as a path it does not (6.3).
    edit(
        &repo,
        "proj1/src/reader/pages.cpl",
        "page.len > 0 and",
        "page.len >= 1 and",
    );
    repo.write("proj1/src/new.txt", "untracked\n");
    let (_, env) = json_of(&bracket, &["inspect", "paginate"]);
    assert_eq!(env["revision"]["commit"], json!(first));
    assert_eq!(env["revision"]["state"], "clean");
    let (_, env) = json_of(&plain, &["inspect", "paginate"]);
    assert_eq!(env["revision"]["state"], "worktree");
    repo.commit_all("only proj1 changes");
    let (code, env) = json_of(&bracket, &["diff", "HEAD~1..HEAD"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(env["statements"], json!([]));
    assert_eq!(env["old"]["digest"], env["new"]["digest"]);
    let (_, env) = json_of(&plain, &["diff", "HEAD~1..HEAD"]);
    assert_eq!(
        statements(&env),
        [st("contract_changed", "reader.pages.paginate")]
    );
}

#[test]
fn glob_characters_in_a_source_directory_match_only_themselves() {
    if !git_available() {
        return;
    }
    let root = temp_dir("adv-glob-sources");
    if std::fs::create_dir_all(root.join("lib*")).is_err() {
        eprintln!("skipped: this file system has no `*` in names");
        return;
    }
    let repo = fixture_at(root.clone());
    repo.write(
        "cpl.toml",
        "[project]\nname = \"globs\"\nedition = \"0.1\"\n\n[build]\nsources = [\"lib*\"]\n",
    );
    repo.write(
        "lib*/globs/one.cpl",
        "module globs.one\n\npub fn one() -> Int {\n    1\n}\n",
    );
    repo.write("library/notes.txt", "not a source directory\n");
    repo.git(&["init", "-q", "-b", "main"]);
    let commit = repo.commit_all("a source directory named lib*");
    let digest = source_digest(&source_files(&root, &["lib*"]));
    let revision = |state: &str| json!({ "commit": commit, "state": state, "digest": digest });
    let (code, env) = json_of(&root, &["inspect", "globs.one.one"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(env["revision"], revision("clean"));
    repo.write("library/notes.txt", "changed\n");
    repo.write("library/new.txt", "untracked\n");
    let (_, env) = json_of(&root, &["inspect", "globs.one.one"]);
    assert_eq!(env["revision"], revision("clean"));
    repo.write("lib*/globs/notes.txt", "untracked under the source root\n");
    let (_, env) = json_of(&root, &["inspect", "globs.one.one"]);
    assert_eq!(env["revision"], revision("worktree"));
}

#[test]
fn the_repository_is_found_from_the_project_root_not_the_current_directory() {
    let Some(fx) = Fixture::committed("adv-root-not-cwd") else {
        return;
    };
    let other = temp_dir("adv-other-repo");
    let other_repo = fixture_at(other.clone());
    other_repo.git(&["init", "-q", "-b", "main"]);
    other_repo.write("README", "another repository\n");
    let other_head = other_repo.commit_all("another history");
    let reader = fx.reader.to_str().expect("a UTF-8 path");
    let (_, env) = json_of(&other, &["--project", reader, "inspect", "paginate"]);
    assert_eq!(env["revision"]["commit"], json!(head(&fx)));
    assert_eq!(env["revision"]["state"], "clean");
    assert_ne!(env["revision"]["commit"], json!(other_head));
    let (code, env) = json_of(&other, &["--project", reader, "diff", "HEAD"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(env["old"]["commit"], json!(head(&fx)));
    let work = fx.reader.join("work/pagination.work");
    let (code, env) = json_of(
        &other,
        &[
            "--project",
            reader,
            "work",
            "check",
            work.to_str().expect("a UTF-8 path"),
        ],
    );
    assert_eq!(code, 0, "{env}");
    assert_eq!(env["file"], "work/pagination.work");
    assert_eq!(env["changes"][0]["base_commit"], json!(head(&fx)));
    // An unversioned project named from inside another repository.
    let loose = Fixture::new("adv-loose");
    let (_, env) = json_of(
        &other,
        &[
            "--project",
            loose.reader.to_str().expect("a UTF-8 path"),
            "inspect",
            "paginate",
        ],
    );
    assert_eq!(env["revision"]["commit"], Value::Null);
}

#[test]
fn without_git_on_path_inspect_and_prove_are_unversioned_and_silent() {
    let Some(fx) = Fixture::committed("adv-no-git") else {
        return;
    };
    let path = path_without_git();
    let no_git = [("PATH", path.as_str())];
    let digest = project_digest(&fx.reader);
    let (code, env) = json_env(&fx.reader, &["inspect", "paginate"], &no_git);
    assert_eq!(code, 0, "{env}");
    assert_eq!(
        env["revision"],
        json!({ "commit": null, "state": "worktree", "digest": digest })
    );
    // 6.3: inspect never emits git_unavailable.
    assert_eq!(env["diagnostics"], json!([]));
    let (code, env) = json_env(&fx.reader, &["prove", "paginate"], &no_git);
    assert_eq!(code, 1, "{env}");
    assert_eq!(kinds(&env), pairs(&[("info", "verification_result")]));
    assert_eq!(env["revision"]["commit"], Value::Null);
    assert_eq!(env["receipt"]["core"]["source_revision"], "unversioned");
    assert!(env["receipt"]["core"].get("dirty").is_none());
    let run = cpl_env(&fx.reader, &["inspect", "paginate"], &no_git);
    run.expect_code(0);
    assert_eq!(run.stderr, "");
}

#[test]
fn the_dirty_flag_of_a_receipt_follows_the_revision_state() {
    let Some(fx) = Fixture::committed("adv-dirty-flag") else {
        return;
    };
    let dirty = |env: &Value| {
        (
            env["receipt"]["core"].get("dirty").cloned(),
            only(env, "verification_result")["receipt"]
                .get("dirty")
                .cloned(),
        )
    };
    let (_, env) = json_of(&fx.reader, &["prove", "paginate"]);
    assert_eq!(env["revision"]["state"], "clean");
    assert_eq!(dirty(&env), (None, None));
    // A file outside the source set is no input of the evidence (design
    // line 423 binds evidence to its source; 10.7's dirty check is the git
    // status of the source-set roots): not dirty.
    fx.write("reader/README.md", "x\n");
    let (_, env) = json_of(&fx.reader, &["prove", "paginate"]);
    assert_eq!(env["revision"]["state"], "clean");
    assert_eq!(dirty(&env), (None, None));
    // An uncommitted member: dirty, present only as `true` (Appendix C).
    edit(&fx, PAGES, "fn clean(", "// tidied\nfn clean(");
    let (_, env) = json_of(&fx.reader, &["prove", "paginate"]);
    assert_eq!(env["revision"]["state"], "worktree");
    assert_eq!(dirty(&env), (Some(json!(true)), Some(json!(true))));
    assert_eq!(env["receipt"]["core"]["source_revision"], json!(head(&fx)));
}

// ===========================================================================
// cpl diff: the statements of Section 9.1
// ===========================================================================

#[test]
fn identity_added_and_removed_carry_the_version_of_their_side() {
    let Some(fx) = Fixture::committed("adv-added-removed") else {
        return;
    };
    let (_, before) = json_of(&fx.reader, &["inspect", "reader.pages.label"]);
    edit(
        &fx,
        PAGES,
        "fn label(result: Result<Page, StoreError>) -> Text {\n    match result {\n        Ok(page) => page.text\n        Err(Missing(_)) => \"Missing page\"\n        Err(Unavailable) => \"Storage unavailable\"\n    }\n}\n\n",
        "",
    );
    fx.write(
        PAGES,
        &format!(
            "{}\n@id(\"fn_aaaaaaaaaaaaaaaa\")\npub fn fresh() -> Int {{\n    1\n}}\n",
            fx.read(PAGES)
        ),
    );
    let (code, env) = json_of(&fx.reader, &["diff", "HEAD"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(
        statements(&env),
        [
            st("identity_added", "reader.pages.fresh"),
            st("identity_removed", "reader.pages.label"),
        ]
    );
    let (_, fresh) = json_of(&fx.reader, &["inspect", "fn_aaaaaaaaaaaaaaaa"]);
    let added = &env["statements"][0];
    assert_eq!(added["id"], "fn_aaaaaaaaaaaaaaaa");
    assert_eq!(added["kind"], "fn");
    assert_eq!(added["facet"], Value::Null);
    assert_eq!(
        added["detail"],
        json!({ "version": version_of(&fresh["symbol"]) })
    );
    assert_eq!(added["detail"]["version"]["provisional"], false);
    let removed = &env["statements"][1];
    assert_eq!(removed["id"], "reader.pages.label");
    assert_eq!(removed["kind"], "fn");
    assert_eq!(removed["facet"], Value::Null);
    assert_eq!(
        removed["detail"],
        json!({ "version": version_of(&before["symbol"]) })
    );
}

#[test]
fn a_rename_is_identity_renamed_and_never_a_signature_change() {
    let Some(fx) = Fixture::committed("adv-rename") else {
        return;
    };
    edit(&fx, PAGES, "pub fn paginate<T>(", "pub fn pages_of<T>(");
    edit(
        &fx,
        PAGES,
        "    Ok(paginate(items, checked_size))",
        "    Ok(pages_of(items, checked_size))",
    );
    fx.commit_all("rename paginate");
    let (code, env) = json_of(&fx.reader, &["diff", "HEAD~1..HEAD"]);
    assert_eq!(code, 0, "{env}");
    // T24: the signature's text has `_` for the name. 4.6: the caller's
    // body changes.
    assert_eq!(
        statements(&env),
        [
            st("identity_renamed", "reader.pages.pages_of"),
            st("body_changed", "reader.pages.paginate_ints"),
        ]
    );
    let renamed = &env["statements"][0];
    assert_eq!(renamed["id"], "pgn_7c2d");
    assert_eq!(renamed["kind"], "fn");
    assert_eq!(renamed["facet"], "name");
    assert_eq!(
        renamed["detail"],
        json!({ "from": "reader.pages.paginate", "to": "reader.pages.pages_of" })
    );
    assert_eq!(env["statements"][1]["facet"], "body");
}

#[test]
fn moving_a_declaration_with_an_id_is_moved() {
    let Some(fx) = Fixture::committed("adv-moved") else {
        return;
    };
    let pages = fx.read(PAGES);
    let start = pages.find("@id(\"pgn_7c2d\")").expect("paginate");
    let end = pages.find("pub fn paginate_ints(").expect("paginate_ints");
    let declaration = pages[start..end].trim_end().to_string();
    fx.write(PAGES, &format!("{}{}", &pages[..start], &pages[end..]));
    fx.write(
        "reader/src/reader/paging.cpl",
        &format!("module reader.paging\n\nuse reader.pages.{{Positive}}\n\n{declaration}\n"),
    );
    fx.commit_all("move paginate");
    let (code, env) = json_of(&fx.reader, &["diff", "HEAD~1..HEAD"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(statements(&env), [st("moved", "reader.paging.paginate")]);
    let moved = &env["statements"][0];
    assert_eq!(moved["id"], "pgn_7c2d");
    assert_eq!(moved["kind"], "fn");
    assert_eq!(moved["facet"], Value::Null);
    assert_eq!(
        moved["detail"],
        json!({ "from_module": "reader.pages", "to_module": "reader.paging" })
    );
}

#[test]
fn facet_statements_come_in_table_order_and_carry_both_hashes() {
    let Some(fx) = Fixture::committed("adv-facet-order") else {
        return;
    };
    let (_, before) = json_of(&fx.reader, &["inspect", "pgn_7c2d"]);
    // Docs added, `@prove` removed, `pub` removed (the signature).
    edit(
        &fx,
        PAGES,
        "@id(\"pgn_7c2d\")\n@prove\npub fn paginate<T>(",
        "/// Pages of at most `size` items.\n@id(\"pgn_7c2d\")\nfn paginate<T>(",
    );
    edit_contract(&fx);
    edit(
        &fx,
        PAGES,
        "    var rest = items\n",
        "    var rest: List<T> = items\n",
    );
    edit(&fx, PAGES, "    a / b\n", "    (a / b)\n");
    let (_, after) = json_of(&fx.reader, &["inspect", "pgn_7c2d"]);
    let (code, env) = json_of(&fx.reader, &["diff", "HEAD"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(
        statements(&env),
        [
            st("body_changed", "reader.pages.divide_exactly"),
            st("signature_changed", "reader.pages.paginate"),
            st("contract_changed", "reader.pages.paginate"),
            st("body_changed", "reader.pages.paginate"),
            st("docs_changed", "reader.pages.paginate"),
            st("verification_policy_changed", "reader.pages.paginate"),
        ]
    );
    let list = env["statements"].as_array().expect("statements");
    for (statement, facet) in list[1..5]
        .iter()
        .zip(["signature", "contract", "body", "docs"])
    {
        assert_eq!(statement["facet"], facet);
        assert_eq!(statement["id"], "pgn_7c2d");
        assert_eq!(
            statement["detail"],
            json!({
                "old": before["symbol"]["facets"][facet],
                "new": after["symbol"]["facets"][facet],
            }),
            "{facet}"
        );
    }
    // The policy statement carries the facet texts, not their hashes.
    assert_eq!(list[5]["facet"], "verification_policy");
    assert_eq!(list[5]["detail"], json!({ "old": "@prove", "new": "" }));
    assert_eq!(list[0]["facet"], "body");
}

#[test]
fn adding_prove_is_a_policy_change_from_the_empty_text() {
    let Some(fx) = Fixture::committed("adv-add-prove") else {
        return;
    };
    edit(
        &fx,
        PAGES,
        "fn clean(page: Page)",
        "@prove\nfn clean(page: Page)",
    );
    let (code, env) = json_of(&fx.reader, &["diff", "HEAD"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(
        statements(&env),
        [st("verification_policy_changed", "reader.pages.clean")]
    );
    let s = &env["statements"][0];
    assert_eq!(s["id"], "reader.pages.clean");
    assert_eq!(s["facet"], "verification_policy");
    assert_eq!(s["detail"], json!({ "old": "", "new": "@prove" }));
}

#[test]
fn effect_bounds_widen_narrow_or_both_and_are_sets() {
    let Some(fx) = Fixture::committed("adv-effects") else {
        return;
    };
    let original = fx.read(PAGES);
    let header = "    using db: PageRead,\n) -> Result<Page, StoreError>\n";
    let bound = format!("{header}    effects {{ db.get }}\n");
    assert!(original.contains(&bound));
    let with = |text: &str| original.replacen(&bound, &format!("{header}{text}"), 1);
    let cases = vec![
        (
            "    effects { db.get, db.zeta, db.alpha }\n",
            vec![(
                "effect_bound_widened",
                json!({ "added": ["db.alpha", "db.zeta"] }),
            )],
        ),
        (
            "",
            vec![("effect_bound_narrowed", json!({ "removed": ["db.get"] }))],
        ),
        (
            "    effects { db.put }\n",
            vec![
                ("effect_bound_widened", json!({ "added": ["db.put"] })),
                ("effect_bound_narrowed", json!({ "removed": ["db.get"] })),
            ],
        ),
        // T26: the bound is a set, so a repetition is nothing new.
        ("    effects { db.get, db.get }\n", vec![]),
        ("    effects {db.get}\n", vec![]),
    ];
    for (text, expected) in cases {
        fx.write(PAGES, &with(text));
        let (code, env) = json_of(&fx.reader, &["diff", "HEAD"]);
        assert_eq!(code, 0, "{text:?}: {env}");
        let got: Vec<(String, Value, Value, Value)> = env["statements"]
            .as_array()
            .expect("statements")
            .iter()
            .map(|s| {
                (
                    s["statement"].as_str().expect("a statement").to_string(),
                    s["path"].clone(),
                    s["facet"].clone(),
                    s["detail"].clone(),
                )
            })
            .collect();
        let want: Vec<(String, Value, Value, Value)> = expected
            .iter()
            .map(|(kind, detail)| {
                (
                    kind.to_string(),
                    json!("reader.pages.load_page"),
                    json!("effects"),
                    detail.clone(),
                )
            })
            .collect();
        assert_eq!(got, want, "{text:?}");
    }
    // Reordering a committed bound is no change either.
    fx.write(PAGES, &with("    effects { db.get, db.put }\n"));
    fx.commit_all("two effects");
    fx.write(PAGES, &with("    effects { db.put, db.get }\n"));
    let (_, env) = json_of(&fx.reader, &["diff", "HEAD"]);
    assert_eq!(env["statements"], json!([]));
}

#[test]
fn a_kind_change_that_changes_the_facet_set_is_removal_and_addition() {
    let Some(fx) = Fixture::committed("adv-kind-change") else {
        return;
    };
    let (_, before) = json_of(&fx.reader, &["inspect", "reader.pages.Positive"]);
    edit(
        &fx,
        PAGES,
        "pub type Positive = Int where self > 0\n",
        "pub struct Positive {\n    value: Int\n}\n",
    );
    edit(
        &fx,
        PAGES,
        "pub type PageId(Text)\n",
        "pub struct PageId {\n    value: Text\n}\n",
    );
    let (code, env) = json_of(&fx.reader, &["diff", "HEAD"]);
    assert_eq!(code, 0, "{env}");
    // A wrapper that became a struct keeps its facet set (identity, name,
    // signature, docs) and is compared facet by facet; a refinement that
    // became a struct loses its contract facet: removal plus addition, each
    // noted, in table order for the one path.
    assert_eq!(
        statements(&env),
        [
            st("signature_changed", "reader.pages.PageId"),
            st("identity_added", "reader.pages.Positive"),
            st("identity_removed", "reader.pages.Positive"),
        ]
    );
    assert_eq!(env["statements"][0]["facet"], "signature");
    let added = &env["statements"][1];
    let removed = &env["statements"][2];
    for s in [added, removed] {
        assert_eq!(s["id"], "reader.pages.Positive");
        assert_eq!(s["kind"], "ty");
        assert_eq!(s["facet"], Value::Null);
        assert_eq!(keys(&s["detail"]), ["note", "version"]);
        assert_eq!(s["detail"]["note"], "kind_changed");
    }
    assert_eq!(removed["detail"]["version"], version_of(&before["symbol"]));
    assert!(removed["detail"]["version"]["facets"]
        .get("contract")
        .is_some());
    assert!(added["detail"]["version"]["facets"]
        .get("contract")
        .is_none());
}

#[test]
fn both_statements_of_a_renaming_kind_change_name_the_new_path() {
    let Some(fx) = Fixture::committed("adv-kind-rename") else {
        return;
    };
    let refinement = "pub type Positive = Int where self > 0\n";
    edit(
        &fx,
        PAGES,
        refinement,
        &format!("@id(\"ty_aaaaaaaaaaaaaaaa\")\n{refinement}"),
    );
    fx.commit_all("an id for Positive");
    edit(
        &fx,
        PAGES,
        refinement,
        "pub struct Zed {\n    value: Int\n}\n",
    );
    edit(
        &fx,
        PAGES,
        "    Unavailable\n",
        "    Unavailable\n    Gone\n",
    );
    let (code, env) = json_of(&fx.reader, &["diff", "HEAD"]);
    assert_eq!(code, 0, "{env}");
    // 9.1: statements are ordered by `path`, the new side's when the id is
    // in the new map, else the old side's; both statements of this kind
    // change are about an id the new map has. (7.2's human lines are
    // "ordered by path" too, which holds only if each shows its key.)
    assert_eq!(
        statements(&env),
        [
            st("signature_changed", "reader.pages.StoreError"),
            st("identity_added", "reader.pages.Zed"),
            st("identity_removed", "reader.pages.Zed"),
        ]
    );
    let removed = &env["statements"][2];
    assert_eq!(removed["id"], "ty_aaaaaaaaaaaaaaaa");
    assert_eq!(removed["detail"]["note"], "kind_changed");
    assert_eq!(
        removed["detail"]["version"]["path"],
        "reader.pages.Positive"
    );
}

/// An edit (`from` replaced by `to`) and the `(statement, path)` pairs it
/// must produce.
type EditCase<'a> = (&'a str, &'a str, Vec<(&'a str, &'a str)>);

#[test]
fn facet_boundaries_of_section_5_show_in_the_diff() {
    let Some(fx) = Fixture::committed("adv-facet-boundaries") else {
        return;
    };
    let original = fx.read(PAGES);
    let ensures = "    ensures flatten(result) == items\n";
    let cases: Vec<EditCase> = vec![
        // T25: a field's doc comment is docs, never signature.
        (
            "    id: PageId\n",
            "    /// The page's id.\n    id: PageId\n",
            vec![("docs_changed", "reader.pages.Page")],
        ),
        // T23: loop invariants and loop measures belong to the body.
        (
            "invariant forall page in pages: page.len == size",
            "invariant forall page in pages: page.len <= size",
            vec![("body_changed", "reader.pages.paginate")],
        ),
        (
            "        decreases rest.len\n",
            "        decreases rest.len + 0\n",
            vec![("body_changed", "reader.pages.paginate")],
        ),
        // 5.2: parentheses are kept exactly as written...
        (
            ensures,
            "    ensures (flatten(result) == items)\n",
            vec![("contract_changed", "reader.pages.paginate")],
        ),
        // ...line breaks inside an expression are not reproduced...
        (
            ensures,
            "    ensures flatten(\n        result,\n    ) == items\n",
            vec![],
        ),
        // ...and comments are part of no facet.
        (
            ensures,
            "    ensures flatten(result) == items // every element\n",
            vec![],
        ),
    ];
    for (from, to, expected) in cases {
        assert!(original.contains(from), "{from:?}");
        fx.write(PAGES, &original.replacen(from, to, 1));
        let (code, env) = json_of(&fx.reader, &["diff", "HEAD"]);
        assert_eq!(code, 0, "{to:?}: {env}");
        let want: Vec<(String, String)> = expected.iter().map(|(s, p)| st(s, p)).collect();
        assert_eq!(statements(&env), want, "{to:?}");
    }
    // 5.2: a doc line is taken without its trailing whitespace.
    fx.write(
        PAGES,
        &original.replacen("@id(\"pgn_7c2d\")", "/// Pages.\n@id(\"pgn_7c2d\")", 1),
    );
    fx.write("reader/src/reader/extra.cpl", EXTRA);
    fx.commit_all("docs and a module");
    let committed = fx.read(PAGES);
    fx.write(
        PAGES,
        &committed.replacen("/// Pages.\n", "/// Pages.   \n", 1),
    );
    let (_, env) = json_of(&fx.reader, &["diff", "HEAD"]);
    assert_eq!(env["statements"], json!([]));
    fx.write(PAGES, &committed);
    // T23: a function's own `decreases` belongs to its contract.
    edit(
        &fx,
        "reader/src/reader/extra.cpl",
        "fn forever(n: Int) -> Int {\n",
        "fn forever(n: Int) -> Int\n    decreases n\n{\n",
    );
    let (_, env) = json_of(&fx.reader, &["diff", "HEAD"]);
    assert_eq!(
        statements(&env),
        [st("contract_changed", "reader.extra.forever")]
    );
}

#[test]
fn changing_an_id_is_removal_and_addition_and_identity_has_no_statement() {
    let Some(fx) = Fixture::committed("adv-id-change") else {
        return;
    };
    edit(&fx, PAGES, "@id(\"pgn_7c2d\")", "@id(\"pgn_7c2e\")");
    let (code, env) = json_of(&fx.reader, &["diff", "HEAD"]);
    assert_eq!(code, 0, "{env}");
    let got: Vec<(&str, &str)> = env["statements"]
        .as_array()
        .expect("statements")
        .iter()
        .map(|s| {
            (
                s["statement"].as_str().expect("a statement"),
                s["id"].as_str().expect("an id"),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("identity_added", "pgn_7c2e"),
            ("identity_removed", "pgn_7c2d")
        ]
    );
    for s in env["statements"].as_array().expect("statements") {
        assert_eq!(s["path"], "reader.pages.paginate");
        assert_eq!(s["facet"], Value::Null);
        assert_eq!(keys(&s["detail"]), ["version"], "no kind_changed note");
    }
}

#[test]
fn renaming_a_declaration_without_an_id_is_removal_and_addition() {
    let Some(fx) = Fixture::committed("adv-provisional-rename") else {
        return;
    };
    edit(&fx, PAGES, "fn clean(page: Page)", "fn tidy(page: Page)");
    let (code, env) = json_of(&fx.reader, &["diff", "HEAD"]);
    assert_eq!(code, 0, "{env}");
    // Section 4.3: provisional identities do not survive a rename.
    assert_eq!(
        statements(&env),
        [
            st("identity_removed", "reader.pages.clean"),
            st("identity_added", "reader.pages.tidy"),
        ]
    );
    assert_eq!(
        env["statements"][1]["detail"]["version"]["provisional"],
        true
    );
    assert_eq!(env["statements"][1]["id"], "reader.pages.tidy");
}

#[test]
fn comments_blank_lines_and_line_breaks_are_no_statements() {
    let Some(fx) = Fixture::committed("adv-layout") else {
        return;
    };
    let committed = project_digest(&fx.reader);
    edit(
        &fx,
        PAGES,
        "pub fn paginate_ints(\n    items: List<Int>,\n    size: Int,\n) -> Result<List<List<Int>>, ConstraintError>\n{\n",
        "// a comment before the declaration\npub fn paginate_ints(items: List<Int>, size: Int) -> Result<List<List<Int>>, ConstraintError> {\n    // a comment in the body\n\n",
    );
    edit(
        &fx,
        PAGES,
        "    var rest = items\n",
        "    var rest = items // a trailing comment\n\n\n",
    );
    let (code, env) = json_of(&fx.reader, &["diff", "HEAD", "--exit-code"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(env["statements"], json!([]));
    assert_eq!(env["new"]["state"], "worktree");
    assert_eq!(env["old"]["digest"], json!(committed));
    assert_eq!(env["new"]["digest"], json!(project_digest(&fx.reader)));
    assert_ne!(env["new"]["digest"], env["old"]["digest"]);
}

#[test]
fn a_file_that_does_not_parse_contributes_no_declarations() {
    let Some(fx) = Fixture::committed("adv-unparsable") else {
        return;
    };
    fx.write(PAGES, &format!("{}\nfn broken( {{\n", fx.read(PAGES)));
    // T31: a diff works between revisions that do not check, and 5.4: a
    // file that failed to parse contributes nothing.
    let (code, env) = json_of(&fx.reader, &["diff", "HEAD"]);
    assert_eq!(code, 0, "{env}");
    let mut removed: Vec<String> = Vec::new();
    for s in env["statements"].as_array().expect("statements") {
        assert_eq!(s["statement"], "identity_removed", "{s}");
        removed.push(s["id"].as_str().expect("an id").to_string());
    }
    removed.sort();
    let mut expected: Vec<String> = [
        "pgn_7c2d",
        "reader.pages.Page",
        "reader.pages.PageId",
        "reader.pages.PageRead",
        "reader.pages.PageWrite",
        "reader.pages.Positive",
        "reader.pages.StoreError",
        "reader.pages.clean",
        "reader.pages.divide_exactly",
        "reader.pages.label",
        "reader.pages.load_page",
        "reader.pages.load_text",
        "reader.pages.paginate_ints",
        "reader.pages.parse_size",
        "reader.pages.positive_size",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    expected.sort();
    assert_eq!(removed, expected);
}

#[test]
fn tests_and_properties_have_statements_of_their_own() {
    let Some(fx) = Fixture::committed("adv-test-statements") else {
        return;
    };
    edit(
        &fx,
        TESTS_CPL,
        "    for items in gen.list(gen.int)\n",
        "    for items in gen.list(gen.positive_int)\n",
    );
    edit(
        &fx,
        TESTS_CPL,
        "    assert paginate(items, 2) == []\n",
        "    assert paginate(items, 3) == []\n",
    );
    edit(
        &fx,
        TESTS_CPL,
        "test \"pagination preserves a short tail\"",
        "test \"pagination keeps a short tail\"",
    );
    fx.commit_all("edit the tests");
    let (code, env) = json_of(&fx.reader, &["diff", "HEAD~1..HEAD"]);
    assert_eq!(code, 0, "{env}");
    let got: Vec<(String, String, String)> = env["statements"]
        .as_array()
        .expect("statements")
        .iter()
        .map(|s| {
            (
                s["statement"].as_str().expect("a statement").to_string(),
                s["path"].as_str().expect("a path").to_string(),
                s["kind"].as_str().expect("a kind").to_string(),
            )
        })
        .collect();
    let row = |s: &str, name: &str, kind: &str| {
        (
            s.to_string(),
            format!("tests.pagination.\"{name}\""),
            kind.to_string(),
        )
    };
    // 5.1: a property's signature is its `for` clauses.
    assert_eq!(
        got,
        [
            row("body_changed", "pagination handles empty input", "test"),
            row("identity_added", "pagination keeps a short tail", "test"),
            row(
                "identity_removed",
                "pagination preserves a short tail",
                "test"
            ),
            row("signature_changed", "pagination preserves input", "prop"),
        ]
    );
}

#[test]
fn work_and_manifest_edits_change_the_digest_but_no_statement() {
    let Some(fx) = Fixture::committed("adv-work-digest") else {
        return;
    };
    edit(&fx, WORK, "    workers 3\n", "    workers 2\n");
    edit(
        &fx,
        "reader/cpl.toml",
        "edition = \"0.1\"\n",
        "edition = \"0.1\"\ndescription = \"the corpus\"\n",
    );
    let (code, env) = json_of(&fx.reader, &["diff", "HEAD", "--exit-code"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(env["statements"], json!([]));
    assert_ne!(env["old"]["digest"], env["new"]["digest"]);
    fx.commit_all("work file and manifest edits");
    let (code, env) = json_of(&fx.reader, &["diff", "HEAD~1..HEAD", "--exit-code"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(env["statements"], json!([]));
    assert_ne!(env["old"]["digest"], env["new"]["digest"]);
    assert_eq!(env["new"]["digest"], json!(project_digest(&fx.reader)));
}

#[test]
fn each_revision_is_read_with_its_own_build_sources() {
    let Some(fx) = Fixture::committed("adv-sources-move") else {
        return;
    };
    let first = project_digest(&fx.reader);
    std::fs::rename(fx.reader.join("src"), fx.reader.join("lib")).expect("move the sources");
    fx.write(
        "reader/cpl.toml",
        &format!(
            "{}\n[build]\nsources = [\"lib\"]\n",
            fx.read("reader/cpl.toml")
        ),
    );
    fx.commit_all("sources under lib");
    let second = source_digest(&source_files(&fx.reader, &["lib"]));
    assert_ne!(first, second);
    for range in ["HEAD~1..HEAD", "HEAD~1"] {
        let (code, env) = json_of(&fx.reader, &["diff", range]);
        assert_eq!(code, 0, "{range}: {env}");
        assert_eq!(env["statements"], json!([]), "{range}");
        assert_eq!(env["old"]["digest"], json!(first), "{range}");
        assert_eq!(env["new"]["digest"], json!(second), "{range}");
    }
}

#[cfg(unix)]
#[test]
fn a_linked_file_is_a_member_with_its_targets_bytes_at_every_revision() {
    if !git_available() {
        return;
    }
    const SHARED: &str = "module reader.shared\n\npub fn shared_one() -> Int {\n    1\n}\n";
    let root = temp_dir("adv-links");
    let reader = root.join("reader");
    copy_dir(&workspace().join("examples/reader"), &reader);
    let repo = fixture_at(root.clone());
    repo.write("shared/shared.cpl", SHARED);
    std::os::unix::fs::symlink(
        "../../../shared/shared.cpl",
        reader.join("src/reader/shared.cpl"),
    )
    .expect("link a file");
    // T54: a link to a directory is never followed.
    std::os::unix::fs::symlink("../../shared", reader.join("src/linked"))
        .expect("link a directory");
    repo.git(&["init", "-q", "-b", "main"]);
    let commit = repo.commit_all("a linked module");
    let files = source_files(&reader, &["src"]);
    assert!(files.contains_key("src/reader/shared.cpl"));
    assert!(!files.keys().any(|p| p.starts_with("src/linked")));
    let digest = source_digest(&files);
    let (code, env) = json_of(&reader, &["inspect", "reader.shared.shared_one"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(
        env["revision"],
        json!({ "commit": commit, "state": "clean", "digest": digest })
    );
    assert_eq!(env["symbol"]["location"]["file"], "src/reader/shared.cpl");
    // The commit's tree follows the link inside the commit (9.3 step 4).
    let (code, env) = json_of(&reader, &["diff", "HEAD"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(env["statements"], json!([]));
    assert_eq!(env["old"]["digest"], json!(digest));
    // The target lies outside every root: changing it leaves the state
    // clean while the loaded tree differs from the commit's (6.3).
    repo.write("shared/shared.cpl", &SHARED.replace("    1\n", "    2\n"));
    let (_, env) = json_of(&reader, &["diff", "HEAD"]);
    assert_eq!(
        statements(&env),
        [st("body_changed", "reader.shared.shared_one")]
    );
    assert_eq!(env["new"]["state"], "clean");
    assert_ne!(env["new"]["digest"], env["old"]["digest"]);
}

#[cfg(unix)]
#[test]
fn links_that_lead_to_no_file_of_the_commit_are_left_out() {
    if !git_available() {
        return;
    }
    let root = temp_dir("adv-links-nowhere");
    let outside = temp_dir("adv-links-outside");
    std::fs::write(
        outside.join("outer.cpl"),
        "module reader.outer\n\npub fn outer_one() -> Int {\n    1\n}\n",
    )
    .expect("write the outside file");
    let reader = root.join("reader");
    copy_dir(&workspace().join("examples/reader"), &reader);
    let dir = reader.join("src/reader");
    let link = |target: &Path, name: &str| {
        std::os::unix::fs::symlink(target, dir.join(name)).expect("make a link");
    };
    // T54: a dangling link and a loop are no members, anywhere.
    link(Path::new("nowhere.cpl"), "dangling.cpl");
    link(Path::new("loop_b.cpl"), "loop_a.cpl");
    link(Path::new("loop_a.cpl"), "loop_b.cpl");
    // A link to a file outside the repository is a member of the working
    // tree, but no file of any commit.
    link(&outside.join("outer.cpl"), "outer.cpl");
    let repo = fixture_at(root.clone());
    repo.git(&["init", "-q", "-b", "main"]);
    let commit = repo.commit_all("links");
    let files = source_files(&reader, &["src"]);
    for absent in ["dangling.cpl", "loop_a.cpl", "loop_b.cpl"] {
        assert!(
            !files.contains_key(&format!("src/reader/{absent}")),
            "{absent}"
        );
    }
    assert!(files.contains_key("src/reader/outer.cpl"));
    let digest = source_digest(&files);
    let (code, env) = json_of(&reader, &["inspect", "reader.outer.outer_one"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(env["diagnostics"], json!([]));
    assert_eq!(
        env["revision"],
        json!({ "commit": commit, "state": "clean", "digest": digest })
    );
    let (code, env) = json_of(&reader, &["diff", "HEAD"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(
        statements(&env),
        [st("identity_added", "reader.outer.outer_one")]
    );
    assert_eq!(
        env["old"]["digest"],
        json!(project_digest(&workspace().join("examples/reader")))
    );
    assert_eq!(env["new"]["digest"], json!(digest));
}

#[test]
fn ids_declared_twice_in_the_old_revision_are_listed_on_its_side() {
    let Some(fx) = Fixture::committed("adv-old-duplicates") else {
        return;
    };
    fx.write(
        "reader/src/reader/twin.cpl",
        "module reader.twin\n\n@id(\"pgn_7c2d\")\nfn twin() -> Int {\n    1\n}\n\n@id(\"aaa_bbb\")\nfn one() -> Int {\n    1\n}\n\n@id(\"aaa_bbb\")\nfn two() -> Int {\n    2\n}\n",
    );
    fx.commit_all("duplicated ids");
    std::fs::remove_file(fx.reader.join("src/reader/twin.cpl")).expect("remove the twin");
    let (code, env) = json_of(&fx.reader, &["diff", "HEAD"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(
        env["duplicates"],
        json!({
            // 9.2: in id order; a file that declares an id twice is listed twice.
            "old": [
                { "id": "aaa_bbb", "files": ["src/reader/twin.cpl", "src/reader/twin.cpl"] },
                { "id": "pgn_7c2d", "files": ["src/reader/pages.cpl", "src/reader/twin.cpl"] },
            ],
            "new": [],
        })
    );
    // The first occurrence in path order is paginate on both sides; the
    // first `aaa_bbb` is gone from the new side.
    assert_eq!(
        statements(&env),
        [st("identity_removed", "reader.twin.one")]
    );
}

// ===========================================================================
// cpl diff: ranges and revisions (Section 7.2)
// ===========================================================================

#[test]
fn two_dot_and_three_dot_ranges_name_their_sides() {
    let Some(fx) = Fixture::committed("adv-ranges") else {
        return;
    };
    let first = head(&fx);
    fx.write(
        TESTS_CPL,
        &format!(
            "{}\ntest \"pagination keeps one page\" {{\n    assert paginate([1], 1) == [[1]]\n}}\n",
            fx.read(TESTS_CPL)
        ),
    );
    let second = fx.commit_all("a new test");
    let keeps = "tests.pagination.\"pagination keeps one page\"";
    let sides = |range: &str| {
        let (code, env) = json_of(&fx.reader, &["diff", range]);
        assert_eq!(code, 0, "{range}: {env}");
        (
            env["old"]["commit"].clone(),
            env["new"]["commit"].clone(),
            statements(&env),
        )
    };
    // A..B: from A to B, whichever is older.
    assert_eq!(
        sides("HEAD..HEAD~1"),
        (
            json!(second),
            json!(first),
            vec![st("identity_removed", keeps)]
        )
    );
    assert_eq!(
        sides("HEAD~1..HEAD"),
        (
            json!(first),
            json!(second),
            vec![st("identity_added", keeps)]
        )
    );
    // A...B: from the merge base of A and B to B.
    assert_eq!(sides("HEAD...HEAD~1"), (json!(first), json!(first), vec![]));
    assert_eq!(
        sides("HEAD~1...HEAD"),
        (
            json!(first),
            json!(second),
            vec![st("identity_added", keeps)]
        )
    );
    // One commit on both sides, however spelled.
    let spelled = format!("{second}..main");
    for range in [
        "HEAD..HEAD",
        "HEAD...HEAD",
        "main..HEAD",
        "HEAD...main",
        spelled.as_str(),
    ] {
        let run = cpl(&fx.reader, &["diff", range, "--exit-code"]);
        run.expect_code(0);
        assert_eq!(run.stdout, "", "{range}");
    }
    // A alone: from A to the working tree, whose revision the envelope has.
    edit(&fx, PAGES, "    a / b\n", "    (a / b)\n");
    let (code, env) = json_of(&fx.reader, &["diff", "HEAD~1"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(env["old"]["commit"], json!(first));
    assert_eq!(
        env["new"],
        json!({ "commit": second, "state": "worktree", "digest": project_digest(&fx.reader) })
    );
    assert_eq!(env["new"], env["revision"]);
    assert_eq!(
        statements(&env),
        [
            st("body_changed", "reader.pages.divide_exactly"),
            st("identity_added", keeps),
        ]
    );
}

#[test]
fn tags_and_abbreviated_ids_resolve_to_full_commit_ids() {
    let Some(fx) = Fixture::committed("adv-tags") else {
        return;
    };
    let first = head(&fx);
    fx.git(&["tag", "-a", "v1.0", "-m", "an annotated tag"]);
    let tag_object = fx.git(&["rev-parse", "v1.0"]);
    assert_ne!(tag_object, first);
    edit_contract(&fx);
    let second = fx.commit_all("a contract edit");
    fx.git(&["tag", "v1.1"]);
    let short = format!("{}..{}", &first[..8], &second[..8]);
    // 7.2: `rev-parse --verify <rev>^{commit}`: a tag is peeled to its commit.
    for range in ["v1.0..v1.1", "v1.0...v1.1", short.as_str()] {
        let (code, env) = json_of(&fx.reader, &["diff", range]);
        assert_eq!(code, 0, "{range}: {env}");
        assert_eq!(env["old"]["commit"], json!(first), "{range}");
        assert_eq!(env["new"]["commit"], json!(second), "{range}");
        assert_eq!(
            statements(&env),
            [st("contract_changed", "reader.pages.paginate")],
            "{range}"
        );
    }
    let (code, env) = json_of(&fx.reader, &["diff", "v1.0"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(env["old"]["commit"], json!(first));
    assert_eq!(env["new"], env["revision"]);
    // inspect's `changes` base is peeled the same way.
    let (code, env) = json_of(
        &fx.reader,
        &[
            "inspect",
            "paginate",
            "--include",
            "changes",
            "--since",
            "v1.0",
        ],
    );
    assert_eq!(code, 0, "{env}");
    assert_eq!(env["changes"]["since"]["commit"], json!(first));
    let names: Vec<&str> = env["changes"]["statements"]
        .as_array()
        .expect("statements")
        .iter()
        .map(|s| s["statement"].as_str().expect("a statement"))
        .collect();
    assert_eq!(names, ["contract_changed"]);
}

#[test]
fn names_of_objects_that_are_no_commits_are_unknown_revisions() {
    let Some(fx) = Fixture::committed("adv-not-commits") else {
        return;
    };
    let tree = fx.git(&["rev-parse", "HEAD^{tree}"]);
    let blob = fx.git(&["rev-parse", "HEAD:reader/cpl.toml"]);
    fx.git(&["tag", "tree-tag", tree.as_str()]);
    for name in [
        "tree-tag",
        "HEAD^{tree}",
        "HEAD:reader/cpl.toml",
        tree.as_str(),
        blob.as_str(),
        "@{upstream}",
        "main@{upstream}",
        "HEAD~100",
        "refs/heads/nope",
    ] {
        let run = cpl(&fx.reader, &["diff", "--json", "--", name]);
        run.expect_code(2);
        let env = run.json();
        assert_eq!(env["status"], "error", "{name}");
        assert_eq!(
            kinds(&env),
            pairs(&[("error", "git_unknown_revision")]),
            "{name}"
        );
        assert_eq!(env["diagnostics"][0]["category"], "git");
        for absent in ["old", "new", "statements", "duplicates"] {
            assert!(env.get(absent).is_none(), "{name}: {absent}");
        }
    }
}

#[test]
fn empty_and_option_like_names_in_a_range_are_unknown_revisions() {
    let Some(fx) = Fixture::committed("adv-empty-names") else {
        return;
    };
    // 7.2: an empty name, or one that starts with `-`, is refused.
    for range in [
        "HEAD...",
        "...HEAD",
        "..HEAD",
        "..",
        "...",
        "-x..HEAD",
        "HEAD..-x",
        "-x...HEAD",
        "HEAD...-x",
        "--all",
        "-",
        "-HEAD",
    ] {
        let run = cpl(&fx.reader, &["diff", "--json", "--", range]);
        run.expect_code(2);
        assert_eq!(
            kinds(&run.json()),
            pairs(&[("error", "git_unknown_revision")]),
            "{range:?}"
        );
    }
}

#[test]
fn a_side_without_the_project_is_no_project_at_revision() {
    if !git_available() {
        return;
    }
    let fx = Fixture::new("adv-no-project-side");
    fx.git(&["init", "-q", "-b", "main"]);
    fx.write("README", "the project arrives later\n");
    fx.git(&["add", "README"]);
    fx.git(&["commit", "-q", "--no-verify", "-m", "a readme"]);
    let readme = fx.git(&["rev-parse", "HEAD"]);
    let project = fx.commit_all("the project");
    let backwards = format!("{project}..{readme}");
    // The new side of A..B, and inspect's `changes` base, are revisions too.
    for args in [
        vec!["diff", "HEAD..HEAD~1"],
        vec!["diff", backwards.as_str()],
        vec!["inspect", "paginate", "--include", "changes"],
        vec![
            "inspect",
            "paginate",
            "--include",
            "changes",
            "--since",
            readme.as_str(),
        ],
    ] {
        let (code, env) = json_of(&fx.reader, &args);
        assert_eq!(code, 2, "{args:?}: {env}");
        assert_eq!(env["status"], "error");
        assert_eq!(
            kinds(&env),
            pairs(&[("error", "git_no_project_at_revision")]),
            "{args:?}"
        );
        assert_eq!(env["diagnostics"][0]["category"], "git");
        for absent in ["statements", "symbol", "changes"] {
            assert!(env.get(absent).is_none(), "{args:?}: {absent}");
        }
    }
}

#[test]
fn a_lock_is_read_at_a_revision_but_never_digested() {
    let Some(fx) = Fixture::committed("adv-locks") else {
        return;
    };
    let digest = project_digest(&fx.reader);
    cpl(&fx.reader, &["check", "--lock"]).expect_code(0);
    fx.commit_all("a lock");
    let (code, env) = json_of(&fx.reader, &["diff", "HEAD~1..HEAD"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(env["statements"], json!([]));
    assert_eq!(env["old"]["digest"], json!(digest));
    assert_eq!(env["new"]["digest"], json!(digest));
    // A lock that does not parse, at a revision: the load error (7.1, exit 2).
    fx.write("reader/cpl.lock", "this is not a lock [\n");
    fx.commit_all("a broken lock");
    std::fs::remove_file(fx.reader.join("cpl.lock")).expect("remove the lock");
    let (code, env) = json_of(&fx.reader, &["diff", "HEAD"]);
    assert_eq!(code, 2, "{env}");
    assert_eq!(kinds(&env), pairs(&[("error", "project_lock_invalid")]));
    let d = &env["diagnostics"][0];
    assert_eq!(d["category"], "project");
    assert!(d.get("span").is_none(), "semantics S63 (6): span-less");
}

#[test]
fn diff_runs_from_any_directory_of_the_project_and_with_project() {
    let Some(fx) = Fixture::committed("adv-diff-cwd") else {
        return;
    };
    edit_contract(&fx);
    fx.commit_all("a contract edit");
    let (_, at_root) = json_of(&fx.reader, &["diff", "HEAD~1..HEAD"]);
    assert_eq!(
        statements(&at_root),
        [st("contract_changed", "reader.pages.paginate")]
    );
    let (_, nested) = json_of(&fx.reader.join("src/reader"), &["diff", "HEAD~1..HEAD"]);
    assert_eq!(nested, at_root);
    let outside = temp_dir("adv-diff-outside");
    let (_, flagged) = json_of(
        &outside,
        &[
            "--project",
            fx.reader.to_str().expect("a UTF-8 path"),
            "diff",
            "HEAD~1..HEAD",
        ],
    );
    assert_eq!(flagged, at_root);
}

#[test]
fn diff_json_keys_come_in_declaration_order() {
    let Some(fx) = Fixture::committed("adv-diff-order") else {
        return;
    };
    // `load_page` gets an id first, so that it keeps its identity as it moves.
    edit(
        &fx,
        PAGES,
        "pub fn load_page(",
        "@id(\"fn_bbbbbbbbbbbbbbbb\")\npub fn load_page(",
    );
    fx.commit_all("an id for load_page");
    let pages = fx.read(PAGES);
    let start = pages
        .find("@id(\"fn_bbbbbbbbbbbbbbbb\")")
        .expect("load_page");
    let end = pages.find("pub fn load_text(").expect("load_text");
    let moved = pages[start..end].trim_end().to_string();
    fx.write(PAGES, &format!("{}{}", &pages[..start], &pages[end..]));
    fx.write(
        "reader/src/reader/loading.cpl",
        &format!("module reader.loading\n\n{moved}\n"),
    );
    // A rename, a contract edit, a kind change, and an addition.
    edit(&fx, PAGES, "pub fn paginate<T>(", "pub fn pages_of<T>(");
    edit_contract(&fx);
    edit(
        &fx,
        PAGES,
        "pub type Positive = Int where self > 0\n",
        "pub struct Positive {\n    value: Int\n}\n",
    );
    fx.write(
        PAGES,
        &format!(
            "{}\n@id(\"fn_aaaaaaaaaaaaaaaa\")\n@prove\npub fn fresh() -> Int {{\n    1\n}}\n",
            fx.read(PAGES)
        ),
    );
    let run = cpl(&fx.reader, &["diff", "HEAD", "--json"]);
    run.expect_code(0);
    let doc = Ordered::parse(&run.stdout);
    assert_eq!(
        doc.keys(),
        [
            "command",
            "toolchain",
            "revision",
            "status",
            "diagnostics",
            "old",
            "new",
            "statements",
            "duplicates"
        ]
    );
    for side in ["revision", "old", "new"] {
        assert_eq!(
            doc.get(side).keys(),
            ["commit", "state", "digest"],
            "{side}"
        );
    }
    assert_eq!(doc.get("duplicates").keys(), ["old", "new"]);
    let mut seen = BTreeSet::new();
    for s in doc.get("statements").items() {
        assert_eq!(
            s.keys(),
            ["statement", "id", "path", "kind", "facet", "detail"]
        );
        let statement = s.get("statement").text();
        let detail = s.get("detail");
        let kind_change = s.get("path").text().ends_with("Positive");
        let expected: &[&str] = match statement.as_str() {
            "identity_added" | "identity_removed" if kind_change => &["version", "note"],
            "identity_added" | "identity_removed" => &["version"],
            "identity_renamed" => &["from", "to"],
            "moved" => &["from_module", "to_module"],
            _ => &["old", "new"],
        };
        assert_eq!(detail.keys(), expected, "{statement}");
        if expected[0] == "version" {
            let version = detail.get("version");
            assert_eq!(
                version.keys(),
                ["id", "path", "provisional", "facets", "definition"]
            );
            assert_declared_order(&version.get("facets").keys(), &FACETS);
        }
        seen.insert(statement);
    }
    let expected: BTreeSet<String> = [
        "identity_added",
        "identity_removed",
        "identity_renamed",
        "moved",
        "contract_changed",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(seen, expected);
}

#[test]
fn diff_human_lines_follow_the_json_and_exit_code_keeps_them() {
    let Some(fx) = Fixture::committed("adv-diff-human") else {
        return;
    };
    edit(&fx, PAGES, "pub fn paginate<T>(", "pub fn pages_of<T>(");
    edit_contract(&fx);
    edit(&fx, PAGES, "fn clean(page: Page)", "fn tidy(page: Page)");
    let (_, env) = json_of(&fx.reader, &["diff", "HEAD"]);
    let run = cpl(&fx.reader, &["diff", "HEAD"]);
    run.expect_code(0);
    assert_eq!(run.stderr, "");
    let lines: Vec<&str> = run.stdout.lines().collect();
    let list = env["statements"].as_array().expect("statements");
    assert_eq!(lines.len(), list.len(), "{}", run.stdout);
    for (line, s) in lines.iter().zip(list) {
        let start = format!(
            "{}  {}  ({})",
            s["statement"].as_str().expect("a statement"),
            s["path"].as_str().expect("a path"),
            s["id"].as_str().expect("an id")
        );
        assert!(line.starts_with(&start), "{line:?} for {s}");
    }
    assert!(lines.contains(
        &"identity_renamed  reader.pages.pages_of  (pgn_7c2d)  was reader.pages.paginate"
    ));
    let again = cpl(&fx.reader, &["diff", "HEAD", "--semantic", "--exit-code"]);
    again.expect_code(1);
    assert_eq!(again.stdout, run.stdout);
}

// ===========================================================================
// cpl inspect (Sections 6, 7.2)
// ===========================================================================

#[test]
fn an_existing_stable_id_is_tried_before_a_bare_name() {
    let fx = Fixture::new("adv-id-first");
    fx.write(
        PAGES,
        &format!("{}\nfn pgn_7c2d() -> Int {{\n    1\n}}\n", fx.read(PAGES)),
    );
    // 6.1: the id syntax is tried first when an entry exists.
    let (code, env) = json_of(&fx.reader, &["inspect", "pgn_7c2d"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(env["symbol"]["id"], "pgn_7c2d");
    assert_eq!(env["symbol"]["path"], "reader.pages.paginate");
    let (code, env) = json_of(&fx.reader, &["inspect", "reader.pages.pgn_7c2d"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(env["symbol"]["id"], "reader.pages.pgn_7c2d");
    assert_eq!(env["symbol"]["provisional"], true);
}

#[test]
fn an_id_shaped_name_without_an_entry_is_a_bare_name() {
    let fx = Fixture::new("adv-id-shaped");
    for (name, path) in [
        ("divide_exactly", "reader.pages.divide_exactly"),
        ("paginate_ints", "reader.pages.paginate_ints"),
        ("load_page", "reader.pages.load_page"),
        ("image_width", "reader.images.image_width"),
        ("parse_int", "std.text.parse_int"),
    ] {
        let (code, env) = json_of(&fx.reader, &["inspect", name]);
        assert_eq!(code, 0, "{name}: {env}");
        assert_eq!(env["symbol"]["path"], path, "{name}");
    }
}

#[test]
fn member_stubs_are_bare_names_too() {
    let fx = Fixture::new("adv-member-names");
    // 6.1: every definition named so in project and std; one resolves.
    for (name, path) in [
        ("take", "std.prelude.List.take"),
        ("min", "std.prelude.Int.min"),
        ("flatten", "std.prelude.flatten"),
    ] {
        let (code, env) = json_of(&fx.reader, &["inspect", name]);
        assert_eq!(code, 0, "{name}: {env}");
        assert_eq!(env["symbol"]["path"], path, "{name}");
    }
    // Several are index_ambiguous_symbol, listing them (sorted).
    for (name, candidates) in [
        (
            "len",
            [
                "std.prelude.Bytes.len",
                "std.prelude.List.len",
                "std.prelude.Text.len",
            ],
        ),
        (
            "is_empty",
            [
                "std.prelude.Bytes.is_empty",
                "std.prelude.List.is_empty",
                "std.prelude.Text.is_empty",
            ],
        ),
    ] {
        let (code, env) = json_of(&fx.reader, &["inspect", name]);
        assert_eq!(code, 1, "{name}: {env}");
        let d = only(&env, "index_ambiguous_symbol");
        assert_eq!(d["candidates"], json!(candidates), "{name}");
    }
}

#[test]
fn module_paths_and_malformed_targets_name_no_declaration() {
    let fx = Fixture::new("adv-bad-targets");
    // 6.1: a target that resolves to no declaration is index_unknown_symbol
    // (exit 1, 7.2); 7.4 gives exit 2 to a bad include only.
    for target in [
        "reader.pages",
        "reader",
        "tests.pagination",
        "std.text",
        "",
        "reader.pages.",
        ".paginate",
        "reader..pages.paginate",
        "tests.pagination.\"unterminated",
        "reader.pages.paginate.body",
        "tests.pagination.\"\\q\"",
    ] {
        let (code, env) = json_of(&fx.reader, &["inspect", target]);
        assert_eq!(code, 1, "{target:?}: {env}");
        assert_eq!(env["status"], "unsuccessful");
        assert_eq!(
            kinds(&env),
            pairs(&[("error", "index_unknown_symbol")]),
            "{target:?}"
        );
        assert_eq!(env["diagnostics"][0]["category"], "index");
        assert!(env.get("symbol").is_none());
    }
}

#[test]
fn a_duplicated_id_names_its_first_declaration() {
    let fx = Fixture::new("adv-duplicate-inspect");
    fx.write(
        "reader/src/reader/twin.cpl",
        "module reader.twin\n\n@id(\"pgn_7c2d\")\nfn twin() -> Int {\n    1\n}\n",
    );
    // 4.5: the later occurrence in path order is given its provisional
    // identity, which the index takes from the Program; duplicate_identity
    // is a check error, so the answer comes with index_partial.
    let (code, env) = json_of(&fx.reader, &["inspect", "pgn_7c2d"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(env["symbol"]["path"], "reader.pages.paginate");
    assert_eq!(kinds(&env), pairs(&[("warning", "index_partial")]));
    let (code, env) = json_of(&fx.reader, &["inspect", "reader.twin.twin"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(env["symbol"]["id"], "reader.twin.twin");
    assert_eq!(env["symbol"]["provisional"], true);
}

#[test]
fn quoted_test_names_resolve_with_their_escapes() {
    let fx = Fixture::new("adv-quoted");
    fx.write(
        TESTS_CPL,
        &format!(
            "{}\ntest \"say \\\"hi\\\" and \\\\ more\" {{\n    assert true\n}}\n",
            fx.read(TESTS_CPL)
        ),
    );
    let path = "tests.pagination.\"say \\\"hi\\\" and \\\\ more\"";
    let (code, env) = json_of(&fx.reader, &["inspect", path]);
    assert_eq!(code, 0, "{env}");
    let symbol = &env["symbol"];
    assert_eq!(symbol["path"], path);
    assert_eq!(symbol["id"], path);
    assert_eq!(symbol["kind"], "test");
    assert_eq!(symbol["provisional"], true);
    // 5.2: the name facet is the canonically escaped literal, quotes included.
    assert_eq!(
        symbol["facets"]["name"],
        json!(facet_hash("\"say \\\"hi\\\" and \\\\ more\""))
    );
}

#[test]
fn the_include_key_is_the_effective_set_in_include_order() {
    let Some(fx) = Fixture::committed("adv-include-order") else {
        return;
    };
    let run = cpl(
        &fx.reader,
        &[
            "inspect",
            "paginate",
            "--include",
            "changes,tests,obligations,dependencies,callers,docs,implementation,effects,contract,signature",
            "--since",
            "HEAD",
            "--json",
        ],
    );
    run.expect_code(0);
    let env = run.json();
    // 1.8: `include` is in the derived order of `Include`.
    assert_eq!(
        env["include"],
        json!([
            "signature",
            "contract",
            "effects",
            "implementation",
            "docs",
            "callers",
            "dependencies",
            "obligations",
            "tests",
            "changes"
        ])
    );
    let doc = Ordered::parse(&run.stdout);
    // The keys of 6.4's table, in its order.
    assert_eq!(
        doc.keys(),
        [
            "command",
            "toolchain",
            "revision",
            "status",
            "diagnostics",
            "include",
            "symbol",
            "dependencies",
            "callers",
            "obligations",
            "tests",
            "changes"
        ]
    );
    let symbol = doc.get("symbol");
    assert_eq!(
        symbol.keys(),
        [
            "id",
            "path",
            "provisional",
            "facets",
            "definition",
            "kind",
            "module",
            "public",
            "location",
            "verification_policy",
            "signature",
            "contract",
            "effects",
            "docs",
            "implementation",
            "holes"
        ]
    );
    assert_eq!(symbol.get("facets").keys(), FACETS);
    assert_eq!(
        symbol.get("location").keys(),
        ["file", "line", "col", "end_line", "end_col"]
    );
    for list in ["dependencies", "callers", "tests"] {
        for block in doc.get(list).items() {
            assert_declared_order(&block.keys(), &BLOCK_KEYS);
            assert_declared_order(&block.get("facets").keys(), &FACETS);
        }
    }
    assert_eq!(doc.get("changes").keys(), ["since", "statements"]);
    assert_eq!(
        doc.get("changes").get("since").keys(),
        ["commit", "state", "digest"]
    );
}

#[test]
fn implementation_is_given_for_functions_tests_and_properties_only() {
    let fx = Fixture::new("adv-implementation");
    let (code, env) = json_of(
        &fx.reader,
        &[
            "inspect",
            "reader.pages.Positive",
            "--include",
            "implementation",
        ],
    );
    assert_eq!(code, 0, "{env}");
    assert!(env["symbol"].get("implementation").is_none(), "{env}");
    for target in [TESTS[1], TESTS[2]] {
        let (code, env) = json_of(
            &fx.reader,
            &["inspect", target, "--include", "implementation"],
        );
        assert_eq!(code, 0, "{target}: {env}");
        let body = env["symbol"]["implementation"]
            .as_str()
            .unwrap_or_else(|| panic!("a body for {target}: {env}"));
        assert!(body.starts_with('{') && body.ends_with('}'), "{body}");
        assert!(body.contains("assert "), "{body}");
        assert_eq!(env["symbol"]["holes"], json!([]), "{target}");
    }
}

#[test]
fn docs_are_text_or_null_when_included_and_absent_otherwise() {
    let fx = Fixture::new("adv-docs");
    let (_, env) = json_of(&fx.reader, &["inspect", "paginate", "--include", "docs"]);
    // Appendix C: a symbol block's `docs` is present and null when included.
    assert!(env["symbol"]
        .as_object()
        .expect("a block")
        .contains_key("docs"));
    assert_eq!(env["symbol"]["docs"], Value::Null);
    let (_, env) = json_of(
        &fx.reader,
        &["inspect", "std.text.trim", "--include", "docs"],
    );
    assert!(
        env["symbol"]["docs"]
            .as_str()
            .is_some_and(|d| d.contains("White_Space")),
        "{env}"
    );
    edit(
        &fx,
        PAGES,
        "@id(\"pgn_7c2d\")",
        "/// Split items into pages.\n@id(\"pgn_7c2d\")",
    );
    let (_, env) = json_of(&fx.reader, &["inspect", "paginate", "--include", "docs"]);
    assert!(
        env["symbol"]["docs"]
            .as_str()
            .is_some_and(|d| d.contains("Split items into pages.")),
        "{env}"
    );
    let (_, env) = json_of(
        &fx.reader,
        &["inspect", "paginate", "--include", "signature"],
    );
    assert!(env["symbol"].get("docs").is_none());
}

#[test]
fn effects_are_sorted_param_op_strings() {
    let fx = Fixture::new("adv-effects-sorted");
    fx.write("reader/src/reader/store.cpl", STORE);
    for (target, effects) in [
        ("reader.store.touch", json!(["db.get", "db.put"])),
        ("reader.pages.load_page", json!(["db.get"])),
        ("reader.pages.clean", json!([])),
    ] {
        let (code, env) = json_of(&fx.reader, &["inspect", target, "--include", "effects"]);
        assert_eq!(code, 0, "{target}: {env}");
        assert_eq!(env["symbol"]["effects"], effects, "{target}");
    }
}

#[test]
fn facet_and_definition_hashes_follow_sections_5_2_to_5_4() {
    let fx = Fixture::new("adv-facet-hashes");
    fx.write("reader/src/reader/store.cpl", STORE);
    let contract = PAGINATE_CONTRACT.join("\n");
    let cases: Vec<(&str, Vec<(&str, &str)>)> = vec![
        (
            "pgn_7c2d",
            vec![
                ("identity", "pgn_7c2d"),
                ("name", "paginate"),
                (
                    "signature",
                    "pub fn _<T>(items: List<T>, size: Positive) -> List<List<T>>",
                ),
                ("contract", contract.as_str()),
                ("effects", ""),
                ("docs", ""),
                ("verification_policy", "@prove"),
            ],
        ),
        (
            "reader.pages.clean",
            vec![
                ("identity", "reader.pages.clean"),
                ("name", "clean"),
                ("signature", "fn _(page: Page) -> Page"),
                ("contract", ""),
                ("effects", ""),
                ("verification_policy", ""),
            ],
        ),
        (
            "reader.pages.divide_exactly",
            vec![
                ("signature", "fn _(a: Int, b: Int) -> Int"),
                (
                    "contract",
                    "requires b != 0\nrequires a % b == 0\nensures result * b == a",
                ),
            ],
        ),
        (
            "reader.pages.load_page",
            vec![
                (
                    "signature",
                    "pub fn _(id: PageId, using db: PageRead) -> Result<Page, StoreError>",
                ),
                ("effects", "effects { db.get }"),
            ],
        ),
        (
            "reader.store.touch",
            vec![("effects", "effects { db.get, db.put }")],
        ),
        (
            "reader.pages.PageId",
            vec![("signature", "pub type _(Text)")],
        ),
        (
            "reader.pages.Positive",
            vec![
                ("identity", "reader.pages.Positive"),
                ("name", "Positive"),
                ("signature", "pub type _ = Int"),
                ("contract", "where self > 0"),
                ("docs", ""),
            ],
        ),
        (
            "reader.pages.Page",
            vec![(
                "signature",
                "pub struct _ {\n    id: PageId\n    text: Text\n}",
            )],
        ),
        (
            "reader.pages.StoreError",
            vec![(
                "signature",
                "pub enum _ {\n    Missing(PageId)\n    Unavailable\n}",
            )],
        ),
        (
            "reader.pages.PageRead",
            vec![
                (
                    "signature",
                    "pub capability _ {\n    fn get(id: PageId) -> Result<Page, StoreError>\n}",
                ),
                ("contract", ""),
            ],
        ),
        (
            TESTS[0],
            vec![
                ("identity", TESTS[0]),
                ("name", "\"pagination handles empty input\""),
            ],
        ),
        (
            TESTS[2],
            vec![(
                "signature",
                "for items in gen.list(gen.int)\nfor size in gen.positive_int",
            )],
        ),
    ];
    for (target, texts) in cases {
        let (code, env) = json_of(&fx.reader, &["inspect", target]);
        assert_eq!(code, 0, "{target}: {env}");
        let facets = &env["symbol"]["facets"];
        for (facet, text) in texts {
            assert_eq!(
                facets[facet],
                json!(facet_hash(text)),
                "{target}.{facet}: the hash of {text:?}"
            );
        }
        assert_eq!(
            env["symbol"]["definition"],
            json!(definition_hash(facets)),
            "{target}"
        );
        for dep in env["interface_dependencies"]
            .as_array()
            .into_iter()
            .flatten()
        {
            assert_eq!(
                dep["definition"],
                json!(definition_hash(&dep["facets"])),
                "{dep}"
            );
        }
    }
    // 5.1: the facets that apply, by kind, in `Facet::ALL` order.
    for (target, expected) in [
        ("pgn_7c2d", FACETS.to_vec()),
        (
            "reader.pages.Positive",
            vec!["identity", "name", "signature", "contract", "docs"],
        ),
        (
            "reader.pages.PageId",
            vec!["identity", "name", "signature", "docs"],
        ),
        (
            "reader.pages.PageRead",
            vec!["identity", "name", "signature", "contract", "docs"],
        ),
        (TESTS[0], vec!["identity", "name", "body", "docs"]),
        (
            TESTS[2],
            vec!["identity", "name", "signature", "body", "docs"],
        ),
    ] {
        let run = cpl(&fx.reader, &["inspect", target, "--json"]);
        run.expect_code(0);
        let doc = Ordered::parse(&run.stdout);
        assert_eq!(doc.get("symbol").get("facets").keys(), expected, "{target}");
    }
}

#[test]
fn changes_compare_since_with_the_working_tree() {
    let Some(fx) = Fixture::committed("adv-changes-worktree") else {
        return;
    };
    let committed = project_digest(&fx.reader);
    edit_contract(&fx);
    let (code, env) = json_of(
        &fx.reader,
        &[
            "inspect",
            "paginate",
            "--include",
            "changes",
            "--since",
            "HEAD",
        ],
    );
    assert_eq!(code, 0, "{env}");
    assert_eq!(env["revision"]["state"], "worktree");
    let changes = &env["changes"];
    assert_eq!(
        changes["since"],
        json!({ "commit": head(&fx), "state": "clean", "digest": committed })
    );
    let list = changes["statements"].as_array().expect("statements");
    assert_eq!(list.len(), 1, "{list:?}");
    assert_eq!(list[0]["statement"], "contract_changed");
    // The statement `cpl diff` makes about the same two trees.
    let (_, diff) = json_of(&fx.reader, &["diff", "HEAD"]);
    assert_eq!(diff["statements"], json!([list[0]]));
    // Only statements about the target; std is never part of a revision.
    for target in ["divide_exactly", "std.text.trim"] {
        let (code, env) = json_of(
            &fx.reader,
            &["inspect", target, "--include", "changes", "--since", "HEAD"],
        );
        assert_eq!(code, 0, "{target}: {env}");
        assert_eq!(env["changes"]["statements"], json!([]), "{target}");
    }
}

#[test]
fn changes_of_added_and_renamed_declarations() {
    let Some(fx) = Fixture::committed("adv-changes-identity") else {
        return;
    };
    fx.write(
        PAGES,
        &format!("{}\npub fn fresh() -> Int {{\n    1\n}}\n", fx.read(PAGES)),
    );
    edit(&fx, PAGES, "pub fn paginate<T>(", "pub fn pages_of<T>(");
    edit_contract(&fx);
    let renamed = vec!["identity_renamed", "contract_changed"];
    for (target, expected) in [
        ("reader.pages.fresh", vec!["identity_added"]),
        ("pgn_7c2d", renamed.clone()),
        ("reader.pages.pages_of", renamed.clone()),
    ] {
        let (code, env) = json_of(
            &fx.reader,
            &["inspect", target, "--include", "changes", "--since", "HEAD"],
        );
        assert_eq!(code, 0, "{target}: {env}");
        let list = env["changes"]["statements"].as_array().expect("statements");
        let names: Vec<&str> = list
            .iter()
            .map(|s| s["statement"].as_str().expect("a statement"))
            .collect();
        assert_eq!(names, expected, "{target}");
    }
}

#[test]
fn changes_bases_that_are_no_commits_are_unknown_revisions() {
    let Some(fx) = Fixture::committed("adv-changes-errors") else {
        return;
    };
    for since in [
        "",
        "-x",
        "HEAD^{tree}",
        "HEAD:reader/cpl.toml",
        "@{upstream}",
    ] {
        let flag = format!("--since={since}");
        let (code, env) = json_of(
            &fx.reader,
            &["inspect", "paginate", "--include", "changes", flag.as_str()],
        );
        assert_eq!(code, 2, "{since:?}: {env}");
        assert_eq!(env["status"], "error");
        assert_eq!(
            kinds(&env),
            pairs(&[("error", "git_unknown_revision")]),
            "{since:?}"
        );
        assert!(env.get("symbol").is_none() && env.get("changes").is_none());
    }
}

#[test]
fn changes_are_answered_with_index_partial_when_the_tree_does_not_check() {
    let Some(fx) = Fixture::committed("adv-changes-partial") else {
        return;
    };
    edit_contract(&fx);
    edit(&fx, PAGES, "    a / b\n", "    a / \"b\"\n");
    let (code, env) = json_of(
        &fx.reader,
        &[
            "inspect",
            "paginate",
            "--include",
            "changes,callers",
            "--since",
            "HEAD",
        ],
    );
    assert_eq!(code, 0, "{env}");
    assert_eq!(kinds(&env), pairs(&[("warning", "index_partial")]));
    let names: Vec<&str> = env["changes"]["statements"]
        .as_array()
        .expect("statements")
        .iter()
        .map(|s| s["statement"].as_str().expect("a statement"))
        .collect();
    assert_eq!(names, ["contract_changed"]);
    assert!(env["callers"].is_array());
}

#[test]
fn inspect_human_output_has_a_changes_section() {
    let Some(fx) = Fixture::committed("adv-changes-human") else {
        return;
    };
    edit_contract(&fx);
    let run = cpl(
        &fx.reader,
        &[
            "inspect",
            "paginate",
            "--include",
            "changes",
            "--since",
            "HEAD",
        ],
    );
    run.expect_code(0);
    assert_eq!(run.stderr, "");
    let lines: Vec<&str> = run.stdout.lines().collect();
    assert!(
        lines.iter().any(|l| l.starts_with("changes")),
        "{}",
        run.stdout
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("contract_changed") && l.contains("reader.pages.paginate")),
        "{}",
        run.stdout
    );
}

// ===========================================================================
// cpl prove (Sections 7.2, 8.1)
// ===========================================================================

#[test]
fn the_obligations_of_paginate_are_those_of_section_8_1() {
    let fx = Fixture::new("adv-paginate-obligations");
    let (code, env) = json_of(&fx.reader, &["prove", "paginate"]);
    assert_eq!(code, 1, "{env}");
    let by_id = obligations_by_id(&env);
    for (id, kind, clause, line) in [
        (
            "pgn_7c2d#ensures/1",
            "ensures",
            "flatten(result) == items",
            46,
        ),
        (
            "pgn_7c2d#ensures/2",
            "ensures",
            "forall page in result: page.len > 0 and page.len <= size",
            47,
        ),
        (
            "pgn_7c2d#ensures/3",
            "ensures",
            "forall page in result.drop_last(): page.len == size",
            48,
        ),
        (
            "pgn_7c2d#loop/1/invariant/1/init",
            "invariant_init",
            "flatten(pages) ++ rest == items",
            54,
        ),
        (
            "pgn_7c2d#loop/1/invariant/1/preserve",
            "invariant_preserve",
            "flatten(pages) ++ rest == items",
            54,
        ),
        (
            "pgn_7c2d#loop/1/invariant/2/init",
            "invariant_init",
            "forall page in pages: page.len == size",
            55,
        ),
        (
            "pgn_7c2d#loop/1/invariant/2/preserve",
            "invariant_preserve",
            "forall page in pages: page.len == size",
            55,
        ),
        (
            "pgn_7c2d#loop/1/decreases",
            "loop_decreases",
            "rest.len",
            56,
        ),
    ] {
        let o = by_id
            .get(id)
            .unwrap_or_else(|| panic!("no {id} in {:?}", by_id.keys()));
        assert_eq!(o["kind"], kind, "{id}");
        assert_eq!(o["clause"], clause, "{id}");
        assert_eq!(o["location"]["file"], "src/reader/pages.cpl", "{id}");
        assert_eq!(o["location"]["line"], line, "{id}");
    }
    // The preconditions of the body's calls `rest.take(size)` and
    // `rest.drop(size)` (`requires n >= 0`, one each), and nothing else.
    let calls: Vec<&String> = by_id
        .iter()
        .filter(|(_, o)| o["kind"] == "call_requires")
        .map(|(id, _)| id)
        .collect();
    assert_eq!(calls.len(), 2, "{calls:?}");
    for id in &calls {
        assert!(
            id.starts_with("pgn_7c2d#call/") && id.ends_with("/requires/1"),
            "{id}"
        );
    }
    assert_eq!(by_id.len(), 10, "{:?}", by_id.keys());
    for o in by_id.values() {
        assert_eq!(o["status"], "unknown", "{o}");
        assert_eq!(o["reason"], "verifier_unavailable", "{o}");
        assert_eq!(o["evidence"], Value::Null, "{o}");
        assert_eq!(o["assumptions"], json!([]), "{o}");
    }
}

#[test]
fn divisions_in_clauses_and_bodies_are_obligations_and_requires_are_not() {
    let fx = Fixture::new("adv-divisions");
    let (code, env) = json_of(&fx.reader, &["prove", "reader.pages.divide_exactly"]);
    assert_eq!(code, 1, "{env}");
    let by_id = obligations_by_id(&env);
    let ids: Vec<&str> = by_id.keys().map(String::as_str).collect();
    // 8.1: the n-th `/` or `%`, body order, clauses included: the `%` of the
    // second `requires`, then the body's `/`. A `requires` clause is the
    // caller's obligation, not the function's.
    assert_eq!(
        ids,
        [
            "reader.pages.divide_exactly#div/1",
            "reader.pages.divide_exactly#div/2",
            "reader.pages.divide_exactly#ensures/1",
        ]
    );
    let ensures = &by_id["reader.pages.divide_exactly#ensures/1"];
    assert_eq!(ensures["kind"], "ensures");
    assert_eq!(ensures["clause"], "result * b == a");
    assert_eq!(ensures["location"]["line"], 108);
    let first = &by_id["reader.pages.divide_exactly#div/1"];
    let second = &by_id["reader.pages.divide_exactly#div/2"];
    assert_eq!(first["kind"], "division");
    assert_eq!(second["kind"], "division");
    assert_eq!(first["location"]["line"], 107);
    assert_eq!(second["location"]["line"], 110);
}

#[test]
fn loops_asserts_and_divisions_are_numbered_in_body_order() {
    let fx = Fixture::new("adv-numbering");
    fx.write("reader/src/reader/nest.cpl", NEST);
    let (code, env) = json_of(&fx.reader, &["prove", "reader.nest.loops"]);
    assert_eq!(code, 1, "{env}");
    let by_id = obligations_by_id(&env);
    // 8.1: the n-th loop and the n-th `assert` in body order (an inner
    // loop after the loop that holds it), invariants in order; `requires`
    // is not the function's obligation.
    let expected = [
        ("loop/1/invariant/1/init", "invariant_init", "i <= n", 9),
        (
            "loop/1/invariant/1/preserve",
            "invariant_preserve",
            "i <= n",
            9,
        ),
        ("loop/1/decreases", "loop_decreases", "n - i", 10),
        ("loop/2/invariant/1/init", "invariant_init", "j <= i", 14),
        (
            "loop/2/invariant/1/preserve",
            "invariant_preserve",
            "j <= i",
            14,
        ),
        ("loop/2/decreases", "loop_decreases", "i - j", 15),
        ("loop/3/invariant/1/init", "invariant_init", "k <= n", 23),
        (
            "loop/3/invariant/1/preserve",
            "invariant_preserve",
            "k <= n",
            23,
        ),
        ("loop/3/decreases", "loop_decreases", "n - k", 24),
        ("assert/1", "assert", "k == n", 28),
        ("assert/2", "assert", "i == n", 29),
    ];
    for (suffix, kind, clause, line) in expected {
        let id = format!("reader.nest.loops#{suffix}");
        let o = by_id
            .get(&id)
            .unwrap_or_else(|| panic!("no {id} in {:?}", by_id.keys()));
        assert_eq!(o["kind"], kind, "{id}");
        assert_eq!(o["clause"], clause, "{id}");
        assert_eq!(o["location"]["line"], line, "{id}");
    }
    for suffix in ["div/1", "div/2"] {
        let id = format!("reader.nest.loops#{suffix}");
        assert_eq!(by_id[&id]["kind"], "division", "{id}");
        assert_eq!(by_id[&id]["location"]["line"], 30, "{id}");
    }
    assert_eq!(by_id.len(), expected.len() + 2, "{:?}", by_id.keys());
}

#[test]
fn inspect_and_prove_report_the_same_obligations() {
    let fx = Fixture::new("adv-same-obligations");
    fx.write("reader/src/reader/extra.cpl", EXTRA);
    // T3 (design line 261): one index answers both commands.
    for target in [
        "paginate",
        "reader.pages.divide_exactly",
        "reader.extra.two_pages",
    ] {
        let (_, inspect) = json_of(&fx.reader, &["inspect", target, "--include", "obligations"]);
        let (_, prove) = json_of(&fx.reader, &["prove", target]);
        assert_eq!(inspect["obligations"], prove["obligations"], "{target}");
        assert_eq!(inspect["revision"], prove["revision"], "{target}");
        assert_eq!(version_of(&inspect["symbol"]), prove["symbol"], "{target}");
    }
}

#[test]
fn a_plain_function_reports_its_status_receipt_and_policy() {
    let fx = Fixture::new("adv-plain-prove");
    let (code, env) = json_of(&fx.reader, &["prove", "reader.pages.divide_exactly"]);
    assert_eq!(code, 1, "{env}");
    assert_eq!(env["status"], "unsuccessful");
    assert_eq!(env["result"], "unknown");
    assert_eq!(env["symbol"]["id"], "reader.pages.divide_exactly");
    assert_eq!(env["symbol"]["provisional"], true);
    let d = only(&env, "verification_result");
    assert_eq!(d["severity"], "info");
    assert_eq!(d["category"], "contract");
    assert_eq!(d["symbol"], "reader.pages.divide_exactly");
    assert_eq!(d["qualified_name"], "reader.pages.divide_exactly");
    assert_eq!(d["facet"], "verification_policy");
    assert_eq!(d["status"], "unknown");
    assert_eq!(d["reason"], "verifier_unavailable");
    // Semantics S63 (3): the name token.
    assert_eq!(
        d["span"],
        json!({ "file": "src/reader/pages.cpl", "line": 105, "col": 4, "end_line": 105, "end_col": 18 })
    );
    assert_eq!(d["receipt"], env["receipt"]["core"]);
    let receipt = &env["receipt"];
    assert_eq!(receipt["status"], "unknown");
    assert_eq!(receipt["target"], env["symbol"]);
    assert_eq!(receipt["obligations"], env["obligations"]);
    assert_eq!(
        receipt["configuration"]["policy"],
        json!({ "prove": false })
    );
    let run = cpl(&fx.reader, &["prove", "reader.pages.divide_exactly"]);
    run.expect_code(1);
    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(lines.len(), 3, "{}", run.stdout);
    assert_eq!(
        lines[0],
        "prove reader.pages.divide_exactly (reader.pages.divide_exactly): unknown"
    );
    assert_eq!(
        lines[1],
        "  3 obligations: 0 proved, 0 failed, 3 unknown (verifier unavailable in cpl 0.1.0)"
    );
}

#[test]
fn a_literal_refinement_is_proved_by_the_type_checker() {
    let fx = Fixture::new("adv-literal-refinement");
    fx.write("reader/src/reader/extra.cpl", EXTRA);
    let (code, env) = json_of(&fx.reader, &["prove", "reader.extra.two_pages"]);
    // Semantics 10 and S36: no `proved` result in Milestone 1; T29: exit 1.
    assert_eq!(code, 1, "{env}");
    assert_eq!(env["result"], "unknown");
    let list = env["obligations"].as_array().expect("obligations");
    assert_eq!(list.len(), 1, "{list:?}");
    let o = &list[0];
    assert_eq!(o["id"], "reader.extra.two_pages#refine/1");
    assert_eq!(o["kind"], "refinement");
    // 8.1: statically established, so proved with `{"by": "literal"}`.
    assert_eq!(o["status"], "proved");
    assert_eq!(o["reason"], Value::Null);
    assert_eq!(o["evidence"], json!({ "by": "literal" }));
    assert_eq!(env["receipt"]["obligations"], env["obligations"]);
    let run = cpl(&fx.reader, &["prove", "reader.extra.two_pages"]);
    let line = run.stdout.lines().nth(1).expect("a second line");
    assert!(line.contains(": 1 proved, 0 failed, 0 unknown"), "{line}");
}

#[test]
fn asserts_measures_and_termination_are_obligations() {
    let fx = Fixture::new("adv-measures");
    fx.write("reader/src/reader/extra.cpl", EXTRA);
    let (code, env) = json_of(&fx.reader, &["prove", "reader.extra.checked"]);
    assert_eq!(code, 1, "{env}");
    let by_id = obligations_by_id(&env);
    let ids: Vec<&str> = by_id.keys().map(String::as_str).collect();
    assert_eq!(ids, ["reader.extra.checked#assert/1"]);
    assert_eq!(by_id["reader.extra.checked#assert/1"]["kind"], "assert");
    assert_eq!(by_id["reader.extra.checked#assert/1"]["clause"], "n == n");
    // A recursive function with a measure: `#decreases`, and the
    // precondition at its recursive call.
    let (_, env) = json_of(&fx.reader, &["prove", "reader.extra.count"]);
    let by_id = obligations_by_id(&env);
    assert_eq!(by_id["reader.extra.count#decreases"]["kind"], "decreases");
    let calls: Vec<&String> = by_id
        .keys()
        .filter(|id| id.starts_with("reader.extra.count#call/"))
        .collect();
    assert_eq!(calls.len(), 1, "{:?}", by_id.keys());
    assert!(calls[0].ends_with("/requires/1"), "{}", calls[0]);
    assert_eq!(by_id.len(), 2, "{:?}", by_id.keys());
    // Without a measure: `#termination` in place of `#decreases`, unknown.
    let (_, env) = json_of(&fx.reader, &["prove", "reader.extra.forever"]);
    let by_id = obligations_by_id(&env);
    let ids: Vec<&str> = by_id.keys().map(String::as_str).collect();
    assert_eq!(ids, ["reader.extra.forever#termination"]);
    let o = &by_id["reader.extra.forever#termination"];
    assert_eq!(o["kind"], "termination");
    assert_eq!(o["status"], "unknown");
    // 8.1's row says `no_measure`, its Milestone 1 paragraph
    // `verifier_unavailable` for every obligation: either is unknown.
    assert!(
        o["reason"] == "no_measure" || o["reason"] == "verifier_unavailable",
        "{o}"
    );
}

#[test]
fn unknown_termination_of_a_prove_function_is_noted_on_its_result() {
    let fx = Fixture::new("adv-termination-note");
    fx.write("reader/src/reader/loops.cpl", LOOPS);
    let check = cpl(&fx.reader, &["check", "--json"]).json();
    // Warnings alone leave the index complete: no index_partial (7.2).
    let (code, env) = json_of(&fx.reader, &["inspect", "reader.loops.walk"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(env["diagnostics"], json!([]));
    // Semantics S36 (b) and 5.5: a `@prove` function with a
    // `missing_decreases` warning (5.4: recursive without a measure, or a
    // `while` loop without `decreases`) is unknown for termination too, and
    // its verification_result has a note saying so.
    for target in ["reader.loops.spin", "reader.loops.walk"] {
        let warned = with_code(&check, "missing_decreases")
            .iter()
            .any(|d| d["qualified_name"] == target);
        assert!(warned, "{target} has a missing_decreases warning: {check}");
        let (code, env) = json_of(&fx.reader, &["prove", target]);
        assert_eq!(code, 1, "{target}: {env}");
        let d = only(&env, "verification_result");
        let notes = d["notes"].as_array().map_or(0, Vec::len);
        assert!(notes >= 1, "{target}: a note on {d}");
    }
}

#[test]
fn a_builtin_has_no_obligations_of_its_own() {
    // Semantics S61: a `@builtin`'s clauses are axioms about it (read by the
    // Milestone 5 verifier as such), never obligations to discharge.
    let fx = Fixture::new("adv-builtin");
    let (code, env) = json_of(&fx.reader, &["prove", "std.prelude.List.take"]);
    assert_eq!(code, 1, "{env}");
    assert_eq!(env["symbol"]["path"], "std.prelude.List.take");
    assert_eq!(env["obligations"], json!([]));
    assert_eq!(env["result"], "unknown");
    let d = only(&env, "verification_result");
    assert_eq!(d["symbol"], "std.prelude.List.take");
    assert_eq!(d["qualified_name"], "std.prelude.List.take");
    assert_eq!(d["span"]["file"], "std/prelude.cpl");
}

#[test]
fn tests_capabilities_and_types_are_not_provable() {
    let fx = Fixture::new("adv-not-provable");
    for target in [
        TESTS[0],
        TESTS[1],
        "std.prelude.Option",
        "std.prelude.List",
        "std.foreign.ForeignError",
        "reader.images.ImageCodec",
        "reader.images.Dimensions",
    ] {
        let (code, env) = json_of(&fx.reader, &["prove", target]);
        assert_eq!(code, 2, "{target}: {env}");
        assert_eq!(env["status"], "error");
        assert_eq!(
            kinds(&env),
            pairs(&[("error", "usage_not_provable")]),
            "{target}"
        );
        assert_eq!(env["diagnostics"][0]["category"], "usage");
        for absent in ["symbol", "result", "obligations", "receipt"] {
            assert!(env.get(absent).is_none(), "{target}: {absent}");
        }
    }
}

#[test]
fn a_function_whose_body_is_a_hole_keeps_its_contract_obligations() {
    let fx = Fixture::new("adv-prove-hole");
    let (code, env) = json_of(&fx.holes, &["prove", "holes.pages.paginate"]);
    assert_eq!(code, 1, "{env}");
    let by_id = obligations_by_id(&env);
    let ids: Vec<&str> = by_id.keys().map(String::as_str).collect();
    assert_eq!(
        ids,
        [
            "pgn_7c2d#ensures/1",
            "pgn_7c2d#ensures/2",
            "pgn_7c2d#ensures/3"
        ]
    );
    assert_eq!(
        module_names(&env["receipt"]["core"]["modules"]),
        ["holes.pages"]
    );
    // S59: no named assumptions in Milestone 1.
    assert_eq!(env["receipt"]["assumptions"], json!([]));
    assert_eq!(env["receipt"]["core"]["assumptions"], json!([]));
    let (code, env) = json_of(&fx.holes, &["prove", "holes.pages.summarize"]);
    assert_eq!(code, 1, "{env}");
    assert_eq!(env["obligations"], json!([]));
}

// ===========================================================================
// Receipts (Section 8)
// ===========================================================================

#[test]
fn receipt_inputs_are_the_digests_of_the_files() {
    let Some(fx) = Fixture::committed("adv-receipt-inputs") else {
        return;
    };
    let files = source_files(&fx.reader, &["src"]);
    let sources = source_digests(&files);
    let (_, env) = json_of(&fx.reader, &["prove", "paginate"]);
    let receipt = &env["receipt"];
    assert_eq!(receipt["receipt"], "verification");
    assert_eq!(receipt["schema"], 1);
    assert_eq!(receipt["toolchain"], env["toolchain"]);
    assert_eq!(receipt["revision"], env["revision"]);
    assert_eq!(receipt["revision"]["digest"], json!(source_digest(&files)));
    let inputs = &receipt["inputs"];
    assert_eq!(inputs["sources"], json!(sources));
    assert_eq!(inputs["manifest"], json!(b3::hex(&files["cpl.toml"])));
    assert_eq!(inputs["lock"], Value::Null);
    assert_eq!(inputs["std"], json!(std_digest()));
    assert_eq!(
        receipt["core"]["toolchain"],
        json!({ "cpl": "0.1.0", "edition": "0.1", "std": std_digest() })
    );
    assert_eq!(receipt["core"]["source_revision"], json!(head(&fx)));
    // 1.8: a module's content hash is blake3 of its files' format_module
    // texts; the corpus is formatted, so for a one-file module it is the
    // file's digest.
    assert_eq!(
        receipt["core"]["modules"],
        json!([{ "module": "reader.pages", "content_hash": sources["src/reader/pages.cpl"] }])
    );
    // With a lock: `inputs.lock` is its digest, and it is no source.
    cpl(&fx.reader, &["check", "--lock"]).expect_code(0);
    let lock = std::fs::read(fx.reader.join("cpl.lock")).expect("read the lock");
    let (_, env) = json_of(&fx.reader, &["prove", "paginate"]);
    assert_eq!(env["receipt"]["inputs"]["lock"], json!(b3::hex(&lock)));
    assert_eq!(env["receipt"]["inputs"]["sources"], json!(sources));
    assert_eq!(
        env["receipt"]["revision"]["digest"],
        json!(source_digest(&files))
    );
}

#[test]
fn the_receipt_digest_names_the_evidence() {
    let Some(fx) = Fixture::committed("adv-receipt-digest") else {
        return;
    };
    let digest = || {
        let (_, env) = json_of(&fx.reader, &["prove", "paginate"]);
        receipt_digest(&env["receipt"])
    };
    let first = digest();
    // The human output names the receipt by this digest (Section 8; the
    // example of 7.2 abbreviates it).
    let name = human_receipt_name(&fx.reader, "paginate");
    let prefix = name.trim_end_matches('…');
    assert!(
        !prefix.is_empty() && first.starts_with(prefix),
        "{name} names {first}"
    );
    // Another run on the same revision: the same evidence (produced_at and
    // duration_ms are left out).
    assert_eq!(digest(), first);
    // A file outside the source set is no input.
    fx.write("reader/README.md", "x\n");
    assert_eq!(digest(), first);
    // Any source-set file is, even through a comment in a work file.
    let work = fx.read(WORK);
    fx.write(WORK, &format!("// a comment\n{work}"));
    assert_ne!(digest(), first);
    fx.write(WORK, &work);
    assert_eq!(digest(), first);
    // So is the lock.
    cpl(&fx.reader, &["check", "--lock"]).expect_code(0);
    assert_ne!(digest(), first);
}

#[test]
fn the_toolchains_receipt_digest_is_the_definition_of_section_8() {
    let Some(fx) = Fixture::committed("adv-receipt-definition") else {
        return;
    };
    let (_, env) = json_of(&fx.reader, &["prove", "paginate"]);
    let receipt = &env["receipt"];
    assert_eq!(
        cpl_index::receipt_digest(receipt).0,
        receipt_digest(receipt)
    );
    // Keys of the informational names deeper down are evidence and stay.
    let mut deeper = receipt.clone();
    deeper["core"]["check_configuration"]["duration_ms"] = json!(5);
    assert_ne!(receipt_digest(&deeper), receipt_digest(receipt));
    assert_eq!(
        cpl_index::receipt_digest(&deeper).0,
        receipt_digest(&deeper)
    );
    let mut top = receipt.clone();
    top["duration_ms"] = json!(123_456);
    top["produced_at"] = json!("1999-01-01T00:00:00Z");
    assert_eq!(receipt_digest(&top), receipt_digest(receipt));
    assert_eq!(cpl_index::receipt_digest(&top).0, receipt_digest(receipt));
}

#[test]
fn test_receipts_are_named_by_their_digest_and_share_the_inputs() {
    let Some(fx) = Fixture::committed("adv-test-receipt") else {
        return;
    };
    let write = |seed: &str, file: &str| {
        let path = fx.root.join(file);
        cpl(
            &fx.reader,
            &[
                "test",
                "--seed",
                seed,
                "--receipt",
                path.to_str().expect("a UTF-8 path"),
            ],
        )
        .expect_code(0);
        let text = std::fs::read_to_string(&path).expect("read the receipt");
        serde_json::from_str::<Value>(&text).expect("a JSON receipt")
    };
    let a = write("1", "a.json");
    let b = write("1", "b.json");
    let c = write("2", "c.json");
    assert_eq!(a["receipt"], "test");
    assert_eq!(
        receipt_digest(&a),
        receipt_digest(&b),
        "one seed, one evidence"
    );
    assert_ne!(
        receipt_digest(&a),
        receipt_digest(&c),
        "the seed is evidence"
    );
    assert_eq!(cpl_index::receipt_digest(&a).0, receipt_digest(&a));
    let (_, prove) = json_of(&fx.reader, &["prove", "paginate"]);
    assert_eq!(a["inputs"], prove["receipt"]["inputs"]);
    assert_eq!(a["revision"], prove["receipt"]["revision"]);
}

#[test]
fn a_test_receipt_has_the_shape_of_section_8_2() {
    let Some(fx) = Fixture::committed("adv-test-receipt-shape") else {
        return;
    };
    let path = fx.root.join("receipt.json");
    let path_text = path.to_str().expect("a UTF-8 path");
    cpl(
        &fx.reader,
        &[
            "test",
            "--seed",
            "1",
            "--receipt",
            path_text,
            "tests.pagination",
        ],
    )
    .expect_code(0);
    let text = std::fs::read_to_string(&path).expect("read the receipt");
    let receipt: Value = serde_json::from_str(&text).expect("a JSON receipt");
    let doc = Ordered::parse(&text);
    assert_eq!(
        doc.keys(),
        [
            "receipt",
            "schema",
            "toolchain",
            "revision",
            "inputs",
            "core",
            "configuration",
            "tested",
            "tests",
            "summary",
            "fixtures",
            "assumptions",
            "environment",
            "status",
            "produced_at",
            "duration_ms"
        ]
    );
    assert_eq!(receipt["receipt"], "test");
    assert_eq!(receipt["status"], "tested");
    assert_eq!(
        receipt["summary"],
        json!({ "selected": 3, "tested": 3, "failed": 0 })
    );
    // The three parts of the selector are always present (Appendix C).
    assert_eq!(
        receipt["configuration"]["selector"],
        json!({ "namespace": ["tests", "pagination"], "name": null, "id": null })
    );
    assert_eq!(receipt["configuration"]["seed"], 1);
    assert_eq!(
        receipt["core"]["check_configuration"],
        json!({ "cases": 100, "max_shrinks": 1000, "max_call_depth": 10000, "seed": 1 })
    );
    assert_eq!(receipt["core"]["fixtures"], json!([]));
    assert_eq!(receipt["fixtures"], json!([]));
    assert_eq!(receipt["assumptions"], json!([]));
    assert_eq!(receipt["core"]["environment"], receipt["environment"]);
    // `tests`: the deterministic part of each record only.
    for entry in receipt["tests"].as_array().expect("tests") {
        for absent in ["duration_ms", "seed", "receipt"] {
            assert!(entry.get(absent).is_none(), "{absent} in {entry}");
        }
        let property = entry["kind"] == "property";
        assert_eq!(entry.get("property_seed").is_some(), property, "{entry}");
        assert_eq!(entry.get("cases").is_some(), property, "{entry}");
    }
    // `tested`: sorted by display path, one entry per id, the tests included.
    let tested = receipt["tested"].as_array().expect("tested");
    let paths: Vec<&str> = tested
        .iter()
        .map(|v| v["path"].as_str().expect("a path"))
        .collect();
    let mut sorted = paths.clone();
    sorted.sort_unstable();
    assert_eq!(paths, sorted);
    let ids: BTreeSet<&str> = tested
        .iter()
        .map(|v| v["id"].as_str().expect("an id"))
        .collect();
    assert_eq!(ids.len(), tested.len());
    assert!(paths.contains(&"reader.pages.paginate"));
    for t in TESTS {
        assert!(paths.contains(&t), "{t}");
    }
    assert_eq!(
        module_names(&receipt["core"]["modules"]),
        ["reader.pages", "tests.pagination"]
    );
}

#[test]
fn receipts_and_prove_output_keys_come_in_declaration_order() {
    let Some(fx) = Fixture::committed("adv-prove-order") else {
        return;
    };
    let run = cpl(&fx.reader, &["prove", "paginate", "--json"]);
    run.expect_code(1);
    let doc = Ordered::parse(&run.stdout);
    assert_eq!(
        doc.keys(),
        [
            "command",
            "toolchain",
            "revision",
            "status",
            "diagnostics",
            "symbol",
            "result",
            "obligations",
            "receipt"
        ]
    );
    let version = ["id", "path", "provisional", "facets", "definition"];
    assert_eq!(doc.get("symbol").keys(), version);
    assert_eq!(doc.get("symbol").get("facets").keys(), FACETS);
    let receipt = doc.get("receipt");
    assert_eq!(
        receipt.keys(),
        [
            "receipt",
            "schema",
            "toolchain",
            "verifier",
            "revision",
            "core",
            "target",
            "dependencies",
            "inputs",
            "configuration",
            "status",
            "obligations",
            "assumptions",
            "produced_at",
            "duration_ms"
        ]
    );
    assert_eq!(
        receipt.get("verifier").keys(),
        ["name", "version", "configuration"]
    );
    assert_eq!(
        receipt.get("core").keys(),
        [
            "source_revision",
            "toolchain",
            "modules",
            "check_configuration",
            "assumptions"
        ]
    );
    assert_eq!(
        receipt.get("core").get("toolchain").keys(),
        ["cpl", "edition", "std"]
    );
    assert_eq!(
        receipt.get("core").get("modules").items()[0].keys(),
        ["module", "content_hash"]
    );
    assert_eq!(receipt.get("target").keys(), version);
    for dependency in receipt.get("dependencies").items() {
        assert_eq!(dependency.keys(), version);
        assert_declared_order(&dependency.get("facets").keys(), &FACETS);
    }
    assert_eq!(
        receipt.get("inputs").keys(),
        ["manifest", "lock", "std", "sources"]
    );
    let sources = receipt.get("inputs").get("sources").keys();
    let mut sorted = sources.clone();
    sorted.sort_unstable();
    assert_eq!(sources, sorted, "a map: sorted keys");
    assert_eq!(
        receipt.get("configuration").keys(),
        ["release", "policy", "proof_timeout_ms", "admitted"]
    );
    for obligations in [doc.get("obligations"), receipt.get("obligations")] {
        for o in obligations.items() {
            assert_eq!(
                o.keys(),
                [
                    "id",
                    "kind",
                    "clause",
                    "location",
                    "status",
                    "reason",
                    "evidence",
                    "assumptions"
                ]
            );
        }
    }
}

#[test]
fn produced_at_is_utc_rfc3339_in_whole_seconds() {
    let fx = Fixture::new("adv-produced-at");
    let (_, env) = json_of(&fx.reader, &["prove", "paginate"]);
    let at = env["receipt"]["produced_at"].as_str().expect("produced_at");
    let bytes = at.as_bytes();
    assert_eq!(bytes.len(), 20, "{at}");
    for (i, b) in bytes.iter().enumerate() {
        let ok = match i {
            4 | 7 => *b == b'-',
            10 => *b == b'T',
            13 | 16 => *b == b':',
            19 => *b == b'Z',
            _ => b.is_ascii_digit(),
        };
        assert!(ok, "{at} at {i}");
    }
    assert!(env["receipt"]["duration_ms"].is_u64());
}

#[test]
fn the_receipt_configuration_reads_the_verification_section() {
    let fx = Fixture::new("adv-verification-config");
    fx.write(
        "reader/cpl.toml",
        &format!(
            "{}\n[verification]\nproof_timeout_ms = 2500\nadmit = [\"fn_aaaaaaaaaaaaaaaa\"]\n",
            fx.read("reader/cpl.toml")
        ),
    );
    let (code, env) = json_of(&fx.reader, &["prove", "paginate"]);
    assert_eq!(code, 1, "{env}");
    assert_eq!(
        env["receipt"]["configuration"],
        json!({
            "release": false,
            "policy": { "prove": true },
            "proof_timeout_ms": 2500,
            "admitted": ["fn_aaaaaaaaaaaaaaaa"],
        })
    );
    // S59: a verification receipt's check_configuration is `{ release }`.
    assert_eq!(
        env["receipt"]["core"]["check_configuration"],
        json!({ "release": false })
    );
}

#[test]
fn a_module_spanning_two_files_hashes_both_in_path_order() {
    let fx = Fixture::new("adv-two-file-module");
    let more = "module reader.pages\n\npub fn more_pages() -> Int {\n    1\n}\n";
    fx.write("reader/src/reader/pages/more.cpl", more);
    // Both files are formatted, so their format_module texts are their bytes.
    cpl(&fx.reader, &["fmt", "--check"]).expect_code(0);
    let (code, env) = json_of(&fx.reader, &["prove", "paginate"]);
    assert_eq!(code, 1, "{env}");
    // 1.8: every file of the module in path order (`pages.cpl` before
    // `pages/more.cpl`), joined with LF.
    let expected = b3::hex(format!("{}\n{more}", fx.read(PAGES)).as_bytes());
    assert_eq!(
        env["receipt"]["core"]["modules"],
        json!([{ "module": "reader.pages", "content_hash": expected }])
    );
}

#[test]
fn receipt_modules_and_dependencies_follow_the_target() {
    let fx = Fixture::new("adv-closure-modules");
    fx.write("reader/src/reader/extra.cpl", EXTRA);
    cpl(&fx.reader, &["fmt", "--check"]).expect_code(0);
    let digest = |path: &str| b3::hex(fx.read(path).as_bytes());
    let (_, env) = json_of(&fx.reader, &["prove", "reader.extra.two_pages"]);
    // S59: the modules of the static call closure (project modules; std is
    // covered by toolchain.std, as the example of 8.1 shows).
    assert_eq!(
        env["receipt"]["core"]["modules"],
        json!([
            { "module": "reader.extra", "content_hash": digest("reader/src/reader/extra.cpl") },
            { "module": "reader.pages", "content_hash": digest(PAGES) },
        ])
    );
    let (_, env) = json_of(&fx.reader, &["prove", "reader.extra.checked"]);
    assert_eq!(
        module_names(&env["receipt"]["core"]["modules"]),
        ["reader.extra"]
    );
    // 8.1: `dependencies` are the target's dependencies, interface and body,
    // sorted by display path and unique: inspect's `dependencies`.
    let (_, inspect) = json_of(
        &fx.reader,
        &["inspect", "paginate", "--include", "dependencies"],
    );
    let (_, prove) = json_of(&fx.reader, &["prove", "paginate"]);
    let expected: Vec<Value> = inspect["dependencies"]
        .as_array()
        .expect("dependencies")
        .iter()
        .map(version_of)
        .collect();
    assert_eq!(prove["receipt"]["dependencies"], json!(expected));
    let paths: Vec<&str> = expected
        .iter()
        .map(|v| v["path"].as_str().expect("a path"))
        .collect();
    let mut sorted = paths.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(paths, sorted);
}

// ===========================================================================
// cpl work check (Section 7.2)
// ===========================================================================

/// `cpl work check work/pagination.work --json` in a committed copy of the
/// corpus after `edits` to the work file and with `manifest` appended to
/// cpl.toml: the exit code and the envelope; None without git.
fn work_check(name: &str, edits: &[(&str, &str)], manifest: &str) -> Option<(i32, Value)> {
    let fx = Fixture::committed(name)?;
    for (from, to) in edits {
        edit(&fx, WORK, from, to);
    }
    if !manifest.is_empty() {
        fx.write(
            "reader/cpl.toml",
            &format!("{}{manifest}", fx.read("reader/cpl.toml")),
        );
    }
    Some(json_of(
        &fx.reader,
        &["work", "check", "work/pagination.work"],
    ))
}

/// What every check of the corpus work file reports in a repository:
/// `Positive` is named without an `@id` (12.4).
const PROVISIONAL: (&str, &str) = ("warning", "work_provisional_identity");

/// The exit code, the exact (severity, code) multiset, and the location
/// and category of every `work_*` diagnostic.
#[track_caller]
fn assert_work(result: &(i32, Value), code: i32, expected: &[(&str, &str)]) {
    let (exit, env) = result;
    assert_eq!(kinds(env), pairs(expected), "{env}");
    assert_eq!(*exit, code, "{env}");
    assert_eq!(env["status"], if code == 0 { "ok" } else { "unsuccessful" });
    for d in env["diagnostics"].as_array().expect("diagnostics") {
        if d["code"].as_str().is_some_and(|c| c.starts_with("work_")) {
            assert_eq!(d["category"], "work", "{d}");
            assert_eq!(d["span"]["file"], "work/pagination.work", "{d}");
        }
    }
}

#[test]
fn work_check_requires_base_and_target() {
    let Some(r) = work_check("adv-work-no-base", &[("    base git(\"main\")\n", "")], "") else {
        return;
    };
    assert_work(&r, 1, &[("error", "work_missing_base"), PROVISIONAL]);
    assert!(r.1["changes"][0].get("base_commit").is_none());
    let Some(r) = work_check(
        "adv-work-no-target",
        &[("    target git(\"main\")\n", "")],
        "",
    ) else {
        return;
    };
    assert_work(&r, 1, &[("error", "work_missing_target"), PROVISIONAL]);
}

#[test]
fn work_check_requires_at_least_one_worker_and_defaults_to_one() {
    let Some(r) = work_check(
        "adv-work-no-workers",
        &[("    workers 3\n", "    workers 0\n")],
        "",
    ) else {
        return;
    };
    assert_work(&r, 1, &[("error", "work_bad_workers"), PROVISIONAL]);
    let Some(r) = work_check("adv-work-default-workers", &[("    workers 3\n", "")], "") else {
        return;
    };
    assert_work(&r, 0, &[PROVISIONAL]);
    assert_eq!(r.1["changes"][0]["workers"], 1);
}

#[test]
fn work_check_resolves_base_to_a_commit_and_wants_a_branch_as_target() {
    let Some(fx) = Fixture::committed("adv-work-refs") else {
        return;
    };
    let main = head(&fx);
    fx.git(&["tag", "-a", "v1", "-m", "an annotated tag"]);
    fx.git(&["branch", "feature"]);
    let original = fx.read(WORK);
    let check = |base: &str, target: &str| {
        let text = original
            .replace(
                "    base git(\"main\")\n",
                &format!("    base git(\"{base}\")\n"),
            )
            .replace(
                "    target git(\"main\")\n",
                &format!("    target git(\"{target}\")\n"),
            );
        fx.write(WORK, &text);
        json_of(&fx.reader, &["work", "check", "work/pagination.work"])
    };
    // D35: `base` may be any ref, resolved to a full commit id; `target`
    // must name a branch.
    for (base, target) in [
        ("main", "main"),
        ("v1", "main"),
        ("refs/tags/v1", "feature"),
        (main.as_str(), "refs/heads/main"),
        ("HEAD", "feature"),
    ] {
        let (code, env) = check(base, target);
        assert_eq!(code, 0, "{base}, {target}: {env}");
        assert_eq!(kinds(&env), pairs(&[PROVISIONAL]), "{base}, {target}");
        let change = &env["changes"][0];
        assert_eq!(change["base"], base);
        assert_eq!(change["target"], target);
        assert_eq!(change["base_commit"], json!(main), "{base}");
    }
    for target in ["v1", "refs/tags/v1", main.as_str(), "HEAD~0"] {
        let (code, env) = check("main", target);
        assert_eq!(code, 1, "{target}: {env}");
        assert_eq!(
            kinds(&env),
            pairs(&[("error", "work_target_not_branch"), PROVISIONAL]),
            "{target}"
        );
        assert_eq!(env["changes"][0]["base_commit"], json!(main));
    }
    for (base, target, unknown) in [
        ("nope", "main", 1),
        ("main", "nope", 1),
        ("nope", "nada", 2),
    ] {
        let (code, env) = check(base, target);
        assert_eq!(code, 1, "{base}, {target}: {env}");
        let mut expected = vec![PROVISIONAL];
        expected.extend(std::iter::repeat_n(("error", "work_unknown_ref"), unknown));
        assert_eq!(kinds(&env), pairs(&expected), "{base}, {target}");
        assert_eq!(
            env["changes"][0].get("base_commit").is_some(),
            base == "main",
            "{base}"
        );
    }
}

#[test]
fn work_check_selectors_must_select_but_add_namespaces_may_be_new() {
    let Some(r) = work_check(
        "adv-work-empty-selector",
        &[("        write {}\n", "        write { nope.body }\n")],
        "",
    ) else {
        return;
    };
    assert_work(&r, 1, &[("error", "work_selector_empty"), PROVISIONAL]);
    let Some(r) = work_check(
        "adv-work-new-namespace",
        &[("        write {}\n", "        add { brand.new.* }\n")],
        "",
    ) else {
        return;
    };
    assert_work(&r, 0, &[("warning", "work_add_namespace_new"), PROVISIONAL]);
    assert_eq!(
        r.1["changes"][0]["tasks"][2]["add"],
        json!([{ "selector": "brand.new.*", "namespace": "brand.new" }])
    );
    // 7.2: an add namespace is a module path not under std or gen.
    for namespace in ["std.extra.*", "gen.extra.*"] {
        let add = format!("        add {{ {namespace} }}\n");
        let Some((code, env)) = work_check(
            "adv-work-reserved-namespace",
            &[("        write {}\n", add.as_str())],
            "",
        ) else {
            return;
        };
        assert_eq!(code, 1, "{namespace}: {env}");
        assert!(!codes_with(&env, "error").is_empty(), "{namespace}");
    }
}

#[test]
fn work_check_facets_must_apply_and_identity_is_never_writable() {
    let Some(r) = work_check(
        "adv-work-facet",
        &[("Positive.definition", "Positive.body")],
        "",
    ) else {
        return;
    };
    assert_work(
        &r,
        1,
        &[("error", "work_facet_not_applicable"), PROVISIONAL],
    );
    let Some(r) = work_check(
        "adv-work-identity",
        &[(
            "        write { paginate.body }\n",
            "        write { paginate.identity }\n",
        )],
        "",
    ) else {
        return;
    };
    assert_work(
        &r,
        1,
        &[("error", "work_identity_not_writable"), PROVISIONAL],
    );
}

#[test]
fn work_check_after_and_review_name_tasks_and_after_is_acyclic() {
    let Some(r) = work_check(
        "adv-work-unknown-after",
        &[(
            "task audit after implementation, examples",
            "task audit after implementation, nobody",
        )],
        "",
    ) else {
        return;
    };
    assert_work(&r, 1, &[("error", "work_unknown_task"), PROVISIONAL]);
    // Unreviewed, `audit` (with `write {}`) also has no authority.
    let Some(r) = work_check(
        "adv-work-unknown-review",
        &[(
            "        require review(audit)\n",
            "        require review(nobody)\n",
        )],
        "",
    ) else {
        return;
    };
    assert_work(
        &r,
        1,
        &[
            ("error", "work_unknown_task"),
            ("warning", "work_task_without_authority"),
            PROVISIONAL,
        ],
    );
    let Some(r) = work_check(
        "adv-work-cycle",
        &[(
            "    task implementation {\n",
            "    task implementation after audit {\n",
        )],
        "",
    ) else {
        return;
    };
    assert_work(&r, 1, &[("error", "work_cycle"), PROVISIONAL]);
}

#[test]
fn work_check_overlap_counts_definitions_and_namespaces_but_not_files() {
    // `.definition` covers `.body`.
    let Some(r) = work_check(
        "adv-work-overlap-definition",
        &[(
            "        add { tests.pagination.* }\n",
            "        write { paginate }\n        add { tests.pagination.* }\n",
        )],
        "",
    ) else {
        return;
    };
    assert_work(&r, 1, &[("error", "work_overlapping_writes"), PROVISIONAL]);
    // T43: `tests.*` covers `tests.pagination`.
    let Some(r) = work_check(
        "adv-work-overlap-namespace",
        &[(
            "        write { paginate.body }\n",
            "        write { paginate.body }\n        add { tests.* }\n",
        )],
        "",
    ) else {
        return;
    };
    assert_work(&r, 1, &[("error", "work_overlapping_writes"), PROVISIONAL]);
    // File selectors are not compared.
    let Some(r) = work_check(
        "adv-work-overlap-files",
        &[
            (
                "        write { paginate.body }\n",
                "        write { paginate.body, project.manifest }\n",
            ),
            (
                "        add { tests.pagination.* }\n",
                "        write { project.manifest }\n        add { tests.pagination.* }\n",
            ),
        ],
        "\n[work.policy]\nfiles = [\"project.manifest\"]\n",
    ) else {
        return;
    };
    assert_work(&r, 0, &[PROVISIONAL]);
}

#[test]
fn work_check_requirements_must_match() {
    let Some(r) = work_check(
        "adv-work-empty-test",
        &[(
            "require test(tests.pagination.*)",
            "require test(nothing.here.*)",
        )],
        "",
    ) else {
        return;
    };
    assert_work(&r, 1, &[("error", "work_test_selector_empty"), PROVISIONAL]);
    let Some((code, env)) = work_check(
        "adv-work-prove-type",
        &[("require prove(paginate)", "require prove(Positive)")],
        "",
    ) else {
        return;
    };
    assert_eq!(code, 1, "{env}");
    assert_eq!(codes_with(&env, "error"), ["work_prove_not_function"]);
    assert!(codes_with(&env, "warning")
        .iter()
        .all(|c| c == "work_provisional_identity"));
}

#[test]
fn a_test_requirement_selects_a_namespace_or_the_tests_of_a_declaration() {
    // T43: `tests.*` covers `tests.pagination`.
    let Some(r) = work_check(
        "adv-work-test-namespace",
        &[("require test(tests.pagination.*)", "require test(tests.*)")],
        "",
    ) else {
        return;
    };
    assert_work(&r, 0, &[PROVISIONAL]);
    assert_eq!(r.1["changes"][0]["integrate"][2]["matched"], 3);
    // A declaration selector selects the tests that reach the declaration.
    let Some(r) = work_check(
        "adv-work-test-declaration",
        &[("require test(tests.pagination.*)", "require test(paginate)")],
        "",
    ) else {
        return;
    };
    assert_work(&r, 0, &[PROVISIONAL]);
    // No test reaches `load_page`, which is named without an `@id` too.
    let Some(r) = work_check(
        "adv-work-test-untested",
        &[(
            "require test(tests.pagination.*)",
            "require test(reader.pages.load_page)",
        )],
        "",
    ) else {
        return;
    };
    assert_work(
        &r,
        1,
        &[
            ("error", "work_test_selector_empty"),
            PROVISIONAL,
            PROVISIONAL,
        ],
    );
    // A facet selector selects no test.
    let Some(r) = work_check(
        "adv-work-test-facet",
        &[(
            "require test(tests.pagination.*)",
            "require test(paginate.body)",
        )],
        "",
    ) else {
        return;
    };
    assert_work(
        &r,
        1,
        &[("error", "work_facet_not_applicable"), PROVISIONAL],
    );
}

#[test]
fn ordered_tasks_may_overlap_and_namespaces_cover_their_declarations() {
    // `audit` comes after both other tasks: its writes overlap nothing.
    let Some(r) = work_check(
        "adv-work-ordered-write",
        &[("        write {}\n", "        write { paginate.body }\n")],
        "",
    ) else {
        return;
    };
    assert_work(&r, 0, &[PROVISIONAL]);
    // `.definition` covers every facet but identity (10.3), so writing it
    // is no identity write.
    let Some(r) = work_check(
        "adv-work-definition-write",
        &[("        write {}\n", "        write { paginate }\n")],
        "",
    ) else {
        return;
    };
    assert_work(&r, 0, &[PROVISIONAL]);
    // A namespace wildcard in a write set covers the declarations of the
    // namespace (D38) and names none of them individually (7.2).
    let Some(r) = work_check(
        "adv-work-namespace-write",
        &[(
            "        add { tests.pagination.* }\n",
            "        write { reader.pages.* }\n        add { tests.pagination.* }\n",
        )],
        "",
    ) else {
        return;
    };
    assert_work(&r, 1, &[("error", "work_overlapping_writes"), PROVISIONAL]);
    // A preserved `.definition` covers the task's written body.
    let Some(r) = work_check(
        "adv-work-preserve-definition",
        &[(
            "            paginate.signature,\n",
            "            paginate,\n",
        )],
        "",
    ) else {
        return;
    };
    assert_work(&r, 1, &[("error", "work_preserve_written"), PROVISIONAL]);
}

#[test]
fn an_invalid_work_policy_is_a_manifest_error_for_every_command() {
    let fx = Fixture::new("adv-invalid-policy");
    let original = fx.read("reader/cpl.toml");
    // T53: a `[work.policy] files` entry that is no file selector, or an
    // unknown boundary value, is the fatal project_manifest_invalid.
    for table in [
        "[work.policy]\nfiles = [\"cpl.toml\"]\n",
        "[work.policy]\nfilesystem = \"everything\"\n",
    ] {
        fx.write("reader/cpl.toml", &format!("{original}\n{table}"));
        for args in [
            vec!["work", "check", "work/pagination.work"],
            vec!["inspect", "paginate"],
            vec!["prove", "paginate"],
        ] {
            let (code, env) = json_of(&fx.reader, &args);
            assert_eq!(code, 2, "{table:?} {args:?}: {env}");
            assert_eq!(env["status"], "error");
            assert_eq!(
                kinds(&env),
                pairs(&[("error", "project_manifest_invalid")]),
                "{table:?} {args:?}"
            );
            let d = &env["diagnostics"][0];
            assert_eq!(d["category"], "project");
            assert!(d.get("span").is_none(), "{d}");
        }
    }
}

#[test]
fn work_check_rejects_preserving_a_written_facet() {
    let Some(r) = work_check(
        "adv-work-preserve-written",
        &[(
            "            paginate.signature,\n",
            "            paginate.body,\n            paginate.signature,\n",
        )],
        "",
    ) else {
        return;
    };
    assert_work(&r, 1, &[("error", "work_preserve_written"), PROVISIONAL]);
}

#[test]
fn work_check_warns_about_a_task_without_authority() {
    let Some(r) = work_check(
        "adv-work-no-authority",
        &[("        require review(audit)\n", "")],
        "",
    ) else {
        return;
    };
    assert_work(
        &r,
        0,
        &[("warning", "work_task_without_authority"), PROVISIONAL],
    );
}

#[test]
fn work_check_resolves_use_lines_as_a_module_does() {
    for (line, code) in [
        ("use reader.nope.{thing}\n", "unresolved_module"),
        ("use reader.pages.{nothing}\n", "unresolved_name"),
        ("use reader.pages.{clean}\n", "private_item"),
    ] {
        let replacement = format!("{line}use reader.pages.{{paginate, Positive}}\n");
        let Some((exit, env)) = work_check(
            "adv-work-use",
            &[(
                "use reader.pages.{paginate, Positive}\n",
                replacement.as_str(),
            )],
            "",
        ) else {
            return;
        };
        assert_eq!(exit, 1, "{line}: {env}");
        assert_eq!(codes_with(&env, "error"), [code], "{line}");
        let d = only(&env, code);
        assert_eq!(d["category"], "resolve");
        assert_eq!(d["span"]["file"], "work/pagination.work");
        assert_eq!(d["span"]["line"], 1);
    }
}

#[test]
fn work_check_grants_file_selectors_by_policy_only() {
    let Some(fx) = Fixture::committed("adv-work-files") else {
        return;
    };
    edit(
        &fx,
        WORK,
        "        write {}\n",
        "        write { project.manifest, project.lock, work.pagination }\n",
    );
    let result = json_of(&fx.reader, &["work", "check", "work/pagination.work"]);
    assert_work(
        &result,
        1,
        &[
            ("error", "work_file_not_grantable"),
            ("error", "work_file_not_grantable"),
            ("error", "work_file_not_grantable"),
            PROVISIONAL,
        ],
    );
    // T42: the three file selectors and the files they name.
    assert_eq!(
        result.1["changes"][0]["tasks"][2]["write"],
        json!([
            { "selector": "project.manifest", "file": "cpl.toml" },
            { "selector": "project.lock", "file": "cpl.lock" },
            { "selector": "work.pagination", "file": "work/pagination.work" },
        ])
    );
    fx.write(
        "reader/cpl.toml",
        &format!(
            "{}\n[work.policy]\nfiles = [\"project.manifest\", \"project.lock\", \"work.pagination\"]\n",
            fx.read("reader/cpl.toml")
        ),
    );
    let result = json_of(&fx.reader, &["work", "check", "work/pagination.work"]);
    assert_work(&result, 0, &[PROVISIONAL]);
}

#[test]
fn a_declaration_shadows_a_file_selector_of_its_spelling() {
    let Some(fx) = Fixture::committed("adv-work-shadow") else {
        return;
    };
    fx.write(
        "reader/src/project.cpl",
        "module project\n\npub fn manifest() -> Int {\n    1\n}\n",
    );
    edit(
        &fx,
        WORK,
        "        write {}\n",
        "        write { project.manifest }\n",
    );
    let (code, env) = json_of(&fx.reader, &["work", "check", "work/pagination.work"]);
    // T42: the declaration wins, with the warning; no grant is needed.
    assert_eq!(code, 0, "{env}");
    assert!(codes_with(&env, "error").is_empty(), "{env}");
    assert!(codes_with(&env, "warning").contains(&"work_file_selector_shadowed".to_string()));
    let write = &env["changes"][0]["tasks"][2]["write"][0];
    assert_eq!(write["selector"], "project.manifest");
    assert!(write.get("file").is_none(), "{write}");
    assert_eq!(
        write["targets"],
        json!([{ "id": "project.manifest", "facet": "definition" }])
    );
}

#[test]
fn work_check_json_keys_and_optional_fields() {
    let Some(fx) = Fixture::committed("adv-work-json") else {
        return;
    };
    edit(
        &fx,
        WORK,
        "        goal \"Review both patches against the frozen pagination contract.\"\n",
        "",
    );
    edit(
        &fx,
        WORK,
        "        require no_holes\n",
        "        require no_holes\n        require no_unresolved_assumptions\n",
    );
    let run = cpl(
        &fx.reader,
        &["work", "check", "work/pagination.work", "--json"],
    );
    run.expect_code(0);
    let env = run.json();
    let change = &env["changes"][0];
    assert_eq!(change["base_commit"], json!(head(&fx)));
    // Appendix C: WorkTaskJson::goal is omitted when absent.
    assert!(
        change["tasks"][2].get("goal").is_none(),
        "{}",
        change["tasks"][2]
    );
    assert_eq!(
        change["integrate"].as_array().and_then(|r| r.last()),
        Some(&json!({ "require": "no_unresolved_assumptions" }))
    );
    let doc = Ordered::parse(&run.stdout);
    assert_eq!(
        doc.keys(),
        [
            "command",
            "toolchain",
            "revision",
            "status",
            "diagnostics",
            "file",
            "changes"
        ]
    );
    let change = &doc.get("changes").items()[0];
    assert_eq!(
        change.keys(),
        [
            "name",
            "base",
            "base_commit",
            "target",
            "workers",
            "tasks",
            "integrate"
        ]
    );
    let tasks = change.get("tasks").items();
    assert_eq!(
        tasks[0].keys(),
        ["name", "after", "write", "add", "depend", "preserve", "goal"]
    );
    assert_eq!(
        tasks[2].keys(),
        ["name", "after", "write", "add", "depend", "preserve"]
    );
    let write = &tasks[0].get("write").items()[0];
    assert_eq!(write.keys(), ["selector", "targets"]);
    assert_eq!(write.get("targets").items()[0].keys(), ["id", "facet"]);
    assert_eq!(
        tasks[1].get("add").items()[0].keys(),
        ["selector", "namespace"]
    );
    let integrate = change.get("integrate").items();
    assert_eq!(integrate[1].keys(), ["require", "targets"]);
    assert_eq!(integrate[2].keys(), ["require", "namespace", "matched"]);
    assert_eq!(integrate[3].keys(), ["require", "task"]);
}

#[test]
fn work_check_validates_every_change_of_the_file() {
    let Some(fx) = Fixture::committed("adv-work-two-changes") else {
        return;
    };
    let second = "\nchange docs_update {\n    base git(\"main\")\n    target git(\"main\")\n\n    task docs {\n        write { paginate.docs }\n    }\n}\n";
    let original = fx.read(WORK);
    fx.write(WORK, &format!("{original}{second}"));
    let result = json_of(&fx.reader, &["work", "check", "work/pagination.work"]);
    assert_work(&result, 0, &[PROVISIONAL]);
    let env = &result.1;
    assert_eq!(env["changes"].as_array().map(Vec::len), Some(2));
    assert_eq!(env["changes"][1]["name"], "docs_update");
    assert_eq!(env["changes"][1]["workers"], 1);
    let run = cpl(&fx.reader, &["work", "check", "work/pagination.work"]);
    run.expect_code(0);
    assert_eq!(
        run.stdout,
        "work check: work/pagination.work: 2 changes, 4 tasks, ok\n"
    );
    // Grammar 4.1: `after` names tasks of the same change.
    let after = second.replace("task docs {", "task docs after audit {");
    fx.write(WORK, &format!("{original}{after}"));
    let result = json_of(&fx.reader, &["work", "check", "work/pagination.work"]);
    assert_work(&result, 1, &[("error", "work_unknown_task"), PROVISIONAL]);
}

#[test]
fn work_check_of_a_patch_file_is_its_parse_error() {
    let fx = Fixture::new("adv-work-patch");
    let (code, env) = json_of(&fx.reader, &["work", "check", "work/pagination.patch"]);
    assert_eq!(code, 1, "{env}");
    let list = env["diagnostics"].as_array().expect("diagnostics");
    // D59: one parse diagnostic, the earliest.
    assert_eq!(list.len(), 1, "{list:?}");
    assert_eq!(list[0]["category"], "parse");
    assert_eq!(list[0]["span"]["file"], "work/pagination.patch");
    assert!(env.get("changes").is_none());
}

// ===========================================================================
// Reserved commands (Sections 7.1, 7.2)
// ===========================================================================

#[test]
fn reserved_commands_are_not_implemented_whatever_surrounds_them() {
    let fx = Fixture::new("adv-reserved");
    let nowhere = fx.root.join("no-such-project");
    let nowhere = nowhere.to_str().expect("a UTF-8 path");
    let path = path_without_git();
    let no_git = [("PATH", path.as_str())];
    // A project whose manifest does not load is no obstacle: nothing is read.
    let broken = fx.root.join("broken");
    copy_dir(&fx.reader, &broken);
    std::fs::write(broken.join("cpl.toml"), "[project\n").expect("break the manifest");
    let cases: Vec<(&Path, Vec<&str>, &str)> = vec![
        (
            &fx.reader,
            vec!["work", "run", "work/nope.work"],
            "work run",
        ),
        (
            &broken,
            vec!["work", "run", "work/pagination.work"],
            "work run",
        ),
        (
            &broken,
            vec!["work", "integrate", "pagination_update"],
            "work integrate",
        ),
        (
            &fx.reader,
            vec!["work", "integrate", "no_such_change"],
            "work integrate",
        ),
        (
            &fx.root,
            vec!["--project", nowhere, "work", "run", "x.work"],
            "work run",
        ),
        (
            &fx.root,
            vec![
                "work",
                "integrate",
                "pagination_update",
                "--project",
                nowhere,
            ],
            "work integrate",
        ),
    ];
    for (dir, args, command) in cases {
        for env_vars in [&[][..], &no_git[..]] {
            let (code, env) = json_env(dir, &args, env_vars);
            assert_eq!(code, 2, "{args:?}: {env}");
            assert_eq!(env["command"], command);
            assert_eq!(env["status"], "error");
            assert_eq!(
                kinds(&env),
                pairs(&[("error", "usage_not_implemented")]),
                "{args:?}"
            );
            let d = &env["diagnostics"][0];
            assert_eq!(d["category"], "usage");
            let notes: Vec<&str> = d["notes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|n| n["message"].as_str())
                .collect();
            assert!(notes.iter().any(|n| n.contains("Milestone 6")), "{d}");
            let run = cpl_env(dir, &args, env_vars);
            run.expect_code(2);
            assert_eq!(run.stdout, "");
            assert!(
                run.stderr.starts_with("error[usage_not_implemented]: "),
                "{}",
                run.stderr
            );
        }
    }
}
