//! The Python target's manifest rules (Milestone 3, `spec/python.md`
//! Sections 1, 5, 6, 8; toolchain 11.1): [`check_python`], and the name
//! tables and patterns the Python package generator shares
//! ([`PYTHON_KEYWORDS`], [`PYTHON_STDLIB_MODULES`],
//! [`is_python_identifier`], [`is_python_module_path`]).

use crate::codes;
use crate::manifest::Manifest;
use cpl_diag::Diagnostic;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::Path;

/// The keywords and soft keywords of Python 3.10 (`keyword.kwlist`,
/// `keyword.softkwlist`): a generated name equal to one gets a trailing `_`
/// (`spec/python.md` 3.1), and no package may be named one.
pub const PYTHON_KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue",
    "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import",
    "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while",
    "with", "yield", "_", "case", "match",
];

/// The module names of the Python 3.10 standard library
/// (`sys.stdlib_module_names`): no package may be named one, since it would
/// shadow, or be shadowed by, the standard module (`spec/python.md` 1).
pub const PYTHON_STDLIB_MODULES: &[&str] = &[
    "__future__",
    "_abc",
    "_aix_support",
    "_ast",
    "_asyncio",
    "_bisect",
    "_blake2",
    "_bootsubprocess",
    "_bz2",
    "_codecs",
    "_codecs_cn",
    "_codecs_hk",
    "_codecs_iso2022",
    "_codecs_jp",
    "_codecs_kr",
    "_codecs_tw",
    "_collections",
    "_collections_abc",
    "_compat_pickle",
    "_compression",
    "_contextvars",
    "_crypt",
    "_csv",
    "_ctypes",
    "_curses",
    "_curses_panel",
    "_datetime",
    "_dbm",
    "_decimal",
    "_elementtree",
    "_frozen_importlib",
    "_frozen_importlib_external",
    "_functools",
    "_gdbm",
    "_hashlib",
    "_heapq",
    "_imp",
    "_io",
    "_json",
    "_locale",
    "_lsprof",
    "_lzma",
    "_markupbase",
    "_md5",
    "_msi",
    "_multibytecodec",
    "_multiprocessing",
    "_opcode",
    "_operator",
    "_osx_support",
    "_overlapped",
    "_pickle",
    "_posixshmem",
    "_posixsubprocess",
    "_py_abc",
    "_pydecimal",
    "_pyio",
    "_queue",
    "_random",
    "_scproxy",
    "_sha1",
    "_sha256",
    "_sha3",
    "_sha512",
    "_signal",
    "_sitebuiltins",
    "_socket",
    "_sqlite3",
    "_sre",
    "_ssl",
    "_stat",
    "_statistics",
    "_string",
    "_strptime",
    "_struct",
    "_symtable",
    "_thread",
    "_threading_local",
    "_tkinter",
    "_tracemalloc",
    "_uuid",
    "_warnings",
    "_weakref",
    "_weakrefset",
    "_winapi",
    "_zoneinfo",
    "abc",
    "aifc",
    "antigravity",
    "argparse",
    "array",
    "ast",
    "asynchat",
    "asyncio",
    "asyncore",
    "atexit",
    "audioop",
    "base64",
    "bdb",
    "binascii",
    "binhex",
    "bisect",
    "builtins",
    "bz2",
    "cProfile",
    "calendar",
    "cgi",
    "cgitb",
    "chunk",
    "cmath",
    "cmd",
    "code",
    "codecs",
    "codeop",
    "collections",
    "colorsys",
    "compileall",
    "concurrent",
    "configparser",
    "contextlib",
    "contextvars",
    "copy",
    "copyreg",
    "crypt",
    "csv",
    "ctypes",
    "curses",
    "dataclasses",
    "datetime",
    "dbm",
    "decimal",
    "difflib",
    "dis",
    "distutils",
    "doctest",
    "email",
    "encodings",
    "ensurepip",
    "enum",
    "errno",
    "faulthandler",
    "fcntl",
    "filecmp",
    "fileinput",
    "fnmatch",
    "fractions",
    "ftplib",
    "functools",
    "gc",
    "genericpath",
    "getopt",
    "getpass",
    "gettext",
    "glob",
    "graphlib",
    "grp",
    "gzip",
    "hashlib",
    "heapq",
    "hmac",
    "html",
    "http",
    "idlelib",
    "imaplib",
    "imghdr",
    "imp",
    "importlib",
    "inspect",
    "io",
    "ipaddress",
    "itertools",
    "json",
    "keyword",
    "lib2to3",
    "linecache",
    "locale",
    "logging",
    "lzma",
    "mailbox",
    "mailcap",
    "marshal",
    "math",
    "mimetypes",
    "mmap",
    "modulefinder",
    "msilib",
    "msvcrt",
    "multiprocessing",
    "netrc",
    "nis",
    "nntplib",
    "nt",
    "ntpath",
    "nturl2path",
    "numbers",
    "opcode",
    "operator",
    "optparse",
    "os",
    "ossaudiodev",
    "pathlib",
    "pdb",
    "pickle",
    "pickletools",
    "pipes",
    "pkgutil",
    "platform",
    "plistlib",
    "poplib",
    "posix",
    "posixpath",
    "pprint",
    "profile",
    "pstats",
    "pty",
    "pwd",
    "py_compile",
    "pyclbr",
    "pydoc",
    "pydoc_data",
    "pyexpat",
    "queue",
    "quopri",
    "random",
    "re",
    "readline",
    "reprlib",
    "resource",
    "rlcompleter",
    "runpy",
    "sched",
    "secrets",
    "select",
    "selectors",
    "shelve",
    "shlex",
    "shutil",
    "signal",
    "site",
    "smtpd",
    "smtplib",
    "sndhdr",
    "socket",
    "socketserver",
    "spwd",
    "sqlite3",
    "sre_compile",
    "sre_constants",
    "sre_parse",
    "ssl",
    "stat",
    "statistics",
    "string",
    "stringprep",
    "struct",
    "subprocess",
    "sunau",
    "symtable",
    "sys",
    "sysconfig",
    "syslog",
    "tabnanny",
    "tarfile",
    "telnetlib",
    "tempfile",
    "termios",
    "textwrap",
    "this",
    "threading",
    "time",
    "timeit",
    "tkinter",
    "token",
    "tokenize",
    "trace",
    "traceback",
    "tracemalloc",
    "tty",
    "turtle",
    "turtledemo",
    "types",
    "typing",
    "unicodedata",
    "unittest",
    "urllib",
    "uu",
    "uuid",
    "venv",
    "warnings",
    "wave",
    "weakref",
    "webbrowser",
    "winreg",
    "winsound",
    "wsgiref",
    "xdrlib",
    "xml",
    "xmlrpc",
    "zipapp",
    "zipfile",
    "zipimport",
    "zlib",
    "zoneinfo",
];

/// `^[A-Za-z_][A-Za-z0-9_]*$`: an ASCII Python identifier.
pub fn is_python_identifier(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// A dotted path of ASCII Python identifiers (`reader_adapters.images`),
/// none of them a keyword: the `adapter` of a backend-`python` binding
/// (`spec/python.md` 5.3).
pub fn is_python_module_path(path: &str) -> bool {
    !path.is_empty()
        && path
            .split('.')
            .all(|segment| is_python_identifier(segment) && !PYTHON_KEYWORDS.contains(&segment))
}

/// Why `package` is not an import name of `spec/python.md` Section 1, or
/// `None`: `^[a-z][a-z0-9_]*$`, not a Python keyword or soft keyword, not a
/// standard-library module.
pub fn python_package_problem(package: &str) -> Option<&'static str> {
    let mut bytes = package.bytes();
    let shaped = bytes.next().is_some_and(|b| b.is_ascii_lowercase())
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if !shaped {
        Some("an import name matches ^[a-z][a-z0-9_]*$")
    } else if PYTHON_KEYWORDS.contains(&package) {
        Some("a Python keyword cannot be imported")
    } else if PYTHON_STDLIB_MODULES.contains(&package) {
        Some("it is a module of the Python standard library")
    } else {
        None
    }
}

/// Why `version` is not a distribution version of `spec/python.md` Section
/// 1 (`^[0-9]+(\.[0-9]+)*$`), or `None`.
pub fn python_version_problem(version: &str) -> Option<&'static str> {
    let ok = !version.is_empty()
        && version
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
    (!ok).then_some("a version is a PEP 440 release segment, ^[0-9]+(\\.[0-9]+)*$")
}

fn error(code: cpl_diag::DiagCode, message: String) -> Diagnostic {
    Diagnostic::error(code, codes::CATEGORY, message)
}

/// The Milestone 3 manifest rules of toolchain 11.1, span-less errors of
/// category `project` in this order:
///
/// 1. `project_python_target_not_enabled` (`key`, `targets`) when
///    `[interop.python]` is present while `"python"` is not in `[interop]
///    targets`;
/// 2. `project_python_option_invalid` (`key`, `found`, `reason`) for a
///    `package` or `version` outside its domain;
/// 3. with a root, `project_binding_path_missing` (`key`
///    `"interop.python.paths"`, `path`) for each `paths` entry that is no
///    directory;
/// 4. per binding, in name order: `project_binding_adapter_invalid`
///    (`binding`, `found`) for a backend-`python` binding whose `adapter` is
///    not a dotted module path; `project_binding_timeout_invalid`
///    (`binding`, `found`) for a non-positive `timeout_ms` or one on a
///    backend-`c` binding with `isolation = "in_process"`;
/// 5. per capability, in capability order, that a backend-`python` binding
///    and another binding of backend `c` or `python` bind:
///    `project_binding_duplicate_capability` (`capability`, `bindings` in
///    name order; not when two or more of them are backend `c`, which
///    `check_c_bindings` reports);
/// 6. per `[test.fixtures]` entry, in key order:
///    `project_fixture_binding_unknown` (`capability`, `binding`) when the
///    binding does not exist, else `project_fixture_capability_mismatch`
///    (`capability`, `binding`, `found`) when its capability is not the key.
pub fn check_python(manifest: &Manifest, root: Option<&Path>) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let targets = &manifest.interop.targets;
    if let Some(python) = &manifest.interop_python {
        if !targets.iter().any(|t| t == "python") {
            out.push(
                error(
                    codes::PROJECT_PYTHON_TARGET_NOT_ENABLED,
                    "`[interop.python]` is present but \"python\" is not in `[interop] targets`"
                        .to_string(),
                )
                .with_data("key", json!("interop.python"))
                .with_data("targets", json!(targets)),
            );
        }
        let options = [
            (
                "package",
                python.package.as_deref(),
                python_package_problem as fn(&str) -> Option<&'static str>,
            ),
            ("version", python.version.as_deref(), python_version_problem),
        ];
        for (key, value, problem) in options {
            let Some(value) = value else { continue };
            if let Some(reason) = problem(value) {
                out.push(
                    error(
                        codes::PROJECT_PYTHON_OPTION_INVALID,
                        format!("`interop.python.{key}` {value:?} is invalid: {reason}"),
                    )
                    .with_data("key", json!(format!("interop.python.{key}")))
                    .with_data("found", json!(value))
                    .with_data("reason", json!(reason)),
                );
            }
        }
        if let (Some(root), Some(paths)) = (root, &python.paths) {
            for path in paths {
                if !root.join(path).is_dir() {
                    out.push(
                        error(
                            codes::PROJECT_BINDING_PATH_MISSING,
                            format!("`interop.python.paths`: {path:?} is not a directory of the project"),
                        )
                        .with_data("key", json!("interop.python.paths"))
                        .with_data("path", json!(path)),
                    );
                }
            }
        }
    }
    for (name, binding) in &manifest.bindings {
        // the name becomes the Python function `bind_<name>` of the package, and a TOML key can
        // be any text: a name that is not an identifier would be code in the generated module
        if binding.backend == "python" && !crate::c_bindings::is_binding_name(name) {
            out.push(
                error(
                    codes::PROJECT_BINDING_NAME_INVALID,
                    format!(
                        "binding `{name}`: a backend-python binding's name must match ^[a-z][a-z0-9_]*$, so that its grant `bind_{name}` is a Python identifier"
                    ),
                )
                .with_data("binding", json!(name)),
            );
        }
        if binding.backend == "python" && !is_python_module_path(&binding.adapter) {
            out.push(
                error(
                    codes::PROJECT_BINDING_ADAPTER_INVALID,
                    format!(
                        "binding `{name}`: a Python adapter is a dotted path of Python identifiers (`reader_adapters.images`), found {:?}",
                        binding.adapter
                    ),
                )
                .with_data("binding", json!(name))
                .with_data("found", json!(binding.adapter)),
            );
        }
        if let Some(&timeout) = manifest.binding_timeouts.get(name) {
            let in_process_c = binding.backend == "c" && !binding.is_worker();
            if timeout <= 0 || in_process_c {
                let message = if timeout <= 0 {
                    format!("binding `{name}`: `timeout_ms` must be a positive number of milliseconds, found {timeout}")
                } else {
                    format!("binding `{name}`: `timeout_ms` bounds a worker's answers, and a backend-c binding with `isolation = \"in_process\"` has no worker")
                };
                out.push(
                    error(codes::PROJECT_BINDING_TIMEOUT_INVALID, message)
                        .with_data("binding", json!(name))
                        .with_data("found", json!(timeout)),
                );
            }
        }
    }
    // a capability that a backend-python binding binds has no other binding of backend c or python (cpl run grants the
    // capability's one binding, semantics S78); one with two or more backend-c bindings is check_c_bindings' already
    let mut by_capability: BTreeMap<&str, Vec<(&str, &str)>> = BTreeMap::new();
    for (name, binding) in &manifest.bindings {
        if binding.backend == "c" || binding.backend == "python" {
            by_capability
                .entry(binding.capability.as_str())
                .or_default()
                .push((name.as_str(), binding.backend.as_str()));
        }
    }
    for (capability, bindings) in by_capability {
        let python = bindings.iter().filter(|(_, b)| *b == "python").count();
        let c = bindings.len() - python;
        if python == 0 || bindings.len() < 2 || c >= 2 {
            continue;
        }
        let names: Vec<&str> = bindings.iter().map(|(n, _)| *n).collect();
        out.push(
            error(
                codes::PROJECT_BINDING_DUPLICATE_CAPABILITY,
                format!(
                    "capability `{capability}` has {} bindings ({}); a capability with a backend-python binding has no other",
                    names.len(),
                    names.join(", ")
                ),
            )
            .with_data("capability", json!(capability))
            .with_data("bindings", json!(names)),
        );
    }
    for (capability, binding_name) in &manifest.test_fixtures {
        match manifest.bindings.get(binding_name) {
            None => out.push(
                error(
                    codes::PROJECT_FIXTURE_BINDING_UNKNOWN,
                    format!("`[test.fixtures]`: {capability:?} names the binding `{binding_name}`, which `[bindings]` does not declare"),
                )
                .with_data("capability", json!(capability))
                .with_data("binding", json!(binding_name)),
            ),
            Some(binding) if binding.capability != *capability => out.push(
                error(
                    codes::PROJECT_FIXTURE_CAPABILITY_MISMATCH,
                    format!(
                        "`[test.fixtures]`: the binding `{binding_name}` supplies `{}`, not {capability:?}",
                        binding.capability
                    ),
                )
                .with_data("capability", json!(capability))
                .with_data("binding", json!(binding_name))
                .with_data("found", json!(binding.capability)),
            ),
            Some(_) => {}
        }
    }
    out
}

/// The input digest of a backend-python binding (`spec/python.md` Section 9, P11): the blake3 of the adapter's module
/// path and, when the module resolves under one of `[interop.python] paths` of the project root (`a/b.py`, or the
/// package `a/b/__init__.py`, the first path that has one), the project-relative path and content of the module's file,
/// or of every `.py` file under the package's directory, in path order. `None` for a binding that is not
/// backend-python.
pub fn python_input_digest(
    manifest: &Manifest,
    root: Option<&Path>,
    binding: &str,
) -> Option<cpl_diag::json::Digest> {
    let b = manifest
        .bindings
        .get(binding)
        .filter(|b| b.backend == "python")?;
    let mut h = blake3::Hasher::new();
    h.update(b"python adapter\0");
    h.update(b.adapter.as_bytes());
    h.update(b"\0");
    if let Some(root) = root {
        let rel: Vec<&str> = b.adapter.split('.').collect();
        for dir in manifest.python_paths() {
            let base = if dir == "." {
                root.to_path_buf()
            } else {
                root.join(&dir)
            };
            let file = base.join(format!("{}.py", rel.join("/")));
            let package = base.join(rel.join("/"));
            let mut files: Vec<std::path::PathBuf> = Vec::new();
            if file.is_file() {
                files.push(file);
            } else if package.join("__init__.py").is_file() {
                collect_py(&package, &mut files);
            } else {
                continue;
            }
            files.sort();
            for f in files {
                let shown = f
                    .strip_prefix(root)
                    .unwrap_or(&f)
                    .to_string_lossy()
                    .replace('\\', "/");
                h.update(shown.as_bytes());
                h.update(b"\0");
                h.update(blake3::hash(&std::fs::read(&f).unwrap_or_default()).as_bytes());
            }
            break;
        }
    }
    Some(cpl_diag::json::Digest(h.finalize().to_hex().to_string()))
}

fn collect_py(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        let Ok(kind) = e.file_type() else { continue };
        if kind.is_dir() {
            if path.file_name().is_some_and(|n| n != "__pycache__") {
                collect_py(&path, out);
            }
        } else if kind.is_file() && path.extension().is_some_and(|x| x == "py") {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_paths() {
        assert!(is_python_module_path("reader_adapters.images"));
        assert!(is_python_module_path("_private.Mod2"));
        assert!(!is_python_module_path("reader-adapters"));
        assert!(!is_python_module_path("a..b"));
        assert!(!is_python_module_path("a.class"));
        assert!(!is_python_module_path(""));
        assert_eq!(python_package_problem("reader_cpl"), None);
        assert!(python_package_problem("json").is_some());
        assert!(python_package_problem("match").is_some());
        assert!(python_package_problem("Reader").is_some());
        assert_eq!(python_version_problem("1.2.10"), None);
        assert!(python_version_problem("1.2-beta").is_some());
        assert!(python_version_problem("").is_some());
        assert!(python_version_problem("1..2").is_some());
    }
}
