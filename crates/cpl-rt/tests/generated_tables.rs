//! `cpl_rt_gen.h` against `cpl-rtspec` (toolchain 12, the C runtime row; spec/cabi.md 2.9 and Appendix B.2): a C
//! program compiled against the header that `cpl_rt::generated_header` writes prints back, as the compiler reads them,
//! every message template, the White_Space ranges, the builtin table with its precondition clauses, the limits, the
//! `ForeignErrorKind` names, the detail order, and the interop codes, and this test compares each with the Rust table;
//! and the C `{name}` renderer (`CPLRT(write_template)`) equals `cpl_rtspec::messages::render` for every template,
//! with argument values that hold braces, quotes, placeholders of their own, and UTF-8.

use cpl_rtspec::{builtins, foreign, interop, limits, messages, whitespace};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn cc_or_skip() -> Option<PathBuf> {
    let cc = cpl_rt::testing::find_cc();
    if cc.is_none() {
        assert!(
            std::env::var("CPL_REQUIRE_CC").map_or(true, |v| v != "1"),
            "CPL_REQUIRE_CC=1, but there is no C compiler"
        );
        println!("skipped: no C compiler (set CPL_REQUIRE_CC=1 to fail)");
    }
    cc
}

/// The bytes of `s` as a C string literal of octal escapes: no escaping question can arise.
fn literal(s: &str) -> String {
    let mut out = String::from("\"");
    for b in s.bytes() {
        let _ = write!(out, "\\{b:03o}");
    }
    out.push('"');
    out
}

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

/// The placeholder names of a template, in order of first occurrence, as `render` finds them.
fn placeholders(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' && bytes.get(i + 1).is_some_and(u8::is_ascii_lowercase) {
            let len = bytes[i + 1..]
                .iter()
                .take_while(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || **b == b'_')
                .count();
            if bytes.get(i + 1 + len) == Some(&b'}') {
                let name = text[i + 1..i + 1 + len].to_string();
                if !out.contains(&name) {
                    out.push(name);
                }
                i += len + 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// The argument value of one placeholder: braces, a quote, a placeholder of another name (never substituted again),
/// a backslash, and UTF-8; empty for every third name, so that empty values are covered too.
fn value_of(name: &str, i: usize) -> String {
    if i % 3 == 2 {
        String::new()
    } else {
        format!("<{name}: {{x}} \"q\" \\ {{{name}}} \u{e9}\u{1f600}>")
    }
}

#[test]
fn the_generated_header_is_cpl_rtspec_and_the_c_renderer_is_render() {
    let Some(cc) = cc_or_skip() else { return };
    let work = Path::new(env!("CARGO_TARGET_TMPDIR")).join("generated_tables");
    let _ = std::fs::remove_dir_all(&work);
    for (relative, contents) in
        cpl_rt::amalgamation("rttest", &cpl_rt::RuntimeSource::Embedded).unwrap()
    {
        let path = work.join(&relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
    }
    // the header the amalgamation carries is the one generated_header returns
    let header = std::fs::read_to_string(work.join("src/cpl_rt_gen.h")).unwrap();
    assert_eq!(
        header,
        cpl_rt::generated_header(),
        "the amalgamation's cpl_rt_gen.h"
    );

    // the C program and the output it must print, line by line
    let mut c = String::from(
        "#define _GNU_SOURCE 1\n#define CPLRT_NS rttest\n#define CPLRT(name) cplrt_rttest__##name\n\
         #define CPLPKG(name) cplpkg_rttest__##name\n#include \"cpl_rt_internal.h\"\n#include \"cpl_rt_gen.h\"\n\
         #include <stdio.h>\n#include <string.h>\n\n\
         void rttest_runtime_init(cplrt_runtime *rt, uint64_t max_heap_bytes);\n\
         uint64_t rttest_runtime_fini(cplrt_runtime *rt);\n\
         static cplrt_runtime rt;\n\
         static void hex(const char *s, size_t n) { for (size_t i = 0; i < n; i++) printf(\"%02x\", (unsigned char)s[i]); }\n\
         static void line(const char *tag, const char *s, size_t n) { printf(\"%s \", tag); hex(s, n); printf(\"\\n\"); }\n\
         static void str(const char *tag, const char *s) { if (s) line(tag, s, strlen(s)); else printf(\"%s NULL\\n\", tag); }\n\
         static void render(const char *name, const char *tmpl, const char *const *names, const char *const *values, uint32_t n)\n\
         {\n    cplrt_sbuf b;\n    cplrt_sbuf_init(&b, &rt);\n    CPLRT(write_template)(&b, tmpl, names, values, n);\n    \
         if (b.status == CPLRT_INVALID) printf(\"render %s status INVALID\\n\", name);\n    \
         else if (b.status != CPLRT_OK) printf(\"render %s status %d\\n\", name, b.status);\n    \
         else { printf(\"render %s \", name); hex(b.ptr ? b.ptr : \"\", b.len); printf(\"\\n\"); }\n    \
         CPLRT(sbuf_free)(&b);\n}\n\nint main(void)\n{\n    rttest_runtime_init(&rt, 0);\n",
    );
    let mut expected = Vec::new();
    // the templates, as literals and rendered
    for (k, t) in messages::ALL.iter().enumerate() {
        let _ = writeln!(
            c,
            "    line(\"msg {}\", CPLRT_MSG_{}, sizeof(CPLRT_MSG_{}) - 1u);",
            t.name, t.name, t.name
        );
        expected.push(format!("msg {} {}", t.name, hex(t.text)));
        let names = placeholders(t.text);
        let values: Vec<String> = names
            .iter()
            .enumerate()
            .map(|(i, n)| value_of(n, i + k))
            .collect();
        // an unused argument after the used ones: render ignores it, and so must C
        let mut all_names: Vec<String> = names.clone();
        let mut all_values = values.clone();
        all_names.push("unused_extra".to_string());
        all_values.push("{never}".to_string());
        let args: Vec<(&str, &str)> = names
            .iter()
            .map(String::as_str)
            .zip(values.iter().map(String::as_str))
            .collect();
        let _ = writeln!(
            c,
            "    {{ static const char *const n[] = {{ {} }}; static const char *const v[] = {{ {} }}; render(\"{}\", CPLRT_MSG_{}, n, v, {}u); }}",
            all_names.iter().map(|s| literal(s)).collect::<Vec<_>>().join(", "),
            all_values.iter().map(|s| literal(s)).collect::<Vec<_>>().join(", "),
            t.name,
            t.name,
            all_names.len()
        );
        expected.push(format!(
            "render {} {}",
            t.name,
            hex(&messages::render(t, &args))
        ));
        // a placeholder without an argument is CPLRT_INVALID (render panics: a toolchain defect)
        if !names.is_empty() {
            let _ = writeln!(
                c,
                "    {{ static const char *const n[] = {{ \"unused_extra\" }}; static const char *const v[] = {{ \"x\" }}; render(\"{}-missing\", CPLRT_MSG_{}, n, v, 1u); }}",
                t.name, t.name
            );
            expected.push(format!("render {}-missing status {}", t.name, "INVALID"));
        }
    }
    // White_Space
    let _ = writeln!(
        c,
        "    printf(\"ws %u\\n\", (unsigned)CPLRT_WHITE_SPACE_N);"
    );
    let _ = writeln!(
        c,
        "    for (uint32_t i = 0; i < CPLRT_WHITE_SPACE_N; i++) printf(\"ws %u %x %x\\n\", (unsigned)i, (unsigned)CPLRT_WHITE_SPACE[i].lo, (unsigned)CPLRT_WHITE_SPACE[i].hi);"
    );
    expected.push(format!("ws {}", whitespace::RANGES.len()));
    for (i, (lo, hi)) in whitespace::RANGES.iter().enumerate() {
        expected.push(format!("ws {i} {lo:x} {hi:x}"));
    }
    // the builtin table
    c.push_str(
        "    printf(\"builtins %u\\n\", (unsigned)CPLRT_BUILTINS_N);\n    \
         for (uint32_t i = 0; i < CPLRT_BUILTINS_N; i++) {\n        \
         const cplrt_builtin_spec *b = &CPLRT_BUILTINS[i];\n        \
         str(\"id\", b->id);\n        \
         printf(\"callable %u requires %u\\n\", (unsigned)b->callable, (unsigned)b->nrequires);\n        \
         str(\"fallback\", b->fallback_symbol);\n        \
         for (uint32_t r = 0; r < b->nrequires; r++) {\n            \
         str(\"clause\", b->requires[r].clause);\n            \
         for (uint32_t j = 0; j < b->requires[r].ninputs; j++) str(\"input\", b->requires[r].inputs[j]);\n        \
         }\n    }\n",
    );
    expected.push(format!("builtins {}", builtins::BUILTINS.len()));
    for b in &builtins::BUILTINS {
        expected.push(format!("id {}", hex(b.id)));
        expected.push(format!(
            "callable {} requires {}",
            u32::from(b.callable),
            b.requires.len()
        ));
        expected.push(format!("fallback {}", hex(b.fallback_symbol)));
        for r in b.requires {
            expected.push(format!("clause {}", hex(r.clause)));
            for i in r.inputs {
                expected.push(format!("input {}", hex(i)));
            }
        }
    }
    // the limits
    c.push_str(
        "    printf(\"limits %llu %llu %llu %u %u %u\\n\", (unsigned long long)CPLRT_LIMIT_DEFAULT_MAX_CALL_DEPTH,\n           \
         (unsigned long long)CPLRT_LIMIT_EVAL_STACK_SIZE, (unsigned long long)CPLRT_LIMIT_GUARD_MARGIN,\n           \
         (unsigned)CPLRT_LIMIT_API_MAJOR, (unsigned)CPLRT_LIMIT_API_MINOR, (unsigned)CPLRT_LIMIT_DRIVER_PROTOCOL);\n",
    );
    expected.push(format!(
        "limits {} {} {} {} {} {}",
        limits::DEFAULT_MAX_CALL_DEPTH,
        limits::EVAL_STACK_SIZE,
        limits::GUARD_MARGIN,
        limits::C_API_VERSION.0,
        limits::C_API_VERSION.1,
        limits::DRIVER_PROTOCOL
    ));
    // std.foreign
    c.push_str(
        "    printf(\"kinds %u\\n\", (unsigned)CPLRT_FOREIGN_KINDS_N);\n    \
         for (uint32_t i = 0; i < CPLRT_FOREIGN_KINDS_N; i++) str(\"kind\", CPLRT_FOREIGN_KINDS[i]);\n    \
         printf(\"order %u\\n\", (unsigned)CPLRT_FOREIGN_DETAIL_ORDER_N);\n    \
         for (uint32_t i = 0; i < CPLRT_FOREIGN_DETAIL_ORDER_N; i++) str(\"key\", CPLRT_FOREIGN_DETAIL_ORDER[i]);\n",
    );
    expected.push(format!("kinds {}", foreign::KINDS.len()));
    expected.extend(foreign::KINDS.iter().map(|k| format!("kind {}", hex(k))));
    expected.push(format!("order {}", foreign::DETAIL_ORDER.len()));
    expected.extend(
        foreign::DETAIL_ORDER
            .iter()
            .map(|k| format!("key {}", hex(k))),
    );
    // the interop codes, and the index macros
    c.push_str(
        "    printf(\"interop %u\\n\", (unsigned)CPLRT_INTEROP_N);\n    \
         for (uint32_t i = 0; i < CPLRT_INTEROP_N; i++) { str(\"code\", CPLRT_INTEROP[i].code); printf(\"status %d\\n\", (int)CPLRT_INTEROP[i].status); }\n",
    );
    expected.push(format!("interop {}", interop::CODES.len()));
    for code in interop::CODES {
        expected.push(format!("code {}", hex(code.code)));
        expected.push(format!("status {}", code.status));
    }
    for (i, code) in interop::CODES.iter().enumerate() {
        let name = code
            .code
            .strip_prefix("interop_")
            .unwrap_or(code.code)
            .to_ascii_uppercase();
        let _ = writeln!(
            c,
            "    printf(\"index {name} %u\\n\", (unsigned)CPLRT_INTEROP_{name});"
        );
        expected.push(format!("index {name} {i}"));
    }
    c.push_str("    if (rttest_runtime_fini(&rt) != 0) printf(\"leak\\n\");\n    return 0;\n}\n");
    std::fs::write(work.join("gen_tables.c"), &c).unwrap();
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/c/program_stub.c"),
        work.join("program_stub.c"),
    )
    .unwrap();

    let src = work.join("src");
    let mut flags: Vec<String> = cpl_rt::flags::BASE.iter().map(|s| s.to_string()).collect();
    flags.extend(cpl_rt::flags::DEBUG.iter().map(|s| s.to_string()));
    flags.push(cpl_rt::flags::WERROR.to_string());
    flags.push(format!("-I{}", src.display()));
    flags.push(format!("-I{}", work.join("include").display()));
    let flags: Vec<&str> = flags.iter().map(String::as_str).collect();
    let out = cpl_rt::testing::compile_and_run(
        &cc,
        &[
            work.join("gen_tables.c"),
            src.join("cpl_rt.c"),
            work.join("program_stub.c"),
        ],
        &flags,
    )
    .unwrap_or_else(|e| panic!("the table program does not build: {e}"));
    assert!(
        out.status.success(),
        "the table program failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    let got: Vec<&str> = stdout.lines().collect();
    let mut mismatches = Vec::new();
    for (i, want) in expected.iter().enumerate() {
        if got.get(i) != Some(&want.as_str()) {
            mismatches.push(format!(
                "line {i}: want {want}\n         got  {}",
                got.get(i).unwrap_or(&"<none>")
            ));
            if mismatches.len() > 20 {
                break;
            }
        }
    }
    assert_eq!(
        got.len(),
        expected.len(),
        "the program printed {} lines, {} expected",
        got.len(),
        expected.len()
    );
    assert!(!stdout.contains("leak"), "the runtime leaked");
    assert!(
        mismatches.is_empty(),
        "{} mismatches:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
    println!(
        "generated_tables: {} lines compared, {} templates rendered",
        expected.len(),
        messages::ALL.len()
    );
}
