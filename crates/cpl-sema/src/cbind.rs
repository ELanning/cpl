//! The semantic half of `project_cbind_invalid` (`spec/toolchain.md` 11.1, `spec/cabi.md` 6.3): a declarative
//! binding's operations and types checked against the capability it binds, after name resolution.

use crate::hir::{DefId, Program, Ty};
use crate::project::cbind::{ArgSpec, BindType, CBindError, CBindSpec, ReturnsSpec, Scalar};
use cpl_diag::Diagnostic;

fn strip(p: &Program, ty: &Ty) -> Ty {
    let mut t = ty.clone();
    for _ in 0..64 {
        match &t {
            Ty::Refined(_, d) => match p.def(*d).kind.as_refinement() {
                Some(rd) => t = rd.base.clone(),
                None => return t,
            },
            _ => return t,
        }
    }
    t
}

fn is_foreign_result(p: &Program, ty: &Ty) -> Option<Ty> {
    match ty {
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

fn is_unit(p: &Program, ty: &Ty) -> bool {
    matches!(ty, Ty::Adt(d, a) if *d == p.std_defs.unit && a.is_empty())
}

/// Whether the CPL type `ty` agrees with the binding type `bt` (cabi.md 6.3's table).
fn agrees(p: &Program, spec: &CBindSpec, bt: &BindType, ty: &Ty) -> Result<(), String> {
    let base = strip(p, ty);
    let ok = match bt {
        BindType::Scalar(Scalar::Bool) => base == Ty::Bool,
        BindType::Scalar(_) => base == Ty::Int,
        BindType::BytesSpan | BindType::OwnedBytes => base == Ty::Bytes,
        BindType::TextSpan | BindType::CStr | BindType::OwnedText => base == Ty::Text,
        BindType::Struct(name) => {
            let Some(s) = spec.structs.get(name) else {
                return Err(format!("struct:{name} names no [structs.{name}] table"));
            };
            match &base {
                Ty::Adt(d, _) => match p.def(*d).kind.as_struct() {
                    Some(sd) => {
                        let mut want: Vec<&str> =
                            sd.fields.iter().map(|f| f.name.as_str()).collect();
                        let mut have: Vec<&str> =
                            s.fields.iter().map(|(f, _, _)| f.as_str()).collect();
                        want.sort_unstable();
                        have.sort_unstable();
                        if want != have {
                            return Err(format!(
                                "[structs.{name}] maps the fields {have:?}, the CPL struct {} has {want:?}",
                                p.display_ty(ty)
                            ));
                        }
                        for (field, _, scalar) in &s.fields {
                            let fty = &sd
                                .fields
                                .iter()
                                .find(|f| &f.name == field)
                                .expect("compared above")
                                .ty;
                            agrees(p, spec, &BindType::Scalar(*scalar), fty)
                                .map_err(|reason| format!("field `{field}`: {reason}"))?;
                        }
                        true
                    }
                    None => false,
                },
                _ => false,
            }
        }
    };
    if ok {
        Ok(())
    } else {
        Err(format!(
            "the binding type does not fit the CPL type {}",
            p.display_ty(ty)
        ))
    }
}

/// The `project_cbind_invalid` diagnostics of binding `binding`, whose parsed `.cbind.toml` is `spec`, against
/// `capability`: exactly one `[ops.<name>]` per operation, every parameter once in `args`, the binding types against
/// the CPL types, the `ForeignError` arm of a fallible operation (cabi.md 6.3).
pub(crate) fn check_binding(
    program: &Program,
    capability: DefId,
    binding: &str,
    spec: &CBindSpec,
) -> Vec<Diagnostic> {
    let p = program;
    let mut out = Vec::new();
    let mut fail = |key: String, reason: String| {
        out.push(crate::project::cbind_invalid(
            binding,
            &CBindError {
                key,
                line: None,
                reason,
            },
        ));
    };
    let Some(cd) = p.def(capability).kind.as_capability() else {
        return out;
    };
    for name in spec.ops.keys() {
        if !cd.ops.iter().any(|o| &o.name == name) {
            fail(
                format!("ops.{name}"),
                format!("the capability has no operation `{name}`"),
            );
        }
    }
    for op in &cd.ops {
        let key = format!("ops.{}", op.name);
        let Some(os) = spec.ops.get(&op.name) else {
            fail(
                key,
                format!("the operation `{}` has no [ops.{}] table", op.name, op.name),
            );
            continue;
        };
        let mut seen: Vec<&str> = Vec::new();
        for a in &os.args {
            if let ArgSpec::Param { name, ty } = a {
                match op.params.iter().find(|x| &x.name == name) {
                    None => fail(
                        format!("{key}.args"),
                        format!("`{name}` is no parameter of the operation"),
                    ),
                    Some(param) => {
                        if seen.contains(&name.as_str()) {
                            fail(format!("{key}.args"), format!("`{name}` appears twice"));
                        }
                        seen.push(name);
                        if let Err(reason) = agrees(p, spec, ty, &param.ty) {
                            fail(format!("{key}.args"), format!("`{name}`: {reason}"));
                        }
                    }
                }
            }
        }
        for param in &op.params {
            if !seen.contains(&param.name.as_str()) {
                fail(
                    format!("{key}.args"),
                    format!("the parameter `{}` does not appear in `args`", param.name),
                );
            }
        }
        match &os.returns {
            ReturnsSpec::Status { .. } => match is_foreign_result(p, &op.ret) {
                None => fail(
                    format!("{key}.returns"),
                    "a fallible operation returns Result<T, std.foreign.ForeignError>".to_string(),
                ),
                Some(t) => match &os.result {
                    None if !is_unit(p, &t) => fail(
                        format!("{key}.result"),
                        format!("the Ok payload {} needs `result`", p.display_ty(&t)),
                    ),
                    Some(res) => {
                        if let Some(o) = os.out.iter().find(|o| &o.name == res) {
                            if let Err(reason) = agrees(p, spec, &o.ty, &t) {
                                fail(format!("{key}.result"), reason);
                            }
                        }
                    }
                    None => {}
                },
            },
            ReturnsSpec::Result { ty } => {
                let t = is_foreign_result(p, &op.ret).unwrap_or_else(|| op.ret.clone());
                if let Err(reason) = agrees(p, spec, &BindType::Scalar(*ty), &t) {
                    fail(format!("{key}.returns"), reason);
                }
            }
            ReturnsSpec::Void => {
                let t = is_foreign_result(p, &op.ret).unwrap_or_else(|| op.ret.clone());
                if !is_unit(p, &t) {
                    fail(
                        format!("{key}.returns"),
                        "a void return gives Unit".to_string(),
                    );
                }
            }
        }
    }
    out
}
