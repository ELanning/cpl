//! The value mapping of exports and typed callbacks (`spec/cabi.md` 5.4) and the C names of generated declarations
//! (5.2).

use cpl_eval::types::strip;
use cpl_hir::{Program, Ty};
use cpl_project::IntMapping;
use std::collections::BTreeSet;

/// The form of one C argument or output (`CPLRT_AF_*`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Form {
    Handle,
    I64,
    I32,
    U64,
    U32,
    Bool,
    Text,
    Bytes,
    Unit,
}

impl Form {
    /// `CPLRT_AF_*`.
    pub fn code(self) -> &'static str {
        match self {
            Form::Handle => "CPLRT_AF_HANDLE",
            Form::I64 => "CPLRT_AF_I64",
            Form::I32 => "CPLRT_AF_I32",
            Form::U64 => "CPLRT_AF_U64",
            Form::U32 => "CPLRT_AF_U32",
            Form::Bool => "CPLRT_AF_BOOL",
            Form::Text => "CPLRT_AF_TEXT",
            Form::Bytes => "CPLRT_AF_BYTES",
            Form::Unit => "CPLRT_AF_UNIT",
        }
    }

    /// The C type of a scalar form (an argument), or of an output's pointee.
    pub fn c_type(self) -> &'static str {
        match self {
            Form::Handle => "cpl_value",
            Form::I64 => "int64_t",
            Form::I32 => "int32_t",
            Form::U64 => "uint64_t",
            Form::U32 => "uint32_t",
            Form::Bool => "int32_t",
            Form::Text => "const char *",
            Form::Bytes => "const uint8_t *",
            Form::Unit => "void",
        }
    }

    /// The `cplrt_arg` or `cplrt_out` member of the form.
    pub fn member(self) -> &'static str {
        match self {
            Form::Handle => "value",
            Form::I64 => "i64",
            Form::I32 | Form::Bool => "i32",
            Form::U64 => "u64",
            Form::U32 => "u32",
            Form::Text | Form::Bytes => "span",
            Form::Unit => "value",
        }
    }
}

fn int_form(int: IntMapping) -> Form {
    match int {
        IntMapping::Handle => Form::Handle,
        IntMapping::Int64 => Form::I64,
        IntMapping::Int32 => Form::I32,
        IntMapping::Uint64 => Form::U64,
        IntMapping::Uint32 => Form::U32,
    }
}

fn is_unit(p: &Program, ty: &Ty) -> bool {
    matches!(ty, Ty::Adt(d, args) if *d == p.std_defs.unit && args.is_empty())
}

/// The form of a positional parameter (5.4): Int and its refinements by the entry's mapping, Bool as `int32_t`, Text
/// and Bytes as spans, Unit without a parameter, every other type a handle.
pub(crate) fn param_form(p: &Program, ty: &Ty, int: IntMapping) -> Form {
    let base = strip(p, ty);
    match base {
        Ty::Int => int_form(int),
        Ty::Bool => Form::Bool,
        Ty::Text => Form::Text,
        Ty::Bytes => Form::Bytes,
        _ if is_unit(p, &base) => Form::Unit,
        _ => Form::Handle,
    }
}

/// The form of a result (5.4): Int and its refinements by the mapping, Bool as `int32_t`, Unit without an output,
/// every other type (Text and Bytes included) a handle.
pub(crate) fn result_form(p: &Program, ty: &Ty, int: IntMapping) -> Form {
    let base = strip(p, ty);
    match base {
        Ty::Int => int_form(int),
        Ty::Bool => Form::Bool,
        _ if is_unit(p, &base) => Form::Unit,
        _ => Form::Handle,
    }
}

/// A top-level `Result<T, E>` return is split into `ok_out` and `err_out` (5.4): Some((T, E)).
pub(crate) fn split_result(p: &Program, ret: &Ty) -> Option<(Ty, Ty)> {
    match ret {
        Ty::Adt(d, args) if *d == p.std_defs.result && args.len() == 2 => {
            Some((args[0].clone(), args[1].clone()))
        }
        _ => None,
    }
}

/// The reserved C spellings of 5.2: keywords of C11 and C++17, alternative tokens, standard typedefs and macros, and
/// predefined GNU macros.
const RESERVED: &[&str] = &[
    "auto",
    "break",
    "case",
    "char",
    "const",
    "continue",
    "default",
    "do",
    "double",
    "else",
    "enum",
    "extern",
    "float",
    "for",
    "goto",
    "if",
    "inline",
    "int",
    "long",
    "register",
    "restrict",
    "return",
    "short",
    "signed",
    "sizeof",
    "static",
    "struct",
    "switch",
    "typedef",
    "union",
    "unsigned",
    "void",
    "volatile",
    "while",
    "_Alignas",
    "_Alignof",
    "_Atomic",
    "_Bool",
    "_Complex",
    "_Generic",
    "_Imaginary",
    "_Noreturn",
    "_Static_assert",
    "_Thread_local",
    "alignas",
    "alignof",
    "and",
    "and_eq",
    "asm",
    "bitand",
    "bitor",
    "bool",
    "catch",
    "char16_t",
    "char32_t",
    "class",
    "compl",
    "concept",
    "const_cast",
    "constexpr",
    "decltype",
    "delete",
    "dynamic_cast",
    "explicit",
    "export",
    "false",
    "friend",
    "mutable",
    "namespace",
    "new",
    "noexcept",
    "not",
    "not_eq",
    "nullptr",
    "operator",
    "or",
    "or_eq",
    "private",
    "protected",
    "public",
    "reinterpret_cast",
    "requires",
    "static_assert",
    "static_cast",
    "template",
    "this",
    "thread_local",
    "throw",
    "true",
    "try",
    "typeid",
    "typename",
    "using",
    "virtual",
    "wchar_t",
    "xor",
    "xor_eq",
    "size_t",
    "ptrdiff_t",
    "max_align_t",
    "wchar_t",
    "NULL",
    "offsetof",
    "int8_t",
    "int16_t",
    "int32_t",
    "int64_t",
    "uint8_t",
    "uint16_t",
    "uint32_t",
    "uint64_t",
    "intptr_t",
    "uintptr_t",
    "intmax_t",
    "uintmax_t",
    "errno",
    "stdin",
    "stdout",
    "stderr",
    "math_errhandling",
    "noreturn",
    "complex",
    "imaginary",
    "linux",
    "unix",
    "rt",
    "options",
    "error",
    "out",
    "ok_out",
    "err_out",
    "user",
    "call",
    "result",
    "state",
    "struct_size",
    "reserved",
    "drop",
];

/// The C names of a declaration's parameters (5.2): the CPL names, `x` before a reserved leading underscore, `_`
/// appended while a name is reserved, begins with `cpl_`, or repeats an earlier one.
pub(crate) fn c_names(names: &[String]) -> Vec<String> {
    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::with_capacity(names.len());
    for n in names {
        let mut c = n.clone();
        let b = c.as_bytes();
        if b.len() >= 2 && b[0] == b'_' && (b[1].is_ascii_uppercase() || b[1] == b'_') {
            c = format!("x{c}");
        }
        while RESERVED.contains(&c.as_str()) || c.starts_with("cpl_") || used.contains(&c) {
            c.push('_');
        }
        used.insert(c.clone());
        out.push(c);
    }
    out
}

/// The name of a span's length parameter: the C name followed by `_len`, renamed by the same rule against `taken`.
pub(crate) fn len_name(name: &str, taken: &BTreeSet<String>) -> String {
    let mut c = format!("{name}_len");
    while RESERVED.contains(&c.as_str()) || taken.contains(&c) {
        c.push('_');
    }
    c
}
