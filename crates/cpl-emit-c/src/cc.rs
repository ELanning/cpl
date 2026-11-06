//! Compiler discovery (`spec/toolchain.md` 1.13, 7.2, T72; `spec/cabi.md` 1.5-1.7). Owner: the build track (D).

use crate::codes;
use cpl_diag::json::Digest;
use cpl_diag::{Category, Diagnostic};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The compiler families `cpl build` accepts (T72).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Family {
    Gcc,
    Clang,
}

/// The flag sets of `spec/cabi.md` 1.5 (`cpl_rt::flags::OPTIMIZED`, `cpl_rt::flags::DEBUG`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Profile {
    Optimized,
    Debug,
}

impl Profile {
    /// `optimized` or `debug`.
    pub fn as_str(self) -> &'static str {
        match self {
            Profile::Optimized => "optimized",
            Profile::Debug => "debug",
        }
    }

    /// The profile's flag set.
    pub fn flags(self) -> &'static [&'static str] {
        match self {
            Profile::Optimized => cpl_rt::flags::OPTIMIZED,
            Profile::Debug => cpl_rt::flags::DEBUG,
        }
    }
}

/// A sanitizer of `--sanitize` (`address` and `undefined` together or alone, or `thread` alone).
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Sanitizer {
    Address,
    Undefined,
    Thread,
}

impl Sanitizer {
    pub fn as_str(self) -> &'static str {
        match self {
            Sanitizer::Address => "address",
            Sanitizer::Undefined => "undefined",
            Sanitizer::Thread => "thread",
        }
    }
}

/// The flags of a sanitizer set (cabi.md 1.5): address and undefined share one set; thread has its own.
pub fn sanitizer_flags(sanitizers: &[Sanitizer]) -> Vec<&'static str> {
    if sanitizers.contains(&Sanitizer::Thread) {
        cpl_rt::flags::SANITIZE_THREAD.to_vec()
    } else if sanitizers.is_empty() {
        Vec::new()
    } else {
        let mut f: Vec<&'static str> = vec!["-O1", "-g", "-fno-omit-frame-pointer"];
        let mut list = Vec::new();
        if sanitizers.contains(&Sanitizer::Address) {
            list.push("address");
        }
        if sanitizers.contains(&Sanitizer::Undefined) {
            list.push("undefined");
        }
        f.push(match list.as_slice() {
            ["address"] => "-fsanitize=address",
            ["undefined"] => "-fsanitize=undefined",
            _ => "-fsanitize=address,undefined",
        });
        f.push("-fno-sanitize-recover=all");
        f
    }
}

/// A probed C compiler.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Compiler {
    /// The resolved driver binary.
    pub path: PathBuf,
    pub family: Family,
    /// `11.4.0`: `-dumpfullversion` of gcc; `__clang_major__.__clang_minor__.__clang_patchlevel__` of clang.
    pub version: String,
    /// `-dumpmachine`: `x86_64-linux-gnu`.
    pub target: String,
    /// blake3 of the binary.
    pub digest: Digest,
}

fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// A command as T72 reads one: a path is taken as written, a bare name is looked up on `PATH`.
fn resolve(command: &str) -> Option<PathBuf> {
    if command.contains('/') {
        Some(PathBuf::from(command)).filter(|p| p.is_file())
    } else {
        on_path(command)
    }
}

/// A command of the compiler in T72's scrubbed environment.
pub fn command(cc: &Path) -> Command {
    let mut c = Command::new(cc);
    for v in cpl_rt::testing::SCRUBBED {
        c.env_remove(v);
    }
    c.env("LC_ALL", "C");
    c
}

fn run(cc: &Path, args: &[&str]) -> Option<Output> {
    command(cc).args(args).output().ok()
}

fn diag(code: cpl_diag::DiagCode, message: String) -> Diagnostic {
    Diagnostic::error(code, Category::Build, message)
}

impl Compiler {
    /// T72's precedence: `explicit` (`--cc`), `manifest` (`[interop.c] compiler`), the environment variable `CPL_CC`,
    /// then `cc` on `PATH`; `$CC` is never read. A bare command name is looked up on `PATH`, a path is taken as
    /// written, and `path` is the resolved driver binary. The probe reads the family from the predefined macros (clang
    /// when `__clang__` is defined, else gcc when `__GNUC__` is), the version of clang from `__clang_major__`,
    /// `__clang_minor__`, and `__clang_patchlevel__` and that of gcc from `-dumpfullversion`, the target from
    /// `-dumpmachine`, and `digest` is blake3 of the binary. None found: `build_cc_unavailable` (`searched`); a family
    /// other than gcc or clang, gcc before 9, or clang before 12: `build_cc_unsupported` (`found`, `minimum`).
    pub fn discover(
        explicit: Option<&Path>,
        manifest: Option<&str>,
    ) -> Result<Compiler, Diagnostic> {
        let mut searched: Vec<String> = Vec::new();
        let candidate = if let Some(e) = explicit {
            searched.push(e.display().to_string());
            resolve(&e.display().to_string())
        } else if let Some(m) = manifest {
            searched.push(m.to_string());
            resolve(m)
        } else {
            match std::env::var("CPL_CC") {
                Ok(cc) if !cc.is_empty() => {
                    searched.push(format!("CPL_CC={cc}"));
                    resolve(&cc)
                }
                _ => {
                    searched.push("cc".to_string());
                    resolve("cc")
                }
            }
        };
        let Some(path) = candidate else {
            return Err(diag(
                codes::BUILD_CC_UNAVAILABLE,
                format!("no C compiler was found (searched {})", searched.join(", ")),
            )
            .with_data("searched", json!(searched)));
        };
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        let macros = run(&path, &["-dM", "-E", "-x", "c", "/dev/null"])
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        let has = |name: &str| {
            macros
                .lines()
                .any(|l| l.starts_with(&format!("#define {name} ")))
        };
        let value = |name: &str| -> Option<String> {
            macros.lines().find_map(|l| {
                l.strip_prefix(&format!("#define {name} "))
                    .map(|v| v.trim().to_string())
            })
        };
        let (family, version) = if has("__clang__") {
            let v = format!(
                "{}.{}.{}",
                value("__clang_major__").unwrap_or_default(),
                value("__clang_minor__").unwrap_or_default(),
                value("__clang_patchlevel__").unwrap_or_default()
            );
            (Family::Clang, v)
        } else if has("__GNUC__") {
            let v = run(&path, &["-dumpfullversion"])
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default();
            (Family::Gcc, v)
        } else {
            return Err(diag(
                codes::BUILD_CC_UNSUPPORTED,
                format!("{} is neither gcc nor clang", path.display()),
            )
            .with_data("found", json!(path.display().to_string()))
            .with_data("minimum", json!("gcc 9 or clang 12")));
        };
        let major: u32 = version
            .split('.')
            .next()
            .and_then(|m| m.parse().ok())
            .unwrap_or(0);
        let (minimum, name) = match family {
            Family::Gcc => (9, "gcc"),
            Family::Clang => (12, "clang"),
        };
        if major < minimum {
            return Err(diag(
                codes::BUILD_CC_UNSUPPORTED,
                format!("{name} {version} is older than {name} {minimum}"),
            )
            .with_data("found", json!(format!("{name} {version}")))
            .with_data("minimum", json!(format!("{name} {minimum}"))));
        }
        let target = run(&path, &["-dumpmachine"])
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        let digest = match std::fs::read(&path) {
            Ok(bytes) => Digest(blake3::hash(&bytes).to_hex().to_string()),
            Err(_) => Digest(String::new()),
        };
        Ok(Compiler {
            path,
            family,
            version,
            target,
            digest,
        })
    }

    /// `gcc 11.4.0`: the compiler as the library information names it.
    pub fn name(&self) -> String {
        match self.family {
            Family::Gcc => format!("gcc {}", self.version),
            Family::Clang => format!("clang {}", self.version),
        }
    }

    /// Whether a one-line program compiles and links with these sanitizers' flags (toolchain 12.8's skip policy, and
    /// `build_sanitizer_unavailable`).
    pub fn supports(&self, sanitizers: &[Sanitizer]) -> bool {
        let Ok(dir) = cpl_diag::fsutil::private_temp_dir("cpl-sanitizer-probe") else {
            return false;
        };
        let src = dir.join("probe.c");
        let out = dir.join("probe");
        let ok = std::fs::write(&src, "int main(void) { return 0; }\n").is_ok()
            && command(&self.path)
                .args(sanitizer_flags(sanitizers))
                .arg(&src)
                .arg("-o")
                .arg(&out)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
        let _ = std::fs::remove_dir_all(&dir);
        ok
    }
}
