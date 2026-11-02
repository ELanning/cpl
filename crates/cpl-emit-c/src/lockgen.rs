//! Internal: the C entries of `cpl.lock` and the comparisons of `cpl build --release` and `--lock` (toolchain 11.2,
//! T75, T81; spec/cabi.md 1.5, 6.6, 9.3): the export signature and exports digests, the flags digest, the digests of
//! the headers a binding's compilations read and of the platform files the link opened, the libraries a binding's `-l`
//! resolved, and the lock comparisons (`build_lock_mismatch`, `build_abi_changed`).

use crate::codes;
use cpl_diag::json::Digest;
use cpl_diag::{Category, Diagnostic};
use cpl_hir::{DefId, DefKind, Program, Ty};
use cpl_project::{ForeignC, IntMapping, Lock, Project, TargetC};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The blake3 of the lines, each ended by LF.
fn digest_lines<S: AsRef<str>>(lines: &[S]) -> Digest {
    let mut h = blake3::Hasher::new();
    for l in lines {
        h.update(l.as_ref().as_bytes());
        h.update(b"\n");
    }
    Digest(h.finalize().to_hex().to_string())
}

/// The blake3 of a file's bytes; `None` when it cannot be read.
pub(crate) fn file_digest(path: &Path) -> Option<Digest> {
    std::fs::read(path)
        .ok()
        .map(|b| Digest(blake3::hash(&b).to_hex().to_string()))
}

/// A path as the lock records it: project-relative under the project root, absolute elsewhere.
pub(crate) fn lock_path(path: &Path, project: &Project) -> String {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    match project.root.as_ref().and_then(|r| abs.strip_prefix(r).ok()) {
        Some(rel) => rel.display().to_string(),
        None => abs.display().to_string(),
    }
}

/// The digest of a set of files (cabi.md 6.6): the blake3 of `path LF digest LF` for every file, sorted by its lock
/// path; a file that cannot be read contributes its path and an empty digest.
pub(crate) fn files_digest(files: &BTreeSet<PathBuf>, project: &Project) -> Digest {
    let mut entries: Vec<(String, String)> = files
        .iter()
        .map(|f| {
            (
                lock_path(f, project),
                file_digest(f).map(|d| d.0).unwrap_or_default(),
            )
        })
        .collect();
    entries.sort();
    entries.dedup();
    let mut lines = Vec::with_capacity(entries.len() * 2);
    for (p, d) in entries {
        lines.push(p);
        lines.push(d);
    }
    digest_lines(&lines)
}

/// The signature facet text of a declaration (toolchain 5.2), when its file's syntax tree is at hand.
fn signature_text(p: &Program, d: DefId) -> Option<String> {
    let def = p.def(d);
    let module = p
        .asts
        .iter()
        .find(|(f, _)| *f == def.file)
        .map(|(_, m)| m.clone())?;
    let decl = module.declarations.get(def.decl_index)?;
    // the canonical signature fragment, as cpl_index::facet_text gives the signature facet
    Some(cpl_syntax::format_fragment(
        cpl_syntax::Fragment::Signature(decl),
        cpl_syntax::FragmentMode::Canonical,
    ))
}

/// A facet's content hash (toolchain 5.3): the first 32 hex digits of the blake3 of its canonical text.
fn facet_hash(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex()[..32].to_string()
}

/// The declarations a type reaches directly.
fn reach(ty: &Ty, out: &mut Vec<DefId>) {
    match ty {
        Ty::Adt(d, args) => {
            out.push(*d);
            for a in args {
                reach(a, out);
            }
        }
        Ty::Wrapper(d) | Ty::Refined(_, d) | Ty::Capability(d) => out.push(*d),
        Ty::List(t) | Ty::Gen(t) => reach(t, out),
        _ => {}
    }
}

/// Every declaration a function's signature reaches transitively through fields, payloads, and capability operations
/// (cabi.md 9.3), by qualified name.
fn reached(p: &Program, function: DefId) -> BTreeMap<String, DefId> {
    let mut work = Vec::new();
    if let Some(fd) = p.def(function).kind.as_fn() {
        for prm in fd.params.iter().chain(&fd.using) {
            reach(&prm.ty, &mut work);
        }
        reach(&fd.ret, &mut work);
    }
    let mut seen: BTreeSet<DefId> = BTreeSet::new();
    let mut out = BTreeMap::new();
    while let Some(d) = work.pop() {
        if !seen.insert(d) {
            continue;
        }
        out.insert(p.display_path(d), d);
        match &p.def(d).kind {
            DefKind::Struct(s) => s.fields.iter().for_each(|f| reach(&f.ty, &mut work)),
            DefKind::Enum(e) => e
                .variants
                .iter()
                .flat_map(|v| &v.payload)
                .for_each(|t| reach(t, &mut work)),
            DefKind::Wrapper(w) => reach(&w.inner, &mut work),
            DefKind::Refinement(r) => reach(&r.base, &mut work),
            DefKind::Capability(c) => {
                for op in &c.ops {
                    op.params.iter().for_each(|x| reach(&x.ty, &mut work));
                    reach(&op.ret, &mut work);
                }
            }
            _ => {}
        }
    }
    out
}

/// An export's signature digest (cabi.md 9.3): its symbol; its integer mapping; the content hash of its signature
/// facet; then, for every declaration its signature reaches, in byte order of their qualified names, the qualified
/// name and the declaration's signature facet text (its shape).
pub(crate) fn signature_digest(
    p: &Program,
    symbol: &str,
    int: IntMapping,
    function: DefId,
) -> Digest {
    let facet = signature_text(p, function).unwrap_or_else(|| p.display_path(function));
    let mut lines = vec![
        symbol.to_string(),
        int.as_str().to_string(),
        facet_hash(&facet),
    ];
    for (name, d) in reached(p, function) {
        lines.push(name);
        lines.push(signature_text(p, d).unwrap_or_default().replace('\n', " "));
    }
    digest_lines(&lines)
}

/// The exports digest (cabi.md 9.3): every export's symbol and signature digest, in export order, then the qualified
/// type text of every listed type, in id order (the byte order of the texts).
pub(crate) fn exports_digest(p: &Program, plan: &crate::InteropPlan) -> Digest {
    let mut lines = Vec::new();
    for e in &plan.exports {
        lines.push(e.symbol.clone());
        lines.push(signature_digest(p, &e.symbol, e.int, e.function).0);
    }
    let mut texts: Vec<&str> = plan.types.iter().map(|t| t.text.as_str()).collect();
    texts.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    lines.extend(texts.into_iter().map(str::to_string));
    digest_lines(&lines)
}

/// The flags digest (cabi.md 1.5): the base set, the profile's set, and the sanitizers' sets, with `<out>` written as
/// `.` and the per-unit flags left out; the shared link's flags; `[interop.c] cflags` and `ldflags`; then, per
/// backend-c binding in name order, its include_dirs, defines, cflags, libraries, and library_dirs as written.
pub(crate) fn flags_digest(project: &Project, profile: &[&str], sanitize: &[&str]) -> Digest {
    flags_digest_with(project, profile, sanitize, &[])
}

/// [`flags_digest`] with `extra` lines after the C set: the Python extension's own compile and link flags
/// (`spec/python.md` 9).
pub(crate) fn flags_digest_with(
    project: &Project,
    profile: &[&str],
    sanitize: &[&str],
    extra: &[String],
) -> Digest {
    let mut lines: Vec<String> = cpl_rt::flags::BASE.iter().map(|s| s.to_string()).collect();
    lines.push("-ffile-prefix-map=.=.".to_string());
    lines.push("-I./src".to_string());
    lines.push("-I./include".to_string());
    lines.extend(profile.iter().map(|s| s.to_string()));
    lines.extend(sanitize.iter().map(|s| s.to_string()));
    lines.extend(cpl_rt::flags::SHARED_LINK.iter().map(|s| s.to_string()));
    if let Some(c) = &project.manifest.interop_c {
        lines.extend(c.cflags.iter().cloned());
        lines.extend(c.ldflags.iter().cloned());
    }
    for (name, b) in &project.manifest.bindings {
        if b.backend != "c" {
            continue;
        }
        if let Some(c) = project.manifest.binding_c.get(name) {
            for group in [
                &c.include_dirs,
                &c.defines,
                &c.cflags,
                &c.libraries,
                &c.library_dirs,
            ] {
                lines.extend(group.iter().cloned());
            }
        }
    }
    lines.extend(extra.iter().cloned());
    digest_lines(&lines)
}

/// The headers a depfile lists (`-MD -MF`): every prerequisite after the source itself.
pub(crate) fn depfile_headers(text: &str) -> Vec<PathBuf> {
    let joined = text.replace("\\\n", " ");
    let Some(colon) = joined.find(": ") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = joined[colon + 2..].chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&' ') => {
                cur.push(' ');
                chars.next();
            }
            c if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(PathBuf::from(std::mem::take(&mut cur)));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(PathBuf::from(cur));
    }
    if !out.is_empty() {
        out.remove(0);
    }
    out
}

/// What the link of a build opened (`-Wl,--trace`), in order and without repeats.
pub(crate) fn trace_files(stdout: &str) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        // a library found for -l<name> may be printed as "-l<name> (<path>)"
        let path = match (line.find('('), line.strip_suffix(')')) {
            (Some(open), Some(_)) if line.starts_with("-l") => &line[open + 1..line.len() - 1],
            _ => line,
        };
        if path.is_empty()
            || !path.starts_with('/') && !path.starts_with('.') && !path.contains('/')
        {
            continue;
        }
        if seen.insert(path.to_string()) {
            out.push(PathBuf::from(path));
        }
    }
    out
}

/// The comparisons of `cpl build --release` against the lock (toolchain 11.2): `build_abi_changed` per ABI break
/// (cabi.md 9.3, when `prefix` and `abi` are the lock's), then `build_lock_mismatch` with the dotted keys of
/// `targets.c`, `foreign.c.<b>.headers`, and `foreign.c.<b>.libraries` that differ.
pub(crate) fn compare_release(
    lock: &Lock,
    target: &TargetC,
    foreign: &BTreeMap<String, ForeignC>,
) -> Vec<Diagnostic> {
    let mut out = abi_changes(lock, target);
    let abi_broken = !out.is_empty();
    let Some(locked) = lock.target_c() else {
        return out;
    };
    let mut differences: Vec<String> = Vec::new();
    let mut key = |name: &str, same: bool| {
        let skip = abi_broken && matches!(name, "api" | "exports" | "exports_digest" | "symbols");
        if !same && !skip {
            differences.push(format!("targets.c.{name}"));
        }
    };
    // the keys of targets.c in key order
    key("abi", locked.abi == target.abi);
    key("api", locked.api == target.api);
    key("compiler", locked.compiler == target.compiler);
    key(
        "compiler_digest",
        locked.compiler_digest == target.compiler_digest,
    );
    key(
        "compiler_path",
        locked.compiler_path == target.compiler_path,
    );
    key("exports", locked.exports == target.exports);
    key(
        "exports_digest",
        locked.exports_digest == target.exports_digest,
    );
    key("flags_digest", locked.flags_digest == target.flags_digest);
    key("prefix", locked.prefix == target.prefix);
    key("runtime", locked.runtime == target.runtime);
    key(
        "runtime_digest",
        locked.runtime_digest == target.runtime_digest,
    );
    key("symbols", locked.symbols == target.symbols);
    key(
        "system_digest",
        locked.system_digest == target.system_digest,
    );
    key("target", locked.target == target.target);
    let locked_foreign = lock.foreign_c();
    let names: BTreeSet<&String> = locked_foreign.keys().chain(foreign.keys()).collect();
    for name in names {
        let (a, b) = (locked_foreign.get(name), foreign.get(name));
        if a.map(|f| &f.headers) != b.map(|f| &f.headers) {
            differences.push(format!("foreign.c.{name}.headers"));
        }
        if a.map(|f| &f.libraries) != b.map(|f| &f.libraries) {
            differences.push(format!("foreign.c.{name}.libraries"));
        }
    }
    if !differences.is_empty() {
        out.push(
            Diagnostic::error(
                codes::BUILD_LOCK_MISMATCH,
                Category::Build,
                format!(
                    "the build differs from cpl.lock in {}; rebuild with the locked toolchain and inputs, or record the change with cpl build --lock",
                    differences.join(", ")
                ),
            )
            .with_data("differences", json!(differences)),
        );
    }
    out
}

/// The comparisons of `cpl build --target python --release` against the lock (`spec/python.md` 9): `build_lock_mismatch`
/// with the dotted keys of `targets.python` and of the backend-python `adapters.<binding>` that differ (an adapter
/// present on one side only differs as a whole).
pub(crate) fn compare_release_python(
    lock: &Lock,
    entries: &crate::package::PythonLockEntries,
) -> Vec<Diagnostic> {
    let Some(locked) = lock.target_python() else {
        return Vec::new();
    };
    let t = &entries.target;
    let mut differences: Vec<String> = Vec::new();
    for (name, same) in [
        ("api", locked.api == t.api),
        (
            "compiler_digest",
            locked.compiler_digest == t.compiler_digest,
        ),
        ("compiler_path", locked.compiler_path == t.compiler_path),
        ("flags_digest", locked.flags_digest == t.flags_digest),
        ("package", locked.package == t.package),
        (
            "python_headers_digest",
            locked.python_headers_digest == t.python_headers_digest,
        ),
        ("runtime_digest", locked.runtime_digest == t.runtime_digest),
        ("version", locked.version == t.version),
    ] {
        if !same {
            differences.push(format!("targets.python.{name}"));
        }
    }
    let locked_adapters = lock.python_adapters();
    let names: BTreeSet<&String> = locked_adapters
        .keys()
        .chain(entries.adapters.keys())
        .collect();
    for name in names {
        match (locked_adapters.get(name), entries.adapters.get(name)) {
            (Some(a), Some(b)) => {
                if a.module != b.module {
                    differences.push(format!("adapters.{name}.module"));
                }
                if a.digest != b.digest {
                    differences.push(format!("adapters.{name}.digest"));
                }
            }
            _ => differences.push(format!("adapters.{name}")),
        }
    }
    if differences.is_empty() {
        return Vec::new();
    }
    vec![Diagnostic::error(
        codes::BUILD_LOCK_MISMATCH,
        Category::Build,
        format!(
            "the build differs from cpl.lock in {}; rebuild with the locked toolchain and inputs, or record the change with cpl build --target python --lock",
            differences.join(", ")
        ),
    )
    .with_data("differences", json!(differences))]
}

/// The ABI breaks of a build against a lock's `targets.c` (cabi.md 9.3): none when the lock has no `targets.c` or the
/// package's `prefix` or `abi` differs from the lock's.
pub(crate) fn abi_changes(lock: &Lock, target: &TargetC) -> Vec<Diagnostic> {
    let Some(locked) = lock.target_c() else {
        return Vec::new();
    };
    if locked.prefix != target.prefix || locked.abi != target.abi {
        return Vec::new();
    }
    let mut out = Vec::new();
    let changed = |symbol: Option<&str>,
                   id: Option<&str>,
                   expected: serde_json::Value,
                   found: serde_json::Value,
                   why: String| {
        // the C identifier is `c_symbol`: `symbol` is a reserved key of the diagnostic object (T5), as for
        // project_export_symbol_invalid (cpl_sema::interop::SYMBOL_KEY)
        Diagnostic::error(codes::BUILD_ABI_CHANGED, Category::Build, why)
            .with_data(cpl_sema::interop::SYMBOL_KEY, json!(symbol))
            .with_data("id", json!(id))
            .with_data("expected", expected)
            .with_data("found", found)
    };
    for (symbol, e) in &locked.exports {
        match target.exports.get(symbol) {
            None => out.push(changed(
                Some(symbol),
                Some(e.id.as_str()),
                json!(e.signature.0),
                serde_json::Value::Null,
                format!("the locked export {symbol} is gone: raise [interop.c] abi"),
            )),
            Some(f) if f.signature != e.signature => out.push(changed(
                Some(symbol),
                Some(e.id.as_str()),
                json!(e.signature.0),
                json!(f.signature.0),
                format!("the ABI of {symbol} changed: raise [interop.c] abi"),
            )),
            Some(f) if f.int != e.int => out.push(changed(
                Some(symbol),
                Some(e.id.as_str()),
                json!(e.int.as_str()),
                json!(f.int.as_str()),
                format!("the integer mapping of {symbol} changed: raise [interop.c] abi"),
            )),
            Some(_) => {}
        }
    }
    let now: BTreeSet<&String> = target.symbols.iter().collect();
    for s in &locked.symbols {
        if !locked.exports.contains_key(s) && !now.contains(s) {
            out.push(changed(
                Some(s),
                None,
                json!(s),
                serde_json::Value::Null,
                format!("the locked symbol {s} is no longer exported: raise [interop.c] abi"),
            ));
        }
    }
    let major = |api: &str| api.split('.').next().unwrap_or("").to_string();
    if major(&locked.api) != major(&target.api) {
        out.push(changed(
            None,
            None,
            json!(locked.api),
            json!(target.api),
            format!(
                "the C API major changed from {} to {}: raise [interop.c] abi",
                locked.api, target.api
            ),
        ));
    }
    out
}
