//! cpl-rt: the C11 runtime of CPL and C API 0.1 as source embedded in the `cpl` binary (`spec/toolchain.md` 1.12,
//! T69; `spec/cabi.md` Section 2, C6).
//!
//! The runtime is the C under `crates/cpl-rt/c/`: the public headers `include/cpl.h` and `include/cpl_adapter.h`
//! (`spec/cabi.md` Appendix A), the internal contract with emitted code `cpl_rt.h` (Appendix B.1), the runtime's own
//! cross-file contract `cpl_rt_internal.h` (2.1), and the runtime's `.c` files. `build.rs` embeds every `*.c` and
//! `*.h` file there as [`RUNTIME_FILES`]; Cargo compiles no C. `cpl build` writes the runtime into the package it
//! builds and compiles it with the emitted code (`cpl_emit_c::package`), so no prebuilt runtime exists to be stale;
//! this crate's own C unit tests compile it through [`testing`].
//!
//! | File of `c/` | Contents (`spec/cabi.md` 2.1) | Owner |
//! | - | - | - |
//! | `include/cpl.h`, `include/cpl_adapter.h`, `cpl_rt.h` | the frozen contracts of Appendices A and B.1 | contracts (track 0) |
//! | `cpl_rt_internal.h` | the fault record and runtime object layouts, the cross-file functions in three sections | contracts; each runtime track appends to its section |
//! | `cpl_rt_value.c`, `cpl_rt_int.c`, `cpl_rt_text.c`, `cpl_rt_list.c`, `cpl_rt_json.c`, `cpl_rt_render.c`, `cpl_rt_platform.h`, `cpl_rt_platform_linux.c` | values and the handle table, `Int`, `Text` and `Bytes`, `List`, the JSON reader and writers, value text, the platform | R1 (with this crate's Rust side) |
//! | `cpl_rt_eval.c`, `cpl_rt_fault.c`, `cpl_rt_builtins.c`, `cpl_rt_boundary.c` | evaluation contexts, fault records and claims, the remaining builtins, boundary checks | R2 |
//! | `cpl_rt_api.c`, `cpl_rt_cap.c`, `cpl_rt_driver.c`, `cpl_rt_exe.c` | the C API table and lifecycle, capabilities, the two mains | R3 |
//!
//! The order of `struct cpl_api` is frozen in `crates/cpl-rt/api_v0.txt` (toolchain 12, T78).

use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};

include!(concat!(env!("OUT_DIR"), "/runtime_files.rs"));

/// The runtime's version: the toolchain's (`cpl_diag::version::TOOLCHAIN`), since the runtime is versioned with the
/// toolchain that embeds it (`spec/cabi.md` 3.8 banner `"runtime"`; `cpl_library_info.runtime_version`).
pub const RUNTIME_VERSION: &str = cpl_diag::version::TOOLCHAIN;

/// The name of the generated tables header (`spec/cabi.md` 2.9, Appendix B.2), as the digest and the amalgamation
/// spell it.
const GENERATED_HEADER: &str = "cpl_rt_gen.h";

/// The two mains, which the amalgamation leaves out and writes as units of their own (`spec/cabi.md` 2.2).
const MAINS: [&str; 2] = ["cpl_rt_driver.c", "cpl_rt_exe.c"];

/// Where the runtime files come from: the embedded [`RUNTIME_FILES`], or the `CPL_RUNTIME_DIR` override, every runtime
/// file below that directory, for runtime development only, as `StdSource::Dir` is for std (toolchain 1.6); the CLI
/// sets `Dir` from the environment variable when it is present (`spec/cabi.md` 2.2).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum RuntimeSource {
    #[default]
    Embedded,
    Dir(std::path::PathBuf),
}

// ---- the runtime files of a source ------------------------------------------------------------------------------------

/// The runtime files of `source` as (relative path with `/` separators, text), sorted by path: the embedded
/// [`RUNTIME_FILES`], or every `*.c` and `*.h` file below the override directory, read as `build.rs` reads `c/`
/// (`spec/cabi.md` 2.2). A file that is not UTF-8 is an [`io::ErrorKind::InvalidData`] error.
fn runtime_files(source: &RuntimeSource) -> io::Result<Vec<(String, String)>> {
    match source {
        RuntimeSource::Embedded => Ok(RUNTIME_FILES
            .iter()
            .map(|(path, text)| ((*path).to_string(), (*text).to_string()))
            .collect()),
        RuntimeSource::Dir(dir) => {
            let mut paths = Vec::new();
            collect_dir(dir, dir, &mut paths)?;
            paths.sort();
            let mut files = Vec::with_capacity(paths.len());
            for (relative, path) in paths {
                let bytes = std::fs::read(&path)?;
                let text = String::from_utf8(bytes).map_err(|e| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "{} is not valid UTF-8 (byte {}): runtime files are UTF-8 text",
                            path.display(),
                            e.utf8_error().valid_up_to()
                        ),
                    )
                })?;
                files.push((relative, text));
            }
            Ok(files)
        }
    }
}

/// Every `*.c` and `*.h` file below `dir` as (path relative to `root` with `/` separators, path).
fn collect_dir(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_dir(root, &path, out)?;
            continue;
        }
        let is_runtime_file = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e == "c" || e == "h");
        if !is_runtime_file || !path.is_file() {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|e| io::Error::other(e.to_string()))?;
        let mut parts = Vec::new();
        for component in relative.components() {
            let part = component.as_os_str().to_str().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{} is not a UTF-8 path", path.display()),
                )
            })?;
            parts.push(part.to_string());
        }
        out.push((parts.join("/"), path));
    }
    Ok(())
}

/// The last component of a `/`-separated relative path.
fn file_name(relative: &str) -> &str {
    relative.rsplit('/').next().unwrap_or(relative)
}

// ---- cpl_rt_gen.h ---------------------------------------------------------------------------------------------------------

/// `s` as the body of a C string literal: `\\`, `\"`, the C escapes of the controls, `?` before another `?` escaped (no
/// trigraph can form under `-std=c11`), every other printable ASCII byte as itself, and every other byte in octal, so
/// that the literal denotes exactly the UTF-8 bytes of `s`.
fn c_string_body(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    let bytes = s.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'"' => out.push_str("\\\""),
            b'\n' => out.push_str("\\n"),
            b'\t' => out.push_str("\\t"),
            b'\r' => out.push_str("\\r"),
            b'?' if bytes.get(i + 1) == Some(&b'?') => out.push_str("\\?"),
            0x20..=0x7e => out.push(char::from(b)),
            _ => {
                let _ = write!(out, "\\{b:03o}");
            }
        }
    }
    out
}

/// `s` as a C string literal.
fn c_string(s: &str) -> String {
    format!("\"{}\"", c_string_body(s))
}

/// `cpl_rt_gen.h`: the tables of cpl-rtspec formatted as C (the message templates, the White_Space ranges, the builtin
/// table, the limits, the interop codes, the `ForeignErrorKind` names and the detail order), with exactly the
/// declarations of `spec/cabi.md` Appendix B.2; written at emit time, never checked in.
pub fn generated_header() -> String {
    use cpl_rtspec::{builtins, foreign, interop, limits, messages, whitespace};
    let mut h = String::with_capacity(32 * 1024);
    h.push_str(
        "/* cpl_rt_gen.h: the tables of cpl-rtspec formatted as C (spec/cabi.md 2.9 and Appendix B.2). Generated at emit\n   \
         time by cpl_rt::generated_header; never edited or checked in. Included only by the runtime's units. */\n\
         #ifndef CPL_RT_GEN_H\n\
         #define CPL_RT_GEN_H 1\n\
         #include <stddef.h>\n\
         #include <stdint.h>\n\n",
    );

    h.push_str(
        "/* ---- message templates: one per entry of cpl_rtspec::messages::ALL, in its order, named CPLRT_MSG_ and the Rust\n   \
         constant's name, its text a C string literal holding the {name} placeholders of the Rust text unchanged -------- */\n",
    );
    for t in messages::ALL {
        let _ = writeln!(h, "#define CPLRT_MSG_{} {}", t.name, c_string(t.text));
    }

    h.push_str(
        "\n/* ---- White_Space (cpl_rtspec::whitespace::RANGES): the 25 scalars of stdlib.md S22 as inclusive ranges --------- */\n\
         typedef struct cplrt_range { uint32_t lo, hi; } cplrt_range;\n",
    );
    let _ = writeln!(
        h,
        "#define CPLRT_WHITE_SPACE_N {}u",
        whitespace::RANGES.len()
    );
    h.push_str("static const cplrt_range CPLRT_WHITE_SPACE[CPLRT_WHITE_SPACE_N] = {");
    for (i, (lo, hi)) in whitespace::RANGES.iter().enumerate() {
        if i % 5 == 0 {
            h.push_str("\n    ");
        } else {
            h.push(' ');
        }
        let _ = write!(h, "{{ {lo:#x}u, {hi:#x}u }}");
        if i + 1 < whitespace::RANGES.len() {
            h.push(',');
        }
    }
    h.push_str(" };\n");

    h.push_str(
        "\n/* ---- the builtin registry (cpl_rtspec::builtins::BUILTINS, sorted by id) ------------------------------------- */\n\
         typedef struct cplrt_requires_spec {            /* one precondition clause, the fallback of requires_spec */\n    \
         const char *clause;                         /* its canonical text */\n    \
         uint32_t ninputs;\n    \
         const char *const *inputs;                  /* the input names, \"self\" for a member's receiver */\n\
         } cplrt_requires_spec;\n\
         typedef struct cplrt_builtin_spec {\n    \
         const char *id;\n    \
         uint32_t callable;                          /* 0 for the 7 type ids and the 7 generator ids */\n    \
         uint32_t nrequires;\n    \
         const char *fallback_symbol;                /* the stub's qualified name, for a program that does not declare it */\n    \
         const cplrt_requires_spec *requires;        /* the precondition clauses in source order (stdlib.md 10.4) */\n\
         } cplrt_builtin_spec;\n",
    );
    // The clauses' input arrays and clause arrays first, each named after its id, so the table can point at them.
    for b in &builtins::BUILTINS {
        for (i, r) in b.requires.iter().enumerate() {
            let inputs: Vec<String> = r.inputs.iter().map(|s| c_string(s)).collect();
            let _ = writeln!(
                h,
                "static const char *const CPLRT_BUILTIN_INPUTS_{}_{i}[{}] = {{ {} }};",
                b.id,
                r.inputs.len(),
                inputs.join(", ")
            );
        }
        if !b.requires.is_empty() {
            let clauses: Vec<String> = b
                .requires
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    format!(
                        "{{ {}, {}u, CPLRT_BUILTIN_INPUTS_{}_{i} }}",
                        c_string(r.clause),
                        r.inputs.len(),
                        b.id
                    )
                })
                .collect();
            let _ = writeln!(
                h,
                "static const cplrt_requires_spec CPLRT_BUILTIN_REQUIRES_{}[{}] = {{ {} }};",
                b.id,
                b.requires.len(),
                clauses.join(", ")
            );
        }
    }
    let _ = writeln!(h, "#define CPLRT_BUILTINS_N {}u", builtins::BUILTINS.len());
    h.push_str("static const cplrt_builtin_spec CPLRT_BUILTINS[CPLRT_BUILTINS_N] = {\n");
    for (i, b) in builtins::BUILTINS.iter().enumerate() {
        let requires = if b.requires.is_empty() {
            "NULL".to_string()
        } else {
            format!("CPLRT_BUILTIN_REQUIRES_{}", b.id)
        };
        let _ = writeln!(
            h,
            "    {{ {}, {}u, {}u, {}, {requires} }}{}",
            c_string(b.id),
            u32::from(b.callable),
            b.requires.len(),
            c_string(b.fallback_symbol),
            if i + 1 < builtins::BUILTINS.len() {
                ","
            } else {
                ""
            }
        );
    }
    h.push_str("};\n");

    h.push_str(
        "\n/* ---- limits (cpl_rtspec::limits) --------------------------------------------------------------------------- */\n",
    );
    let _ = writeln!(
        h,
        "#define CPLRT_LIMIT_DEFAULT_MAX_CALL_DEPTH {}u     /* semantics S38 */",
        limits::DEFAULT_MAX_CALL_DEPTH
    );
    let _ = writeln!(
        h,
        "#define CPLRT_LIMIT_EVAL_STACK_SIZE {}u       /* the evaluation thread: 1 GiB */",
        limits::EVAL_STACK_SIZE
    );
    let _ = writeln!(
        h,
        "#define CPLRT_LIMIT_GUARD_MARGIN {}u            /* 64 MiB, or an eighth of a smaller stack */",
        limits::GUARD_MARGIN
    );
    let _ = writeln!(
        h,
        "#define CPLRT_LIMIT_API_MAJOR {}u                      /* C API 0.1 */",
        limits::C_API_VERSION.0
    );
    let _ = writeln!(
        h,
        "#define CPLRT_LIMIT_API_MINOR {}u",
        limits::C_API_VERSION.1
    );
    let _ = writeln!(
        h,
        "#define CPLRT_LIMIT_DRIVER_PROTOCOL {}u                /* cpl-driver protocol 1 */",
        limits::DRIVER_PROTOCOL
    );

    h.push_str(
        "\n/* ---- std.foreign (cpl_rtspec::foreign) ---------------------------------------------------------------------- */\n",
    );
    let _ = writeln!(
        h,
        "#define CPLRT_FOREIGN_KINDS_N {}u                       /* ForeignErrorKind in declaration order: the CPL_FOREIGN_* */",
        foreign::KINDS.len()
    );
    let kinds: Vec<String> = foreign::KINDS.iter().map(|k| c_string(k)).collect();
    let _ = writeln!(
        h,
        "static const char *const CPLRT_FOREIGN_KINDS[CPLRT_FOREIGN_KINDS_N] = {{\n    {} }};",
        kinds.join(", ")
    );
    let _ = writeln!(
        h,
        "#define CPLRT_FOREIGN_DETAIL_ORDER_N {}u                /* the fixed detail keys of stdlib.md S45, in their order */",
        foreign::DETAIL_ORDER.len()
    );
    let order: Vec<String> = foreign::DETAIL_ORDER.iter().map(|k| c_string(k)).collect();
    let _ = writeln!(
        h,
        "static const char *const CPLRT_FOREIGN_DETAIL_ORDER[CPLRT_FOREIGN_DETAIL_ORDER_N] = {{\n    {} }};",
        order.join(", ")
    );

    h.push_str(
        "\n/* ---- the interop codes (cpl_rtspec::interop::CODES, in the order of spec/cabi.md 4.9) ----------------------- */\n\
         typedef struct cplrt_interop_code { const char *code; int32_t status; } cplrt_interop_code;\n",
    );
    let _ = writeln!(h, "#define CPLRT_INTEROP_N {}u", interop::CODES.len());
    h.push_str("static const cplrt_interop_code CPLRT_INTEROP[CPLRT_INTEROP_N] = {\n");
    for (i, c) in interop::CODES.iter().enumerate() {
        let _ = writeln!(
            h,
            "    {{ {}, {} }}{}",
            c_string(c.code),
            c.status,
            if i + 1 < interop::CODES.len() {
                ","
            } else {
                ""
            }
        );
    }
    h.push_str("};\n");
    h.push_str(
        "/* the index of each code in CPLRT_INTEROP: CPLRT_INTEROP_ and the code without its interop_ prefix, in upper case */\n",
    );
    for (i, c) in interop::CODES.iter().enumerate() {
        let name = c.code.strip_prefix("interop_").unwrap_or(c.code);
        let _ = writeln!(
            h,
            "#define CPLRT_INTEROP_{} {i}u",
            name.to_ascii_uppercase()
        );
    }
    h.push_str("#endif\n");
    h
}

// ---- the amalgamation and the digest ----------------------------------------------------------------------------------

/// The namespace prologue of `spec/cabi.md` Appendix B.1 for `prefix`, behind `#define _GNU_SOURCE 1`: the runtime
/// prologue of every runtime unit (2.2).
fn runtime_prologue(prefix: &str) -> String {
    format!(
        "#define _GNU_SOURCE 1\n#define CPLRT_NS {prefix}\n#define CPLRT(name) cplrt_{prefix}__##name\n#define CPLPKG(name) cplpkg_{prefix}__##name\n"
    )
}

/// `text`, ending in a newline.
fn with_final_newline(text: &str) -> String {
    let mut s = text.to_string();
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// The runtime as a package's `src/` receives it (toolchain 7.2), assembled as `spec/cabi.md` 2.2 states: `cpl_rt.c`
/// (the amalgamation behind its prologue for `prefix`), the runtime's headers, `cpl_rt_gen.h`, and the two mains,
/// `cpl_rt_exe.c` (written with the executable artifact) and `cpl_rt_driver.c` (drivers only), as (relative path, text)
/// pairs.
///
/// The paths are relative to the package's output root, as `cpl_emit_c`'s emitted files are (`spec/cabi.md` 1.6):
/// `src/cpl_rt.c`; `src/<header>` for every runtime header outside `include/` (`cpl_rt.h`, `cpl_rt_internal.h`,
/// `cpl_rt_platform.h`); `src/cpl_rt_gen.h`; `include/cpl.h` and `include/cpl_adapter.h`; and `src/cpl_rt_driver.c`
/// and `src/cpl_rt_exe.c`, each behind the same runtime prologue, when the runtime has them. The pairs are sorted by
/// path. `cpl_rt.c` is `#define _GNU_SOURCE 1`, the namespace prologue, `#include "cpl_rt.h"`, then the text of every
/// runtime `.c` file but the two mains in sorted path order, each preceded by `#line 1 "<file name>"`.
pub fn amalgamation(
    prefix: &str,
    source: &RuntimeSource,
) -> std::io::Result<Vec<(String, String)>> {
    debug_assert!(
        !prefix.is_empty()
            && prefix
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_'),
        "a package prefix is a C identifier fragment (toolchain 11.1)"
    );
    let files = runtime_files(source)?;
    let prologue = runtime_prologue(prefix);
    let mut out = Vec::new();
    let mut unit = prologue.clone();
    unit.push_str("#include \"cpl_rt.h\"\n");
    for (relative, text) in &files {
        let name = file_name(relative);
        if relative.ends_with(".h") {
            let target = if relative.starts_with("include/") {
                relative.clone()
            } else {
                format!("src/{relative}")
            };
            out.push((target, text.clone()));
        } else if MAINS.contains(&name) {
            let mut main = prologue.clone();
            let _ = writeln!(main, "#line 1 \"{name}\"");
            main.push_str(&with_final_newline(text));
            out.push((format!("src/{name}"), main));
        } else {
            let _ = writeln!(unit, "#line 1 \"{name}\"");
            unit.push_str(&with_final_newline(text));
        }
    }
    out.push(("src/cpl_rt.c".to_string(), unit));
    out.push((format!("src/{GENERATED_HEADER}"), generated_header()));
    out.sort();
    Ok(out)
}

/// blake3 over `path LF content LF` for every runtime file in sorted path order and then `cpl_rt_gen.h LF` and the
/// generated header with its LF: the construction of `std_digest` (toolchain 3.6). Recorded in `build.json`
/// (`runtime.digest`), `cpl.lock` (`targets.c.runtime_digest`), the package's library information, and the banner of
/// every generated header (`spec/cabi.md` Section 5). With `CPL_RUNTIME_DIR` it covers the files used, and
/// `BuildOutput`'s `runtime.source` is `"dir"`.
pub fn runtime_digest(source: &RuntimeSource) -> std::io::Result<cpl_diag::json::Digest> {
    let files = runtime_files(source)?;
    let mut hasher = blake3::Hasher::new();
    for (path, content) in &files {
        hasher.update(path.as_bytes());
        hasher.update(b"\n");
        hasher.update(content.as_bytes());
        hasher.update(b"\n");
    }
    hasher.update(GENERATED_HEADER.as_bytes());
    hasher.update(b"\n");
    hasher.update(generated_header().as_bytes());
    hasher.update(b"\n");
    Ok(cpl_diag::json::Digest(
        hasher.finalize().to_hex().to_string(),
    ))
}

/// The fixed flag sets of `spec/cabi.md` 1.5 as data, the one Rust copy of them: `cpl_emit_c::cc` builds every compile
/// and link command from them, and the runtime's own C tests compile with them. The flags that name a path or a unit
/// are the caller's: `-ffile-prefix-map=<out>=.` and `-I<out>/include` (in the base set, after `-fdata-sections`),
/// gcc's `-frandom-seed=<unit digest>`, `-MD -MF <file>`, and the shared link's `-Wl,-soname,lib<prefix>.so.<abi>`
/// and `-Wl,--version-script=<prefix>.map`.
pub mod flags {
    /// The runtime and emitted code, every profile (`spec/cabi.md` 1.5, "base").
    pub const BASE: &[&str] = &[
        "-std=c11",
        "-fPIC",
        "-fvisibility=hidden",
        "-fno-strict-aliasing",
        "-fno-common",
        "-ffunction-sections",
        "-fdata-sections",
        "-Wall",
        "-Wextra",
        "-Wno-unused-parameter",
        "-Wno-unused-label",
        "-Wno-unused-variable",
        "-Wno-unused-function",
    ];
    /// The profile `optimized`, the default of `cpl build`.
    pub const OPTIMIZED: &[&str] = &["-O2", "-DNDEBUG"];
    /// The profile `debug`, the default of drivers (invariant checks on every handle and object header access).
    pub const DEBUG: &[&str] = &["-O0", "-g3", "-DCPLRT_DEBUG=1"];
    /// `--sanitize address,undefined`, at compile and at link time.
    pub const SANITIZE_ADDRESS_UNDEFINED: &[&str] = &[
        "-O1",
        "-g",
        "-fno-omit-frame-pointer",
        "-fsanitize=address,undefined",
        "-fno-sanitize-recover=all",
    ];
    /// `--sanitize thread`, at compile and at link time; run under `setarch $(uname -m) -R` on Linux.
    pub const SANITIZE_THREAD: &[&str] = &["-O1", "-g", "-fsanitize=thread"];
    /// Added to the adapter set (the base set without its four `-Wno-*` flags) for a declarative binding's generated
    /// wrapper, whose `include_dirs` are passed as `-isystem` (`spec/cabi.md` 1.5, 6.3).
    pub const GENERATED_WRAPPER: &[&str] = &[
        "-Werror=conversion",
        "-Werror=sign-conversion",
        "-Werror=incompatible-pointer-types",
    ];
    /// The shared link's fixed flags; the caller adds `-Wl,-soname,...` and `-Wl,--version-script=...`, then the
    /// bindings' `-L` and `-l` options, `[interop.c] ldflags`, and `-lpthread` last, and leaves `-Wl,-z,defs` out
    /// for clang with a sanitizer (`spec/cabi.md` 1.5).
    pub const SHARED_LINK: &[&str] = &[
        "-shared",
        "-Wl,-z,defs",
        "-Wl,-z,now",
        "-Wl,--gc-sections",
        "-Wl,-Bsymbolic",
        "-Wl,--trace",
    ];
    /// The toolchain tests' addition (toolchain 12.8): a warning in emitted code or in the runtime is a toolchain
    /// defect. User builds never use it.
    pub const WERROR: &str = "-Werror";
}

/// Doc-hidden, for the tests of cpl-rt, which cannot reach `cpl_emit_c::cc` or `cpl_emit_c::package` (toolchain 1.2):
/// `find_cc` is the compiler of T72's precedence without `--cc` and a manifest (`CPL_CC`, else `cc` on `PATH`), and
/// `compile_and_run` compiles the given C files with `flags` in T72's scrubbed environment and runs the result.
/// `crates/cpl-rt/tests/c/program_stub.c`, a hand-written minimal program table (`CPLPKG(program)` and
/// `CPLPKG(code)` of the prefix `rttest`, holding the std declarations the runtime constructs), is linked into every
/// C unit test.
#[doc(hidden)]
pub mod testing {
    use std::io;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};

    /// The variables T72 removes from the environment of every compiler and linker process (toolchain 7.2).
    pub const SCRUBBED: &[&str] = &[
        "CPATH",
        "C_INCLUDE_PATH",
        "CPLUS_INCLUDE_PATH",
        "LIBRARY_PATH",
        "COMPILER_PATH",
        "GCC_EXEC_PREFIX",
        "DEPENDENCIES_OUTPUT",
        "SUNPRO_DEPENDENCIES",
        "CCC_OVERRIDE_OPTIONS",
    ];

    /// The sanitizer options of the D8 suite (toolchain 12.6), set for every program `compile_and_run` runs; they
    /// change nothing for a program built without sanitizers.
    pub const ASAN_OPTIONS: &str = "detect_leaks=1:halt_on_error=1";
    pub const UBSAN_OPTIONS: &str = "halt_on_error=1:print_stacktrace=1";

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

    /// The compiler of T72's precedence without `--cc` and a manifest: `CPL_CC`, else `cc` on `PATH`; `$CC` is never
    /// read. `None` when neither names an existing file.
    pub fn find_cc() -> Option<PathBuf> {
        match std::env::var("CPL_CC") {
            Ok(cc) if !cc.is_empty() => resolve(&cc),
            _ => resolve("cc"),
        }
    }

    /// `cc flags... sources... -o output -lpthread` in T72's scrubbed environment, in the directory of `output`; a
    /// source may also be an object file. The compiler's output is returned whatever its status. (An addition to
    /// toolchain 1.12, so that a harness compiles the runtime once and links it into many tests.)
    pub fn compile(
        cc: &Path,
        sources: &[PathBuf],
        flags: &[&str],
        output: &Path,
    ) -> io::Result<Output> {
        let mut command = Command::new(cc);
        command.args(flags).args(sources).arg("-o").arg(output);
        if !flags.contains(&"-c") {
            command.arg("-lpthread");
        }
        if let Some(dir) = output.parent().filter(|d| !d.as_os_str().is_empty()) {
            command.current_dir(dir);
        }
        for variable in SCRUBBED {
            command.env_remove(variable);
        }
        command.output()
    }

    /// Compiles and links the given C files (and objects) with `flags` in T72's scrubbed environment, runs the
    /// program with [`ASAN_OPTIONS`] and [`UBSAN_OPTIONS`] set, deletes it, and returns the program's output. The
    /// program is written beside the first source under a name derived from the sources and flags. A compile or
    /// link failure is an [`io::ErrorKind::Other`] error carrying the compiler's diagnostics.
    pub fn compile_and_run(
        cc: &Path,
        sources: &[PathBuf],
        flags: &[&str],
    ) -> std::io::Result<std::process::Output> {
        let first = sources.first().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "compile_and_run needs a source",
            )
        })?;
        let mut key = blake3::Hasher::new();
        for source in sources {
            key.update(source.as_os_str().as_encoded_bytes());
            key.update(b"\n");
        }
        for flag in flags {
            key.update(flag.as_bytes());
            key.update(b"\n");
        }
        let stem = first
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("program");
        let hex = key.finalize().to_hex();
        let program = first.with_file_name(format!("{stem}-{}", &hex.as_str()[..16]));
        let built = compile(cc, sources, flags, &program)?;
        if !built.status.success() {
            return Err(io::Error::other(format!(
                "{} failed ({}):\n{}{}",
                cc.display(),
                built.status,
                String::from_utf8_lossy(&built.stdout),
                String::from_utf8_lossy(&built.stderr)
            )));
        }
        let run = Command::new(&program)
            .current_dir(program.parent().unwrap_or(Path::new(".")))
            .env("ASAN_OPTIONS", ASAN_OPTIONS)
            .env("UBSAN_OPTIONS", UBSAN_OPTIONS)
            .output();
        let _ = std::fs::remove_file(&program);
        run
    }
}
