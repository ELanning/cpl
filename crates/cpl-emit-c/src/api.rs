//! The exports of a package (`spec/cabi.md` Section 5): the export models shared by the generated header, the API unit
//! `<prefix>_api.c` (the export functions, the type accessors, the typed capability constructors with their binding
//! layers), and the export table of the program tables.

use crate::map::{self, Form};
use crate::print::{c_string, comment_text};
use crate::tables::TypeTable;
use crate::InteropPlan;
use cpl_hir::{DefId, DefKind, Program, Ty};
use cpl_project::IntMapping;
use cpl_sema::interop::type_text;
use std::collections::BTreeSet;
use std::fmt::Write as _;

/// One positional parameter of an export or of a typed callback.
pub(crate) struct ParamModel {
    #[allow(dead_code)]
    pub cpl: String,
    pub c: String,
    pub len: Option<String>,
    pub form: Form,
    pub ty: Ty,
}

/// One entry of the export set (5.1).
pub(crate) struct ExportModel {
    pub symbol: String,
    pub function: DefId,
    pub params: Vec<ParamModel>,
    /// (C name, CPL name, capability)
    pub using: Vec<(String, String, DefId)>,
    pub split: bool,
    pub ok_form: Form,
    pub err_form: Form,
    pub ok_ty: Ty,
    pub err_ty: Ty,
    pub int: IntMapping,
}

impl ExportModel {
    /// The outputs as (C type, C name).
    pub fn outputs(&self) -> Vec<(&'static str, &'static str, Form)> {
        let mut out = Vec::new();
        if self.split {
            if self.ok_form != Form::Unit {
                out.push((self.ok_form.c_type(), "ok_out", self.ok_form));
            }
            if self.err_form != Form::Unit {
                out.push((self.err_form.c_type(), "err_out", self.err_form));
            }
        } else if self.ok_form != Form::Unit {
            out.push((self.ok_form.c_type(), "out", self.ok_form));
        }
        out
    }
}

pub(crate) fn export_models(p: &Program, plan: &InteropPlan) -> Vec<ExportModel> {
    let mut models = Vec::new();
    for e in &plan.exports {
        let Some(fd) = p.def(e.function).kind.as_fn() else {
            continue;
        };
        let mut names: Vec<String> = fd.params.iter().map(|x| x.name.clone()).collect();
        names.extend(fd.using.iter().map(|u| u.name.clone()));
        let c = map::c_names(&names);
        let mut taken: BTreeSet<String> = c.iter().cloned().collect();
        let mut params = Vec::new();
        for (i, param) in fd.params.iter().enumerate() {
            let form = map::param_form(p, &param.ty, e.int);
            let len = if matches!(form, Form::Text | Form::Bytes) {
                let l = map::len_name(&c[i], &taken);
                taken.insert(l.clone());
                Some(l)
            } else {
                None
            };
            params.push(ParamModel {
                cpl: param.name.clone(),
                c: c[i].clone(),
                len,
                form,
                ty: param.ty.clone(),
            });
        }
        let using = fd
            .using
            .iter()
            .enumerate()
            .map(|(i, u)| {
                let cap = match &u.ty {
                    Ty::Capability(d) => *d,
                    _ => DefId(u32::MAX),
                };
                (c[fd.params.len() + i].clone(), u.name.clone(), cap)
            })
            .collect();
        let (split, ok_form, err_form, ok_ty, err_ty) = match map::split_result(p, &fd.ret) {
            Some((t, er)) => (
                true,
                map::result_form(p, &t, e.int),
                map::result_form(p, &er, IntMapping::Handle),
                t,
                er,
            ),
            None => (
                false,
                map::result_form(p, &fd.ret, e.int),
                Form::Unit,
                fd.ret.clone(),
                Ty::Error,
            ),
        };
        models.push(ExportModel {
            symbol: e.symbol.clone(),
            function: e.function,
            params,
            using,
            split,
            ok_form,
            err_form,
            ok_ty,
            err_ty,
            int: e.int,
        });
    }
    models
}

/// The prototype of an export, its parameter groups on lines of their own aligned after the parenthesis (5.3).
pub(crate) fn export_prototype(m: &ExportModel, prefix_kw: &str) -> String {
    let head = format!("{prefix_kw}cpl_status {}(", m.symbol);
    let pad = " ".repeat(head.len());
    let mut first = vec!["cpl_runtime *rt".to_string()];
    for prm in &m.params {
        match prm.form {
            Form::Unit => {}
            Form::Text => first.push(format!(
                "const char *{}, size_t {}",
                prm.c,
                prm.len.as_deref().unwrap_or("len")
            )),
            Form::Bytes => first.push(format!(
                "const uint8_t *{}, size_t {}",
                prm.c,
                prm.len.as_deref().unwrap_or("len")
            )),
            f => first.push(format!("{} {}", f.c_type(), prm.c)),
        }
    }
    for (c, _, _) in &m.using {
        first.push(format!("cpl_cap {c}"));
    }
    let mut s = head.clone();
    s.push_str(&first.join(", "));
    s.push(',');
    let outs = m.outputs();
    if !outs.is_empty() {
        s.push('\n');
        s.push_str(&pad);
        let items: Vec<String> = outs.iter().map(|(t, n, _)| format!("{t} *{n}")).collect();
        s.push_str(&items.join(", "));
        s.push(',');
    }
    s.push('\n');
    s.push_str(&pad);
    s.push_str("const cpl_call_options *options, cpl_error **error)");
    s
}

/// The type text of a type as written in a comment (`Program::display_ty`).
fn written(p: &Program, ty: &Ty) -> String {
    p.display_ty(ty)
}

/// The signature text of a function after its name: `(items: List<Int>, size: Int) -> Result<...>`, using parameters
/// as `using db: PageRead`.
pub(crate) fn signature_text(p: &Program, f: DefId) -> String {
    let fd = p.def(f).kind.as_fn().expect("a function");
    let g = p.def(f).kind.generics();
    let mut parts: Vec<String> = fd
        .params
        .iter()
        .map(|x| format!("{}: {}", x.name, p.display_ty_in(&x.ty, g)))
        .collect();
    parts.extend(
        fd.using
            .iter()
            .map(|u| format!("using {}: {}", u.name, p.display_ty_in(&u.ty, g))),
    );
    format!("({}) -> {}", parts.join(", "), p.display_ty_in(&fd.ret, g))
}

/// `effects { db.get, ... }` of a function (its declared bound).
pub(crate) fn effects_text(p: &Program, f: DefId) -> String {
    let fd = p.def(f).kind.as_fn().expect("a function");
    let locals = &fd.locals;
    let refs: Vec<String> = match &fd.effects.declared {
        Some(set) => set
            .iter()
            .map(|e| {
                let name = locals
                    .get(e.param.0 as usize)
                    .map(|l| l.name.clone())
                    .unwrap_or_default();
                format!("{name}.{}", e.op)
            })
            .collect(),
        None => Vec::new(),
    };
    if refs.is_empty() {
        "effects {}".to_string()
    } else {
        format!("effects {{ {} }}", refs.join(", "))
    }
}

fn mapping_text(int: IntMapping) -> Option<&'static str> {
    match int {
        IntMapping::Handle => None,
        IntMapping::Int64 => Some("int64"),
        IntMapping::Int32 => Some("int32"),
        IntMapping::Uint64 => Some("uint64"),
        IntMapping::Uint32 => Some("uint32"),
    }
}

/// The comment of an export (5.3).
pub(crate) fn export_comment(p: &Program, m: &ExportModel) -> String {
    let d = p.def(m.function);
    let mut lines = Vec::new();
    lines.push(format!(
        "{}{}",
        p.display_path(m.function),
        signature_text(p, m.function)
    ));
    let identity = if d.provisional {
        format!("identity {} (provisional)", d.stable_id.as_str())
    } else {
        format!("identity {}", d.stable_id.as_str())
    };
    let mut l2 = format!("{identity}; {}", effects_text(p, m.function));
    if let Some(mt) = mapping_text(m.int) {
        l2.push_str(&format!("; {mt}"));
    }
    lines.push(l2);
    let mut notes = Vec::new();
    for prm in &m.params {
        match prm.form {
            Form::Text | Form::Bytes => notes.push(format!("{} is copied in.", prm.c)),
            Form::Handle => notes.push(format!("{} is borrowed.", prm.c)),
            _ => {}
        }
    }
    for (c, _, _) in &m.using {
        notes.push(format!("{c} is borrowed."));
    }
    if !notes.is_empty() {
        lines.push(notes.join(" "));
    }
    let describe = |form: Form, ty: &Ty, name: &str| -> String {
        match form {
            Form::I64 | Form::I32 | Form::U64 | Form::U32 => {
                format!("*{name} is the value as {}", form.c_type())
            }
            Form::Bool => format!("*{name} is 0 or 1"),
            _ => format!("*{name} is an owned {}", written(p, ty)),
        }
    };
    if m.split {
        let mut s = String::new();
        if m.ok_form == Form::Unit {
            s.push_str("CPL_OK: no output.");
        } else {
            s.push_str(&format!(
                "CPL_OK: {}.",
                describe(m.ok_form, &m.ok_ty, "ok_out")
            ));
        }
        if m.err_form != Form::Unit {
            s.push_str(&format!(
                " CPL_ERR: {}.",
                describe(m.err_form, &m.err_ty, "err_out")
            ));
        }
        lines.push(s);
    } else if m.ok_form == Form::Unit {
        lines.push("CPL_OK: no output.".to_string());
    } else {
        lines.push(format!("CPL_OK: {}.", describe(m.ok_form, &m.ok_ty, "out")));
    }
    lines.push("CPL_FAULT and CPL_E_*: *error, when error is not NULL.".to_string());
    // comment_text replaced the line breaks: rebuild the lines
    let mut out2 = String::from("/* ");
    for (i, l) in lines.iter().enumerate() {
        if i > 0 {
            out2.push_str("\n   ");
        }
        out2.push_str(&comment_text(l));
    }
    out2.push_str(" */\n");
    out2
}

/// The type accessors' name part and the listed types (5.2, 5.3): (c_name, text, ty).
pub(crate) fn listed_types(plan: &InteropPlan) -> Vec<(String, String, Ty)> {
    let mut v: Vec<(String, String, Ty)> = plan
        .types
        .iter()
        .map(|t| (t.c_name.clone(), t.text.clone(), t.ty.clone()))
        .collect();
    v.sort_by(|a, b| a.1.as_bytes().cmp(b.1.as_bytes()));
    v
}

/// The callback parameter list of operation `op` of a capability (5.7): spans for Text and Bytes, `int32_t` for
/// Bool, borrowed handles otherwise.
pub(crate) fn callback_params(p: &Program, cap: DefId, op: usize) -> Vec<ParamModel> {
    let od = &p.def(cap).kind.as_capability().expect("a capability").ops[op];
    let names: Vec<String> = od.params.iter().map(|x| x.name.clone()).collect();
    let c = map::c_names(&names);
    let mut taken: BTreeSet<String> = c.iter().cloned().collect();
    od.params
        .iter()
        .enumerate()
        .map(|(i, x)| {
            let base = cpl_eval::types::strip(p, &x.ty);
            let form = match base {
                Ty::Text => Form::Text,
                Ty::Bytes => Form::Bytes,
                Ty::Bool => Form::Bool,
                _ => Form::Handle,
            };
            let len = if matches!(form, Form::Text | Form::Bytes) {
                let l = map::len_name(&c[i], &taken);
                taken.insert(l.clone());
                Some(l)
            } else {
                None
            };
            ParamModel {
                cpl: x.name.clone(),
                c: c[i].clone(),
                len,
                form,
                ty: x.ty.clone(),
            }
        })
        .collect()
}

/// The C names of a capability's operations as struct members (unique, not reserved).
pub(crate) fn op_members(p: &Program, cap: DefId) -> Vec<String> {
    let cd = p.def(cap).kind.as_capability().expect("a capability");
    let names: Vec<String> = cd.ops.iter().map(|o| o.name.clone()).collect();
    map::c_names(&names)
}

/// The listed capabilities in qualified-name order.
pub(crate) fn listed_capabilities(p: &Program, plan: &InteropPlan) -> Vec<DefId> {
    let mut caps = plan.capabilities.clone();
    caps.sort_by_key(|d| p.display_path(*d));
    caps.dedup();
    caps
}

/// The API unit's export functions, type accessors, and typed capability constructors.
pub(crate) fn render_api(
    p: &Program,
    plan: &InteropPlan,
    prefix: &str,
    tt: &TypeTable,
    models: &[ExportModel],
) -> String {
    let mut out = String::new();
    for (c_name, text, _) in listed_types(plan) {
        let id = tt.ids.get(&text).copied().unwrap_or(0);
        let _ = writeln!(
            out,
            "CPLRT_EXPORT cpl_type {prefix}_type_{c_name}(void)        /* {} */\n{{\n    return {id}u;\n}}",
            comment_text(&text)
        );
    }
    for cap in listed_capabilities(p, plan) {
        let name = cpl_sema::interop::snake_case(&p.def(cap).name.display());
        let ops_t = format!("{prefix}_{name}_ops");
        let cd = p.def(cap).kind.as_capability().expect("a capability");
        let members = op_members(p, cap);
        let n = cap.0;
        let _ = writeln!(
            out,
            "/* {}: the binding layer over the typed ops struct */",
            p.display_path(cap)
        );
        let _ = writeln!(
            out,
            "static cpl_status cpl_c{n}_invoke(const cplrt_capref *cap, cpl_call *call, uint32_t op, const cplrt_w *args,\n                                 cplrt_w *result)\n{{"
        );
        let _ = writeln!(
            out,
            "    const {ops_t} *ops = (const {ops_t} *)cplrt_cap_ops(cap);"
        );
        out.push_str("    cpl_value r = { { 0, 0 } };\n    cpl_status st;\n    switch (op) {\n");
        for (i, _op) in cd.ops.iter().enumerate() {
            let params = callback_params(p, cap, i);
            let mut args = vec!["cap->user".to_string(), "call".to_string()];
            for (k, prm) in params.iter().enumerate() {
                match prm.form {
                    Form::Text => {
                        args.push(format!("cplrt_text_of(args[{k}])->utf8"));
                        args.push(format!("cplrt_text_of(args[{k}])->bytes"));
                    }
                    Form::Bytes => {
                        args.push(format!("cplrt_bytes_of(args[{k}])->data"));
                        args.push(format!("cplrt_bytes_of(args[{k}])->len"));
                    }
                    Form::Bool => args.push(format!("(int32_t)(args[{k}] == CPLRT_TRUE)")),
                    _ => args.push(format!("cplrt_call_borrow(call, args[{k}])")),
                }
            }
            args.push("&r".to_string());
            let _ = writeln!(
                out,
                "    case {i}:\n        st = ops->{}({});\n        break;",
                members[i],
                args.join(", ")
            );
        }
        out.push_str("    default:\n        return CPL_E_INTERNAL;\n    }\n    return cplrt_call_take(call, st, r, result);\n}\n");
        let _ = writeln!(
            out,
            "static void cpl_c{n}_drop(const cplrt_capref *cap)\n{{"
        );
        let _ = writeln!(
            out,
            "    const {ops_t} *ops = (const {ops_t} *)cplrt_cap_ops(cap);"
        );
        let _ = writeln!(
            out,
            "    if (ops->struct_size >= offsetof({ops_t}, drop) + sizeof ops->drop && ops->drop != NULL)\n        ops->drop(cap->user);\n}}"
        );
        let _ = writeln!(
            out,
            "static const cplrt_capimpl cpl_c{n}_impl = {{ {}u, cpl_c{n}_invoke, cpl_c{n}_drop }};",
            cd.ops.len()
        );
        let offsets: Vec<String> = members
            .iter()
            .map(|m| format!("offsetof({ops_t}, {m})"))
            .collect();
        let _ = writeln!(
            out,
            "static const size_t cpl_c{n}_ops[{}] = {{ {} }};",
            offsets.len().max(1),
            if offsets.is_empty() {
                "0".to_string()
            } else {
                offsets.join(", ")
            }
        );
        let _ = writeln!(
            out,
            "CPLRT_EXPORT cpl_status {prefix}_{name}_new(cpl_runtime *rt, const {ops_t} *ops, void *user, cpl_cap *out,\n                                 cpl_error **error)\n{{\n    return cplrt_cap_create(rt, {n}u, &cpl_c{n}_impl, ops, sizeof({ops_t}), cpl_c{n}_ops, user, out, error);\n}}"
        );
    }
    for (index, m) in models.iter().enumerate() {
        let _ = writeln!(out, "/* {}: export {index} */", p.display_path(m.function));
        out.push_str(&export_prototype(m, "CPLRT_EXPORT "));
        out.push_str("\n{\n");
        let nargs = m.params.len();
        if nargs > 0 {
            let _ = writeln!(out, "    cplrt_arg cpl_args_[{nargs}];");
        }
        let outs = m.outputs();
        let _ = writeln!(out, "    cplrt_out cpl_outs_[2];");
        out.push_str("    memset(cpl_outs_, 0, sizeof cpl_outs_);\n");
        if nargs > 0 {
            out.push_str("    memset(cpl_args_, 0, sizeof cpl_args_);\n");
        }
        for (i, prm) in m.params.iter().enumerate() {
            match prm.form {
                Form::Unit => {}
                Form::Text | Form::Bytes => {
                    let _ = writeln!(out, "    cpl_args_[{i}].span.ptr = {};", prm.c);
                    let _ = writeln!(
                        out,
                        "    cpl_args_[{i}].span.len = {};",
                        prm.len.as_deref().unwrap_or("0")
                    );
                }
                f => {
                    let _ = writeln!(out, "    cpl_args_[{i}].{} = {};", f.member(), prm.c);
                }
            }
        }
        // the outputs in the table's order: [ok_out, err_out] for a split Result, else [out]
        if m.split {
            for (k, name) in ["ok_out", "err_out"].iter().enumerate() {
                if let Some((_, _, f)) = outs.iter().find(|(_, n, _)| n == name) {
                    let _ = writeln!(out, "    cpl_outs_[{k}].{} = {name};", f.member());
                }
            }
        } else if let Some((_, _, f)) = outs.first() {
            let _ = writeln!(out, "    cpl_outs_[0].{} = out;", f.member());
        }
        let caps = if m.using.is_empty() {
            "NULL".to_string()
        } else {
            let names: Vec<String> = m.using.iter().map(|(c, _, _)| c.clone()).collect();
            let _ = writeln!(
                out,
                "    const cpl_cap cpl_caps_[{}] = {{ {} }};",
                names.len(),
                names.join(", ")
            );
            "cpl_caps_".to_string()
        };
        let _ = writeln!(
            out,
            "    return cplrt_export_call(rt, {index}u, {}, {caps}, cpl_outs_, options, error);\n}}",
            if nargs > 0 { "cpl_args_" } else { "NULL" }
        );
    }
    out
}

/// The export table of the program tables (Appendix B.1 `cplrt_export`).
pub(crate) fn render_export_table(p: &Program, tt: &TypeTable, models: &[ExportModel]) -> String {
    if models.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n/* exports, in export order (5.1) */\n");
    let mut rows = Vec::new();
    for (i, m) in models.iter().enumerate() {
        let forms: Vec<String> = m.params.iter().map(|x| x.form.code().to_string()).collect();
        let types: Vec<String> = m
            .params
            .iter()
            .map(|x| {
                format!(
                    "{}u",
                    tt.ids.get(&type_text(p, &x.ty)).copied().unwrap_or(0)
                )
            })
            .collect();
        let caps: Vec<String> = m
            .using
            .iter()
            .map(|(_, _, c)| format!("{}u", c.0))
            .collect();
        let fname = if forms.is_empty() {
            "NULL".to_string()
        } else {
            let _ = writeln!(
                out,
                "static const uint32_t cpl_ef{i}[{}] = {{ {} }};",
                forms.len(),
                forms.join(", ")
            );
            let _ = writeln!(
                out,
                "static const uint32_t cpl_et{i}[{}] = {{ {} }};",
                types.len(),
                types.join(", ")
            );
            format!("cpl_ef{i}")
        };
        let tname = if forms.is_empty() {
            "NULL".to_string()
        } else {
            format!("cpl_et{i}")
        };
        let cname = if caps.is_empty() {
            "NULL".to_string()
        } else {
            let _ = writeln!(
                out,
                "static const uint32_t cpl_ec{i}[{}] = {{ {} }};",
                caps.len(),
                caps.join(", ")
            );
            format!("cpl_ec{i}")
        };
        rows.push(format!(
            "    {{ {}, {}u, {}u, {fname}, {tname}, {}u, {cname}, {}u, {}, {} }}",
            c_string(&m.symbol),
            m.function.0,
            m.params.len(),
            m.using.len(),
            u32::from(m.split),
            m.ok_form.code(),
            m.err_form.code()
        ));
    }
    let _ = writeln!(
        out,
        "static const cplrt_export cpl_exports[{}] = {{\n{}\n}};",
        rows.len(),
        rows.join(",\n")
    );
    out
}

/// Whether a definition is a struct or enum declaration (its constants are listed).
pub(crate) fn decl_kind(p: &Program, ty: &Ty) -> Option<DefId> {
    match ty {
        Ty::Adt(d, args) if args.is_empty() => match &p.def(*d).kind {
            DefKind::Struct(_) | DefKind::Enum(_) => Some(*d),
            _ => None,
        },
        _ => None,
    }
}

/// The adapter contract names of a binding's operations (5.2): the operation's name with `_` appended while it is
/// `init`, `fini`, or the name an earlier operation received.
pub(crate) fn adapter_op_names(p: &Program, cap: DefId) -> Vec<String> {
    let cd = p.def(cap).kind.as_capability().expect("a capability");
    let mut used: BTreeSet<String> = ["init".to_string(), "fini".to_string()]
        .into_iter()
        .collect();
    let mut out = Vec::new();
    for op in &cd.ops {
        let mut n = op.name.clone();
        while used.contains(&n) {
            n.push('_');
        }
        used.insert(n.clone());
        out.push(n);
    }
    out
}

/// The parameters of an adapter operation function: the typed callback's (5.7) after `state` and `call`.
pub(crate) fn adapter_params(p: &Program, cap: DefId, op: usize) -> Vec<String> {
    callback_params(p, cap, op)
        .iter()
        .map(|x| match x.form {
            Form::Text => format!(
                "const char *{}, size_t {}",
                x.c,
                x.len.as_deref().unwrap_or("len")
            ),
            Form::Bytes => format!(
                "const uint8_t *{}, size_t {}",
                x.c,
                x.len.as_deref().unwrap_or("len")
            ),
            Form::Bool => format!("int32_t {}", x.c),
            _ => format!("cpl_value {}", x.c),
        })
        .collect()
}

/// `include/<prefix>_adapter_<binding>.h` of a reviewed-wrapper binding (6.2, Appendix C.2).
pub(crate) fn render_adapter_header(
    p: &Program,
    prefix: &str,
    b: &cpl_sema::interop::BindingPlan,
) -> String {
    let guard = format!(
        "{}_ADAPTER_{}_H",
        prefix.to_ascii_uppercase(),
        b.name.to_ascii_uppercase()
    );
    let base = format!("{prefix}_adapter_{}", b.name);
    let mut h = String::new();
    let _ = writeln!(
        h,
        "/* {base}.h: generated by cpl {} for binding \"{}\"\n   (capability {}, backend c, isolation {}). Define every function below in\n   {}. They have hidden visibility and are called only by the runtime. */",
        cpl_diag::version::TOOLCHAIN,
        b.name,
        p.display_path(b.capability),
        if b.worker { "worker" } else { "in_process" },
        b.adapter
    );
    let _ = writeln!(
        h,
        "#ifndef {guard}\n#define {guard} 1\n#include \"cpl_adapter.h\"\n#include \"{prefix}.h\""
    );
    h.push_str("#ifdef __cplusplus\nextern \"C\" {\n#endif\n");
    h.push_str("/* once per grant: the adapter's state for this capability (*state and *error are NULL on entry); on any status\n   but CPL_OK the grant fails and fini is not called */\n");
    let _ = writeln!(
        h,
        "CPL_ADAPTER cpl_status {base}_init(cpl_runtime *rt, void **state, cpl_error **error);"
    );
    h.push_str("/* once, after the last reference to the granted capability is released and no call is in flight; any thread */\n");
    let _ = writeln!(h, "CPL_ADAPTER void {base}_fini(void *state);");
    let cd = p
        .def(b.capability)
        .kind
        .as_capability()
        .expect("a capability");
    let names = adapter_op_names(p, b.capability);
    for (i, op) in cd.ops.iter().enumerate() {
        let params = callback_params(p, b.capability, i);
        let ps: Vec<String> = op
            .params
            .iter()
            .map(|x| format!("{}: {}", x.name, p.display_ty(&x.ty)))
            .collect();
        let _ = writeln!(
            h,
            "/* fn {}({}) -> {}",
            op.name,
            ps.join(", "),
            p.display_ty(&op.ret)
        );
        let notes: Vec<String> = params
            .iter()
            .map(|x| match x.form {
                Form::Bytes => format!("{}: a borrowed byte span, valid for the call", x.c),
                Form::Text => format!("{}: a borrowed UTF-8 span, valid for the call", x.c),
                Form::Bool => format!("{}: 0 or 1", x.c),
                _ => format!("{}: borrowed for the call", x.c),
            })
            .collect();
        if !notes.is_empty() {
            let _ = writeln!(h, "   {}", notes.join("; "));
        }
        let _ = writeln!(
            h,
            "   on CPL_OK *result: an owned {}, validated by the runtime before CPL sees it */",
            p.display_ty(&op.ret)
        );
        let head = format!("CPL_ADAPTER cpl_status {base}_{}(", names[i]);
        let pad = " ".repeat(head.len());
        let args = adapter_params(p, b.capability, i);
        if args.is_empty() {
            let _ = writeln!(
                h,
                "{head}void *state, cpl_call *call,\n{pad}cpl_value *result);"
            );
        } else {
            let _ = writeln!(
                h,
                "{head}void *state, cpl_call *call,\n{pad}{},\n{pad}cpl_value *result);",
                args.join(", ")
            );
        }
    }
    h.push_str("#ifdef __cplusplus\n}\n#endif\n#endif\n");
    h
}

/// The grants of the API unit (5.7, 6.1): per backend-c binding, the binding layer over its adapter's functions and
/// `<prefix>_bind_<binding>`.
pub(crate) fn render_grants(p: &Program, plan: &InteropPlan, prefix: &str) -> String {
    let mut out = String::new();
    for (gi, b) in plan.bindings.iter().enumerate() {
        let base = format!("{prefix}_adapter_{}", b.name);
        let cd = p
            .def(b.capability)
            .kind
            .as_capability()
            .expect("a capability");
        let names = adapter_op_names(p, b.capability);
        let _ = writeln!(
            out,
            "/* [bindings.{}]: the binding layer over the adapter's functions */",
            b.name
        );
        let _ = writeln!(
            out,
            "static cpl_status cpl_g{gi}_invoke(const cplrt_capref *cap, cpl_call *call, uint32_t op, const cplrt_w *args,\n                                 cplrt_w *result)\n{{"
        );
        out.push_str("    cpl_value r = { { 0, 0 } };\n    cpl_status st;\n    switch (op) {\n");
        for (i, _op) in cd.ops.iter().enumerate() {
            let params = callback_params(p, b.capability, i);
            let mut args = vec!["cap->user".to_string(), "call".to_string()];
            for (k, prm) in params.iter().enumerate() {
                match prm.form {
                    Form::Text => {
                        args.push(format!("cplrt_text_of(args[{k}])->utf8"));
                        args.push(format!("cplrt_text_of(args[{k}])->bytes"));
                    }
                    Form::Bytes => {
                        args.push(format!("cplrt_bytes_of(args[{k}])->data"));
                        args.push(format!("cplrt_bytes_of(args[{k}])->len"));
                    }
                    Form::Bool => args.push(format!("(int32_t)(args[{k}] == CPLRT_TRUE)")),
                    _ => args.push(format!("cplrt_call_borrow(call, args[{k}])")),
                }
            }
            args.push("&r".to_string());
            let _ = writeln!(
                out,
                "    case {i}:\n        st = {base}_{}({});\n        break;",
                names[i],
                args.join(", ")
            );
        }
        out.push_str("    default:\n        return CPL_E_INTERNAL;\n    }\n    return cplrt_call_take(call, st, r, result);\n}\n");
        let _ = writeln!(out, "static void cpl_g{gi}_drop(const cplrt_capref *cap)\n{{\n    {base}_fini(cap->user);\n}}");
        let _ = writeln!(
            out,
            "const cplrt_capimpl CPLPKG(grant_{gi}) = {{ {}u, cpl_g{gi}_invoke, cpl_g{gi}_drop }};",
            cd.ops.len()
        );
        let _ = writeln!(
            out,
            "CPLRT_EXPORT cpl_status {prefix}_bind_{}(cpl_runtime *rt, cpl_cap *out, cpl_error **error)\n{{\n    return cplrt_grant_call(rt, {gi}u, out, error);\n}}",
            b.name
        );
    }
    // Milestone 3: a backend-python binding's grant starts its worker (spec/python.md 6)
    for (i, b) in plan.python_bindings.iter().enumerate() {
        let gi = plan.bindings.len() + i;
        let _ = writeln!(
            out,
            "/* [bindings.{}]: backend python, {}: its adapter runs in a Python worker (spec/python.md 6) */",
            b.name, b.module
        );
        let _ = writeln!(
            out,
            "CPLRT_EXPORT cpl_status {prefix}_bind_{}(cpl_runtime *rt, cpl_cap *out, cpl_error **error)\n{{\n    return cplrt_grant_call(rt, {gi}u, out, error);\n}}",
            b.name
        );
    }
    out
}

/// The grant table of the program tables (Appendix B.1 `cplrt_grant`), with the declarations it needs.
pub(crate) fn render_grant_table(p: &Program, plan: &InteropPlan, prefix: &str) -> String {
    if plan.ngrants() == 0 {
        return String::new();
    }
    let mut out = String::from(
        "\n/* grants: the backend-c bindings, in name order (6.1), then the backend-python ones (spec/python.md 6) */\n",
    );
    let mut rows = Vec::new();
    for (gi, b) in plan.bindings.iter().enumerate() {
        let base = format!("{prefix}_adapter_{}", b.name);
        let _ = writeln!(
            out,
            "cpl_status {base}_init(cpl_runtime *rt, void **state, cpl_error **error);"
        );
        let _ = writeln!(out, "void {base}_fini(void *state);");
        let _ = writeln!(out, "extern const cplrt_capimpl CPLPKG(grant_{gi});");
        rows.push(format!(
            "    {{ {}, {}u, {}, {base}_init, {base}_fini, &CPLPKG(grant_{gi}), {}u, {}u, {}u }}",
            c_string(&b.name),
            b.capability.0,
            c_string(&b.name),
            u32::from(b.form == cpl_project::AdapterForm::Declarative),
            if b.worker { 2 } else { 0 },
            b.timeout_ms.unwrap_or(0)
        ));
    }
    // Milestone 3: the backend-python bindings, always a worker outside a Python host (spec/python.md 6)
    for b in &plan.python_bindings {
        rows.push(format!(
            "    {{ {}, {}u, {}, NULL, NULL, NULL, 0u, 1u, {}u }}",
            c_string(&b.name),
            b.capability.0,
            c_string(&b.name),
            b.timeout_ms.unwrap_or(0)
        ));
    }
    let _ = p;
    let _ = writeln!(
        out,
        "static const cplrt_grant cpl_grants[{}] = {{\n{}\n}};",
        rows.len(),
        rows.join(",\n")
    );
    out
}
