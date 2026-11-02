//! The frozen C contracts of `crates/cpl-rt/c/` against `spec/cabi.md` Appendices A and B.1 (toolchain 12.8; the
//! checklist item 6 of 12.9: the headers pass C11 and C++17).
//!
//! - The embedded runtime files are exactly the `*.c` and `*.h` files under `c/`, in path order (cabi.md 2.2).
//! - `cpl.h` and `cpl_adapter.h` compile as C11 and as C++17 with the flags of Appendix A.1, also under
//!   `#pragma pack(1)`, where their layout assertions must still hold (cabi.md 1.7).
//! - A host that uses every macro and every `static inline` wrapper of both headers, against a fake function table
//!   that records each call, compiles as C11 and as C++17 and runs: every wrapper reaches the entry of its own name
//!   with its arguments in order (cabi.md 4.1), and the adapter helpers behave as Appendix A.2 says.
//! - `cpl_rt.h` compiles as C11 behind the namespace prologue of Appendix B.1 (also with the base flags of cabi.md 1.5
//!   and a GNU-mode prefix such as `linux`), refuses C++, and refuses a unit without the prologue; `cpl_rt_internal.h`
//!   compiles behind the runtime prologue of cabi.md 2.2.
//! - `api_v0.txt` is the entry order of `struct cpl_api` (cabi.md 4.1), checked in Rust and by offsets in C.
//!
//! A test that needs a compiler finds it as `cpl_rt::testing::find_cc` does (`CPL_CC`, else `cc` on `PATH`; T72
//! without `--cc` and a manifest), runs it in T72's scrubbed environment through `std::process::Command`, writes
//! everything under `CARGO_TARGET_TMPDIR`, and without a compiler prints the skip reason of toolchain 12.8 and passes,
//! unless `CPL_REQUIRE_CC=1`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const NO_CC: &str = "skipped: no C compiler (set CPL_REQUIRE_CC=1 to fail)";
const NO_CXX: &str = "skipped: no C++ compiler (set CPL_REQUIRE_CC=1 to fail)";

/// The flags under which Appendix A.1 says `cpl.h` compiles as C11 ...
const C11: &[&str] = &[
    "-std=c11",
    "-Wall",
    "-Wextra",
    "-Wconversion",
    "-Wsign-conversion",
    "-Werror",
    "-pedantic",
];
/// ... and as C++17.
const CXX17: &[&str] = &[
    "-std=c++17",
    "-Wall",
    "-Wextra",
    "-Wconversion",
    "-Wsign-conversion",
    "-Wold-style-cast",
    "-Werror",
    "-pedantic",
];
/// The flags under which Appendix B.1 says `cpl_rt.h` compiles.
const INTERNAL_C11: &[&str] = &["-std=c11", "-Wall", "-Wextra", "-Werror", "-pedantic"];

/// The variables T72 removes from the environment of every compiler process.
const SCRUBBED: &[&str] = &[
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

/// The namespace prologue of Appendix B.1 for the prefix `p`.
fn prologue(p: &str) -> String {
    format!("#define CPLRT_NS {p}\n#define CPLRT(name) cplrt_{p}__##name\n#define CPLPKG(name) cplpkg_{p}__##name\n")
}

// ---- finding the compilers -----------------------------------------------------------------------------------------

fn required() -> bool {
    std::env::var("CPL_REQUIRE_CC").is_ok_and(|v| v == "1")
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

/// The compiler of T72's precedence without `--cc` and a manifest: `CPL_CC`, else `cc` on `PATH`.
fn find_cc() -> Option<PathBuf> {
    match std::env::var("CPL_CC") {
        Ok(cc) if !cc.is_empty() => resolve(&cc),
        _ => resolve("cc"),
    }
}

/// The C++ driver of the same installation: `g++` for `gcc`, `clang++` for `clang`, `c++` for `cc` (a target prefix
/// and a version suffix kept, `x86_64-linux-gnu-gcc-11` giving `x86_64-linux-gnu-g++-11`), beside the C compiler or
/// on `PATH`; else `c++` or `g++` on `PATH`.
fn find_cxx(cc: &Path) -> Option<PathBuf> {
    let name = cc.file_name()?.to_str()?;
    let mapped = [("clang", "clang++"), ("gcc", "g++"), ("cc", "c++")]
        .iter()
        .find_map(|(c, cxx)| {
            name.rfind(c)
                .map(|at| format!("{}{cxx}{}", &name[..at], &name[at + c.len()..]))
        });
    let beside = mapped.as_ref().and_then(|m| {
        let candidate = cc.with_file_name(m);
        candidate.is_file().then_some(candidate)
    });
    beside
        .or_else(|| mapped.as_deref().and_then(on_path))
        .or_else(|| on_path("c++"))
        .or_else(|| on_path("g++"))
}

fn cc_or_skip() -> Option<PathBuf> {
    match find_cc() {
        Some(cc) => Some(cc),
        None if required() => {
            panic!("CPL_REQUIRE_CC=1, but there is no C compiler (CPL_CC, or cc on PATH)")
        }
        None => {
            println!("{NO_CC}");
            None
        }
    }
}

fn cxx_or_skip(cc: &Path) -> Option<PathBuf> {
    match find_cxx(cc) {
        Some(cxx) => Some(cxx),
        None if required() => panic!(
            "CPL_REQUIRE_CC=1, but there is no C++ compiler beside {}",
            cc.display()
        ),
        None => {
            println!("{NO_CXX}");
            None
        }
    }
}

// ---- running them ----------------------------------------------------------------------------------------------------

/// A fresh directory for one test under `CARGO_TARGET_TMPDIR`, holding the embedded runtime files at their paths
/// (`cpl_rt.h`, `include/cpl.h`, ...), as a package's `src/` and `include/` would hold them.
fn workdir(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("headers")
        .join(test);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("the old test directory can be removed");
    }
    for (relative, text) in cpl_rt::RUNTIME_FILES {
        let path = dir.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).expect("the test directory can be created");
        std::fs::write(&path, text).expect("a runtime file can be written");
    }
    std::fs::create_dir_all(&dir).expect("the test directory can be created");
    dir
}

fn write(dir: &Path, name: &str, text: &str) {
    std::fs::write(dir.join(name), text).expect("a test source can be written");
}

/// `compiler flags... args...` in `dir`, in T72's scrubbed environment.
fn compile(compiler: &Path, flags: &[&str], args: &[&str], dir: &Path) -> Output {
    let mut command = Command::new(compiler);
    command.args(flags).args(args).current_dir(dir);
    for variable in SCRUBBED {
        command.env_remove(variable);
    }
    command
        .output()
        .unwrap_or_else(|e| panic!("cannot run {}: {e}", compiler.display()))
}

fn assert_ok(out: &Output, what: &str) {
    assert!(
        out.status.success(),
        "{what} failed ({}):\n{}{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn assert_refused(out: &Output, what: &str, message: &str) {
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "{what} compiled, but must be refused"
    );
    assert!(
        stderr.contains(message),
        "{what} must be refused with `{message}`, not:\n{stderr}"
    );
}

const INCLUDES: &[&str] = &["-I", ".", "-I", "include"];

fn args<'a>(extra: &[&'a str]) -> Vec<&'a str> {
    let mut all: Vec<&str> = INCLUDES.to_vec();
    all.extend_from_slice(extra);
    all
}

// ---- the embedded runtime files ------------------------------------------------------------------------------------

fn c_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("c")
}

fn runtime_files_on_disk(dir: &Path, root: &Path, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).expect("c/ is readable") {
        let path = entry.expect("an entry of c/").path();
        if path.is_dir() {
            runtime_files_on_disk(&path, root, out);
        } else if path.extension().is_some_and(|e| e == "c" || e == "h") {
            let relative = path.strip_prefix(root).unwrap();
            let parts: Vec<&str> = relative
                .components()
                .map(|c| c.as_os_str().to_str().unwrap())
                .collect();
            out.push(parts.join("/"));
        }
    }
}

#[test]
fn the_runtime_files_are_every_c_and_h_file_of_c_in_path_order() {
    let mut expected = Vec::new();
    runtime_files_on_disk(&c_dir(), &c_dir(), &mut expected);
    expected.sort();
    let embedded: Vec<&str> = cpl_rt::RUNTIME_FILES.iter().map(|(p, _)| *p).collect();
    assert_eq!(embedded, expected);
    for frozen in [
        "cpl_rt.h",
        "cpl_rt_internal.h",
        "include/cpl.h",
        "include/cpl_adapter.h",
    ] {
        assert!(embedded.contains(&frozen), "{frozen} is a runtime file");
    }
    for (relative, text) in cpl_rt::RUNTIME_FILES {
        let on_disk =
            std::fs::read_to_string(c_dir().join(relative)).expect("a runtime file is readable");
        assert_eq!(*text, on_disk, "the embedded {relative} is the file");
    }
}

// ---- the public headers --------------------------------------------------------------------------------------------

#[test]
fn the_public_headers_compile_as_c11_and_cxx17() {
    let Some(cc) = cc_or_skip() else { return };
    let cxx = cxx_or_skip(&cc);
    let dir = workdir("public");
    for (name, text) in [
        ("cpl_h", "#include \"cpl.h\"\nint main(void) { return 0; }\n"),
        ("cpl_adapter_h", "#include \"cpl_adapter.h\"\nint main(void) { return 0; }\n"),
        ("both", "#include \"cpl_adapter.h\"\n#include \"cpl.h\"\n#include \"cpl_adapter.h\"\nint main(void) { return 0; }\n"),
        // No ABI struct has implicit padding, so the layout assertions hold under #pragma pack(1) (cabi.md 1.7).
        (
            "packed",
            "#pragma pack(push, 1)\n#include \"cpl_adapter.h\"\n#pragma pack(pop)\nint main(void) { return 0; }\n",
        ),
    ] {
        let c = format!("{name}.c");
        write(&dir, &c, text);
        assert_ok(&compile(&cc, C11, &args(&["-fsyntax-only", &c]), &dir), &format!("C11 {c}"));
        if let Some(cxx) = &cxx {
            let cpp = format!("{name}.cpp");
            write(&dir, &cpp, text);
            assert_ok(&compile(cxx, CXX17, &args(&["-fsyntax-only", &cpp]), &dir), &format!("C++17 {cpp}"));
        }
    }
}

#[test]
fn a_host_using_every_macro_and_wrapper_compiles_and_runs_as_c11_and_cxx17() {
    let Some(cc) = cc_or_skip() else { return };
    let cxx = cxx_or_skip(&cc);
    let dir = workdir("host");
    write(&dir, "host.c", HOST);
    let mut builds = vec![(cc.clone(), C11, "host.c", "host_c")];
    if let Some(cxx) = &cxx {
        write(&dir, "host.cpp", HOST);
        builds.push((cxx.clone(), CXX17, "host.cpp", "host_cpp"));
    }
    for (compiler, flags, source, binary) in builds {
        assert_ok(
            &compile(&compiler, flags, &args(&[source, "-o", binary]), &dir),
            &format!("building {source}"),
        );
        let run = Command::new(dir.join(binary))
            .output()
            .expect("the host runs");
        assert_ok(&run, binary);
        assert!(
            String::from_utf8_lossy(&run.stdout).starts_with("host ok"),
            "{binary} reports success"
        );
    }
}

// ---- the internal headers ------------------------------------------------------------------------------------------

#[test]
fn cpl_rt_h_compiles_behind_the_prologue_and_refuses_cxx_and_a_unit_without_it() {
    let Some(cc) = cc_or_skip() else { return };
    let dir = workdir("internal");
    let unit = |p: &str| {
        format!(
            "{}#include \"cpl_rt.h\"\nint main(void) {{ return 0; }}\n",
            prologue(p)
        )
    };
    write(&dir, "rt.c", &unit("rttest"));
    assert_ok(
        &compile(&cc, INTERNAL_C11, &args(&["-fsyntax-only", "rt.c"]), &dir),
        "cpl_rt.h, C11",
    );
    // The base flags of emitted code (cabi.md 1.5) with -Werror -pedantic, compiled to an object.
    let mut base: Vec<&str> = cpl_rt::flags::BASE.to_vec();
    base.extend([cpl_rt::flags::WERROR, "-pedantic"]);
    assert_ok(
        &compile(&cc, &base, &args(&["-c", "rt.c", "-o", "rt.o"]), &dir),
        "cpl_rt.h, base flags",
    );
    // A prefix that GNU modes predefine as a macro cannot reach the namespaced names (Appendix B.1, cabi.md 1.6).
    write(&dir, "rt_linux.c", &unit("linux"));
    let gnu: Vec<&str> = ["-std=gnu11", "-Wall", "-Wextra", "-Werror", "-pedantic"].to_vec();
    assert_ok(
        &compile(&cc, &gnu, &args(&["-fsyntax-only", "rt_linux.c"]), &dir),
        "cpl_rt.h, prefix linux",
    );
    write(
        &dir,
        "no_prologue.c",
        "#include \"cpl_rt.h\"\nint main(void) { return 0; }\n",
    );
    assert_refused(
        &compile(
            &cc,
            INTERNAL_C11,
            &args(&["-fsyntax-only", "no_prologue.c"]),
            &dir,
        ),
        "cpl_rt.h without the prologue",
        "begin the unit with the namespace prologue",
    );
    if let Some(cxx) = find_cxx(&cc) {
        write(&dir, "rt.cpp", &unit("rttest"));
        assert_refused(
            &compile(
                &cxx,
                &["-std=c++17"],
                &args(&["-fsyntax-only", "rt.cpp"]),
                &dir,
            ),
            "cpl_rt.h as C++",
            "cpl_rt.h is C11",
        );
    }
}

#[test]
fn cpl_rt_internal_h_compiles_behind_the_runtime_prologue() {
    let Some(cc) = cc_or_skip() else { return };
    let dir = workdir("runtime_internal");
    // A runtime unit: `#define _GNU_SOURCE 1`, the namespace prologue, then the headers (cabi.md 2.2).
    let unit = format!(
        "#define _GNU_SOURCE 1\n{}#include \"cpl_rt.h\"\n#include \"cpl_rt_internal.h\"\nint main(void) {{ return 0; }}\n",
        prologue("rttest")
    );
    write(&dir, "unit.c", &unit);
    assert_ok(
        &compile(&cc, INTERNAL_C11, &args(&["-fsyntax-only", "unit.c"]), &dir),
        "cpl_rt_internal.h, C11",
    );
    let mut base: Vec<&str> = cpl_rt::flags::BASE.to_vec();
    base.extend([cpl_rt::flags::WERROR, "-pedantic"]);
    assert_ok(
        &compile(&cc, &base, &args(&["-c", "unit.c", "-o", "unit.o"]), &dir),
        "cpl_rt_internal.h, base flags",
    );
    // It includes cpl_rt.h itself, so a unit may include it alone.
    let alone = format!(
        "#define _GNU_SOURCE 1\n{}#include \"cpl_rt_internal.h\"\nint main(void) {{ return 0; }}\n",
        prologue("rttest")
    );
    write(&dir, "alone.c", &alone);
    assert_ok(
        &compile(
            &cc,
            INTERNAL_C11,
            &args(&["-fsyntax-only", "alone.c"]),
            &dir,
        ),
        "cpl_rt_internal.h alone",
    );
}

// ---- api_v0.txt ------------------------------------------------------------------------------------------------------

/// The member names of `struct cpl_api` after `struct_size` and `api_version`, in order.
fn api_members() -> Vec<String> {
    let header = std::fs::read_to_string(c_dir().join("include/cpl.h")).expect("cpl.h is readable");
    let start = header
        .find("struct cpl_api {")
        .expect("cpl.h defines struct cpl_api");
    let body = &header[start..start + header[start..].find("};").expect("the struct ends")];
    body.split("(*")
        .skip(1)
        .map(|piece| {
            piece[..piece.find(')').expect("a member's name")]
                .trim()
                .to_string()
        })
        .collect()
}

/// The entries of `api_v0.txt` as (major, minor, member).
fn api_v0() -> Vec<(u32, u32, String)> {
    let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("api_v0.txt"))
        .expect("crates/cpl-rt/api_v0.txt is readable");
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let mut words = l.split_whitespace();
            let version = words.next().expect("a version");
            let member = words.next().expect("a member").to_string();
            assert!(words.next().is_none(), "one entry per line: {l}");
            let (major, minor) = version.split_once('.').expect("<major>.<minor>");
            (major.parse().unwrap(), minor.parse().unwrap(), member)
        })
        .collect()
}

fn header_define(name: &str) -> u32 {
    let header = std::fs::read_to_string(c_dir().join("include/cpl.h")).expect("cpl.h is readable");
    header
        .lines()
        .find_map(|l| l.strip_prefix(&format!("#define {name} ")))
        .and_then(|rest| rest.split_whitespace().next())
        .map(|v| v.trim_end_matches('u').parse().expect("a number"))
        .unwrap_or_else(|| panic!("cpl.h defines {name}"))
}

#[test]
fn api_v0_txt_is_the_entry_order_of_struct_cpl_api() {
    let entries = api_v0();
    let members = api_members();
    let names: Vec<&str> = entries.iter().map(|(_, _, m)| m.as_str()).collect();
    assert_eq!(
        names,
        members.iter().map(String::as_str).collect::<Vec<_>>(),
        "api_v0.txt against cpl.h"
    );
    assert_eq!(entries.len(), 63, "the 63 entries of C API 0.1");
    let (major, minor) = (
        header_define("CPL_API_VERSION_MAJOR"),
        header_define("CPL_API_VERSION_MINOR"),
    );
    let mut previous = 0;
    for (m, n, member) in &entries {
        assert_eq!(*m, major, "{member}: an entry of another major version");
        assert!(
            *n <= minor,
            "{member} was added in 0.{n}, after the header's 0.{minor}"
        );
        assert!(
            *n >= previous,
            "{member}: entries are appended in version order"
        );
        previous = *n;
    }
    // The size assertion of cpl.h counts the same entries.
    let header = std::fs::read_to_string(c_dir().join("include/cpl.h")).unwrap();
    assert!(header.contains(&format!(
        "CPL_LAYOUT_ASSERT(sizeof(struct cpl_api) == 8 + 8 * {})",
        entries.len()
    )));

    let Some(cc) = cc_or_skip() else { return };
    let dir = workdir("api_v0");
    let mut unit = String::from("#include <stddef.h>\n#include \"cpl.h\"\n");
    for (i, (_, _, member)) in entries.iter().enumerate() {
        unit.push_str(&format!(
            "_Static_assert(offsetof(struct cpl_api, {member}) == 8u + 8u * {i}u, \"api_v0.txt: {member} is entry {i}\");\n"
        ));
    }
    unit.push_str(&format!(
        "_Static_assert(sizeof(struct cpl_api) == 8u + 8u * {}u, \"api_v0.txt lists every entry\");\n",
        entries.len()
    ));
    unit.push_str("int main(void) { return 0; }\n");
    write(&dir, "offsets.c", &unit);
    assert_ok(
        &compile(&cc, C11, &args(&["-fsyntax-only", "offsets.c"]), &dir),
        "the offsets of api_v0.txt",
    );
}

/// The host of `a_host_using_every_macro_and_wrapper_compiles_and_runs_as_c11_and_cxx17`, valid C11 and C++17.
const HOST: &str = r##"/* A host of the CPL C API that uses every macro and every static inline wrapper of cpl.h and cpl_adapter.h
   (spec/cabi.md Appendix A) against a fake function table that records each call and its arguments, so that a
   wrapper that reached another entry, or passed its arguments in another order, fails. It is built as C11 and as
   C++17 with the flags of Appendix A.1 and run by crates/cpl-rt/tests/headers.rs. */
#include <stdio.h>
#include <string.h>
#include "cpl_adapter.h"
#include "cpl.h" /* included twice: the guard holds */

#if CPL_API_VERSION_MAJOR != 0 || CPL_API_VERSION_MINOR < 1
#error "the version macros are usable in #if"
#endif
#if CPL_API_VERSION != ((CPL_API_VERSION_MAJOR << 16) | CPL_API_VERSION_MINOR) || CPL_API_VERSION != 1u
#error "CPL_API_VERSION is major << 16 | minor: 0.1"
#endif

#ifdef __cplusplus
#define U32(x) static_cast<uint32_t>(x)
#else
#define U32(x) ((uint32_t)(x))
#endif
#define CHECK(e) \
    do { if (!(e)) { fprintf(stderr, "host check failed at line %d: %s\n", __LINE__, #e); return 1; } } while (0)

/* CPL_LAYOUT_ASSERT on a host struct; it holds on every target (and is empty off LP64) */
typedef struct host_pair { uint64_t a, b; } host_pair;
CPL_LAYOUT_ASSERT(sizeof(host_pair) == 16)

/* CPL_ADAPTER on a function of the host */
CPL_ADAPTER int host_hidden(void);
int host_hidden(void) { return 3; }

#define NENTRIES 63
#define E_cpl_runtime_close 0
#define E_cpl_runtime_info 1
#define E_cpl_runtime_stats 2
#define E_cpl_type_lookup 3
#define E_cpl_type_name 4
#define E_cpl_value_dup 5
#define E_cpl_value_release 6
#define E_cpl_value_kind 7
#define E_cpl_value_type 8
#define E_cpl_value_equal 9
#define E_cpl_value_to_json 10
#define E_cpl_value_from_json 11
#define E_cpl_value_to_text 12
#define E_cpl_buf_release 13
#define E_cpl_int_from_i64 14
#define E_cpl_int_from_u64 15
#define E_cpl_int_from_decimal 16
#define E_cpl_int_from_words 17
#define E_cpl_int_to_i64 18
#define E_cpl_int_to_u64 19
#define E_cpl_int_to_decimal 20
#define E_cpl_int_to_words 21
#define E_cpl_bool_new 22
#define E_cpl_bool_get 23
#define E_cpl_text_new 24
#define E_cpl_text_view 25
#define E_cpl_text_scalars 26
#define E_cpl_bytes_new 27
#define E_cpl_bytes_view 28
#define E_cpl_list_new 29
#define E_cpl_list_len 30
#define E_cpl_list_get 31
#define E_cpl_list_from_i64 32
#define E_cpl_list_to_i64 33
#define E_cpl_struct_new 34
#define E_cpl_struct_field 35
#define E_cpl_enum_new 36
#define E_cpl_enum_variant 37
#define E_cpl_enum_payload 38
#define E_cpl_wrapper_new 39
#define E_cpl_wrapper_inner 40
#define E_cpl_unit 41
#define E_cpl_option_some 42
#define E_cpl_option_none 43
#define E_cpl_result_ok 44
#define E_cpl_result_err 45
#define E_cpl_foreign_error_new 46
#define E_cpl_cancel_new 47
#define E_cpl_cancel_request 48
#define E_cpl_cancel_release 49
#define E_cpl_cap_new 50
#define E_cpl_cap_release 51
#define E_cpl_call_runtime 52
#define E_cpl_call_operation 53
#define E_cpl_call_cancelled 54
#define E_cpl_call_foreign_detail 55
#define E_cpl_call_foreign_error 56
#define E_cpl_error_status 57
#define E_cpl_error_code 58
#define E_cpl_error_category 59
#define E_cpl_error_message 60
#define E_cpl_error_json 61
#define E_cpl_error_release 62

#ifdef __cplusplus
extern "C" {
#endif

/* ---- the record of the fake table's calls ---- */
static int hits[NENTRIES];
static cpl_status fake_status[NENTRIES];
typedef struct rec { int kind; uint64_t u; int64_t i; const void *p; } rec;
static rec recs[16];
static int nrecs;
static void reset(void) { memset(hits, 0, sizeof hits); nrecs = 0; }
static void hit(int i) { hits[i]++; }
static void put_u(uint64_t u) { if (nrecs < 16) { recs[nrecs].kind = 1; recs[nrecs].u = u; } nrecs++; }
static void put_i(int64_t i) { if (nrecs < 16) { recs[nrecs].kind = 2; recs[nrecs].i = i; } nrecs++; }
static void put_p(const void *p) { if (nrecs < 16) { recs[nrecs].kind = 3; recs[nrecs].p = p; } nrecs++; }
static void put_h(const uint64_t bits[2]) { put_u(bits[0]); put_u(bits[1]); }
static int U(int k, uint64_t u) { return k < 16 && recs[k].kind == 1 && recs[k].u == u; }
static int I(int k, int64_t i) { return k < 16 && recs[k].kind == 2 && recs[k].i == i; }
static int P(int k, const void *p) { return k < 16 && recs[k].kind == 3 && recs[k].p == p; }
static int H(int k, uint64_t lo, uint64_t hi) { return U(k, lo) && U(k + 1, hi); }
/* exactly entry i was called, once (i == -1: none was) */
static int only(int i)
{
    int n;
    for (n = 0; n < NENTRIES; n++)
        if (hits[n] != (n == i ? 1 : 0))
            return 0;
    return 1;
}

static struct cpl_api table;
static cpl_runtime rt;
static cpl_call call;
static cpl_error err;
static cpl_library_info fake_info;
static const cpl_strview fake_view = { "view", 4 };

/* ---- the fake entries: each records its arguments and returns fake_status[its index] ---- */
static cpl_status fake_cpl_runtime_close(cpl_runtime *rt, uint32_t flags, cpl_error **error)
{ hit(0); put_p(rt); put_u(flags); put_p(error); return fake_status[0]; }
static const cpl_library_info *fake_cpl_runtime_info(cpl_runtime *rt)
{ hit(1); put_p(rt); return &fake_info; }
static void fake_cpl_runtime_stats(cpl_runtime *rt, cpl_stats *out)
{ hit(2); put_p(rt); put_p(out); }
static cpl_status fake_cpl_type_lookup(cpl_runtime *rt, const char *name, size_t len, cpl_type *out, cpl_error **error)
{ hit(3); put_p(rt); put_p(name); put_u(len); put_p(out); put_p(error); return fake_status[3]; }
static cpl_status fake_cpl_type_name(cpl_runtime *rt, cpl_type type, cpl_strview *out, cpl_error **error)
{ hit(4); put_p(rt); put_u(type); put_p(out); put_p(error); return fake_status[4]; }
static cpl_status fake_cpl_value_dup(cpl_runtime *rt, cpl_value v, cpl_value *out, cpl_error **error)
{ hit(5); put_p(rt); put_h(v.bits); put_p(out); put_p(error); return fake_status[5]; }
static cpl_status fake_cpl_value_release(cpl_runtime *rt, cpl_value v)
{ hit(6); put_p(rt); put_h(v.bits); return fake_status[6]; }
static cpl_status fake_cpl_value_kind(cpl_runtime *rt, cpl_value v, cpl_kind *out, cpl_error **error)
{ hit(7); put_p(rt); put_h(v.bits); put_p(out); put_p(error); return fake_status[7]; }
static cpl_status fake_cpl_value_type(cpl_runtime *rt, cpl_value v, cpl_type *out, cpl_error **error)
{ hit(8); put_p(rt); put_h(v.bits); put_p(out); put_p(error); return fake_status[8]; }
static cpl_status fake_cpl_value_equal(cpl_runtime *rt, cpl_value a, cpl_value b, int32_t *out, cpl_error **error)
{ hit(9); put_p(rt); put_h(a.bits); put_h(b.bits); put_p(out); put_p(error); return fake_status[9]; }
static cpl_status fake_cpl_value_to_json(cpl_runtime *rt, cpl_value v, cpl_buf *out, cpl_error **error)
{ hit(10); put_p(rt); put_h(v.bits); put_p(out); put_p(error); return fake_status[10]; }
static cpl_status fake_cpl_value_from_json(cpl_runtime *rt, cpl_type type, const char *json, size_t len, cpl_value *out, cpl_error **error)
{ hit(11); put_p(rt); put_u(type); put_p(json); put_u(len); put_p(out); put_p(error); return fake_status[11]; }
static cpl_status fake_cpl_value_to_text(cpl_runtime *rt, cpl_value v, cpl_buf *out, cpl_error **error)
{ hit(12); put_p(rt); put_h(v.bits); put_p(out); put_p(error); return fake_status[12]; }
static void fake_cpl_buf_release(cpl_runtime *rt, cpl_buf *buf)
{ hit(13); put_p(rt); put_p(buf); }
static cpl_status fake_cpl_int_from_i64(cpl_runtime *rt, int64_t n, cpl_value *out, cpl_error **error)
{ hit(14); put_p(rt); put_i(n); put_p(out); put_p(error); return fake_status[14]; }
static cpl_status fake_cpl_int_from_u64(cpl_runtime *rt, uint64_t n, cpl_value *out, cpl_error **error)
{ hit(15); put_p(rt); put_u(n); put_p(out); put_p(error); return fake_status[15]; }
static cpl_status fake_cpl_int_from_decimal(cpl_runtime *rt, const char *s, size_t len, cpl_value *out, cpl_error **error)
{ hit(16); put_p(rt); put_p(s); put_u(len); put_p(out); put_p(error); return fake_status[16]; }
static cpl_status fake_cpl_int_from_words(cpl_runtime *rt, int32_t negative, const uint64_t *words, size_t n, cpl_value *out, cpl_error **error)
{ hit(17); put_p(rt); put_i(negative); put_p(words); put_u(n); put_p(out); put_p(error); return fake_status[17]; }
static cpl_status fake_cpl_int_to_u64(cpl_runtime *rt, cpl_value v, uint64_t *out, cpl_error **error)
{ hit(19); put_p(rt); put_h(v.bits); put_p(out); put_p(error); return fake_status[19]; }
static cpl_status fake_cpl_int_to_decimal(cpl_runtime *rt, cpl_value v, cpl_buf *out, cpl_error **error)
{ hit(20); put_p(rt); put_h(v.bits); put_p(out); put_p(error); return fake_status[20]; }
static cpl_status fake_cpl_int_to_words(cpl_runtime *rt, cpl_value v, int32_t *negative, uint64_t *words, size_t cap, size_t *needed, cpl_error **error)
{ hit(21); put_p(rt); put_h(v.bits); put_p(negative); put_p(words); put_u(cap); put_p(needed); put_p(error); return fake_status[21]; }
static cpl_status fake_cpl_bool_new(cpl_runtime *rt, int32_t b, cpl_value *out, cpl_error **error)
{ hit(22); put_p(rt); put_i(b); put_p(out); put_p(error); return fake_status[22]; }
static cpl_status fake_cpl_bool_get(cpl_runtime *rt, cpl_value v, int32_t *out, cpl_error **error)
{ hit(23); put_p(rt); put_h(v.bits); put_p(out); put_p(error); return fake_status[23]; }
static cpl_status fake_cpl_text_new(cpl_runtime *rt, const char *utf8, size_t len, cpl_value *out, cpl_error **error)
{ hit(24); put_p(rt); put_p(utf8); put_u(len); put_p(out); put_p(error); return fake_status[24]; }
static cpl_status fake_cpl_text_view(cpl_runtime *rt, cpl_value v, cpl_strview *out, cpl_error **error)
{ hit(25); put_p(rt); put_h(v.bits); put_p(out); put_p(error); return fake_status[25]; }
static cpl_status fake_cpl_text_scalars(cpl_runtime *rt, cpl_value v, size_t *out, cpl_error **error)
{ hit(26); put_p(rt); put_h(v.bits); put_p(out); put_p(error); return fake_status[26]; }
static cpl_status fake_cpl_bytes_new(cpl_runtime *rt, const uint8_t *data, size_t len, cpl_value *out, cpl_error **error)
{ hit(27); put_p(rt); put_p(data); put_u(len); put_p(out); put_p(error); return fake_status[27]; }
static cpl_status fake_cpl_bytes_view(cpl_runtime *rt, cpl_value v, cpl_bytesview *out, cpl_error **error)
{ hit(28); put_p(rt); put_h(v.bits); put_p(out); put_p(error); return fake_status[28]; }
static cpl_status fake_cpl_list_new(cpl_runtime *rt, const cpl_value *items, size_t n, cpl_value *out, cpl_error **error)
{ hit(29); put_p(rt); put_p(items); put_u(n); put_p(out); put_p(error); return fake_status[29]; }
static cpl_status fake_cpl_list_len(cpl_runtime *rt, cpl_value v, size_t *out, cpl_error **error)
{ hit(30); put_p(rt); put_h(v.bits); put_p(out); put_p(error); return fake_status[30]; }
static cpl_status fake_cpl_list_get(cpl_runtime *rt, cpl_value v, size_t index, cpl_value *out, cpl_error **error)
{ hit(31); put_p(rt); put_h(v.bits); put_u(index); put_p(out); put_p(error); return fake_status[31]; }
static cpl_status fake_cpl_list_from_i64(cpl_runtime *rt, const int64_t *items, size_t n, cpl_value *out, cpl_error **error)
{ hit(32); put_p(rt); put_p(items); put_u(n); put_p(out); put_p(error); return fake_status[32]; }
static cpl_status fake_cpl_list_to_i64(cpl_runtime *rt, cpl_value v, int64_t *buf, size_t cap, size_t *needed, cpl_error **error)
{ hit(33); put_p(rt); put_h(v.bits); put_p(buf); put_u(cap); put_p(needed); put_p(error); return fake_status[33]; }
static cpl_status fake_cpl_struct_new(cpl_runtime *rt, cpl_type type, const cpl_value *fields, size_t n, cpl_value *out, cpl_error **error)
{ hit(34); put_p(rt); put_u(type); put_p(fields); put_u(n); put_p(out); put_p(error); return fake_status[34]; }
static cpl_status fake_cpl_struct_field(cpl_runtime *rt, cpl_value v, uint32_t index, cpl_value *out, cpl_error **error)
{ hit(35); put_p(rt); put_h(v.bits); put_u(index); put_p(out); put_p(error); return fake_status[35]; }
static cpl_status fake_cpl_enum_new(cpl_runtime *rt, cpl_type type, uint32_t variant, const cpl_value *payload, size_t n, cpl_value *out, cpl_error **error)
{ hit(36); put_p(rt); put_u(type); put_u(variant); put_p(payload); put_u(n); put_p(out); put_p(error); return fake_status[36]; }
static cpl_status fake_cpl_enum_variant(cpl_runtime *rt, cpl_value v, uint32_t *out, cpl_error **error)
{ hit(37); put_p(rt); put_h(v.bits); put_p(out); put_p(error); return fake_status[37]; }
static cpl_status fake_cpl_enum_payload(cpl_runtime *rt, cpl_value v, uint32_t index, cpl_value *out, cpl_error **error)
{ hit(38); put_p(rt); put_h(v.bits); put_u(index); put_p(out); put_p(error); return fake_status[38]; }
static cpl_status fake_cpl_wrapper_new(cpl_runtime *rt, cpl_type type, cpl_value inner, cpl_value *out, cpl_error **error)
{ hit(39); put_p(rt); put_u(type); put_h(inner.bits); put_p(out); put_p(error); return fake_status[39]; }
static cpl_status fake_cpl_wrapper_inner(cpl_runtime *rt, cpl_value v, cpl_value *out, cpl_error **error)
{ hit(40); put_p(rt); put_h(v.bits); put_p(out); put_p(error); return fake_status[40]; }
static cpl_status fake_cpl_unit(cpl_runtime *rt, cpl_value *out, cpl_error **error)
{ hit(41); put_p(rt); put_p(out); put_p(error); return fake_status[41]; }
static cpl_status fake_cpl_option_some(cpl_runtime *rt, cpl_value v, cpl_value *out, cpl_error **error)
{ hit(42); put_p(rt); put_h(v.bits); put_p(out); put_p(error); return fake_status[42]; }
static cpl_status fake_cpl_option_none(cpl_runtime *rt, cpl_value *out, cpl_error **error)
{ hit(43); put_p(rt); put_p(out); put_p(error); return fake_status[43]; }
static cpl_status fake_cpl_result_ok(cpl_runtime *rt, cpl_value v, cpl_value *out, cpl_error **error)
{ hit(44); put_p(rt); put_h(v.bits); put_p(out); put_p(error); return fake_status[44]; }
static cpl_status fake_cpl_result_err(cpl_runtime *rt, cpl_value e, cpl_value *out, cpl_error **error)
{ hit(45); put_p(rt); put_h(e.bits); put_p(out); put_p(error); return fake_status[45]; }
static cpl_status fake_cpl_foreign_error_new(cpl_runtime *rt, cpl_foreign_kind kind, const char *message, size_t len, const cpl_strview *keys, const cpl_strview *values, size_t ndetails, cpl_value *out, cpl_error **error)
{ hit(46); put_p(rt); put_i(kind); put_p(message); put_u(len); put_p(keys); put_p(values); put_u(ndetails); put_p(out); put_p(error); return fake_status[46]; }
static cpl_status fake_cpl_cancel_new(cpl_runtime *rt, cpl_cancel *out, cpl_error **error)
{ hit(47); put_p(rt); put_p(out); put_p(error); return fake_status[47]; }
static cpl_status fake_cpl_cancel_request(cpl_runtime *rt, cpl_cancel token)
{ hit(48); put_p(rt); put_h(token.bits); return fake_status[48]; }
static cpl_status fake_cpl_cancel_release(cpl_runtime *rt, cpl_cancel token)
{ hit(49); put_p(rt); put_h(token.bits); return fake_status[49]; }
static cpl_status fake_cpl_cap_new(cpl_runtime *rt, cpl_type capability, const cpl_cap_ops *ops, void *user, cpl_cap *out, cpl_error **error)
{ hit(50); put_p(rt); put_u(capability); put_p(ops); put_p(user); put_p(out); put_p(error); return fake_status[50]; }
static cpl_status fake_cpl_cap_release(cpl_runtime *rt, cpl_cap cap)
{ hit(51); put_p(rt); put_h(cap.bits); return fake_status[51]; }
static cpl_runtime *fake_cpl_call_runtime(cpl_call *call)
{ hit(52); put_p(call); return &rt; }
static uint32_t fake_cpl_call_operation(cpl_call *call)
{ hit(53); put_p(call); return 4242u; }
static int32_t fake_cpl_call_cancelled(cpl_call *call)
{ hit(54); put_p(call); return 1; }
static cpl_status fake_cpl_error_status(const cpl_error *e)
{ hit(57); put_p(e); return fake_status[57]; }
static const char *fake_cpl_error_code(const cpl_error *e)
{ hit(58); put_p(e); return "fake"; }
static const char *fake_cpl_error_category(const cpl_error *e)
{ hit(59); put_p(e); return "fake"; }
static cpl_strview fake_cpl_error_message(const cpl_error *e)
{ hit(60); put_p(e); return fake_view; }
static cpl_strview fake_cpl_error_json(const cpl_error *e)
{ hit(61); put_p(e); return fake_view; }
static void fake_cpl_error_release(cpl_error *e)
{ hit(62); put_p(e); }

/* the three entries the adapter helpers depend on, with behaviour */
static int64_t fake_i64;
static cpl_status fake_cpl_int_to_i64(cpl_runtime *r, cpl_value v, int64_t *out, cpl_error **error)
{
    hit(E_cpl_int_to_i64); put_p(r); put_h(v.bits); put_p(out); put_p(error);
    if (fake_status[E_cpl_int_to_i64] == CPL_OK)
        *out = fake_i64;
    return fake_status[E_cpl_int_to_i64];
}
static char detail_key[64], detail_value[64];
static size_t detail_key_len, detail_value_len;
static cpl_status fake_cpl_call_foreign_detail(cpl_call *c, const char *key, size_t key_len, const char *value,
                                               size_t value_len)
{
    hit(E_cpl_call_foreign_detail); put_p(c); put_p(key); put_u(key_len); put_p(value); put_u(value_len);
    detail_key_len = key_len < 63u ? key_len : 63u;
    detail_value_len = value_len < 63u ? value_len : 63u;
    if (detail_key_len > 0u) memcpy(detail_key, key, detail_key_len);
    if (detail_value_len > 0u) memcpy(detail_value, value, detail_value_len);
    detail_key[detail_key_len] = '\0';
    detail_value[detail_value_len] = '\0';
    return fake_status[E_cpl_call_foreign_detail];
}
static cpl_foreign_kind error_kind;
static char error_message[128];
static size_t error_len;
static cpl_status fake_cpl_call_foreign_error(cpl_call *c, cpl_foreign_kind kind, const char *message, size_t len)
{
    hit(E_cpl_call_foreign_error); put_p(c); put_i(kind); put_p(message); put_u(len);
    error_kind = kind;
    error_len = len;
    if (len > 0u) memcpy(error_message, message, len < 127u ? len : 127u);
    error_message[len < 127u ? len : 127u] = '\0';
    return fake_status[E_cpl_call_foreign_error];
}

static void install(struct cpl_api *t)
{
    t->cpl_runtime_close = fake_cpl_runtime_close;
    t->cpl_runtime_info = fake_cpl_runtime_info;
    t->cpl_runtime_stats = fake_cpl_runtime_stats;
    t->cpl_type_lookup = fake_cpl_type_lookup;
    t->cpl_type_name = fake_cpl_type_name;
    t->cpl_value_dup = fake_cpl_value_dup;
    t->cpl_value_release = fake_cpl_value_release;
    t->cpl_value_kind = fake_cpl_value_kind;
    t->cpl_value_type = fake_cpl_value_type;
    t->cpl_value_equal = fake_cpl_value_equal;
    t->cpl_value_to_json = fake_cpl_value_to_json;
    t->cpl_value_from_json = fake_cpl_value_from_json;
    t->cpl_value_to_text = fake_cpl_value_to_text;
    t->cpl_buf_release = fake_cpl_buf_release;
    t->cpl_int_from_i64 = fake_cpl_int_from_i64;
    t->cpl_int_from_u64 = fake_cpl_int_from_u64;
    t->cpl_int_from_decimal = fake_cpl_int_from_decimal;
    t->cpl_int_from_words = fake_cpl_int_from_words;
    t->cpl_int_to_i64 = fake_cpl_int_to_i64;
    t->cpl_int_to_u64 = fake_cpl_int_to_u64;
    t->cpl_int_to_decimal = fake_cpl_int_to_decimal;
    t->cpl_int_to_words = fake_cpl_int_to_words;
    t->cpl_bool_new = fake_cpl_bool_new;
    t->cpl_bool_get = fake_cpl_bool_get;
    t->cpl_text_new = fake_cpl_text_new;
    t->cpl_text_view = fake_cpl_text_view;
    t->cpl_text_scalars = fake_cpl_text_scalars;
    t->cpl_bytes_new = fake_cpl_bytes_new;
    t->cpl_bytes_view = fake_cpl_bytes_view;
    t->cpl_list_new = fake_cpl_list_new;
    t->cpl_list_len = fake_cpl_list_len;
    t->cpl_list_get = fake_cpl_list_get;
    t->cpl_list_from_i64 = fake_cpl_list_from_i64;
    t->cpl_list_to_i64 = fake_cpl_list_to_i64;
    t->cpl_struct_new = fake_cpl_struct_new;
    t->cpl_struct_field = fake_cpl_struct_field;
    t->cpl_enum_new = fake_cpl_enum_new;
    t->cpl_enum_variant = fake_cpl_enum_variant;
    t->cpl_enum_payload = fake_cpl_enum_payload;
    t->cpl_wrapper_new = fake_cpl_wrapper_new;
    t->cpl_wrapper_inner = fake_cpl_wrapper_inner;
    t->cpl_unit = fake_cpl_unit;
    t->cpl_option_some = fake_cpl_option_some;
    t->cpl_option_none = fake_cpl_option_none;
    t->cpl_result_ok = fake_cpl_result_ok;
    t->cpl_result_err = fake_cpl_result_err;
    t->cpl_foreign_error_new = fake_cpl_foreign_error_new;
    t->cpl_cancel_new = fake_cpl_cancel_new;
    t->cpl_cancel_request = fake_cpl_cancel_request;
    t->cpl_cancel_release = fake_cpl_cancel_release;
    t->cpl_cap_new = fake_cpl_cap_new;
    t->cpl_cap_release = fake_cpl_cap_release;
    t->cpl_call_runtime = fake_cpl_call_runtime;
    t->cpl_call_operation = fake_cpl_call_operation;
    t->cpl_call_cancelled = fake_cpl_call_cancelled;
    t->cpl_call_foreign_detail = fake_cpl_call_foreign_detail;
    t->cpl_call_foreign_error = fake_cpl_call_foreign_error;
    t->cpl_error_status = fake_cpl_error_status;
    t->cpl_error_code = fake_cpl_error_code;
    t->cpl_error_category = fake_cpl_error_category;
    t->cpl_error_message = fake_cpl_error_message;
    t->cpl_error_json = fake_cpl_error_json;
    t->cpl_error_release = fake_cpl_error_release;
}

#ifdef __cplusplus
}
#endif

static int check_macros(void)
{
    cpl_runtime_options ro = CPL_RUNTIME_OPTIONS_INIT;
    cpl_call_options co = CPL_CALL_OPTIONS_INIT;
    cpl_stats st = CPL_STATS_INIT;
    CHECK(CPL_OK == 0 && CPL_ERR == 1 && CPL_FAULT == 2 && CPL_E_ARGUMENT == 3 && CPL_E_RANGE == 4);
    CHECK(CPL_E_NOMEM == 5 && CPL_E_VERSION == 6 && CPL_E_STATE == 7 && CPL_E_CANCELLED == 8);
    CHECK(CPL_E_UNSUPPORTED == 9 && CPL_E_INTERNAL == 10 && CPL_FOREIGN_ERROR == 11);
    CHECK(CPL_KIND_INT == 1u && CPL_KIND_BOOL == 2u && CPL_KIND_TEXT == 3u && CPL_KIND_BYTES == 4u);
    CHECK(CPL_KIND_LIST == 5u && CPL_KIND_STRUCT == 6u && CPL_KIND_ENUM == 7u && CPL_KIND_WRAPPER == 8u);
    CHECK(CPL_FOREIGN_EXCEPTION == 0 && CPL_FOREIGN_INVALID_ARGUMENT == 1 && CPL_FOREIGN_INVALID_RESULT == 2);
    CHECK(CPL_FOREIGN_TRANSPORT == 3 && CPL_FOREIGN_TIMEOUT == 4 && CPL_FOREIGN_CANCELLED == 5);
    CHECK(CPL_FOREIGN_UNAVAILABLE == 6);
    CHECK(CPL_T_INT == 1u && CPL_T_BOOL == 2u && CPL_T_TEXT == 3u && CPL_T_BYTES == 4u && CPL_T_UNIT == 5u);
    CHECK(CPL_T_OPTION == 6u && CPL_T_RESULT == 7u && CPL_T_CONSTRAINT_ERROR == 8u && CPL_T_FOREIGN_ERROR == 9u);
    CHECK(CPL_T_FOREIGN_ERROR_KIND == 10u && CPL_T_FOREIGN_DETAIL == 11u);
    CHECK(CPL_OPTION_SOME == 0u && CPL_OPTION_NONE == 1u && CPL_RESULT_OK == 0u && CPL_RESULT_ERR == 1u);
    CHECK(CPL_CONSTRAINT_ERROR_F_TYPE_NAME == 0u && CPL_CONSTRAINT_ERROR_F_MESSAGE == 1u);
    CHECK(CPL_FOREIGN_ERROR_F_KIND == 0u && CPL_FOREIGN_ERROR_F_MESSAGE == 1u && CPL_FOREIGN_ERROR_F_DETAILS == 2u);
    CHECK(CPL_FOREIGN_DETAIL_F_KEY == 0u && CPL_FOREIGN_DETAIL_F_VALUE == 1u);
    CHECK(CPL_CALL_UNCHECKED_CONTRACTS == 1u && CPL_CLOSE_FORCE == 1u);
    CHECK(ro.struct_size == sizeof(cpl_runtime_options) && ro.api_version == CPL_API_VERSION);
    CHECK(ro.allocator == NULL && ro.max_heap_bytes == 0u && ro.max_call_depth == 10000u && ro.flags == 0u);
    CHECK(co.struct_size == sizeof(cpl_call_options) && co.flags == 0u && co.time_budget_ms == 0u);
    CHECK(co.stack_bytes == 0u && co.cancel.bits[0] == 0u && co.cancel.bits[1] == 0u);
    CHECK(st.struct_size == sizeof(cpl_stats) && st.reserved == 0u && st.live_handles == 0u && st.live_caps == 0u);
    CHECK(st.live_objects == 0u && st.live_bytes == 0u && st.peak_bytes == 0u && st.active_calls == 0u);
    CHECK(st.live_buffers == 0u);
    CHECK(sizeof(struct cpl_api) == 8u + 8u * NENTRIES && offsetof(struct cpl_api, cpl_runtime_close) == 8u);
    CHECK(host_hidden() == 3);
    return 0;
}

static int check_api_has(void)
{
    CHECK(CPL_API_HAS(&table, cpl_runtime_close) && CPL_API_HAS(rt.api, cpl_error_release));
    table.struct_size = U32(offsetof(struct cpl_api, cpl_error_release));
    CHECK(CPL_API_HAS(&table, cpl_error_json) && !CPL_API_HAS(&table, cpl_error_release));
    table.struct_size = U32(offsetof(struct cpl_api, cpl_runtime_close));
    CHECK(!CPL_API_HAS(rt.api, cpl_runtime_close));
    table.struct_size = U32(sizeof table);
    return 0;
}

/* every wrapper reaches the entry of its name with its arguments in order and returns what the entry returns */
static int check_dispatch(void)
{
    { cpl_error *a2 = NULL;
      reset(); CHECK(cpl_runtime_close(&rt, 1u, &a2) == fake_status[0]); CHECK(only(0) && nrecs == 3 && P(0, &rt) && U(1, 1u) && P(2, &a2)); }
    {
      reset(); CHECK(cpl_runtime_info(&rt) == &fake_info); CHECK(only(1) && nrecs == 1 && P(0, &rt)); }
    { cpl_stats a1[2]; memset(a1, 0, sizeof a1);
      reset(); cpl_runtime_stats(&rt, a1); CHECK(only(2) && nrecs == 2 && P(0, &rt) && P(1, a1)); }
    { const char a1[] = "s3_1"; cpl_type a3[2]; memset(a3, 0, sizeof a3); cpl_error *a4 = NULL;
      reset(); CHECK(cpl_type_lookup(&rt, a1, 50u, a3, &a4) == fake_status[3]); CHECK(only(3) && nrecs == 5 && P(0, &rt) && P(1, a1) && U(2, 50u) && P(3, a3) && P(4, &a4)); }
    { cpl_strview a2[2]; memset(a2, 0, sizeof a2); cpl_error *a3 = NULL;
      reset(); CHECK(cpl_type_name(&rt, 65u, a2, &a3) == fake_status[4]); CHECK(only(4) && nrecs == 4 && P(0, &rt) && U(1, 65u) && P(2, a2) && P(3, &a3)); }
    { cpl_value a1 = { { 81u, 1081u } }; cpl_value a2[2]; memset(a2, 0, sizeof a2); cpl_error *a3 = NULL;
      reset(); CHECK(cpl_value_dup(&rt, a1, a2, &a3) == fake_status[5]); CHECK(only(5) && nrecs == 5 && P(0, &rt) && H(1, 81u, 1081u) && P(3, a2) && P(4, &a3)); }
    { cpl_value a1 = { { 97u, 1097u } };
      reset(); CHECK(cpl_value_release(&rt, a1) == fake_status[6]); CHECK(only(6) && nrecs == 3 && P(0, &rt) && H(1, 97u, 1097u)); }
    { cpl_value a1 = { { 113u, 1113u } }; cpl_kind a2[2]; memset(a2, 0, sizeof a2); cpl_error *a3 = NULL;
      reset(); CHECK(cpl_value_kind(&rt, a1, a2, &a3) == fake_status[7]); CHECK(only(7) && nrecs == 5 && P(0, &rt) && H(1, 113u, 1113u) && P(3, a2) && P(4, &a3)); }
    { cpl_value a1 = { { 129u, 1129u } }; cpl_type a2[2]; memset(a2, 0, sizeof a2); cpl_error *a3 = NULL;
      reset(); CHECK(cpl_value_type(&rt, a1, a2, &a3) == fake_status[8]); CHECK(only(8) && nrecs == 5 && P(0, &rt) && H(1, 129u, 1129u) && P(3, a2) && P(4, &a3)); }
    { cpl_value a1 = { { 145u, 1145u } }; cpl_value a2 = { { 146u, 1146u } }; int32_t a3[2]; memset(a3, 0, sizeof a3); cpl_error *a4 = NULL;
      reset(); CHECK(cpl_value_equal(&rt, a1, a2, a3, &a4) == fake_status[9]); CHECK(only(9) && nrecs == 7 && P(0, &rt) && H(1, 145u, 1145u) && H(3, 146u, 1146u) && P(5, a3) && P(6, &a4)); }
    { cpl_value a1 = { { 161u, 1161u } }; cpl_buf a2[2]; memset(a2, 0, sizeof a2); cpl_error *a3 = NULL;
      reset(); CHECK(cpl_value_to_json(&rt, a1, a2, &a3) == fake_status[10]); CHECK(only(10) && nrecs == 5 && P(0, &rt) && H(1, 161u, 1161u) && P(3, a2) && P(4, &a3)); }
    { const char a2[] = "s11_2"; cpl_value a4[2]; memset(a4, 0, sizeof a4); cpl_error *a5 = NULL;
      reset(); CHECK(cpl_value_from_json(&rt, 177u, a2, 179u, a4, &a5) == fake_status[11]); CHECK(only(11) && nrecs == 6 && P(0, &rt) && U(1, 177u) && P(2, a2) && U(3, 179u) && P(4, a4) && P(5, &a5)); }
    { cpl_value a1 = { { 193u, 1193u } }; cpl_buf a2[2]; memset(a2, 0, sizeof a2); cpl_error *a3 = NULL;
      reset(); CHECK(cpl_value_to_text(&rt, a1, a2, &a3) == fake_status[12]); CHECK(only(12) && nrecs == 5 && P(0, &rt) && H(1, 193u, 1193u) && P(3, a2) && P(4, &a3)); }
    { cpl_buf a1[2]; memset(a1, 0, sizeof a1);
      reset(); cpl_buf_release(&rt, a1); CHECK(only(13) && nrecs == 2 && P(0, &rt) && P(1, a1)); }
    { cpl_value a2[2]; memset(a2, 0, sizeof a2); cpl_error *a3 = NULL;
      reset(); CHECK(cpl_int_from_i64(&rt, -225, a2, &a3) == fake_status[14]); CHECK(only(14) && nrecs == 4 && P(0, &rt) && I(1, -225) && P(2, a2) && P(3, &a3)); }
    { cpl_value a2[2]; memset(a2, 0, sizeof a2); cpl_error *a3 = NULL;
      reset(); CHECK(cpl_int_from_u64(&rt, 241u, a2, &a3) == fake_status[15]); CHECK(only(15) && nrecs == 4 && P(0, &rt) && U(1, 241u) && P(2, a2) && P(3, &a3)); }
    { const char a1[] = "s16_1"; cpl_value a3[2]; memset(a3, 0, sizeof a3); cpl_error *a4 = NULL;
      reset(); CHECK(cpl_int_from_decimal(&rt, a1, 258u, a3, &a4) == fake_status[16]); CHECK(only(16) && nrecs == 5 && P(0, &rt) && P(1, a1) && U(2, 258u) && P(3, a3) && P(4, &a4)); }
    { uint64_t a2[2]; memset(a2, 0, sizeof a2); cpl_value a4[2]; memset(a4, 0, sizeof a4); cpl_error *a5 = NULL;
      reset(); CHECK(cpl_int_from_words(&rt, -273, a2, 275u, a4, &a5) == fake_status[17]); CHECK(only(17) && nrecs == 6 && P(0, &rt) && I(1, -273) && P(2, a2) && U(3, 275u) && P(4, a4) && P(5, &a5)); }
    { cpl_value a1 = { { 305u, 1305u } }; uint64_t a2[2]; memset(a2, 0, sizeof a2); cpl_error *a3 = NULL;
      reset(); CHECK(cpl_int_to_u64(&rt, a1, a2, &a3) == fake_status[19]); CHECK(only(19) && nrecs == 5 && P(0, &rt) && H(1, 305u, 1305u) && P(3, a2) && P(4, &a3)); }
    { cpl_value a1 = { { 321u, 1321u } }; cpl_buf a2[2]; memset(a2, 0, sizeof a2); cpl_error *a3 = NULL;
      reset(); CHECK(cpl_int_to_decimal(&rt, a1, a2, &a3) == fake_status[20]); CHECK(only(20) && nrecs == 5 && P(0, &rt) && H(1, 321u, 1321u) && P(3, a2) && P(4, &a3)); }
    { cpl_value a1 = { { 337u, 1337u } }; int32_t a2[2]; memset(a2, 0, sizeof a2); uint64_t a3[2]; memset(a3, 0, sizeof a3); size_t a5[2]; memset(a5, 0, sizeof a5); cpl_error *a6 = NULL;
      reset(); CHECK(cpl_int_to_words(&rt, a1, a2, a3, 340u, a5, &a6) == fake_status[21]); CHECK(only(21) && nrecs == 8 && P(0, &rt) && H(1, 337u, 1337u) && P(3, a2) && P(4, a3) && U(5, 340u) && P(6, a5) && P(7, &a6)); }
    { cpl_value a2[2]; memset(a2, 0, sizeof a2); cpl_error *a3 = NULL;
      reset(); CHECK(cpl_bool_new(&rt, -353, a2, &a3) == fake_status[22]); CHECK(only(22) && nrecs == 4 && P(0, &rt) && I(1, -353) && P(2, a2) && P(3, &a3)); }
    { cpl_value a1 = { { 369u, 1369u } }; int32_t a2[2]; memset(a2, 0, sizeof a2); cpl_error *a3 = NULL;
      reset(); CHECK(cpl_bool_get(&rt, a1, a2, &a3) == fake_status[23]); CHECK(only(23) && nrecs == 5 && P(0, &rt) && H(1, 369u, 1369u) && P(3, a2) && P(4, &a3)); }
    { const char a1[] = "s24_1"; cpl_value a3[2]; memset(a3, 0, sizeof a3); cpl_error *a4 = NULL;
      reset(); CHECK(cpl_text_new(&rt, a1, 386u, a3, &a4) == fake_status[24]); CHECK(only(24) && nrecs == 5 && P(0, &rt) && P(1, a1) && U(2, 386u) && P(3, a3) && P(4, &a4)); }
    { cpl_value a1 = { { 401u, 1401u } }; cpl_strview a2[2]; memset(a2, 0, sizeof a2); cpl_error *a3 = NULL;
      reset(); CHECK(cpl_text_view(&rt, a1, a2, &a3) == fake_status[25]); CHECK(only(25) && nrecs == 5 && P(0, &rt) && H(1, 401u, 1401u) && P(3, a2) && P(4, &a3)); }
    { cpl_value a1 = { { 417u, 1417u } }; size_t a2[2]; memset(a2, 0, sizeof a2); cpl_error *a3 = NULL;
      reset(); CHECK(cpl_text_scalars(&rt, a1, a2, &a3) == fake_status[26]); CHECK(only(26) && nrecs == 5 && P(0, &rt) && H(1, 417u, 1417u) && P(3, a2) && P(4, &a3)); }
    { uint8_t a1[2]; memset(a1, 0, sizeof a1); cpl_value a3[2]; memset(a3, 0, sizeof a3); cpl_error *a4 = NULL;
      reset(); CHECK(cpl_bytes_new(&rt, a1, 434u, a3, &a4) == fake_status[27]); CHECK(only(27) && nrecs == 5 && P(0, &rt) && P(1, a1) && U(2, 434u) && P(3, a3) && P(4, &a4)); }
    { cpl_value a1 = { { 449u, 1449u } }; cpl_bytesview a2[2]; memset(a2, 0, sizeof a2); cpl_error *a3 = NULL;
      reset(); CHECK(cpl_bytes_view(&rt, a1, a2, &a3) == fake_status[28]); CHECK(only(28) && nrecs == 5 && P(0, &rt) && H(1, 449u, 1449u) && P(3, a2) && P(4, &a3)); }
    { cpl_value a1[2]; memset(a1, 0, sizeof a1); cpl_value a3[2]; memset(a3, 0, sizeof a3); cpl_error *a4 = NULL;
      reset(); CHECK(cpl_list_new(&rt, a1, 466u, a3, &a4) == fake_status[29]); CHECK(only(29) && nrecs == 5 && P(0, &rt) && P(1, a1) && U(2, 466u) && P(3, a3) && P(4, &a4)); }
    { cpl_value a1 = { { 481u, 1481u } }; size_t a2[2]; memset(a2, 0, sizeof a2); cpl_error *a3 = NULL;
      reset(); CHECK(cpl_list_len(&rt, a1, a2, &a3) == fake_status[30]); CHECK(only(30) && nrecs == 5 && P(0, &rt) && H(1, 481u, 1481u) && P(3, a2) && P(4, &a3)); }
    { cpl_value a1 = { { 497u, 1497u } }; cpl_value a3[2]; memset(a3, 0, sizeof a3); cpl_error *a4 = NULL;
      reset(); CHECK(cpl_list_get(&rt, a1, 498u, a3, &a4) == fake_status[31]); CHECK(only(31) && nrecs == 6 && P(0, &rt) && H(1, 497u, 1497u) && U(3, 498u) && P(4, a3) && P(5, &a4)); }
    { int64_t a1[2]; memset(a1, 0, sizeof a1); cpl_value a3[2]; memset(a3, 0, sizeof a3); cpl_error *a4 = NULL;
      reset(); CHECK(cpl_list_from_i64(&rt, a1, 514u, a3, &a4) == fake_status[32]); CHECK(only(32) && nrecs == 5 && P(0, &rt) && P(1, a1) && U(2, 514u) && P(3, a3) && P(4, &a4)); }
    { cpl_value a1 = { { 529u, 1529u } }; int64_t a2[2]; memset(a2, 0, sizeof a2); size_t a4[2]; memset(a4, 0, sizeof a4); cpl_error *a5 = NULL;
      reset(); CHECK(cpl_list_to_i64(&rt, a1, a2, 531u, a4, &a5) == fake_status[33]); CHECK(only(33) && nrecs == 7 && P(0, &rt) && H(1, 529u, 1529u) && P(3, a2) && U(4, 531u) && P(5, a4) && P(6, &a5)); }
    { cpl_value a2[2]; memset(a2, 0, sizeof a2); cpl_value a4[2]; memset(a4, 0, sizeof a4); cpl_error *a5 = NULL;
      reset(); CHECK(cpl_struct_new(&rt, 545u, a2, 547u, a4, &a5) == fake_status[34]); CHECK(only(34) && nrecs == 6 && P(0, &rt) && U(1, 545u) && P(2, a2) && U(3, 547u) && P(4, a4) && P(5, &a5)); }
    { cpl_value a1 = { { 561u, 1561u } }; cpl_value a3[2]; memset(a3, 0, sizeof a3); cpl_error *a4 = NULL;
      reset(); CHECK(cpl_struct_field(&rt, a1, 562u, a3, &a4) == fake_status[35]); CHECK(only(35) && nrecs == 6 && P(0, &rt) && H(1, 561u, 1561u) && U(3, 562u) && P(4, a3) && P(5, &a4)); }
    { cpl_value a3[2]; memset(a3, 0, sizeof a3); cpl_value a5[2]; memset(a5, 0, sizeof a5); cpl_error *a6 = NULL;
      reset(); CHECK(cpl_enum_new(&rt, 577u, 578u, a3, 580u, a5, &a6) == fake_status[36]); CHECK(only(36) && nrecs == 7 && P(0, &rt) && U(1, 577u) && U(2, 578u) && P(3, a3) && U(4, 580u) && P(5, a5) && P(6, &a6)); }
    { cpl_value a1 = { { 593u, 1593u } }; uint32_t a2[2]; memset(a2, 0, sizeof a2); cpl_error *a3 = NULL;
      reset(); CHECK(cpl_enum_variant(&rt, a1, a2, &a3) == fake_status[37]); CHECK(only(37) && nrecs == 5 && P(0, &rt) && H(1, 593u, 1593u) && P(3, a2) && P(4, &a3)); }
    { cpl_value a1 = { { 609u, 1609u } }; cpl_value a3[2]; memset(a3, 0, sizeof a3); cpl_error *a4 = NULL;
      reset(); CHECK(cpl_enum_payload(&rt, a1, 610u, a3, &a4) == fake_status[38]); CHECK(only(38) && nrecs == 6 && P(0, &rt) && H(1, 609u, 1609u) && U(3, 610u) && P(4, a3) && P(5, &a4)); }
    { cpl_value a2 = { { 626u, 1626u } }; cpl_value a3[2]; memset(a3, 0, sizeof a3); cpl_error *a4 = NULL;
      reset(); CHECK(cpl_wrapper_new(&rt, 625u, a2, a3, &a4) == fake_status[39]); CHECK(only(39) && nrecs == 6 && P(0, &rt) && U(1, 625u) && H(2, 626u, 1626u) && P(4, a3) && P(5, &a4)); }
    { cpl_value a1 = { { 641u, 1641u } }; cpl_value a2[2]; memset(a2, 0, sizeof a2); cpl_error *a3 = NULL;
      reset(); CHECK(cpl_wrapper_inner(&rt, a1, a2, &a3) == fake_status[40]); CHECK(only(40) && nrecs == 5 && P(0, &rt) && H(1, 641u, 1641u) && P(3, a2) && P(4, &a3)); }
    { cpl_value a1[2]; memset(a1, 0, sizeof a1); cpl_error *a2 = NULL;
      reset(); CHECK(cpl_unit(&rt, a1, &a2) == fake_status[41]); CHECK(only(41) && nrecs == 3 && P(0, &rt) && P(1, a1) && P(2, &a2)); }
    { cpl_value a1 = { { 673u, 1673u } }; cpl_value a2[2]; memset(a2, 0, sizeof a2); cpl_error *a3 = NULL;
      reset(); CHECK(cpl_option_some(&rt, a1, a2, &a3) == fake_status[42]); CHECK(only(42) && nrecs == 5 && P(0, &rt) && H(1, 673u, 1673u) && P(3, a2) && P(4, &a3)); }
    { cpl_value a1[2]; memset(a1, 0, sizeof a1); cpl_error *a2 = NULL;
      reset(); CHECK(cpl_option_none(&rt, a1, &a2) == fake_status[43]); CHECK(only(43) && nrecs == 3 && P(0, &rt) && P(1, a1) && P(2, &a2)); }
    { cpl_value a1 = { { 705u, 1705u } }; cpl_value a2[2]; memset(a2, 0, sizeof a2); cpl_error *a3 = NULL;
      reset(); CHECK(cpl_result_ok(&rt, a1, a2, &a3) == fake_status[44]); CHECK(only(44) && nrecs == 5 && P(0, &rt) && H(1, 705u, 1705u) && P(3, a2) && P(4, &a3)); }
    { cpl_value a1 = { { 721u, 1721u } }; cpl_value a2[2]; memset(a2, 0, sizeof a2); cpl_error *a3 = NULL;
      reset(); CHECK(cpl_result_err(&rt, a1, a2, &a3) == fake_status[45]); CHECK(only(45) && nrecs == 5 && P(0, &rt) && H(1, 721u, 1721u) && P(3, a2) && P(4, &a3)); }
    { const char a2[] = "s46_2"; cpl_strview a4[2]; memset(a4, 0, sizeof a4); cpl_strview a5[2]; memset(a5, 0, sizeof a5); cpl_value a7[2]; memset(a7, 0, sizeof a7); cpl_error *a8 = NULL;
      reset(); CHECK(cpl_foreign_error_new(&rt, -737, a2, 739u, a4, a5, 742u, a7, &a8) == fake_status[46]); CHECK(only(46) && nrecs == 9 && P(0, &rt) && I(1, -737) && P(2, a2) && U(3, 739u) && P(4, a4) && P(5, a5) && U(6, 742u) && P(7, a7) && P(8, &a8)); }
    { cpl_cancel a1[2]; memset(a1, 0, sizeof a1); cpl_error *a2 = NULL;
      reset(); CHECK(cpl_cancel_new(&rt, a1, &a2) == fake_status[47]); CHECK(only(47) && nrecs == 3 && P(0, &rt) && P(1, a1) && P(2, &a2)); }
    { cpl_cancel a1 = { { 769u, 1769u } };
      reset(); CHECK(cpl_cancel_request(&rt, a1) == fake_status[48]); CHECK(only(48) && nrecs == 3 && P(0, &rt) && H(1, 769u, 1769u)); }
    { cpl_cancel a1 = { { 785u, 1785u } };
      reset(); CHECK(cpl_cancel_release(&rt, a1) == fake_status[49]); CHECK(only(49) && nrecs == 3 && P(0, &rt) && H(1, 785u, 1785u)); }
    { cpl_cap_ops a2[2]; memset(a2, 0, sizeof a2); int a3_target = 0; cpl_cap a4[2]; memset(a4, 0, sizeof a4); cpl_error *a5 = NULL;
      reset(); CHECK(cpl_cap_new(&rt, 801u, a2, &a3_target, a4, &a5) == fake_status[50]); CHECK(only(50) && nrecs == 6 && P(0, &rt) && U(1, 801u) && P(2, a2) && P(3, &a3_target) && P(4, a4) && P(5, &a5)); }
    { cpl_cap a1 = { { 817u, 1817u } };
      reset(); CHECK(cpl_cap_release(&rt, a1) == fake_status[51]); CHECK(only(51) && nrecs == 3 && P(0, &rt) && H(1, 817u, 1817u)); }
    {
      reset(); CHECK(cpl_call_runtime(&call) == &rt); CHECK(only(52) && nrecs == 1 && P(0, &call)); }
    {
      reset(); CHECK(cpl_call_operation(&call) == 4242u); CHECK(only(53) && nrecs == 1 && P(0, &call)); }
    {
      reset(); CHECK(cpl_call_cancelled(&call) == 1); CHECK(only(54) && nrecs == 1 && P(0, &call)); }
    {
      reset(); CHECK(cpl_error_status(&err) == fake_status[57]); CHECK(only(57) && nrecs == 1 && P(0, &err)); }
    {
      reset(); CHECK(strcmp(cpl_error_code(&err), "fake") == 0); CHECK(only(58) && nrecs == 1 && P(0, &err)); }
    {
      reset(); CHECK(strcmp(cpl_error_category(&err), "fake") == 0); CHECK(only(59) && nrecs == 1 && P(0, &err)); }
    {
      reset(); CHECK(cpl_error_message(&err).len == fake_view.len); CHECK(only(60) && nrecs == 1 && P(0, &err)); }
    {
      reset(); CHECK(cpl_error_json(&err).len == fake_view.len); CHECK(only(61) && nrecs == 1 && P(0, &err)); }
    {
      reset(); cpl_error_release(&err); CHECK(only(62) && nrecs == 1 && P(0, &err)); }
    {
        cpl_value v = { { 1u, 2u } };
        int64_t out = 0;
        cpl_error *e = NULL;
        reset();
        fake_status[E_cpl_int_to_i64] = CPL_OK;
        fake_i64 = -7;
        CHECK(cpl_int_to_i64(&rt, v, &out, &e) == CPL_OK && out == -7);
        CHECK(only(E_cpl_int_to_i64) && nrecs == 5 && P(0, &rt) && H(1, 1u, 2u) && P(3, &out) && P(4, &e));
        fake_status[E_cpl_int_to_i64] = 100 + E_cpl_int_to_i64;
        reset();
        CHECK(cpl_call_foreign_detail(&call, "key", 3u, "value", 5u) == fake_status[E_cpl_call_foreign_detail]);
        CHECK(only(E_cpl_call_foreign_detail) && nrecs == 5 && P(0, &call) && U(2, 3u) && U(4, 5u));
        CHECK(strcmp(detail_key, "key") == 0 && strcmp(detail_value, "value") == 0);
        reset();
        CHECK(cpl_call_foreign_error(&call, CPL_FOREIGN_TIMEOUT, "late", 4u) == fake_status[E_cpl_call_foreign_error]);
        CHECK(only(E_cpl_call_foreign_error) && nrecs == 4 && P(0, &call) && I(1, CPL_FOREIGN_TIMEOUT) && U(3, 4u));
        CHECK(error_kind == CPL_FOREIGN_TIMEOUT && strcmp(error_message, "late") == 0);
        reset();
        cpl_error_release(NULL); /* a no-op that never reaches a table */
        CHECK(only(-1) && nrecs == 0);
    }
    return 0;
}

/* the four helpers of cpl_adapter.h, built on the wrappers only */
static int check_adapter(void)
{
    cpl_value v = { { 5u, 6u } }, result = { { 0u, 0u } };
    int64_t n = 0;
    char buf[8];
    fake_status[E_cpl_int_to_i64] = CPL_OK;
    fake_status[E_cpl_call_foreign_error] = CPL_FOREIGN_ERROR;
    fake_status[E_cpl_call_foreign_detail] = CPL_OK;
    fake_i64 = 42;
    reset();
    CHECK(cpl_adapter_int_arg(&call, v, 0, 100, &n) == CPL_OK && n == 42 && hits[E_cpl_call_runtime] == 1);
    CHECK(hits[E_cpl_int_to_i64] == 1 && hits[E_cpl_call_foreign_error] == 0);
    reset();
    n = 0;
    CHECK(cpl_adapter_int_arg(&call, v, 50, 100, &n) == CPL_FOREIGN_ERROR && n == 0);
    CHECK(error_kind == CPL_FOREIGN_INVALID_ARGUMENT);
    CHECK(strcmp(error_message, "an integer argument is outside the range of the C parameter") == 0);
    fake_status[E_cpl_int_to_i64] = CPL_E_RANGE;
    reset();
    CHECK(cpl_adapter_int_arg(&call, v, 0, 100, &n) == CPL_FOREIGN_ERROR && hits[E_cpl_call_foreign_error] == 1);
    fake_status[E_cpl_int_to_i64] = CPL_E_ARGUMENT;
    reset();
    CHECK(cpl_adapter_int_arg(&call, v, 0, 100, &n) == CPL_E_ARGUMENT && hits[E_cpl_call_foreign_error] == 0);

    reset();
    CHECK(cpl_adapter_cstr(&call, "abc", 3u, buf, sizeof buf) == CPL_OK && strcmp(buf, "abc") == 0 && only(-1));
    reset();
    CHECK(cpl_adapter_cstr(&call, NULL, 0u, buf, 1u) == CPL_OK && buf[0] == '\0' && only(-1));
    reset();
    CHECK(cpl_adapter_cstr(&call, "a\0b", 3u, buf, sizeof buf) == CPL_FOREIGN_ERROR);
    CHECK(strcmp(error_message, "a text argument contains U+0000, which a C string cannot carry") == 0);
    reset();
    CHECK(cpl_adapter_cstr(&call, "abcdefgh", 8u, buf, sizeof buf) == CPL_FOREIGN_ERROR);
    CHECK(strcmp(error_message, "a text argument is longer than the C buffer") == 0);
    reset();
    CHECK(cpl_adapter_cstr(&call, "", 0u, buf, 0u) == CPL_FOREIGN_ERROR);

    reset();
    CHECK(cpl_adapter_fail_code(&call, CPL_FOREIGN_EXCEPTION, -123, "boom") == CPL_FOREIGN_ERROR);
    CHECK(strcmp(detail_key, "foreign_code") == 0 && strcmp(detail_value, "-123") == 0);
    CHECK(error_kind == CPL_FOREIGN_EXCEPTION && strcmp(error_message, "boom") == 0 && error_len == 4u);
    reset();
    CHECK(cpl_adapter_fail_code(&call, CPL_FOREIGN_UNAVAILABLE, INT64_MIN, NULL) == CPL_FOREIGN_ERROR);
    CHECK(strcmp(detail_value, "-9223372036854775808") == 0 && error_len == 0u);
    reset();
    CHECK(cpl_adapter_fail_code(&call, CPL_FOREIGN_UNAVAILABLE, INT64_MAX, "x") == CPL_FOREIGN_ERROR);
    CHECK(strcmp(detail_value, "9223372036854775807") == 0);
    reset();
    CHECK(cpl_adapter_fail_code(&call, CPL_FOREIGN_UNAVAILABLE, 0, "x") == CPL_FOREIGN_ERROR);
    CHECK(strcmp(detail_value, "0") == 0);
    fake_status[E_cpl_call_foreign_detail] = CPL_E_NOMEM;
    reset();
    CHECK(cpl_adapter_fail_code(&call, CPL_FOREIGN_UNAVAILABLE, 7, "x") == CPL_E_NOMEM);
    CHECK(hits[E_cpl_call_foreign_error] == 0);

    fake_status[E_cpl_result_ok] = CPL_OK;
    fake_status[E_cpl_value_release] = CPL_OK;
    reset();
    CHECK(cpl_adapter_ok(&rt, v, &result) == CPL_OK);
    CHECK(hits[E_cpl_result_ok] == 1 && hits[E_cpl_value_release] == 1 && nrecs == 8);
    CHECK(P(0, &rt) && H(1, 5u, 6u) && P(3, &result) && P(4, NULL) && P(5, &rt) && H(6, 5u, 6u));
    return 0;
}

int main(void)
{
    int n;
    install(&table);
    table.struct_size = U32(sizeof table);
    table.api_version = CPL_API_VERSION;
    rt.api = &table;
    call.api = &table;
    err.api = &table;
    for (n = 0; n < NENTRIES; n++)
        fake_status[n] = 100 + n;
    if (check_macros() != 0 || check_api_has() != 0 || check_dispatch() != 0 || check_adapter() != 0)
        return 1;
    puts("host ok: 63 entries, every macro, every wrapper, every adapter helper");
    return 0;
}
"##;
