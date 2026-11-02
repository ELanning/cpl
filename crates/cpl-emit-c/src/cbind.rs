//! Internal: the wrappers generated from declarative bindings, `src/<prefix>_bind_<binding>.c` (spec/cabi.md 6.3,
//! 6.5). A generated wrapper defines the adapter contract of 6.2 (`init`, `fini`, one function per operation) over the
//! library's own headers, so the C compiler checks every declared call against the real prototypes: each value
//! reaches the library through a local of its declared binding type, never through a cast, and the wrapper is compiled
//! with `-Werror=conversion -Werror=sign-conversion -Werror=incompatible-pointer-types` (cabi.md 1.5).

use crate::api::{adapter_op_names, callback_params, ParamModel};
use crate::map::Form;
use crate::print::{c_string, comment_text};
use cpl_eval::types::{operation_identity, strip};
use cpl_hir::{DefId, Program, Ty};
use cpl_project::cbind::{ArgSpec, BindType, CBindSpec, OpSpec, OutSpec, ReturnsSpec, Scalar};
use cpl_sema::interop::{BindingPlan, InteropPlan};
use std::fmt::Write as _;

/// The path of a binding's generated wrapper, relative to the output root.
pub(crate) fn wrapper_path(prefix: &str, binding: &str) -> String {
    format!("src/{prefix}_bind_{binding}.c")
}

/// The binding type's name in messages (`int32`, stdlib S45).
fn scalar_name(s: Scalar) -> &'static str {
    match s {
        Scalar::I8 => "int8",
        Scalar::I16 => "int16",
        Scalar::I32 => "int32",
        Scalar::I64 => "int64",
        Scalar::U8 => "uint8",
        Scalar::U16 => "uint16",
        Scalar::U32 => "uint32",
        Scalar::U64 => "uint64",
        Scalar::Int => "int",
        Scalar::Bool => "bool",
    }
}

fn is_unsigned(s: Scalar) -> bool {
    matches!(s, Scalar::U8 | Scalar::U16 | Scalar::U32 | Scalar::U64)
}

/// The bounds of a signed scalar as C expressions of type int64_t.
fn signed_bounds(s: Scalar) -> (&'static str, &'static str) {
    match s {
        Scalar::I8 => ("INT8_MIN", "INT8_MAX"),
        Scalar::I16 => ("INT16_MIN", "INT16_MAX"),
        Scalar::I32 => ("INT32_MIN", "INT32_MAX"),
        Scalar::Int => ("INT_MIN", "INT_MAX"),
        _ => ("INT64_MIN", "INT64_MAX"),
    }
}

fn unsigned_max(s: Scalar) -> &'static str {
    match s {
        Scalar::U8 => "UINT8_MAX",
        Scalar::U16 => "UINT16_MAX",
        Scalar::U32 => "UINT32_MAX",
        _ => "UINT64_MAX",
    }
}

/// A status constant compared in the width of its signedness.
fn status_test(var: &str, ty: Scalar, v: i64) -> String {
    if is_unsigned(ty) {
        format!("(uint64_t){var} == UINT64_C({v})")
    } else if v == i64::MIN {
        format!("(int64_t){var} == INT64_MIN")
    } else {
        format!("(int64_t){var} == INT64_C({v})")
    }
}

fn foreign_kind(kind: &str) -> &'static str {
    match kind {
        "InvalidArgument" => "CPL_FOREIGN_INVALID_ARGUMENT",
        "InvalidResult" => "CPL_FOREIGN_INVALID_RESULT",
        "Transport" => "CPL_FOREIGN_TRANSPORT",
        "Timeout" => "CPL_FOREIGN_TIMEOUT",
        "Cancelled" => "CPL_FOREIGN_CANCELLED",
        "Unavailable" => "CPL_FOREIGN_UNAVAILABLE",
        _ => "CPL_FOREIGN_EXCEPTION",
    }
}

/// The helpers a wrapper uses, emitted only when used (the adapter flag set warns of unused static functions).
#[derive(Default)]
struct Uses {
    fail: bool,
    status: bool,
    int_arg: bool,
    uint_arg: bool,
    dec_i64: bool,
    dec_u64: bool,
}

const HELPER_FAIL: &str = r#"/* a conversion the binding makes fails (stdlib S45): foreign_type, then for a result pointer and expected_type */
static cpl_status cpl_bind_fail(cpl_call *cpl_c, cpl_foreign_kind cpl_kind, const char *cpl_function,
                                const char *cpl_pointer, const char *cpl_expected, const char *cpl_message)
{
    cpl_status cpl_st = cpl_call_foreign_detail(cpl_c, "foreign_type", 12u, cpl_function, strlen(cpl_function));
    if (cpl_st == CPL_OK && cpl_pointer != NULL)
        cpl_st = cpl_call_foreign_detail(cpl_c, "pointer", 7u, cpl_pointer, strlen(cpl_pointer));
    if (cpl_st == CPL_OK && cpl_expected != NULL)
        cpl_st = cpl_call_foreign_detail(cpl_c, "expected_type", 13u, cpl_expected, strlen(cpl_expected));
    if (cpl_st != CPL_OK)
        return cpl_st;
    return cpl_call_foreign_error(cpl_c, cpl_kind, cpl_message, strlen(cpl_message));
}
"#;

const HELPER_STATUS: &str = r#"/* an error status (stdlib S45): foreign_type and foreign_code, the kind of `errors`, and the message of `message`
   when it gives strict UTF-8, else "<function> returned <code>" */
static cpl_status cpl_bind_status(cpl_call *cpl_c, cpl_foreign_kind cpl_kind, const char *cpl_function,
                                  const char *cpl_code, const char *cpl_message)
{
    cpl_status cpl_st = cpl_call_foreign_detail(cpl_c, "foreign_type", 12u, cpl_function, strlen(cpl_function));
    if (cpl_st == CPL_OK)
        cpl_st = cpl_call_foreign_detail(cpl_c, "foreign_code", 12u, cpl_code, strlen(cpl_code));
    if (cpl_st != CPL_OK)
        return cpl_st;
    if (cpl_message != NULL) {
        cpl_st = cpl_call_foreign_error(cpl_c, cpl_kind, cpl_message, strlen(cpl_message));
        if (cpl_st != CPL_E_ARGUMENT)                   /* CPL_FOREIGN_ERROR, or CPL_E_NOMEM */
            return cpl_st;
    }
    size_t cpl_fl = strlen(cpl_function), cpl_cl = strlen(cpl_code);
    char *cpl_m = (char *)malloc(cpl_fl + cpl_cl + 10u);
    if (cpl_m == NULL)
        return CPL_E_NOMEM;
    memcpy(cpl_m, cpl_function, cpl_fl);
    memcpy(cpl_m + cpl_fl, " returned ", 10u);
    memcpy(cpl_m + cpl_fl + 10u, cpl_code, cpl_cl);
    cpl_st = cpl_call_foreign_error(cpl_c, cpl_kind, cpl_m, cpl_fl + 10u + cpl_cl);
    free(cpl_m);
    return cpl_st;
}
"#;

const HELPER_INT_ARG: &str = r#"/* an Int argument within [lo, hi], else InvalidArgument before the call (stdlib S45) */
static cpl_status cpl_bind_int_arg(cpl_call *cpl_c, cpl_value cpl_v, int64_t cpl_lo, int64_t cpl_hi, int64_t *cpl_out,
                                   const char *cpl_function, const char *cpl_message)
{
    cpl_status cpl_st = cpl_int_to_i64(cpl_call_runtime(cpl_c), cpl_v, cpl_out, NULL);
    if (cpl_st == CPL_E_RANGE || (cpl_st == CPL_OK && (*cpl_out < cpl_lo || *cpl_out > cpl_hi)))
        return cpl_bind_fail(cpl_c, CPL_FOREIGN_INVALID_ARGUMENT, cpl_function, NULL, NULL, cpl_message);
    return cpl_st;
}
"#;

const HELPER_UINT_ARG: &str = r#"/* an Int argument within [0, hi], else InvalidArgument before the call (stdlib S45) */
static cpl_status cpl_bind_uint_arg(cpl_call *cpl_c, cpl_value cpl_v, uint64_t cpl_hi, uint64_t *cpl_out,
                                    const char *cpl_function, const char *cpl_message)
{
    cpl_status cpl_st = cpl_int_to_u64(cpl_call_runtime(cpl_c), cpl_v, cpl_out, NULL);
    if (cpl_st == CPL_E_RANGE || (cpl_st == CPL_OK && *cpl_out > cpl_hi))
        return cpl_bind_fail(cpl_c, CPL_FOREIGN_INVALID_ARGUMENT, cpl_function, NULL, NULL, cpl_message);
    return cpl_st;
}
"#;

const HELPER_DEC_I64: &str = r#"/* the decimal text of a status into a buffer of 24 bytes */
static const char *cpl_bind_i64(char *cpl_buf, int64_t cpl_v)
{
    uint64_t cpl_m = cpl_v < 0 ? (uint64_t)0 - (uint64_t)cpl_v : (uint64_t)cpl_v;
    char *cpl_p = cpl_buf + 23;
    *cpl_p = '\0';
    do {
        *--cpl_p = (char)('0' + (int)(cpl_m % 10u));
        cpl_m /= 10u;
    } while (cpl_m != 0u);
    if (cpl_v < 0)
        *--cpl_p = '-';
    return cpl_p;
}
"#;

const HELPER_DEC_U64: &str = r#"/* the decimal text of a status into a buffer of 24 bytes */
static const char *cpl_bind_u64(char *cpl_buf, uint64_t cpl_v)
{
    char *cpl_p = cpl_buf + 23;
    *cpl_p = '\0';
    do {
        *--cpl_p = (char)('0' + (int)(cpl_v % 10u));
        cpl_v /= 10u;
    } while (cpl_v != 0u);
    return cpl_p;
}
"#;

/// The context of one operation's function.
struct OpGen<'a> {
    p: &'a Program,
    prefix: &'a str,
    plan: &'a InteropPlan,
    spec: &'a CBindSpec,
    op: &'a OpSpec,
    /// `<module>.<Capability>.<operation>`
    identity: String,
    /// the operation's return type and, when it is `Result<T, ForeignError>`, `T`
    ret: Ty,
    ok_payload: Option<Ty>,
    out: String,
}

impl OpGen<'_> {
    fn line(&mut self, indent: usize, text: &str) {
        let _ = writeln!(self.out, "{}{text}", "    ".repeat(indent));
    }

    /// The type accessor of a listed struct type.
    fn type_accessor(&self, ty: &Ty) -> String {
        let t = self
            .plan
            .types
            .iter()
            .find(|t| &t.ty == ty)
            .unwrap_or_else(|| {
                panic!(
                    "a struct of a bound capability is listed: {}",
                    self.p.display_ty(ty)
                )
            });
        format!("{}_type_{}()", self.prefix, t.c_name)
    }

    /// The fields of the CPL struct type `ty`: (name, type), in declaration order.
    fn struct_fields(&self, ty: &Ty) -> Vec<(String, Ty)> {
        match strip(self.p, ty) {
            Ty::Adt(d, _) => self
                .p
                .def(d)
                .kind
                .as_struct()
                .map(|s| {
                    s.fields
                        .iter()
                        .map(|f| (f.name.clone(), f.ty.clone()))
                        .collect()
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// `the result of <operation> is not a value of <expected_type>[ at <pointer>]` (stdlib S45).
    fn invalid_result(&self, uses: &mut Uses, pointer: &str, expected: &Ty) -> String {
        uses.fail = true;
        let expected = self.p.display_ty(expected);
        let message = if pointer.is_empty() {
            format!(
                "the result of {} is not a value of {expected}",
                self.identity
            )
        } else {
            format!(
                "the result of {} is not a value of {expected} at {pointer}",
                self.identity
            )
        };
        format!(
            "cpl_bind_fail(cpl_c, CPL_FOREIGN_INVALID_RESULT, {}, {}, {}, {})",
            c_string(&self.op.c),
            c_string(pointer),
            c_string(&expected),
            c_string(&message)
        )
    }

    /// The pointer of the converted payload in the operation's result.
    fn payload_pointer(&self) -> &'static str {
        if self.ok_payload.is_some() {
            "/$payload/0"
        } else {
            ""
        }
    }

    /// Converts the scalar C value `expr` of binding type `s` into the CPL value `dst` (at `pointer`, of CPL type
    /// `ty`), setting cpl_st.
    #[allow(clippy::too_many_arguments)]
    fn scalar_out(
        &mut self,
        uses: &mut Uses,
        indent: usize,
        s: Scalar,
        expr: &str,
        dst: &str,
        pointer: &str,
        ty: &Ty,
    ) {
        match s {
            Scalar::Bool => {
                let fail = self.invalid_result(uses, pointer, ty);
                self.line(indent, &format!("if ({expr} != 0 && {expr} != 1)"));
                self.line(indent + 1, &format!("cpl_st = {fail};"));
                self.line(indent, "else");
                self.line(
                    indent + 1,
                    &format!("cpl_st = cpl_bool_new(cpl_rt, {expr} == 1, &{dst}, NULL);"),
                );
            }
            s if is_unsigned(s) => self.line(
                indent,
                &format!("cpl_st = cpl_int_from_u64(cpl_rt, (uint64_t){expr}, &{dst}, NULL);"),
            ),
            _ => self.line(
                indent,
                &format!("cpl_st = cpl_int_from_i64(cpl_rt, (int64_t){expr}, &{dst}, NULL);"),
            ),
        }
    }

    /// Converts the out-parameter `o` (its C local `local`) into cpl_v, setting cpl_st.
    fn out_value(&mut self, uses: &mut Uses, indent: usize, o: &OutSpec, local: &str, ty: &Ty) {
        let pointer = self.payload_pointer();
        match &o.ty {
            BindType::OwnedBytes => {
                let fail = self.invalid_result(uses, pointer, ty);
                self.line(
                    indent,
                    &format!("cpl_st = cpl_bytes_new(cpl_rt, {local}, {local}_len, &cpl_v, NULL);"),
                );
                self.line(indent, "if (cpl_st == CPL_E_ARGUMENT)");
                self.line(indent + 1, &format!("cpl_st = {fail};"));
            }
            BindType::OwnedText => {
                let fail = self.invalid_result(uses, pointer, ty);
                self.line(
                    indent,
                    &format!("cpl_st = cpl_text_new(cpl_rt, {local}, {local}_len, &cpl_v, NULL);"),
                );
                self.line(
                    indent,
                    "if (cpl_st == CPL_E_ARGUMENT)                 /* not strict UTF-8 */",
                );
                self.line(indent + 1, &format!("cpl_st = {fail};"));
            }
            BindType::Scalar(s) => {
                let s = *s;
                self.scalar_out(uses, indent, s, local, "cpl_v", pointer, ty)
            }
            BindType::Struct(name) => {
                let sspec = self.spec.structs[name].clone();
                let fields = self.struct_fields(ty);
                let n = fields.len();
                self.line(indent, "{");
                self.line(
                    indent + 1,
                    &format!(
                        "cpl_value cpl_f[{}] = {{ {} }};",
                        n.max(1),
                        vec!["{ { 0, 0 } }"; n.max(1)].join(", ")
                    ),
                );
                self.line(indent + 1, "cpl_st = CPL_OK;");
                for (k, (fname, fty)) in fields.iter().enumerate() {
                    let (_, member, scalar) = sspec
                        .fields
                        .iter()
                        .find(|(f, _, _)| f == fname)
                        .cloned()
                        .expect("checked by cpl_sema::cbind");
                    let tmp = format!("cpl_x{k}");
                    self.line(indent + 1, "if (cpl_st == CPL_OK) {");
                    self.line(
                        indent + 2,
                        &format!("{} {tmp} = {local}.{member};", scalar.c_type()),
                    );
                    let pointer = format!("{pointer}/{fname}");
                    self.scalar_out(
                        uses,
                        indent + 2,
                        scalar,
                        &tmp,
                        &format!("cpl_f[{k}]"),
                        &pointer,
                        fty,
                    );
                    self.line(indent + 1, "}");
                }
                let accessor = self.type_accessor(&strip(self.p, ty));
                self.line(indent + 1, "if (cpl_st == CPL_OK)");
                self.line(
                    indent + 2,
                    &format!(
                        "cpl_st = cpl_struct_new(cpl_rt, {accessor}, cpl_f, {n}u, &cpl_v, NULL);"
                    ),
                );
                for k in 0..n {
                    self.line(
                        indent + 1,
                        &format!("(void)cpl_value_release(cpl_rt, cpl_f[{k}]);"),
                    );
                }
                self.line(indent, "}");
            }
            BindType::BytesSpan | BindType::TextSpan | BindType::CStr => {
                unreachable!("no out binding type")
            }
        }
    }
}

/// The C type of an out-parameter's local and how it is passed.
fn out_decl(spec: &CBindSpec, o: &OutSpec, local: &str) -> (Vec<String>, Vec<String>) {
    match &o.ty {
        BindType::OwnedBytes => (
            vec![
                format!("uint8_t *{local} = NULL;"),
                format!("size_t {local}_len = 0;"),
            ],
            vec![format!("&{local}"), format!("&{local}_len")],
        ),
        BindType::OwnedText => (
            vec![
                format!("char *{local} = NULL;"),
                format!("size_t {local}_len = 0;"),
            ],
            vec![format!("&{local}"), format!("&{local}_len")],
        ),
        BindType::Scalar(s) => (
            vec![format!("{} {local} = 0;", s.c_type())],
            vec![format!("&{local}")],
        ),
        BindType::Struct(name) => (
            vec![
                format!("{} {local};", spec.structs[name].c_type),
                format!("memset(&{local}, 0, sizeof {local});"),
            ],
            vec![format!("&{local}")],
        ),
        BindType::BytesSpan | BindType::TextSpan | BindType::CStr => {
            unreachable!("no out binding type")
        }
    }
}

fn result_payload(p: &Program, ret: &Ty) -> Option<Ty> {
    match ret {
        Ty::Adt(r, args)
            if *r == p.std_defs.result
                && args.len() == 2
                && matches!(&args[1], Ty::Adt(e, a) if *e == p.std_defs.foreign_error && a.is_empty()) =>
        {
            Some(args[0].clone())
        }
        _ => None,
    }
}

/// The function of operation `i`.
#[allow(clippy::too_many_arguments)]
fn render_op(
    p: &Program,
    prefix: &str,
    plan: &InteropPlan,
    b: &BindingPlan,
    spec: &CBindSpec,
    i: usize,
    name: &str,
    uses: &mut Uses,
) -> String {
    let cd = p
        .def(b.capability)
        .kind
        .as_capability()
        .expect("a capability");
    let od = &cd.ops[i];
    let op = &spec.ops[&od.name];
    let params: Vec<ParamModel> = callback_params(p, b.capability, i);
    let mut g = OpGen {
        p,
        prefix,
        plan,
        spec,
        op,
        identity: operation_identity(p, b.capability, &od.name)
            .as_str()
            .to_string(),
        ret: od.ret.clone(),
        ok_payload: result_payload(p, &od.ret),
        out: String::new(),
    };
    // the signature: the parameters of the typed callback, named cpl_p<k>
    let mut sig = vec!["void *cpl_state".to_string(), "cpl_call *cpl_c".to_string()];
    for (k, prm) in params.iter().enumerate() {
        match prm.form {
            Form::Text => sig.push(format!("const char *cpl_p{k}, size_t cpl_p{k}_len")),
            Form::Bytes => sig.push(format!("const uint8_t *cpl_p{k}, size_t cpl_p{k}_len")),
            Form::Bool => sig.push(format!("int32_t cpl_p{k}")),
            _ => sig.push(format!("cpl_value cpl_p{k}")),
        }
    }
    sig.push("cpl_value *cpl_result".to_string());
    let fname = format!("{prefix}_adapter_{}_{name}", b.name);
    let _ = writeln!(
        g.out,
        "/* {} -> {}: {} */\ncpl_status {fname}({})\n{{",
        comment_text(&g.identity),
        comment_text(&p.display_ty(&od.ret)),
        comment_text(&op.c),
        sig.join(", ")
    );
    let uses_state = op.args.iter().any(|a| matches!(a, ArgSpec::State));
    if uses_state {
        g.line(1, "cpl_bind_state *cpl_s = (cpl_bind_state *)cpl_state;");
    } else {
        g.line(1, "(void)cpl_state;");
    }
    g.line(1, "cpl_runtime *cpl_rt = cpl_call_runtime(cpl_c);");
    g.line(1, "cpl_status cpl_st = CPL_OK;");
    g.line(1, "cpl_value cpl_v = { { 0, 0 } };");
    // the arguments, checked in order before any allocation (stdlib S45: InvalidArgument before the call)
    let mut call_args: Vec<String> = Vec::new();
    let mut cstrs: Vec<(usize, String)> = Vec::new();
    let param_index = |pname: &str| {
        od.params
            .iter()
            .position(|x| x.name == pname)
            .expect("checked by cpl_sema::cbind")
    };
    for a in &op.args {
        match a {
            ArgSpec::State => call_args.push("cpl_s->object".to_string()),
            ArgSpec::Param { name: pname, ty } => {
                let k = param_index(pname);
                let prm = &params[k];
                match ty {
                    BindType::BytesSpan | BindType::TextSpan => {
                        call_args.push(format!("cpl_p{k}"));
                        call_args.push(format!("cpl_p{k}_len"));
                    }
                    BindType::CStr => {
                        uses.fail = true;
                        let message = format!("argument {pname} of {} contains U+0000", g.identity);
                        g.line(
                            1,
                            &format!("if (memchr(cpl_p{k}, 0, cpl_p{k}_len) != NULL)"),
                        );
                        g.line(
                            2,
                            &format!(
                                "return cpl_bind_fail(cpl_c, CPL_FOREIGN_INVALID_ARGUMENT, {}, NULL, NULL, {});",
                                c_string(&op.c),
                                c_string(&message)
                            ),
                        );
                        cstrs.push((k, format!("cpl_z{k}")));
                        call_args.push(format!("cpl_z{k}"));
                    }
                    BindType::Scalar(Scalar::Bool) => {
                        g.line(1, &format!("int cpl_a{k} = (int)cpl_p{k};"));
                        call_args.push(format!("cpl_a{k}"));
                    }
                    BindType::Scalar(s) => {
                        let message = format!(
                            "argument {pname} of {} does not fit {}",
                            g.identity,
                            scalar_name(*s)
                        );
                        int_arg(
                            &mut g,
                            uses,
                            1,
                            *s,
                            &format!("cpl_p{k}"),
                            &format!("cpl_a{k}"),
                            &message,
                        );
                        call_args.push(format!("cpl_a{k}"));
                    }
                    BindType::Struct(sname) => {
                        let sspec = spec.structs[sname].clone();
                        let fields = g.struct_fields(&prm.ty);
                        g.line(1, &format!("{} cpl_a{k};", sspec.c_type));
                        g.line(1, &format!("memset(&cpl_a{k}, 0, sizeof cpl_a{k});"));
                        for (fi, (field, _)) in fields.iter().enumerate() {
                            let (_, member, scalar) = sspec
                                .fields
                                .iter()
                                .find(|(f, _, _)| f == field)
                                .cloned()
                                .expect("checked by cpl_sema::cbind");
                            g.line(1, "{");
                            g.line(2, "cpl_value cpl_f = { { 0, 0 } };");
                            g.line(2, &format!("cpl_st = cpl_struct_field(cpl_rt, cpl_p{k}, {fi}u, &cpl_f, NULL);"));
                            g.line(2, "if (cpl_st != CPL_OK)");
                            g.line(3, "return cpl_st;");
                            if scalar == Scalar::Bool {
                                g.line(2, "int32_t cpl_b = 0;");
                                g.line(2, "cpl_st = cpl_bool_get(cpl_rt, cpl_f, &cpl_b, NULL);");
                                g.line(2, "(void)cpl_value_release(cpl_rt, cpl_f);");
                                g.line(2, "if (cpl_st != CPL_OK)");
                                g.line(3, "return cpl_st;");
                                g.line(2, "int cpl_x = (int)cpl_b;");
                            } else {
                                let message = format!(
                                    "argument {pname}.{field} of {} does not fit {}",
                                    g.identity,
                                    scalar_name(scalar)
                                );
                                int_arg(&mut g, uses, 2, scalar, "cpl_f", "cpl_x", &message);
                                g.line(2, "(void)cpl_value_release(cpl_rt, cpl_f);");
                            }
                            g.line(2, &format!("cpl_a{k}.{member} = cpl_x;"));
                            g.line(1, "}");
                        }
                        call_args.push(format!("&cpl_a{k}"));
                    }
                    BindType::OwnedBytes | BindType::OwnedText => {
                        unreachable!("no argument binding type")
                    }
                }
            }
        }
    }
    // the NUL-terminated copies, after every check
    for (n, (k, z)) in cstrs.iter().enumerate() {
        g.line(
            1,
            &format!("char *{z} = (char *)malloc(cpl_p{k}_len + 1u);"),
        );
        g.line(1, &format!("if ({z} == NULL) {{"));
        for (_, earlier) in &cstrs[..n] {
            g.line(2, &format!("free({earlier});"));
        }
        g.line(2, "return CPL_E_NOMEM;");
        g.line(1, "}");
        g.line(1, &format!("if (cpl_p{k}_len > 0u)"));
        g.line(2, &format!("memcpy({z}, cpl_p{k}, cpl_p{k}_len);"));
        g.line(1, &format!("{z}[cpl_p{k}_len] = '\\0';"));
    }
    // the out-parameters, initialized
    for (j, o) in op.out.iter().enumerate() {
        let (decls, passed) = out_decl(spec, o, &format!("cpl_o{j}"));
        for d in decls {
            g.line(1, &d);
        }
        call_args.extend(passed);
    }
    // the call, under the state's mutex when it receives the state
    let call = format!("{}({})", op.c, call_args.join(", "));
    if uses_state {
        g.line(1, "pthread_mutex_lock(&cpl_s->lock);");
    }
    match &op.returns {
        ReturnsSpec::Status { ty, .. } | ReturnsSpec::Result { ty } => {
            g.line(1, &format!("{} cpl_r = {call};", ty.c_type()));
        }
        ReturnsSpec::Void => g.line(1, &format!("{call};")),
    }
    if uses_state {
        g.line(1, "pthread_mutex_unlock(&cpl_s->lock);");
    }
    for (_, z) in &cstrs {
        g.line(1, &format!("free({z});"));
    }
    // the outcome
    match &op.returns {
        ReturnsSpec::Status { ty, ok } => {
            let ty = *ty;
            let test = if ok.is_empty() {
                "0".to_string()
            } else {
                ok.iter()
                    .map(|v| status_test("cpl_r", ty, *v))
                    .collect::<Vec<_>>()
                    .join(" || ")
            };
            g.line(1, &format!("if ({test}) {{"));
            match &op.result {
                Some(res) => {
                    let j = op
                        .out
                        .iter()
                        .position(|o| &o.name == res)
                        .expect("checked by parse_cbind");
                    let o = op.out[j].clone();
                    let payload = g.ok_payload.clone().unwrap_or(Ty::Error);
                    g.out_value(uses, 2, &o, &format!("cpl_o{j}"), &payload);
                }
                None => g.line(2, "cpl_st = cpl_unit(cpl_rt, &cpl_v, NULL);"),
            }
            g.line(1, "} else {");
            let mut kind =
                foreign_kind(op.default_error.as_deref().unwrap_or("Exception")).to_string();
            for (v, k) in op.errors.iter().rev() {
                kind = format!(
                    "{} ? {} : {kind}",
                    status_test("cpl_r", ty, *v),
                    foreign_kind(k)
                );
            }
            g.line(2, "char cpl_code[24];");
            let code = if is_unsigned(ty) {
                uses.dec_u64 = true;
                "cpl_bind_u64(cpl_code, (uint64_t)cpl_r)"
            } else {
                uses.dec_i64 = true;
                "cpl_bind_i64(cpl_code, (int64_t)cpl_r)"
            };
            let message = match &op.message {
                Some(m) => format!("{m}(cpl_r)"),
                None => "NULL".to_string(),
            };
            uses.status = true;
            g.line(
                2,
                &format!(
                    "cpl_st = cpl_bind_status(cpl_c, {kind}, {}, {code}, {message});",
                    c_string(&op.c)
                ),
            );
            g.line(1, "}");
        }
        ReturnsSpec::Result { ty } => {
            let ty = *ty;
            let payload = g.ok_payload.clone().unwrap_or_else(|| g.ret.clone());
            let pointer = g.payload_pointer();
            g.scalar_out(uses, 1, ty, "cpl_r", "cpl_v", pointer, &payload);
        }
        ReturnsSpec::Void => g.line(1, "cpl_st = cpl_unit(cpl_rt, &cpl_v, NULL);"),
    }
    // owned outputs: released once, whatever the status, when not NULL
    for (j, o) in op.out.iter().enumerate() {
        if let Some(release) = &o.release {
            g.line(1, &format!("if (cpl_o{j} != NULL)"));
            g.line(2, &format!("{release}((void *)cpl_o{j});"));
        }
    }
    if g.ok_payload.is_some() {
        g.line(1, "if (cpl_st == CPL_OK)");
        g.line(
            2,
            "return cpl_adapter_ok(cpl_rt, cpl_v, cpl_result);   /* Ok(v); releases v */",
        );
        g.line(1, "(void)cpl_value_release(cpl_rt, cpl_v);");
    } else {
        g.line(1, "if (cpl_st == CPL_OK)");
        g.line(2, "*cpl_result = cpl_v;");
        g.line(1, "else");
        g.line(2, "(void)cpl_value_release(cpl_rt, cpl_v);");
    }
    g.line(1, "return cpl_st;");
    g.out.push_str("}\n");
    g.out
}

/// The checked conversion of the Int handle `src` into the local `dst` of the scalar's C type.
fn int_arg(
    g: &mut OpGen<'_>,
    uses: &mut Uses,
    indent: usize,
    s: Scalar,
    src: &str,
    dst: &str,
    message: &str,
) {
    uses.fail = true;
    let c = c_string(&g.op.c);
    let m = c_string(message);
    if is_unsigned(s) {
        uses.uint_arg = true;
        g.line(indent, &format!("uint64_t {dst}_n = 0;"));
        g.line(
            indent,
            &format!(
                "cpl_st = cpl_bind_uint_arg(cpl_c, {src}, (uint64_t){}, &{dst}_n, {c}, {m});",
                unsigned_max(s)
            ),
        );
    } else {
        uses.int_arg = true;
        let (lo, hi) = signed_bounds(s);
        g.line(indent, &format!("int64_t {dst}_n = 0;"));
        g.line(
            indent,
            &format!("cpl_st = cpl_bind_int_arg(cpl_c, {src}, (int64_t){lo}, (int64_t){hi}, &{dst}_n, {c}, {m});"),
        );
    }
    if src == "cpl_f" {
        g.line(indent, "if (cpl_st != CPL_OK) {");
        g.line(indent + 1, "(void)cpl_value_release(cpl_rt, cpl_f);");
        g.line(indent + 1, "return cpl_st;");
        g.line(indent, "}");
    } else {
        g.line(indent, "if (cpl_st != CPL_OK)");
        g.line(indent + 1, "return cpl_st;");
    }
    g.line(
        indent,
        &format!("{} {dst} = ({}){dst}_n;", s.c_type(), s.c_type()),
    );
}

/// `src/<prefix>_bind_<binding>.c` of a declarative binding (cabi.md 6.3).
pub(crate) fn render_wrapper(
    p: &Program,
    prefix: &str,
    plan: &InteropPlan,
    b: &BindingPlan,
    spec: &CBindSpec,
) -> String {
    let names = adapter_op_names(p, b.capability);
    let base = format!("{prefix}_adapter_{}", b.name);
    let mut uses = Uses::default();
    let mut ops = Vec::new();
    for (i, n) in names.iter().enumerate() {
        ops.push(render_op(p, prefix, plan, b, spec, i, n, &mut uses));
    }
    let mut out = String::new();
    let _ = writeln!(
        out,
        "/* {}: generated by cpl {} from {} for binding \"{}\"\n   (capability {}, backend c, isolation {}; spec/cabi.md 6.3). It defines the adapter contract of\n   6.2 over the library's own headers. Do not edit. */",
        wrapper_path(prefix, &b.name).trim_start_matches("src/"),
        cpl_diag::version::TOOLCHAIN,
        comment_text(&b.adapter),
        b.name,
        p.display_path(b.capability),
        if b.worker { "worker" } else { "in_process" }
    );
    out.push_str("#include <limits.h>\n#include <stddef.h>\n#include <stdint.h>\n#include <stdlib.h>\n#include <string.h>\n");
    if spec.state.is_some() {
        out.push_str("#include <pthread.h>\n");
    }
    let _ = writeln!(out, "#include \"cpl_adapter.h\"\n#include \"{prefix}.h\"");
    for h in &spec.headers {
        let _ = writeln!(out, "#include {}", c_string(h));
    }
    out.push('\n');
    // the contract's declarations
    let _ = writeln!(
        out,
        "CPL_ADAPTER cpl_status {base}_init(cpl_runtime *rt, void **state, cpl_error **error);"
    );
    let _ = writeln!(out, "CPL_ADAPTER void {base}_fini(void *state);");
    for (i, n) in names.iter().enumerate() {
        let _ = writeln!(
            out,
            "CPL_ADAPTER cpl_status {base}_{n}({});",
            contract_params(p, b.capability, i).join(", ")
        );
    }
    out.push('\n');
    if let Some(st) = &spec.state {
        let _ = writeln!(
            out,
            "/* [state]: one {} per grant, with the mutex held around every library call that receives it */\ntypedef struct cpl_bind_state {{\n    {} object;\n    pthread_mutex_t lock;\n}} cpl_bind_state;\n",
            comment_text(&st.c_type),
            st.c_type
        );
    }
    for (used, text) in [
        (uses.fail, HELPER_FAIL),
        (uses.status, HELPER_STATUS),
        (uses.int_arg, HELPER_INT_ARG),
        (uses.uint_arg, HELPER_UINT_ARG),
        (uses.dec_i64, HELPER_DEC_I64),
        (uses.dec_u64, HELPER_DEC_U64),
    ] {
        if used {
            out.push_str(text);
            out.push('\n');
        }
    }
    // init and fini
    match &spec.state {
        Some(st) => {
            let _ = writeln!(
                out,
                "cpl_status {base}_init(cpl_runtime *cpl_rt, void **cpl_state, cpl_error **cpl_error_)\n{{\n    (void)cpl_rt;\n    (void)cpl_error_;\n    cpl_bind_state *cpl_s = (cpl_bind_state *)malloc(sizeof *cpl_s);\n    if (cpl_s == NULL)\n        return CPL_E_NOMEM;\n    cpl_s->object = {}();\n    if (cpl_s->object == NULL) {{\n        free(cpl_s);\n        return CPL_E_STATE;                             /* interop_bind_failed, kind Unavailable (stdlib S45) */\n    }}\n    if (pthread_mutex_init(&cpl_s->lock, NULL) != 0) {{\n        {}(cpl_s->object);\n        free(cpl_s);\n        return CPL_E_NOMEM;\n    }}\n    *cpl_state = cpl_s;\n    return CPL_OK;\n}}\n",
                st.create, st.destroy
            );
            let _ = writeln!(
                out,
                "void {base}_fini(void *cpl_state)\n{{\n    cpl_bind_state *cpl_s = (cpl_bind_state *)cpl_state;\n    {}(cpl_s->object);\n    (void)pthread_mutex_destroy(&cpl_s->lock);\n    free(cpl_s);\n}}\n",
                st.destroy
            );
        }
        None => {
            let _ = writeln!(
                out,
                "cpl_status {base}_init(cpl_runtime *cpl_rt, void **cpl_state, cpl_error **cpl_error_)\n{{\n    (void)cpl_rt;\n    (void)cpl_error_;\n    *cpl_state = NULL;\n    return CPL_OK;\n}}\n"
            );
            let _ = writeln!(
                out,
                "void {base}_fini(void *cpl_state)\n{{\n    (void)cpl_state;\n}}\n"
            );
        }
    }
    for (i, text) in ops.iter().enumerate() {
        out.push_str(text);
        if i + 1 < ops.len() {
            out.push('\n');
        }
    }
    out
}

/// The parameters of an adapter operation function with the contract's names (6.2).
pub(crate) fn contract_params(p: &Program, cap: DefId, op: usize) -> Vec<String> {
    let mut v = vec!["void *state".to_string(), "cpl_call *call".to_string()];
    v.extend(crate::api::adapter_params(p, cap, op));
    v.push("cpl_value *result".to_string());
    v
}
