//! `cpl.toml` (`spec/toolchain.md` 11.1): the [`Manifest`] model, its
//! two-step parse ([`parse_manifest`]: a TOML table first, then field by
//! field), and the `[bindings.*]` checks that run after name resolution
//! ([`check_bindings`]).
//!
//! From Milestone 2 (T74, T80) the model also holds the C target's tables:
//! `[interop.c]` ([`Manifest::interop_c`], a [`CInteropSection`]) and the
//! `[bindings.<name>.c]` subtables ([`Manifest::binding_c`], each a
//! [`CBindingSection`]). Their values need no name resolution, so T53
//! applies to them as the manifest is parsed: a value outside its domain is
//! the fatal `project_manifest_invalid`. The rules that need the files or
//! the bindings together are [`crate::check_c_bindings`]'s.

use crate::codes;
use crate::util::{join_key, key, line_at};
use cpl_diag::json::{ExecutionBoundary, FsAccess, NetworkAccess, ProcessAccess};
use cpl_diag::{version, Category, Diagnostic, StableId};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::ops::Range;
use toml::de::{DeTable, DeValue};
use toml::Spanned;

/// The backends of `[bindings.*] backend`.
const BACKENDS: [&str; 3] = ["python", "node", "c"];

/// The targets of `[interop] targets`: the backends, and the source-level hosts of
/// `spec/hosts.md` (`kotlin` is `java`: Kotlin uses the Java sources).
const TARGETS: [&str; 7] = ["python", "node", "c", "rust", "go", "java", "kotlin"];
/// The values of `[bindings.*] isolation`.
const ISOLATIONS: [&str; 2] = ["worker", "in_process"];
/// The tables of edition 0.1; any other top-level key is unknown (T35).
const TOP_LEVEL: [&str; 8] = [
    "project",
    "build",
    "interop",
    "bindings",
    "verification",
    "run",
    "test",
    "work",
];

// ---------------------------------------------------------------------------
// The model
// ---------------------------------------------------------------------------

/// The parsed `cpl.toml`. Parsed in two steps, a TOML table first, then
/// field by field, so that unknown keys and bad values are diagnostics
/// (exit 1) or a [`ManifestError`] (exit 2) rather than deserialization
/// failures; no `deny_unknown_fields` derive is used.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub project: ProjectSection,
    pub build: BuildSection,
    pub interop: InteropSection,
    /// `[bindings.<name>]` tables by name.
    pub bindings: BTreeMap<String, BindingSection>,
    pub verification: VerificationSection,
    pub run: RunSection,
    pub test: TestSection,
    pub work: WorkSection,
    /// Milestone 2 (T80): `[interop.c]`, `None` when the table is absent.
    #[serde(default)]
    pub interop_c: Option<CInteropSection>,
    /// Milestone 2 (T80): the `[bindings.<name>.c]` subtables, by binding
    /// name; a binding without the subtable has no entry.
    #[serde(default)]
    pub binding_c: BTreeMap<String, CBindingSection>,
    /// Milestone 3 (`spec/python.md` Section 1): `[interop.python]`, `None`
    /// when the table is absent.
    #[serde(default)]
    pub interop_python: Option<PythonInteropSection>,
    /// Milestone 3 (`spec/python.md` 6.5): `[bindings.<name>] timeout_ms` by
    /// binding name, as written (a non-positive value is `check_python`'s
    /// `project_binding_timeout_invalid`); a binding without one has no
    /// entry. Kept beside `bindings`, as `binding_c` is, so that
    /// `BindingSection` keeps the fields of Milestone 1.
    #[serde(default)]
    pub binding_timeouts: BTreeMap<String, i64>,
    /// Milestone 3 (semantics S83): `[test.fixtures]`, a capability's
    /// qualified path to the binding that supplies it to tests.
    #[serde(default)]
    pub test_fixtures: BTreeMap<String, String>,
}

impl Manifest {
    /// The effective C prefix (toolchain 11.1, `spec/cabi.md` 5.2):
    /// `[interop.c] prefix`, else `[project] name`. With `"c"` in
    /// `[interop] targets` it is in the prefix domain ([`is_c_prefix`]),
    /// which [`parse_manifest`] enforces.
    pub fn c_prefix(&self) -> &str {
        self.interop_c
            .as_ref()
            .and_then(|c| c.prefix.as_deref())
            .unwrap_or(&self.project.name)
    }
}

/// `[project]` (required).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSection {
    /// Required; `^[a-z][a-z0-9_]*$`.
    pub name: String,
    /// Required; `"0.1"` is the only accepted value (any other is the
    /// diagnostic `unsupported_edition`, and the value is kept here).
    pub edition: String,
    pub description: Option<String>,
}

/// `[build]`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildSection {
    /// Project-relative directories, '/' separators. Default `["src"]`. An
    /// entry is stored without a trailing `/`; `"."` is the project root.
    pub sources: Vec<String>,
}

impl Default for BuildSection {
    fn default() -> Self {
        BuildSection {
            sources: vec!["src".to_string()],
        }
    }
}

/// `[interop]`: inert in Milestone 1.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InteropSection {
    /// Each in {python, node, c}. Default `[]`.
    pub targets: Vec<String>,
}

/// `[interop.python]` (Milestone 3, `spec/python.md` Section 1). `package`
/// and `version` are kept as written: their domains are
/// `crate::check_python`'s (`project_python_option_invalid`); `paths` are
/// normalized project-relative directories (a malformed one is the fatal
/// `project_manifest_invalid`, as a `[build] sources` entry is).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PythonInteropSection {
    /// The import name; default `<project name>_cpl`.
    pub package: Option<String>,
    /// The distribution version; default `"0.1.0"`.
    pub version: Option<String>,
    /// The Python path of the workers the toolchain starts; `None` is the
    /// default `["."]`.
    pub paths: Option<Vec<String>>,
}

impl Manifest {
    /// The Python import name (`spec/python.md` Section 1): `[interop.python]
    /// package`, else `<project name>_cpl`.
    pub fn python_package(&self) -> String {
        self.interop_python
            .as_ref()
            .and_then(|p| p.package.clone())
            .unwrap_or_else(|| format!("{}_cpl", self.project.name))
    }

    /// The distribution version: `[interop.python] version`, else `"0.1.0"`.
    pub fn python_version(&self) -> String {
        self.interop_python
            .as_ref()
            .and_then(|p| p.version.clone())
            .unwrap_or_else(|| "0.1.0".to_string())
    }

    /// The project-relative Python path of the toolchain's workers:
    /// `[interop.python] paths`, else `["."]`.
    pub fn python_paths(&self) -> Vec<String> {
        self.interop_python
            .as_ref()
            .and_then(|p| p.paths.clone())
            .unwrap_or_else(|| vec![".".to_string()])
    }
}

/// One `[bindings.<name>]` table: parsed here, validated by
/// [`check_bindings`] after name resolution, inert in Milestone 1. The
/// values are kept as written so that the checks can report them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindingSection {
    /// Required; must name a `pub capability` (`project_binding_unknown_capability`).
    pub capability: String,
    /// Required; one of `python`, `node`, `c` (`project_binding_bad_backend`),
    /// and in `[interop] targets` (`project_binding_target_not_enabled`).
    pub backend: String,
    /// Required; backend-specific, free text.
    pub adapter: String,
    /// `"worker"` (the default when absent) or `"in_process"`
    /// (`project_binding_bad_isolation`).
    pub isolation: Option<String>,
}

impl BindingSection {
    /// Whether the binding runs in a worker in a host that can run it in
    /// process (`spec/python.md` 5.3, 6.6): `isolation` absent or
    /// `"worker"`.
    pub fn is_worker(&self) -> bool {
        self.isolation.as_deref() != Some("in_process")
    }
}

/// `[verification]`; also the locked verification configuration of `cpl.lock`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationSection {
    /// Default 10000; used from Milestone 5.
    pub proof_timeout_ms: u64,
    /// Stable ids of dependencies whose unverified specifications are
    /// admitted (never a declaration's own `@prove` obligations, T36). Default `[]`.
    pub admit: Vec<String>,
}

impl Default for VerificationSection {
    fn default() -> Self {
        VerificationSection {
            proof_timeout_ms: 10_000,
            admit: Vec::new(),
        }
    }
}

/// `[run]`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSection {
    /// Default 10_000 (semantics S38); CLI `--max-call-depth` overrides.
    pub max_call_depth: u32,
}

impl Default for RunSection {
    fn default() -> Self {
        RunSection {
            max_call_depth: 10_000,
        }
    }
}

/// `[test]`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestSection {
    /// Default 100 (semantics S44); CLI `--cases` overrides. At least 1:
    /// `cases = 0` is `project_manifest_invalid` (T53, an integer out of
    /// range). Reading of toolchain 11.1, which states no minimum: a
    /// property run with zero cases evaluates its body never, yet would be
    /// reported, and receipted, as `tested` (semantics S46), which is the
    /// vacuous evidence the design rules out when it makes an empty test
    /// selection a configuration failure (design line 421).
    pub cases: u32,
    /// Default 1000 (semantics S45); CLI `--max-shrinks` overrides.
    pub max_shrinks: u32,
    /// Default 0 = no wall-clock budget per case; CLI `--timeout-ms` overrides.
    pub timeout_ms: u64,
}

impl Default for TestSection {
    fn default() -> Self {
        TestSection {
            cases: 100,
            max_shrinks: 1000,
            timeout_ms: 0,
        }
    }
}

/// `[work]`: reserved for Milestone 6; parsed and validated, inert before.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkSection {
    /// Default 900.
    pub lease_seconds: u64,
    /// Default 5.
    pub max_integration_retries: u32,
    /// Default `Refs`.
    pub evidence: EvidenceLocation,
    /// Default `"evidence"`; project-relative, used with `evidence = "tree"`;
    /// never part of the source set.
    pub evidence_dir: String,
    pub policy: WorkPolicySection,
}

impl Default for WorkSection {
    fn default() -> Self {
        WorkSection {
            lease_seconds: 900,
            max_integration_retries: 5,
            evidence: EvidenceLocation::Refs,
            evidence_dir: "evidence".to_string(),
            policy: WorkPolicySection::default(),
        }
    }
}

/// `[work] evidence` (T34).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceLocation {
    /// `"refs"`: receipts under `refs/cpl/evidence/...`.
    #[default]
    Refs,
    /// `"tree"`: committed files under `evidence_dir`.
    Tree,
}

/// `[work.policy]`: the project's limits on every grant (Section 10.3, 10.8).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkPolicySection {
    /// Selector spellings, resolved when a change opens; default `["*"]`.
    pub write: Vec<String>,
    /// Namespace spellings; default `["*"]`.
    pub add: Vec<String>,
    /// File selectors (`project.manifest`, `project.lock`, `work.<stem>`, T42); default `[]`.
    pub files: Vec<String>,
    /// The flat keys `filesystem`, `process`, `network`; default the strictest
    /// (`worktree`, `none`, `none`).
    pub boundary: ExecutionBoundary,
}

impl Default for WorkPolicySection {
    fn default() -> Self {
        WorkPolicySection {
            write: vec!["*".to_string()],
            add: vec!["*".to_string()],
            files: Vec::new(),
            boundary: ExecutionBoundary::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Milestone 2: the C target (T74, T80)
// ---------------------------------------------------------------------------

/// `[interop.c]` (toolchain 11.1): every key optional. `Default` gives the
/// defaults of 11.1's table: no prefix (the project name), `abi` 0,
/// `artifacts` `["shared", "static"]`, no compiler, no flags, the default
/// export set, no type names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CInteropSection {
    /// The C prefix of every exported name, in the prefix domain
    /// ([`is_c_prefix`]); `None`: `[project] name`.
    pub prefix: Option<String>,
    /// The package ABI major, 0 to 65535 (soname `lib<prefix>.so.<abi>`).
    pub abi: u16,
    /// What `cpl build` produces; a repeated entry counts once (kept at its
    /// first position); empty: the headers and sources alone.
    pub artifacts: Vec<ArtifactKind>,
    /// A command name or a path (T72); `None`: `CPL_CC`, then `cc`.
    pub compiler: Option<String>,
    /// Appended when compiling adapter and vendored sources, never emitted code.
    pub cflags: Vec<String>,
    /// Appended to every link.
    pub ldflags: Vec<String>,
    /// `None`: the default export set (T74); `Some`: exactly these entries,
    /// in manifest order (the export order).
    pub exports: Option<Vec<ExportEntry>>,
    /// The qualified type text of a type to the C `<name>` of its accessor
    /// (`^[a-z][a-z0-9_]*$`), to resolve a collision of generated names.
    pub type_names: BTreeMap<String, String>,
}

impl Default for CInteropSection {
    fn default() -> Self {
        CInteropSection {
            prefix: None,
            abi: 0,
            artifacts: vec![ArtifactKind::Shared, ArtifactKind::Static],
            compiler: None,
            cflags: Vec::new(),
            ldflags: Vec::new(),
            exports: None,
            type_names: BTreeMap::new(),
        }
    }
}

/// One `[interop.c] exports` entry: one C entry point. A string entry is
/// `{ function, symbol: None, int: Handle }`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportEntry {
    /// The target as written (resolved by toolchain 6.1, T59, in `cpl_sema::interop::plan`).
    pub function: String,
    /// An explicit C symbol; `None`: `<prefix>_<function name>`.
    pub symbol: Option<String>,
    /// The mapping of the signature's top-level `Int`s (`spec/cabi.md` 5.4).
    pub int: IntMapping,
}

/// How an export maps a top-level `Int` (`spec/cabi.md` 5.4): a handle (the
/// default) or a checked machine integer.
#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum IntMapping {
    #[default]
    Handle,
    Int64,
    Int32,
    Uint64,
    Uint32,
}

impl IntMapping {
    /// Every mapping, in declaration order.
    pub const ALL: [IntMapping; 5] = [
        IntMapping::Handle,
        IntMapping::Int64,
        IntMapping::Int32,
        IntMapping::Uint64,
        IntMapping::Uint32,
    ];

    /// `"handle"`, `"int64"`, `"int32"`, `"uint64"`, `"uint32"`.
    pub fn as_str(self) -> &'static str {
        match self {
            IntMapping::Handle => "handle",
            IntMapping::Int64 => "int64",
            IntMapping::Int32 => "int32",
            IntMapping::Uint64 => "uint64",
            IntMapping::Uint32 => "uint32",
        }
    }

    /// The mapping with this spelling.
    pub fn parse(s: &str) -> Option<IntMapping> {
        IntMapping::ALL.into_iter().find(|m| m.as_str() == s)
    }
}

/// One `[interop.c] artifacts` entry (and `cpl build --artifact`).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Shared,
    Static,
    Executable,
}

impl ArtifactKind {
    /// Every artifact kind, in declaration order.
    pub const ALL: [ArtifactKind; 3] = [
        ArtifactKind::Shared,
        ArtifactKind::Static,
        ArtifactKind::Executable,
    ];

    /// `"shared"`, `"static"`, `"executable"`.
    pub fn as_str(self) -> &'static str {
        match self {
            ArtifactKind::Shared => "shared",
            ArtifactKind::Static => "static",
            ArtifactKind::Executable => "executable",
        }
    }

    /// The artifact kind with this spelling.
    pub fn parse(s: &str) -> Option<ArtifactKind> {
        ArtifactKind::ALL.into_iter().find(|a| a.as_str() == s)
    }
}

/// `[bindings.<name>.c]` (toolchain 11.1): how to compile and link a
/// backend-`c` binding's adapter. Paths are project-relative (never
/// absolute, never with a `..` segment), kept as written; `Default`: all empty.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CBindingSection {
    /// Vendored C sources compiled into the package (build inputs, pinned by digest).
    pub sources: Vec<String>,
    /// Project-relative include directories.
    pub include_dirs: Vec<String>,
    /// `NAME` or `NAME=VALUE`, `NAME` a C identifier.
    pub defines: Vec<String>,
    /// Libraries by name (`-l<name>`).
    pub libraries: Vec<String>,
    /// Project-relative library directories.
    pub library_dirs: Vec<String>,
    /// Appended when compiling this binding's adapter and sources.
    pub cflags: Vec<String>,
}

/// The form of a backend-`c` adapter (`spec/cabi.md` Section 6): a `.c`
/// reviewed wrapper or a `.cbind.toml` declarative binding.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterForm {
    Wrapper,
    Declarative,
}

impl AdapterForm {
    /// `"wrapper"` or `"declarative"` (`build.json`, `cpl.lock`).
    pub fn as_str(self) -> &'static str {
        match self {
            AdapterForm::Wrapper => "wrapper",
            AdapterForm::Declarative => "declarative",
        }
    }

    /// The form with this spelling.
    pub fn parse(s: &str) -> Option<AdapterForm> {
        match s {
            "wrapper" => Some(AdapterForm::Wrapper),
            "declarative" => Some(AdapterForm::Declarative),
            _ => None,
        }
    }

    /// The form an adapter path names by its suffix: `.cbind.toml` is
    /// declarative, `.c` a wrapper; `None` for any other path
    /// (`project_binding_adapter_kind`).
    pub fn of_adapter(adapter: &str) -> Option<AdapterForm> {
        if adapter.ends_with(".cbind.toml") {
            Some(AdapterForm::Declarative)
        } else if adapter.ends_with(".c") {
            Some(AdapterForm::Wrapper)
        } else {
            None
        }
    }
}

/// The prefix domain of toolchain 11.1 and `spec/cabi.md` 5.2:
/// `^[a-z][a-z0-9]*(_[a-z0-9]+)*$` (words joined by single `_`, so no `__`
/// and no trailing `_`), at most 31 characters, not starting with `cpl`.
pub fn is_c_prefix(prefix: &str) -> bool {
    prefix.len() <= 31
        && !prefix.starts_with("cpl")
        && !SYSTEM_HEADERS.contains(&prefix)
        && prefix
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_lowercase())
        && prefix.split('_').all(|word| {
            !word.is_empty()
                && word
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// The C11 standard headers and the POSIX headers the runtime includes: `<prefix>.h` in `<out>/include`, which the
/// package's units and its hosts reach with `-I`, would shadow the one of that name (`#include <stdlib.h>` finding
/// the package header of a project named `stdlib`), so no prefix is one of these names (toolchain 11.1).
pub const SYSTEM_HEADERS: [&str; 32] = [
    "assert",
    "complex",
    "ctype",
    "errno",
    "fenv",
    "float",
    "inttypes",
    "iso646",
    "limits",
    "locale",
    "math",
    "setjmp",
    "signal",
    "stdalign",
    "stdarg",
    "stdatomic",
    "stdbool",
    "stddef",
    "stdint",
    "stdio",
    "stdlib",
    "stdnoreturn",
    "string",
    "tgmath",
    "threads",
    "time",
    "uchar",
    "wchar",
    "wctype",
    "pthread",
    "unistd",
    "sched",
];

/// Why `cpl.toml` is unusable: it is not TOML, or a required key is missing,
/// or a key has the wrong type or a value outside its domain
/// (`project_manifest_invalid`, the fatal [`crate::LoadError::Manifest`]).
#[derive(thiserror::Error, Clone, Debug, PartialEq, Eq)]
#[error("{message}")]
pub struct ManifestError {
    pub message: String,
    /// 1-based line in `cpl.toml`, when the error has a location.
    pub line: Option<u32>,
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Parse `cpl.toml`. `Ok(manifest, diagnostics)`: the diagnostics are
/// `project_manifest_unknown_key` (one per unknown table or key, with the
/// dotted `key`) and `unsupported_edition` (with `found`), span-less
/// (semantics S63 (6)) and in document order. `Err` when the text is not
/// TOML, a required key (`[project] name` and `edition`, a binding's
/// `capability`, `backend`, and `adapter`) is missing, or any key of
/// Section 11.1 has the wrong type or a value outside its domain (a
/// `[project] name` not matching `^[a-z][a-z0-9_]*$`, a `[build] sources`
/// entry that is not a project-relative directory, an `[interop] targets`
/// entry other than `python`, `node`, `c`, an `admit` entry that is not a
/// stable id, an integer out of range (`[test] cases` included, whose range
/// starts at 1, see `TestSection::cases`), an unknown `evidence`,
/// `filesystem`, `process`, or `network` value, a `[work.policy] files`
/// entry that is not a file selector).
///
/// The values a `[bindings.*]` table may hold are checked by
/// [`check_bindings`], after name resolution.
pub fn parse_manifest(text: &str) -> Result<(Manifest, Vec<Diagnostic>), ManifestError> {
    let document = DeTable::parse(text).map_err(|e| ManifestError {
        message: format!("cpl.toml is not valid TOML: {}", e.message()),
        line: e.span().map(|span| line_at(text.as_bytes(), span.start)),
    })?;
    let mut parser = Parser {
        text,
        diagnostics: Vec::new(),
        interop_python: None,
        binding_timeouts: BTreeMap::new(),
        test_fixtures: BTreeMap::new(),
    };
    let manifest = parser.manifest(document.get_ref())?;
    let mut diagnostics = parser.diagnostics;
    diagnostics.sort_by_key(|(at, _)| *at);
    Ok((manifest, diagnostics.into_iter().map(|(_, d)| d).collect()))
}

type Item<'i> = Spanned<DeValue<'i>>;
/// A string value with the span of its TOML literal.
type Located = (String, Range<usize>);

struct Parser<'t> {
    text: &'t str,
    /// Non-fatal diagnostics with the byte offset of the key or value they
    /// are about, so that they can be put in document order.
    diagnostics: Vec<(usize, Diagnostic)>,
    /// `[interop.python]`, read with `[interop]` and moved into the manifest.
    interop_python: Option<PythonInteropSection>,
    /// `[bindings.<name>] timeout_ms`, read with the bindings.
    binding_timeouts: BTreeMap<String, i64>,
    /// `[test.fixtures]`, read with `[test]`.
    test_fixtures: BTreeMap<String, String>,
}

impl Parser<'_> {
    fn invalid(&self, message: String, at: Option<&Range<usize>>) -> ManifestError {
        ManifestError {
            message,
            line: at.map(|r| line_at(self.text.as_bytes(), r.start)),
        }
    }

    fn ill_typed(&self, path: &str, expected: &str, found: &Item<'_>) -> ManifestError {
        self.invalid(
            format!(
                "`{path}` must be {expected}, found {}",
                describe(found.get_ref())
            ),
            Some(&found.span()),
        )
    }

    /// One `project_manifest_unknown_key` per key of `table` (the table at
    /// the dotted `path`, `""` for the root) that is not in `known`. The
    /// keys of an unknown table are not reported separately.
    fn unknown_keys(&mut self, table: &DeTable<'_>, path: &str, known: &[&str]) {
        for (name, value) in table.iter() {
            let text: &str = name.get_ref();
            if known.contains(&text) {
                continue;
            }
            let full = join_key(path, text);
            let message = match value.get_ref() {
                DeValue::Table(_) => format!("unknown table `[{full}]` in cpl.toml"),
                _ if path.is_empty() => format!("unknown key `{full}` in cpl.toml"),
                _ => format!("unknown key `{}` in `[{path}]` of cpl.toml", key(text)),
            };
            let diagnostic = Diagnostic::error(
                codes::PROJECT_MANIFEST_UNKNOWN_KEY,
                codes::CATEGORY,
                message,
            )
            .with_data("key", json!(full));
            self.diagnostics.push((name.span().start, diagnostic));
        }
    }

    /// `parent.name` as a table, with the span of its key; `None` when absent.
    fn table<'a, 'i>(
        &self,
        parent: &'a DeTable<'i>,
        name: &str,
        path: &str,
    ) -> Result<Option<(&'a DeTable<'i>, Range<usize>)>, ManifestError> {
        match parent.get_key_value(name) {
            None => Ok(None),
            Some((k, item)) => match item.get_ref() {
                DeValue::Table(table) => Ok(Some((table, k.span()))),
                _ => Err(self.ill_typed(path, "a table", item)),
            },
        }
    }

    fn string(
        &self,
        table: &DeTable<'_>,
        name: &str,
        path: &str,
    ) -> Result<Option<Located>, ManifestError> {
        match table.get(name) {
            None => Ok(None),
            Some(item) => match item.get_ref() {
                DeValue::String(s) => Ok(Some((s.to_string(), item.span()))),
                _ => Err(self.ill_typed(path, "a string", item)),
            },
        }
    }

    /// A required string key of the table at `table_path`, whose key spans `table_at`.
    fn required_string(
        &self,
        table: &DeTable<'_>,
        name: &str,
        table_path: &str,
        table_at: &Range<usize>,
    ) -> Result<Located, ManifestError> {
        let path = join_key(table_path, name);
        self.string(table, name, &path)?.ok_or_else(|| {
            self.invalid(
                format!("missing required key `{path}` in cpl.toml"),
                Some(table_at),
            )
        })
    }

    fn unsigned(
        &self,
        table: &DeTable<'_>,
        name: &str,
        path: &str,
        min: u64,
        max: u64,
    ) -> Result<Option<u64>, ManifestError> {
        let Some(item) = table.get(name) else {
            return Ok(None);
        };
        let value = match item.get_ref() {
            DeValue::Integer(i) => u64::from_str_radix(i.as_str(), i.radix())
                .ok()
                .filter(|v| (min..=max).contains(v)),
            _ => None,
        };
        match value {
            Some(v) => Ok(Some(v)),
            None => Err(self.invalid(
                format!(
                    "`{path}` must be an integer from {min} to {max}, found {}",
                    describe(item.get_ref())
                ),
                Some(&item.span()),
            )),
        }
    }

    fn u32(
        &self,
        table: &DeTable<'_>,
        name: &str,
        path: &str,
    ) -> Result<Option<u32>, ManifestError> {
        self.u32_from(table, name, path, 0)
    }

    /// A `u32` key whose value must be at least `min`.
    fn u32_from(
        &self,
        table: &DeTable<'_>,
        name: &str,
        path: &str,
        min: u64,
    ) -> Result<Option<u32>, ManifestError> {
        Ok(self
            .unsigned(table, name, path, min, u64::from(u32::MAX))?
            .map(|v| u32::try_from(v).expect("checked against u32::MAX")))
    }

    fn u64(
        &self,
        table: &DeTable<'_>,
        name: &str,
        path: &str,
    ) -> Result<Option<u64>, ManifestError> {
        self.unsigned(table, name, path, 0, u64::MAX)
    }

    /// An array of strings, each with its span.
    fn strings(
        &self,
        table: &DeTable<'_>,
        name: &str,
        path: &str,
    ) -> Result<Option<Vec<Located>>, ManifestError> {
        let Some(item) = table.get(name) else {
            return Ok(None);
        };
        let DeValue::Array(elements) = item.get_ref() else {
            return Err(self.ill_typed(path, "an array of strings", item));
        };
        let mut out = Vec::with_capacity(elements.len());
        for (i, element) in elements.iter().enumerate() {
            match element.get_ref() {
                DeValue::String(s) => out.push((s.to_string(), element.span())),
                _ => return Err(self.ill_typed(&format!("{path}[{i}]"), "a string", element)),
            }
        }
        Ok(Some(out))
    }

    /// A string key with a closed set of spellings.
    fn choice<T: Copy>(
        &self,
        table: &DeTable<'_>,
        name: &str,
        path: &str,
        options: &[(&str, T)],
    ) -> Result<Option<T>, ManifestError> {
        let Some((value, at)) = self.string(table, name, path)? else {
            return Ok(None);
        };
        match options.iter().find(|(spelling, _)| *spelling == value) {
            Some((_, v)) => Ok(Some(*v)),
            None => Err(self.invalid(
                format!(
                    "`{path}` must be {}, found {value:?}",
                    one_of(options.iter().map(|(spelling, _)| *spelling))
                ),
                Some(&at),
            )),
        }
    }

    fn manifest(&mut self, root: &DeTable<'_>) -> Result<Manifest, ManifestError> {
        self.unknown_keys(root, "", &TOP_LEVEL);
        let (project, name_at) = self.project(root)?;
        let build = self.build(root)?;
        let (interop, interop_c) = self.interop(root)?;
        // Toolchain 11.1: with the C target enabled, the effective prefix
        // must be in the prefix domain; an explicit `prefix` was checked
        // where it was read, so only the project name is left.
        let explicit_prefix = interop_c.as_ref().is_some_and(|c| c.prefix.is_some());
        if interop.targets.iter().any(|t| t == "c")
            && !explicit_prefix
            && !is_c_prefix(&project.name)
        {
            return Err(self.invalid(
                format!(
                    "with \"c\" in `interop.targets`, the C prefix is `project.name` {:?}, which is not a C prefix ({PREFIX_DOMAIN}); set `[interop.c] prefix`",
                    project.name
                ),
                Some(&name_at),
            ));
        }
        let (bindings, binding_c) = self.bindings(root)?;
        let verification = self.verification(root)?;
        let run = self.run(root)?;
        let test = self.test(root)?;
        let work = self.work(root)?;
        Ok(Manifest {
            project,
            build,
            interop,
            bindings,
            verification,
            run,
            test,
            work,
            interop_c,
            binding_c,
            interop_python: self.interop_python.take(),
            binding_timeouts: std::mem::take(&mut self.binding_timeouts),
            test_fixtures: std::mem::take(&mut self.test_fixtures),
        })
    }

    fn project(
        &mut self,
        root: &DeTable<'_>,
    ) -> Result<(ProjectSection, Range<usize>), ManifestError> {
        let Some((table, at)) = self.table(root, "project", "project")? else {
            return Err(self.invalid(
                "missing required table `[project]` in cpl.toml".to_string(),
                None,
            ));
        };
        self.unknown_keys(table, "project", &["name", "edition", "description"]);
        let (name, name_at) = self.required_string(table, "name", "project", &at)?;
        if !is_project_name(&name) {
            return Err(self.invalid(
                format!(
                    "`project.name` must match ^[a-z][a-z0-9_]*$ (a lowercase letter, then lowercase letters, digits, and `_`), found {name:?}"
                ),
                Some(&name_at),
            ));
        }
        let (edition, edition_at) = self.required_string(table, "edition", "project", &at)?;
        if edition != version::EDITION {
            let diagnostic = Diagnostic::error(
                codes::UNSUPPORTED_EDITION,
                Category::Resolve,
                format!(
                    "edition {edition:?} is not supported: this toolchain implements edition {}",
                    version::EDITION
                ),
            )
            .with_data("found", json!(edition));
            self.diagnostics.push((edition_at.start, diagnostic));
        }
        let description = self
            .string(table, "description", "project.description")?
            .map(|(s, _)| s);
        Ok((
            ProjectSection {
                name,
                edition,
                description,
            },
            name_at,
        ))
    }

    fn build(&mut self, root: &DeTable<'_>) -> Result<BuildSection, ManifestError> {
        let Some((table, _)) = self.table(root, "build", "build")? else {
            return Ok(BuildSection::default());
        };
        self.unknown_keys(table, "build", &["sources"]);
        let Some(entries) = self.strings(table, "sources", "build.sources")? else {
            return Ok(BuildSection::default());
        };
        let mut sources = Vec::with_capacity(entries.len());
        for (entry, at) in entries {
            match normalize_dir(&entry) {
                Some(dir) => sources.push(dir),
                None => {
                    return Err(self.invalid(
                        format!(
                            "`build.sources` entries are project-relative directories with `/` separators, such as \"src\", found {entry:?}"
                        ),
                        Some(&at),
                    ))
                }
            }
        }
        Ok(BuildSection { sources })
    }

    fn interop(
        &mut self,
        root: &DeTable<'_>,
    ) -> Result<(InteropSection, Option<CInteropSection>), ManifestError> {
        let Some((table, _)) = self.table(root, "interop", "interop")? else {
            return Ok((InteropSection::default(), None));
        };
        self.unknown_keys(table, "interop", &["targets", "c", "python"]);
        let mut targets = Vec::new();
        for (target, at) in self
            .strings(table, "targets", "interop.targets")?
            .unwrap_or_default()
        {
            if !TARGETS.contains(&target.as_str()) {
                return Err(self.invalid(
                    format!(
                        "`interop.targets` entries must be {}, found {target:?}",
                        one_of(TARGETS)
                    ),
                    Some(&at),
                ));
            }
            targets.push(target);
        }
        let interop_c = match self.table(table, "c", "interop.c")? {
            None => None,
            Some((c, _)) => Some(self.interop_c(c)?),
        };
        self.interop_python = match self.table(table, "python", "interop.python")? {
            None => None,
            Some((python, _)) => Some(self.interop_python(python)?),
        };
        Ok((InteropSection { targets }, interop_c))
    }

    /// `[interop.python]` (Milestone 3): `package` and `version` as written
    /// (their domains are `check_python`'s), `paths` normalized here.
    fn interop_python(
        &mut self,
        table: &DeTable<'_>,
    ) -> Result<PythonInteropSection, ManifestError> {
        const PATH: &str = "interop.python";
        self.unknown_keys(table, PATH, &["package", "version", "paths"]);
        let package = self
            .string(table, "package", "interop.python.package")?
            .map(|(s, _)| s);
        let version = self
            .string(table, "version", "interop.python.version")?
            .map(|(s, _)| s);
        let paths = match self.strings(table, "paths", "interop.python.paths")? {
            None => None,
            Some(entries) => {
                let mut paths = Vec::with_capacity(entries.len());
                for (i, (entry, at)) in entries.into_iter().enumerate() {
                    let Some(dir) = normalize_dir(&entry) else {
                        return Err(self.invalid(
                            format!(
                                "`interop.python.paths[{i}]` must be a project-relative directory with `/` separators, found {entry:?}"
                            ),
                            Some(&at),
                        ));
                    };
                    if !paths.contains(&dir) {
                        paths.push(dir);
                    }
                }
                Some(paths)
            }
        };
        Ok(PythonInteropSection {
            package,
            version,
            paths,
        })
    }

    /// `[interop.c]` (Milestone 2, toolchain 11.1): every value is checked
    /// against its domain here (T53).
    fn interop_c(&mut self, table: &DeTable<'_>) -> Result<CInteropSection, ManifestError> {
        const PATH: &str = "interop.c";
        self.unknown_keys(
            table,
            PATH,
            &[
                "prefix",
                "abi",
                "artifacts",
                "compiler",
                "cflags",
                "ldflags",
                "exports",
                "type_names",
            ],
        );
        let defaults = CInteropSection::default();
        let prefix = match self.string(table, "prefix", "interop.c.prefix")? {
            None => None,
            Some((prefix, _)) if is_c_prefix(&prefix) => Some(prefix),
            Some((prefix, at)) => {
                return Err(self.invalid(
                    format!(
                        "`interop.c.prefix` must be a C prefix ({PREFIX_DOMAIN}), found {prefix:?}"
                    ),
                    Some(&at),
                ))
            }
        };
        let abi = self
            .unsigned(table, "abi", "interop.c.abi", 0, u64::from(u16::MAX))?
            .map_or(defaults.abi, |v| {
                u16::try_from(v).expect("checked against u16::MAX")
            });
        let artifacts = match self.strings(table, "artifacts", "interop.c.artifacts")? {
            None => defaults.artifacts,
            Some(entries) => {
                let mut artifacts = Vec::new();
                for (i, (entry, at)) in entries.into_iter().enumerate() {
                    let Some(kind) = ArtifactKind::parse(&entry) else {
                        return Err(self.invalid(
                            format!(
                                "`interop.c.artifacts[{i}]` must be {}, found {entry:?}",
                                one_of(ArtifactKind::ALL.iter().map(|a| a.as_str()))
                            ),
                            Some(&at),
                        ));
                    };
                    if !artifacts.contains(&kind) {
                        artifacts.push(kind);
                    }
                }
                artifacts
            }
        };
        let compiler = match self.string(table, "compiler", "interop.c.compiler")? {
            None => None,
            Some((compiler, at)) if compiler.is_empty() => {
                return Err(self.invalid(
                    "`interop.c.compiler` must be a command name or a path, found \"\"".to_string(),
                    Some(&at),
                ))
            }
            Some((compiler, _)) => Some(compiler),
        };
        let plain = |entries: Option<Vec<Located>>| -> Vec<String> {
            entries
                .unwrap_or_default()
                .into_iter()
                .map(|(s, _)| s)
                .collect()
        };
        let cflags = plain(self.strings(table, "cflags", "interop.c.cflags")?);
        let ldflags = plain(self.strings(table, "ldflags", "interop.c.ldflags")?);
        let exports = self.exports(table)?;
        let type_names = self.type_names(table)?;
        Ok(CInteropSection {
            prefix,
            abi,
            artifacts,
            compiler,
            cflags,
            ldflags,
            exports,
            type_names,
        })
    }

    /// `[interop.c] exports`: each entry a target string or a table of
    /// `function` (required), `symbol`, and `int` (T53: any other shape,
    /// key, or `int` value is `project_manifest_invalid`).
    fn exports(&self, table: &DeTable<'_>) -> Result<Option<Vec<ExportEntry>>, ManifestError> {
        const SHAPE: &str = "a target string or a table of `function`, `symbol`, and `int`";
        let Some(item) = table.get("exports") else {
            return Ok(None);
        };
        let DeValue::Array(elements) = item.get_ref() else {
            return Err(self.ill_typed("interop.c.exports", "an array", item));
        };
        let mut entries = Vec::with_capacity(elements.len());
        for (i, element) in elements.iter().enumerate() {
            let path = format!("interop.c.exports[{i}]");
            match element.get_ref() {
                DeValue::String(s) => entries.push(ExportEntry {
                    function: s.to_string(),
                    symbol: None,
                    int: IntMapping::Handle,
                }),
                DeValue::Table(entry) => {
                    if let Some((k, _)) = entry.iter().find(|(k, _)| {
                        !["function", "symbol", "int"].contains(&k.get_ref().as_ref())
                    }) {
                        return Err(self.invalid(
                            format!(
                                "`{path}` must be {SHAPE}, found the key {:?}",
                                k.get_ref().as_ref()
                            ),
                            Some(&k.span()),
                        ));
                    }
                    let function =
                        match self.string(entry, "function", &format!("{path}.function"))? {
                            Some((function, _)) => function,
                            None => {
                                return Err(self.invalid(
                                    format!("missing required key `{path}.function` in cpl.toml"),
                                    Some(&element.span()),
                                ))
                            }
                        };
                    let symbol = self
                        .string(entry, "symbol", &format!("{path}.symbol"))?
                        .map(|(s, _)| s);
                    let options: Vec<(&str, IntMapping)> =
                        IntMapping::ALL.iter().map(|m| (m.as_str(), *m)).collect();
                    let int = self
                        .choice(entry, "int", &format!("{path}.int"), &options)?
                        .unwrap_or_default();
                    entries.push(ExportEntry {
                        function,
                        symbol,
                        int,
                    });
                }
                _ => return Err(self.ill_typed(&path, SHAPE, element)),
            }
        }
        Ok(Some(entries))
    }

    /// `[interop.c] type_names`: a table from a qualified type text to a C
    /// name `^[a-z][a-z0-9_]*$`.
    fn type_names(&self, table: &DeTable<'_>) -> Result<BTreeMap<String, String>, ManifestError> {
        let Some((names, _)) = self.table(table, "type_names", "interop.c.type_names")? else {
            return Ok(BTreeMap::new());
        };
        let mut out = BTreeMap::new();
        for (key_item, value) in names.iter() {
            let type_text: &str = key_item.get_ref();
            let path = join_key("interop.c.type_names", type_text);
            let DeValue::String(name) = value.get_ref() else {
                return Err(self.ill_typed(&path, "a string", value));
            };
            if !is_project_name(name) {
                return Err(self.invalid(
                    format!(
                        "`{path}` must be a C name matching ^[a-z][a-z0-9_]*$, found {:?}",
                        name.as_ref()
                    ),
                    Some(&value.span()),
                ));
            }
            out.insert(type_text.to_string(), name.to_string());
        }
        Ok(out)
    }

    /// `[bindings.<name>.c]` (Milestone 2, toolchain 11.1): paths are
    /// project-relative, `defines` are `NAME` or `NAME=VALUE` (T53).
    fn binding_c(
        &mut self,
        table: &DeTable<'_>,
        path: &str,
    ) -> Result<CBindingSection, ManifestError> {
        self.unknown_keys(
            table,
            path,
            &[
                "sources",
                "include_dirs",
                "defines",
                "libraries",
                "library_dirs",
                "cflags",
            ],
        );
        let paths = |parser: &Self, name: &str| -> Result<Vec<String>, ManifestError> {
            let key = join_key(path, name);
            let mut out = Vec::new();
            for (i, (entry, at)) in parser
                .strings(table, name, &key)?
                .unwrap_or_default()
                .into_iter()
                .enumerate()
            {
                if !is_relative_path(&entry) {
                    return Err(parser.invalid(
                        format!(
                            "`{key}[{i}]` must be a project-relative path (not absolute, without a `..` segment), found {entry:?}"
                        ),
                        Some(&at),
                    ));
                }
                out.push(entry);
            }
            Ok(out)
        };
        let sources = paths(self, "sources")?;
        let include_dirs = paths(self, "include_dirs")?;
        let defines_key = join_key(path, "defines");
        let mut defines = Vec::new();
        for (i, (define, at)) in self
            .strings(table, "defines", &defines_key)?
            .unwrap_or_default()
            .into_iter()
            .enumerate()
        {
            if !is_define(&define) {
                return Err(self.invalid(
                    format!(
                        "`{defines_key}[{i}]` must be `NAME` or `NAME=VALUE` with `NAME` a C identifier, found {define:?}"
                    ),
                    Some(&at),
                ));
            }
            defines.push(define);
        }
        let libraries_key = join_key(path, "libraries");
        let mut libraries = Vec::new();
        for (i, (library, at)) in self
            .strings(table, "libraries", &libraries_key)?
            .unwrap_or_default()
            .into_iter()
            .enumerate()
        {
            if library.is_empty() {
                return Err(self.invalid(
                    format!(
                        "`{libraries_key}[{i}]` must be a library name (`-l<name>`), found \"\""
                    ),
                    Some(&at),
                ));
            }
            libraries.push(library);
        }
        let library_dirs = paths(self, "library_dirs")?;
        let cflags = self
            .strings(table, "cflags", &join_key(path, "cflags"))?
            .unwrap_or_default()
            .into_iter()
            .map(|(s, _)| s)
            .collect();
        Ok(CBindingSection {
            sources,
            include_dirs,
            defines,
            libraries,
            library_dirs,
            cflags,
        })
    }

    fn bindings(&mut self, root: &DeTable<'_>) -> Result<(Bindings, CBindings), ManifestError> {
        let Some((table, _)) = self.table(root, "bindings", "bindings")? else {
            return Ok((BTreeMap::new(), BTreeMap::new()));
        };
        let mut bindings = BTreeMap::new();
        let mut binding_c = BTreeMap::new();
        for (name, item) in table.iter() {
            let path = join_key("bindings", name.get_ref());
            let DeValue::Table(binding) = item.get_ref() else {
                return Err(self.ill_typed(&path, "a table", item));
            };
            let at = name.span();
            self.unknown_keys(
                binding,
                &path,
                &[
                    "capability",
                    "backend",
                    "adapter",
                    "isolation",
                    "c",
                    "timeout_ms",
                ],
            );
            let (capability, _) = self.required_string(binding, "capability", &path, &at)?;
            let (backend, _) = self.required_string(binding, "backend", &path, &at)?;
            let (adapter, _) = self.required_string(binding, "adapter", &path, &at)?;
            let isolation = self
                .string(binding, "isolation", &join_key(&path, "isolation"))?
                .map(|(s, _)| s);
            let timeout_ms = match binding.get("timeout_ms") {
                None => None,
                Some(item) => match item.get_ref() {
                    DeValue::Integer(i) => {
                        Some(i64::from_str_radix(i.as_str(), i.radix()).map_err(|_| {
                            self.invalid(
                                format!(
                                    "`{}` must be an integer that fits in 64 bits, found {}",
                                    join_key(&path, "timeout_ms"),
                                    describe(item.get_ref())
                                ),
                                Some(&item.span()),
                            )
                        })?)
                    }
                    _ => {
                        return Err(self.ill_typed(
                            &join_key(&path, "timeout_ms"),
                            "an integer",
                            item,
                        ))
                    }
                },
            };
            let c_path = join_key(&path, "c");
            if let Some((c, _)) = self.table(binding, "c", &c_path)? {
                let section = self.binding_c(c, &c_path)?;
                binding_c.insert(name.get_ref().to_string(), section);
            }
            if let Some(timeout) = timeout_ms {
                self.binding_timeouts
                    .insert(name.get_ref().to_string(), timeout);
            }
            bindings.insert(
                name.get_ref().to_string(),
                BindingSection {
                    capability,
                    backend,
                    adapter,
                    isolation,
                },
            );
        }
        Ok((bindings, binding_c))
    }

    fn verification(&mut self, root: &DeTable<'_>) -> Result<VerificationSection, ManifestError> {
        let defaults = VerificationSection::default();
        let Some((table, _)) = self.table(root, "verification", "verification")? else {
            return Ok(defaults);
        };
        self.unknown_keys(table, "verification", &["proof_timeout_ms", "admit"]);
        let proof_timeout_ms = self
            .u64(table, "proof_timeout_ms", "verification.proof_timeout_ms")?
            .unwrap_or(defaults.proof_timeout_ms);
        let mut admit = Vec::new();
        let entries = self
            .strings(table, "admit", "verification.admit")?
            .unwrap_or_default();
        for (i, (id, at)) in entries.into_iter().enumerate() {
            if let Err(e) = StableId::parse(&id) {
                return Err(self.invalid(
                    format!(
                        "`verification.admit[{i}]` must be a stable id (an @id value or a qualified name), found {id:?}: {e}"
                    ),
                    Some(&at),
                ));
            }
            admit.push(id);
        }
        Ok(VerificationSection {
            proof_timeout_ms,
            admit,
        })
    }

    fn run(&mut self, root: &DeTable<'_>) -> Result<RunSection, ManifestError> {
        let defaults = RunSection::default();
        let Some((table, _)) = self.table(root, "run", "run")? else {
            return Ok(defaults);
        };
        self.unknown_keys(table, "run", &["max_call_depth"]);
        Ok(RunSection {
            max_call_depth: self
                .u32(table, "max_call_depth", "run.max_call_depth")?
                .unwrap_or(defaults.max_call_depth),
        })
    }

    fn test(&mut self, root: &DeTable<'_>) -> Result<TestSection, ManifestError> {
        let defaults = TestSection::default();
        let Some((table, _)) = self.table(root, "test", "test")? else {
            return Ok(defaults);
        };
        self.unknown_keys(
            table,
            "test",
            &["cases", "max_shrinks", "timeout_ms", "fixtures"],
        );
        let mut fixtures = BTreeMap::new();
        let fixture_table = self.table(table, "fixtures", "test.fixtures")?;
        if let Some((entries, _)) = fixture_table {
            for (key_item, value) in entries.iter() {
                let capability: &str = key_item.get_ref();
                let path = join_key("test.fixtures", capability);
                let DeValue::String(binding) = value.get_ref() else {
                    return Err(self.ill_typed(&path, "a string (a binding name)", value));
                };
                fixtures.insert(capability.to_string(), binding.to_string());
            }
        }
        self.test_fixtures = fixtures;
        Ok(TestSection {
            cases: self
                .u32_from(table, "cases", "test.cases", 1)?
                .unwrap_or(defaults.cases),
            max_shrinks: self
                .u32(table, "max_shrinks", "test.max_shrinks")?
                .unwrap_or(defaults.max_shrinks),
            timeout_ms: self
                .u64(table, "timeout_ms", "test.timeout_ms")?
                .unwrap_or(defaults.timeout_ms),
        })
    }

    fn work(&mut self, root: &DeTable<'_>) -> Result<WorkSection, ManifestError> {
        let defaults = WorkSection::default();
        let Some((table, _)) = self.table(root, "work", "work")? else {
            return Ok(defaults);
        };
        self.unknown_keys(
            table,
            "work",
            &[
                "lease_seconds",
                "max_integration_retries",
                "evidence",
                "evidence_dir",
                "policy",
            ],
        );
        let lease_seconds = self
            .u64(table, "lease_seconds", "work.lease_seconds")?
            .unwrap_or(defaults.lease_seconds);
        let max_integration_retries = self
            .u32(
                table,
                "max_integration_retries",
                "work.max_integration_retries",
            )?
            .unwrap_or(defaults.max_integration_retries);
        let evidence = self
            .choice(
                table,
                "evidence",
                "work.evidence",
                &[
                    ("refs", EvidenceLocation::Refs),
                    ("tree", EvidenceLocation::Tree),
                ],
            )?
            .unwrap_or(defaults.evidence);
        let evidence_dir = match self.string(table, "evidence_dir", "work.evidence_dir")? {
            None => defaults.evidence_dir,
            Some((dir, at)) => normalize_dir(&dir).ok_or_else(|| {
                self.invalid(
                    format!(
                        "`work.evidence_dir` must be a project-relative directory with `/` separators, found {dir:?}"
                    ),
                    Some(&at),
                )
            })?,
        };
        let policy = self.policy(table)?;
        Ok(WorkSection {
            lease_seconds,
            max_integration_retries,
            evidence,
            evidence_dir,
            policy,
        })
    }

    fn policy(&mut self, work: &DeTable<'_>) -> Result<WorkPolicySection, ManifestError> {
        let defaults = WorkPolicySection::default();
        let Some((table, _)) = self.table(work, "policy", "work.policy")? else {
            return Ok(defaults);
        };
        self.unknown_keys(
            table,
            "work.policy",
            &["write", "add", "files", "filesystem", "process", "network"],
        );
        let plain = |entries: Vec<Located>| -> Vec<String> {
            entries.into_iter().map(|(s, _)| s).collect()
        };
        let write = self
            .strings(table, "write", "work.policy.write")?
            .map(plain)
            .unwrap_or(defaults.write);
        let add = self
            .strings(table, "add", "work.policy.add")?
            .map(plain)
            .unwrap_or(defaults.add);
        let files = match self.strings(table, "files", "work.policy.files")? {
            None => defaults.files,
            Some(entries) => {
                let mut files = Vec::with_capacity(entries.len());
                for (selector, at) in entries {
                    if !is_file_selector(&selector) {
                        return Err(self.invalid(
                            format!(
                                "`work.policy.files` entries are file selectors (\"project.manifest\", \"project.lock\", or \"work.<stem>\"), found {selector:?}"
                            ),
                            Some(&at),
                        ));
                    }
                    files.push(selector);
                }
                files
            }
        };
        let filesystem = self
            .choice(
                table,
                "filesystem",
                "work.policy.filesystem",
                &[
                    ("worktree", FsAccess::Worktree),
                    ("worktree_and_repository", FsAccess::WorktreeAndRepository),
                    ("host", FsAccess::Host),
                ],
            )?
            .unwrap_or(defaults.boundary.filesystem);
        let process = self
            .choice(
                table,
                "process",
                "work.policy.process",
                &[
                    ("none", ProcessAccess::None),
                    ("toolchain", ProcessAccess::Toolchain),
                    ("any", ProcessAccess::Any),
                ],
            )?
            .unwrap_or(defaults.boundary.process);
        let network = self
            .choice(
                table,
                "network",
                "work.policy.network",
                &[
                    ("none", NetworkAccess::None),
                    ("adapters", NetworkAccess::Adapters),
                    ("any", NetworkAccess::Any),
                ],
            )?
            .unwrap_or(defaults.boundary.network);
        Ok(WorkPolicySection {
            write,
            add,
            files,
            boundary: ExecutionBoundary {
                filesystem,
                process,
                network,
            },
        })
    }
}

/// How a TOML value is named in a message: the scalar itself, or its kind.
fn describe(value: &DeValue<'_>) -> String {
    match value {
        DeValue::String(s) => format!("the string {s:?}"),
        DeValue::Integer(i) => format!("the integer {i}"),
        DeValue::Float(f) => format!("the float {f}"),
        DeValue::Boolean(b) => format!("the boolean {b}"),
        DeValue::Datetime(d) => format!("the datetime {d}"),
        DeValue::Array(_) => "an array".to_string(),
        DeValue::Table(_) => "a table".to_string(),
    }
}

/// `"a"`, `"a" or "b"`, `"a", "b", or "c"`.
fn one_of<'a>(spellings: impl IntoIterator<Item = &'a str>) -> String {
    let quoted: Vec<String> = spellings.into_iter().map(|s| format!("{s:?}")).collect();
    match quoted.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a} or {b}"),
        [init @ .., last] => format!("{}, or {last}", init.join(", ")),
    }
}

/// `^[a-z][a-z0-9_]*$`.
fn is_project_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_lowercase())
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// The `[bindings.<name>]` tables and their `[bindings.<name>.c]` subtables, by name.
type Bindings = BTreeMap<String, BindingSection>;
type CBindings = BTreeMap<String, CBindingSection>;

/// The prefix domain as messages state it.
const PREFIX_DOMAIN: &str =
    "^[a-z][a-z0-9]*(_[a-z0-9]+)*$, at most 31 characters, not starting with `cpl`";

/// A project-relative path of `[bindings.<name>.c]`: not empty, not
/// absolute, and without a `..` segment (toolchain 11.1).
pub(crate) fn is_relative_path(path: &str) -> bool {
    !path.is_empty() && !path.starts_with('/') && !path.split('/').any(|segment| segment == "..")
}

/// `NAME` or `NAME=VALUE` with `NAME` a C identifier (`[A-Za-z_][A-Za-z0-9_]*`).
fn is_define(define: &str) -> bool {
    let name = define.split_once('=').map_or(define, |(name, _)| name);
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// `project.manifest`, `project.lock`, or `work.<stem>` (T42).
fn is_file_selector(selector: &str) -> bool {
    selector == "project.manifest"
        || selector == "project.lock"
        || selector
            .strip_prefix("work.")
            .is_some_and(|stem| !stem.is_empty() && !stem.contains('/'))
}

/// A project-relative directory with `/` separators, without a trailing
/// `/`; `"."` is the project root. `None` for an empty or absolute path, a
/// path with `\`, and one with an empty, `.`, or `..` segment.
pub(crate) fn normalize_dir(entry: &str) -> Option<String> {
    let trimmed = entry.strip_suffix('/').unwrap_or(entry);
    if trimmed == "." {
        return Some(".".to_string());
    }
    if trimmed.is_empty() || trimmed.starts_with('/') || trimmed.contains('\\') {
        return None;
    }
    if trimmed
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return None;
    }
    Some(trimmed.to_string())
}

// ---------------------------------------------------------------------------
// Binding checks
// ---------------------------------------------------------------------------

/// The `[bindings.*]` checks of Section 11.1, which run after name
/// resolution: `is_pub_capability` answers whether a dotted path such as
/// `"reader.images.ImageCodec"` names a `pub capability` of the program.
///
/// Per binding, in key order (T48): `project_binding_unknown_capability`
/// when `capability` does not name one; `project_binding_bad_backend` when
/// `backend` is not `python`, `node`, or `c`, and otherwise
/// `project_binding_target_not_enabled` when it is not in `[interop]
/// targets` (never both); `project_binding_bad_isolation` when `isolation`
/// is present and neither `worker` nor `in_process`. So one binding carries
/// at most three diagnostics, one per offending key. Every diagnostic is an
/// error of category `project` without a span (semantics S63 (6)), with
/// the fields `binding` (the table's name), `key` (the dotted key), and
/// `found` (the value), and `targets` on `project_binding_target_not_enabled`.
/// Bindings are reported in name order.
pub fn check_bindings(
    manifest: &Manifest,
    is_pub_capability: impl Fn(&str) -> bool,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for (name, binding) in &manifest.bindings {
        let table = join_key("bindings", name);
        let diagnostic = |code, field: &str, value: &str, message: String| {
            Diagnostic::error(code, codes::CATEGORY, message)
                .with_data("binding", json!(name))
                .with_data("key", json!(join_key(&table, field)))
                .with_data("found", json!(value))
        };
        if !is_pub_capability(&binding.capability) {
            out.push(diagnostic(
                codes::PROJECT_BINDING_UNKNOWN_CAPABILITY,
                "capability",
                &binding.capability,
                format!(
                    "binding `{name}`: `{}` does not name a `pub capability`",
                    binding.capability
                ),
            ));
        }
        if !BACKENDS.contains(&binding.backend.as_str()) {
            out.push(diagnostic(
                codes::PROJECT_BINDING_BAD_BACKEND,
                "backend",
                &binding.backend,
                format!(
                    "binding `{name}`: backend {:?} is not one of {}",
                    binding.backend,
                    one_of(BACKENDS)
                ),
            ));
        } else if !manifest.interop.targets.contains(&binding.backend) {
            out.push(
                diagnostic(
                    codes::PROJECT_BINDING_TARGET_NOT_ENABLED,
                    "backend",
                    &binding.backend,
                    format!(
                        "binding `{name}`: backend {:?} is not in `[interop] targets`",
                        binding.backend
                    ),
                )
                .with_data("targets", json!(manifest.interop.targets)),
            );
        }
        if let Some(isolation) = &binding.isolation {
            if !ISOLATIONS.contains(&isolation.as_str()) {
                out.push(diagnostic(
                    codes::PROJECT_BINDING_BAD_ISOLATION,
                    "isolation",
                    isolation,
                    format!(
                        "binding `{name}`: isolation {isolation:?} is not {}",
                        one_of(ISOLATIONS)
                    ),
                ));
            }
        }
    }
    out
}
