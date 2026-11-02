//! The builtin registry of the standard library (`spec/stdlib.md` 10.4, 10.6, S47; `spec/cabi.md` 2.7;
//! `spec/toolchain.md` 1.7.4, T70).
//!
//! The 59 ids of stdlib 10.6, sorted by id, each with whether it is callable, its C entry point, the qualified name
//! of its stub, and its precondition clauses. The six type ids (`bool`, `bytes`, `gen`, `int`, `list`, `text`) and
//! the seven generator ids (`gen_*`) are registered so that the check knows them but are not callable (toolchain 3.6,
//! T18): a builtin type is never called, and a generator is `ExprKind::Gen`, drawn by the property runner. The 46
//! callable ones are C functions of the runtime, `cplrt_bi(<id>)` in emitted code (`spec/cabi.md` 2.7, Appendix
//! B.1).
//!
//! The precondition facts here are the *fallback* of `cpl_eval::builtins::requires_spec` (toolchain 1.7.4): every
//! program a project check produces declares the stubs of `std/*.cpl`, and both engines then take a precondition's
//! symbol, clause text, clause span, and input names from the stub; these serve a program built by hand that does
//! not declare the stub. They are the stubs' own texts, so the two sources agree
//! (`crates/cpl-rtspec/tests/tables.rs`).

/// One precondition clause of a builtin (stdlib 10.4): its canonical clause text and the `input` names of the
/// precondition fault, the receiver of a member stub being `self`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct RequiresSpec {
    pub clause: &'static str,
    pub inputs: &'static [&'static str],
}

/// One entry of the builtin registry.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct BuiltinSpec {
    /// The registry id of `@builtin("<id>")` (stdlib 10.6).
    pub id: &'static str,
    /// False for the 6 type ids and the 7 generator ids (toolchain 3.6).
    pub callable: bool,
    /// The C entry point of a callable builtin as emitted code calls it, `cplrt_bi(<id>)`, which `cpl_rt.h` expands
    /// to the namespaced runtime symbol `cplrt_<prefix>__bi_<id>` (`spec/cabi.md` 2.7, Appendix B.1); `""` for an id
    /// that is not callable, whose call lowers to `cplrt_bi_uncallable` (`spec/cabi.md` 1.4).
    pub c_entry: &'static str,
    /// The qualified name of the id's stub (toolchain 4.4; a member stub as `std.prelude.List.take`, stdlib S43), for
    /// a program that does not declare the stub.
    pub fallback_symbol: &'static str,
    /// The precondition clauses in source order (stdlib 10.4); empty for most ids.
    pub requires: &'static [RequiresSpec],
}

const NONE: &[RequiresSpec] = &[];

/// A builtin type: not callable, no entry point, no precondition.
const fn ty(id: &'static str, fallback_symbol: &'static str) -> BuiltinSpec {
    BuiltinSpec {
        id,
        callable: false,
        c_entry: "",
        fallback_symbol,
        requires: NONE,
    }
}

/// A generator of `std.gen`: not callable; `gen_range` keeps its precondition, which the property runner checks at
/// setup and a call that names it checks first (stdlib 8.2, 10.6; `spec/cabi.md` 1.4).
const fn generator(
    id: &'static str,
    fallback_symbol: &'static str,
    requires: &'static [RequiresSpec],
) -> BuiltinSpec {
    BuiltinSpec {
        id,
        callable: false,
        c_entry: "",
        fallback_symbol,
        requires,
    }
}

/// A callable builtin with its C entry point.
const fn call(
    id: &'static str,
    c_entry: &'static str,
    fallback_symbol: &'static str,
    requires: &'static [RequiresSpec],
) -> BuiltinSpec {
    BuiltinSpec {
        id,
        callable: true,
        c_entry,
        fallback_symbol,
        requires,
    }
}

const BYTES_FROM_LIST_REQUIRES: &[RequiresSpec] = &[RequiresSpec {
    clause: "forall v in values: v >= 0 and v <= 255",
    inputs: &["values"],
}];
const GEN_RANGE_REQUIRES: &[RequiresSpec] = &[RequiresSpec {
    clause: "lo <= hi",
    inputs: &["lo", "hi"],
}];
const LIST_DROP_REQUIRES: &[RequiresSpec] = &[RequiresSpec {
    clause: "n >= 0",
    inputs: &["self", "n"],
}];
const LIST_TAKE_REQUIRES: &[RequiresSpec] = &[RequiresSpec {
    clause: "n >= 0",
    inputs: &["self", "n"],
}];
const TEXT_SPLIT_REQUIRES: &[RequiresSpec] = &[RequiresSpec {
    clause: "not sep.is_empty()",
    inputs: &["text", "sep"],
}];

/// The registry of stdlib 10.6, sorted by id (byte order).
pub const BUILTINS: [BuiltinSpec; 59] = [
    ty("bool", "std.prelude.Bool"),
    ty("bytes", "std.prelude.Bytes"),
    call(
        "bytes_from_list",
        "cplrt_bi(bytes_from_list)",
        "std.bytes.from_list",
        BYTES_FROM_LIST_REQUIRES,
    ),
    call(
        "bytes_from_text",
        "cplrt_bi(bytes_from_text)",
        "std.bytes.from_text",
        NONE,
    ),
    call(
        "bytes_is_empty",
        "cplrt_bi(bytes_is_empty)",
        "std.prelude.Bytes.is_empty",
        NONE,
    ),
    call(
        "bytes_len",
        "cplrt_bi(bytes_len)",
        "std.prelude.Bytes.len",
        NONE,
    ),
    call(
        "bytes_to_list",
        "cplrt_bi(bytes_to_list)",
        "std.bytes.to_list",
        NONE,
    ),
    call(
        "bytes_to_text",
        "cplrt_bi(bytes_to_text)",
        "std.bytes.to_text",
        NONE,
    ),
    call("flatten", "cplrt_bi(flatten)", "std.prelude.flatten", NONE),
    ty("gen", "std.gen.Gen"),
    generator("gen_bool", "std.gen.bool", NONE),
    generator("gen_int", "std.gen.int", NONE),
    generator("gen_list", "std.gen.list", NONE),
    generator("gen_option", "std.gen.option", NONE),
    generator("gen_positive_int", "std.gen.positive_int", NONE),
    generator("gen_range", "std.gen.range", GEN_RANGE_REQUIRES),
    generator("gen_text", "std.gen.text", NONE),
    ty("int", "std.prelude.Int"),
    call("int_abs", "cplrt_bi(int_abs)", "std.prelude.Int.abs", NONE),
    call("int_max", "cplrt_bi(int_max)", "std.prelude.Int.max", NONE),
    call("int_min", "cplrt_bi(int_min)", "std.prelude.Int.min", NONE),
    call(
        "int_to_text",
        "cplrt_bi(int_to_text)",
        "std.prelude.Int.to_text",
        NONE,
    ),
    ty("list", "std.prelude.List"),
    call(
        "list_contains",
        "cplrt_bi(list_contains)",
        "std.prelude.List.contains",
        NONE,
    ),
    call(
        "list_drop",
        "cplrt_bi(list_drop)",
        "std.prelude.List.drop",
        LIST_DROP_REQUIRES,
    ),
    call(
        "list_drop_last",
        "cplrt_bi(list_drop_last)",
        "std.prelude.List.drop_last",
        NONE,
    ),
    call(
        "list_first",
        "cplrt_bi(list_first)",
        "std.prelude.List.first",
        NONE,
    ),
    call(
        "list_get",
        "cplrt_bi(list_get)",
        "std.prelude.List.get",
        NONE,
    ),
    call(
        "list_is_empty",
        "cplrt_bi(list_is_empty)",
        "std.prelude.List.is_empty",
        NONE,
    ),
    call(
        "list_last",
        "cplrt_bi(list_last)",
        "std.prelude.List.last",
        NONE,
    ),
    call(
        "list_len",
        "cplrt_bi(list_len)",
        "std.prelude.List.len",
        NONE,
    ),
    call(
        "list_push",
        "cplrt_bi(list_push)",
        "std.prelude.List.push",
        NONE,
    ),
    call(
        "list_reverse",
        "cplrt_bi(list_reverse)",
        "std.prelude.List.reverse",
        NONE,
    ),
    call(
        "list_take",
        "cplrt_bi(list_take)",
        "std.prelude.List.take",
        LIST_TAKE_REQUIRES,
    ),
    call(
        "option_is_none",
        "cplrt_bi(option_is_none)",
        "std.prelude.Option.is_none",
        NONE,
    ),
    call(
        "option_is_some",
        "cplrt_bi(option_is_some)",
        "std.prelude.Option.is_some",
        NONE,
    ),
    call(
        "option_ok_or",
        "cplrt_bi(option_ok_or)",
        "std.prelude.Option.ok_or",
        NONE,
    ),
    call(
        "option_unwrap_or",
        "cplrt_bi(option_unwrap_or)",
        "std.prelude.Option.unwrap_or",
        NONE,
    ),
    call(
        "result_err",
        "cplrt_bi(result_err)",
        "std.prelude.Result.err",
        NONE,
    ),
    call(
        "result_is_err",
        "cplrt_bi(result_is_err)",
        "std.prelude.Result.is_err",
        NONE,
    ),
    call(
        "result_is_ok",
        "cplrt_bi(result_is_ok)",
        "std.prelude.Result.is_ok",
        NONE,
    ),
    call(
        "result_ok",
        "cplrt_bi(result_ok)",
        "std.prelude.Result.ok",
        NONE,
    ),
    call(
        "result_unwrap_or",
        "cplrt_bi(result_unwrap_or)",
        "std.prelude.Result.unwrap_or",
        NONE,
    ),
    ty("text", "std.prelude.Text"),
    call("text_chars", "cplrt_bi(text_chars)", "std.text.chars", NONE),
    call(
        "text_contains",
        "cplrt_bi(text_contains)",
        "std.text.contains",
        NONE,
    ),
    call(
        "text_ends_with",
        "cplrt_bi(text_ends_with)",
        "std.text.ends_with",
        NONE,
    ),
    call(
        "text_from_int",
        "cplrt_bi(text_from_int)",
        "std.text.from_int",
        NONE,
    ),
    call(
        "text_is_empty",
        "cplrt_bi(text_is_empty)",
        "std.prelude.Text.is_empty",
        NONE,
    ),
    call("text_join", "cplrt_bi(text_join)", "std.text.join", NONE),
    call(
        "text_len",
        "cplrt_bi(text_len)",
        "std.prelude.Text.len",
        NONE,
    ),
    call(
        "text_parse_int",
        "cplrt_bi(text_parse_int)",
        "std.text.parse_int",
        NONE,
    ),
    call(
        "text_split",
        "cplrt_bi(text_split)",
        "std.text.split",
        TEXT_SPLIT_REQUIRES,
    ),
    call(
        "text_starts_with",
        "cplrt_bi(text_starts_with)",
        "std.text.starts_with",
        NONE,
    ),
    call(
        "text_to_lower",
        "cplrt_bi(text_to_lower)",
        "std.text.to_lower",
        NONE,
    ),
    call(
        "text_to_upper",
        "cplrt_bi(text_to_upper)",
        "std.text.to_upper",
        NONE,
    ),
    call("text_trim", "cplrt_bi(text_trim)", "std.text.trim", NONE),
    call(
        "text_trim_end",
        "cplrt_bi(text_trim_end)",
        "std.text.trim_end",
        NONE,
    ),
    call(
        "text_trim_start",
        "cplrt_bi(text_trim_start)",
        "std.text.trim_start",
        NONE,
    ),
];

/// The ids of [`BUILTINS`], in the same (sorted) order: what `cpl_sema::codes::BUILTIN_IDS` is defined as from
/// Milestone 2 (toolchain 1.6) and what the evaluator's registry is tested equal to (toolchain 1.7.4).
pub const IDS: [&str; 59] = {
    let mut ids = [""; 59];
    let mut i = 0;
    while i < ids.len() {
        ids[i] = BUILTINS[i].id;
        i += 1;
    }
    ids
};

/// [`BUILTINS`] as a static, so that [`spec`] can hand out `'static` references.
static REGISTRY: [BuiltinSpec; 59] = BUILTINS;

/// The specification of the builtin with registry id `id`, or `None` for an id the registry does not know (which a
/// stub naming it makes the check error `std_builtin_missing`, stdlib 10.2).
pub fn spec(id: &str) -> Option<&'static BuiltinSpec> {
    REGISTRY
        .binary_search_by(|b| b.id.cmp(id))
        .ok()
        .map(|i| &REGISTRY[i])
}
