//! Internal: the object cache under `<project>/.cpl/cache/c/obj` (spec/cabi.md 1.6, step 5 of a package build). An
//! object is cached by the blake3 of the unit's text, the compiler digest, and the unit's flags, together with its
//! depfile and the blake3 of every file the depfile lists, and it is reused only while each of those files still has
//! its recorded digest: a changed header (`cpl_rt.h`, `cpl.h`, a generated header, a vendored or a system header)
//! recompiles every unit that read it. An entry is written into a fresh directory and renamed into place, so builds
//! that share the cache, the parallel tests of one test binary among them, may run at once. (The driver cache, keyed
//! by cabi.md 1.6 as a whole, is `package::driver`'s.)
//!
//! An entry `<cache>/<key[..2]>/<key>/` holds `o` (the object), `d` (its depfile), and `deps` (one line per file the
//! depfile lists, the source left out since its text is in the key: `<blake3 hex> <absolute path>`).

use crate::cc;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

fn file_digest(path: &Path) -> Option<String> {
    std::fs::read(path)
        .ok()
        .map(|b| blake3::hash(&b).to_hex().to_string())
}

/// The key of an object: the unit's text, the compiler digest, and the unit's flags.
fn key(text: &[u8], compiler: &cc::Compiler, args: &[String]) -> String {
    let mut h = blake3::Hasher::new();
    h.update(b"cpl object cache 1\0");
    h.update(&(text.len() as u64).to_le_bytes());
    h.update(text);
    h.update(compiler.digest.as_str().as_bytes());
    h.update(b"\0");
    for a in args {
        h.update(&(a.len() as u64).to_le_bytes());
        h.update(a.as_bytes());
    }
    h.finalize().to_hex().to_string()
}

/// `cc <args> -MD -MF <dep> -c <src> -o <obj>`, through the object cache at `cache` when there is one: `Ok(true)` when
/// the cached object (and its depfile) were reused, `Ok(false)` when the unit was compiled, and `Err` with the
/// compiler's stderr when it was rejected.
pub(crate) fn compile(
    cache: Option<&Path>,
    compiler: &cc::Compiler,
    args: &[String],
    src: &Path,
    obj: &Path,
    dep: &Path,
) -> Result<bool, String> {
    let entry = match cache {
        Some(cache) => {
            let text = std::fs::read(src).map_err(|e| format!("{}: {e}", src.display()))?;
            let k = key(&text, compiler, args);
            Some(cache.join(&k[..2]).join(&k))
        }
        None => None,
    };
    if let Some(entry) = &entry {
        if reusable(entry)
            && std::fs::copy(entry.join("o"), obj).is_ok()
            && std::fs::copy(entry.join("d"), dep).is_ok()
        {
            return Ok(true);
        }
    }
    let out = cc::command(&compiler.path)
        .args(args)
        .arg("-MD")
        .arg("-MF")
        .arg(dep)
        .arg("-c")
        .arg(src)
        .arg("-o")
        .arg(obj)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).to_string());
    }
    if let Some(entry) = &entry {
        store(entry, obj, dep);
    }
    Ok(false)
}

/// Whether the entry is complete and every file its depfile lists still has its recorded digest.
fn reusable(entry: &Path) -> bool {
    let Ok(deps) = std::fs::read_to_string(entry.join("deps")) else {
        return false;
    };
    entry.join("o").is_file()
        && entry.join("d").is_file()
        && deps.lines().all(|l| {
            l.split_once(' ').is_some_and(|(digest, path)| {
                file_digest(Path::new(path)).as_deref() == Some(digest)
            })
        })
}

/// The files a depfile lists but the source, absolute.
fn listed(depfile: &str) -> Vec<PathBuf> {
    let cwd = std::env::current_dir().unwrap_or_default();
    crate::lockgen::depfile_headers(depfile)
        .into_iter()
        .map(|h| if h.is_absolute() { h } else { cwd.join(h) })
        .collect()
}

/// Records a freshly compiled object: written into a fresh directory, then renamed over any stale entry.
fn store(entry: &Path, obj: &Path, dep: &Path) {
    static N: AtomicU64 = AtomicU64::new(0);
    let Ok(text) = std::fs::read_to_string(dep) else {
        return;
    };
    let mut deps = String::new();
    for h in listed(&text) {
        // a file that cannot be read now cannot be compared later: no entry
        let Some(d) = file_digest(&h) else {
            return;
        };
        deps.push_str(&format!("{d} {}\n", h.display()));
    }
    let Some(parent) = entry.parent() else {
        return;
    };
    let tmp = parent.join(format!(
        ".tmp-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let written = std::fs::create_dir_all(&tmp).is_ok()
        && std::fs::copy(obj, tmp.join("o")).is_ok()
        && std::fs::copy(dep, tmp.join("d")).is_ok()
        && std::fs::write(tmp.join("deps"), deps).is_ok();
    if written {
        if entry.exists() {
            // a stale entry of the same key (a header changed since): replaced
            let _ = std::fs::remove_dir_all(entry);
        }
        if std::fs::rename(&tmp, entry).is_ok() {
            return;
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
}
