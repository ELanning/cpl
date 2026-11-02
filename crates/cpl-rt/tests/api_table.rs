//! The function table of C API 0.1 (toolchain 12, the C runtime row; spec/cabi.md 4.1, 4.2, Appendix A.1, T78).
//! `tests/headers.rs` holds `struct cpl_api` of `cpl.h` to the entry order of `crates/cpl-rt/api_v0.txt`; this test
//! holds the rest of the rule:
//!
//! - the entries of 0.1 are frozen: `api_v0.txt` lists exactly [`FROZEN_0_1`] as its 0.1 entries, so an entry of 0.1
//!   is never reordered, removed, or added, and every later entry carries a later minor version, which `cpl.h`'s
//!   `CPL_API_VERSION_MINOR` must reach;
//! - the runtime's table `CPLRT(api_table)` holds, in each member `cpl_<name>`, the runtime function named for it (`api_<name>`),
//!   which a positional initializer that swapped two entries of one type would not, and every entry of 0.1 is present
//!   (`CPL_API_HAS`, not NULL), with `struct_size` and `api_version` the header's.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// The 63 entries of C API 0.1 in their order, frozen: a change to this list is an incompatible change of the C API.
const FROZEN_0_1: [&str; 63] = [
    "cpl_runtime_close",
    "cpl_runtime_info",
    "cpl_runtime_stats",
    "cpl_type_lookup",
    "cpl_type_name",
    "cpl_value_dup",
    "cpl_value_release",
    "cpl_value_kind",
    "cpl_value_type",
    "cpl_value_equal",
    "cpl_value_to_json",
    "cpl_value_from_json",
    "cpl_value_to_text",
    "cpl_buf_release",
    "cpl_int_from_i64",
    "cpl_int_from_u64",
    "cpl_int_from_decimal",
    "cpl_int_from_words",
    "cpl_int_to_i64",
    "cpl_int_to_u64",
    "cpl_int_to_decimal",
    "cpl_int_to_words",
    "cpl_bool_new",
    "cpl_bool_get",
    "cpl_text_new",
    "cpl_text_view",
    "cpl_text_scalars",
    "cpl_bytes_new",
    "cpl_bytes_view",
    "cpl_list_new",
    "cpl_list_len",
    "cpl_list_get",
    "cpl_list_from_i64",
    "cpl_list_to_i64",
    "cpl_struct_new",
    "cpl_struct_field",
    "cpl_enum_new",
    "cpl_enum_variant",
    "cpl_enum_payload",
    "cpl_wrapper_new",
    "cpl_wrapper_inner",
    "cpl_unit",
    "cpl_option_some",
    "cpl_option_none",
    "cpl_result_ok",
    "cpl_result_err",
    "cpl_foreign_error_new",
    "cpl_cancel_new",
    "cpl_cancel_request",
    "cpl_cancel_release",
    "cpl_cap_new",
    "cpl_cap_release",
    "cpl_call_runtime",
    "cpl_call_operation",
    "cpl_call_cancelled",
    "cpl_call_foreign_detail",
    "cpl_call_foreign_error",
    "cpl_error_status",
    "cpl_error_code",
    "cpl_error_category",
    "cpl_error_message",
    "cpl_error_json",
    "cpl_error_release",
];

/// The runtime function of a member `cpl_<name>`: `api_<name>`, but `api_err_release` for `cpl_error_release` (the
/// error object's release is named after the runtime's `api_err` record, cpl_rt_api.c).
fn implementation(member: &str) -> String {
    match member
        .strip_prefix("cpl_")
        .expect("a member is named like its function")
    {
        "error_release" => "api_err_release".to_string(),
        m => format!("api_{m}"),
    }
}

/// The entries of `api_v0.txt` as (major, minor, member).
fn api_v0() -> Vec<(u32, u32, String)> {
    let text =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("api_v0.txt")).unwrap();
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let (version, member) = l
                .split_once(char::is_whitespace)
                .expect("<version> <member>");
            let (major, minor) = version.split_once('.').expect("<major>.<minor>");
            (
                major.parse().unwrap(),
                minor.parse().unwrap(),
                member.trim().to_string(),
            )
        })
        .collect()
}

fn header_minor() -> u32 {
    let header =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("c/include/cpl.h"))
            .unwrap();
    header
        .lines()
        .find_map(|l| l.strip_prefix("#define CPL_API_VERSION_MINOR "))
        .and_then(|rest| rest.split_whitespace().next())
        .map(|v| v.trim_end_matches('u').parse().unwrap())
        .expect("cpl.h defines CPL_API_VERSION_MINOR")
}

#[test]
fn the_entries_of_0_1_are_frozen_and_later_ones_raise_the_minor() {
    let entries = api_v0();
    let of_0_1: Vec<&str> = entries
        .iter()
        .filter(|(_, n, _)| *n == 1)
        .map(|(_, _, m)| m.as_str())
        .collect();
    assert_eq!(
        of_0_1, FROZEN_0_1,
        "the 0.1 entries of api_v0.txt are frozen"
    );
    assert!(
        entries[..FROZEN_0_1.len()]
            .iter()
            .all(|(major, minor, _)| (*major, *minor) == (0, 1)),
        "the 0.1 entries come first"
    );
    let minor = header_minor();
    for (major, n, member) in &entries[FROZEN_0_1.len()..] {
        assert_eq!(*major, 0, "{member}: another major version");
        assert!(*n > 1, "{member} is appended after 0.1 but claims 0.{n}");
        assert!(
            *n <= minor,
            "{member} is of 0.{n}, but cpl.h's CPL_API_VERSION_MINOR is {minor}"
        );
    }
}

#[test]
fn the_runtime_table_holds_the_function_of_every_member() {
    let Some(cc) = cpl_rt::testing::find_cc() else {
        assert!(
            std::env::var("CPL_REQUIRE_CC").map_or(true, |v| v != "1"),
            "CPL_REQUIRE_CC=1, but there is no C compiler"
        );
        println!("skipped: no C compiler (set CPL_REQUIRE_CC=1 to fail)");
        return;
    };
    let work: PathBuf = Path::new(env!("CARGO_TARGET_TMPDIR")).join("api_table");
    let _ = std::fs::remove_dir_all(&work);
    for (relative, contents) in
        cpl_rt::amalgamation("rttest", &cpl_rt::RuntimeSource::Embedded).unwrap()
    {
        let path = work.join(&relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
    }
    // the unit includes the amalgamation, so the runtime's file-scope api_* functions are visible to it
    let mut c = String::from(
        "#include \"cpl_rt.c\"\n#include <stdio.h>\n\nint main(void)\n{\n    int bad = 0;\n    \
         const cpl_api *t = &CPLRT(api_table);\n    \
         if (t->struct_size != sizeof(cpl_api)) { printf(\"struct_size %u\\n\", (unsigned)t->struct_size); bad++; }\n    \
         if (t->api_version != CPL_API_VERSION) { printf(\"api_version %x\\n\", (unsigned)t->api_version); bad++; }\n",
    );
    for (_, _, member) in api_v0() {
        let f = implementation(&member);
        let _ = writeln!(
            c,
            "    if (!CPL_API_HAS(t, {member}) || t->{member} == NULL) {{ printf(\"{member} is missing\\n\"); bad++; }}\n    \
             else if ((void (*)(void))t->{member} != (void (*)(void)){f}) {{ printf(\"{member} is not {f}\\n\"); bad++; }}"
        );
    }
    c.push_str("    printf(\"checked %d\\n\", bad);\n    return bad != 0;\n}\n");
    std::fs::write(work.join("src/api_table.c"), &c).unwrap();
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/c/program_stub.c"),
        work.join("program_stub.c"),
    )
    .unwrap();
    let mut flags: Vec<String> = cpl_rt::flags::BASE.iter().map(|s| s.to_string()).collect();
    flags.extend(cpl_rt::flags::DEBUG.iter().map(|s| s.to_string()));
    flags.push(cpl_rt::flags::WERROR.to_string());
    flags.push(format!("-I{}", work.join("src").display()));
    flags.push(format!("-I{}", work.join("include").display()));
    let flags: Vec<&str> = flags.iter().map(String::as_str).collect();
    let out = cpl_rt::testing::compile_and_run(
        &cc,
        &[work.join("src/api_table.c"), work.join("program_stub.c")],
        &flags,
    )
    .unwrap_or_else(|e| panic!("the table check does not build: {e}"));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("checked 0"),
        "the runtime's table:\n{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
