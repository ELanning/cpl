//! Models as inputs: the value JSON (`spec/toolchain.md` 7.3) of the values a model assigns, so
//! that a counterexample can be read, and run.

use crate::exec::{Exec, Mode, Path, Shared, Sym};
use crate::solver::{self, Model, Outcome};
use cpl_hir::{DefId, DefKind, Program, Ty};
use num_bigint::BigInt;
use num_traits::ToPrimitive;
use serde_json::{json, Map, Value};

const MAX_DEPTH: u32 = 8;

/// `$type`: the bare name of a prelude declaration, the qualified name of any other (T30).
pub fn type_name(p: &Program, def: DefId) -> String {
    let d = p.def(def);
    let prelude = p
        .module(d.module)
        .path
        .iter()
        .map(String::as_str)
        .eq(["std", "prelude"]);
    if prelude {
        d.name.display()
    } else {
        p.display_path(def)
    }
}

pub fn number(n: &BigInt) -> Value {
    serde_json::from_str(&n.to_string()).unwrap_or(Value::Null)
}

/// Renders symbolic values under one model, in the canonical space of `p`.
pub struct Renderer<'a> {
    pub p: &'a Program,
    pub shared: &'a Shared,
    pub model: &'a Model,
}

impl Renderer<'_> {
    pub fn value(&self, s: &Sym) -> Value {
        self.at(s, 0)
    }

    fn at(&self, s: &Sym, depth: u32) -> Value {
        if depth > MAX_DEPTH {
            return Value::Null;
        }
        match s {
            Sym::Int(l) => number(&l.eval(self.model)),
            Sym::Bool(f) => Value::Bool(f.eval(self.model)),
            Sym::Data {
                def,
                variant,
                fields,
            } => {
                let fields: Vec<Value> = fields.iter().map(|f| self.at(f, depth + 1)).collect();
                data(self.p, *def, *variant, fields)
            }
            Sym::Seq(items) => Value::Array(items.iter().map(|i| self.at(i, depth + 1)).collect()),
            Sym::Opaque { id, ty } => {
                if let Some(t) = self.shared.literal_texts.get(id) {
                    return Value::String(t.clone());
                }
                if let Ty::Adt(def, args) = ty {
                    if let Some(DefKind::Enum(e)) = self.p.defs.get(def.0 as usize).map(|d| &d.kind)
                    {
                        let k = self
                            .shared
                            .tags
                            .get(id)
                            .and_then(|v| self.model.get(v))
                            .and_then(|k| k.to_u32())
                            .unwrap_or(0);
                        let payload = match self.shared.payloads.get(&(*id, k)) {
                            Some((fields, _)) => {
                                fields.iter().map(|f| self.at(f, depth + 1)).collect()
                            }
                            None => e
                                .variants
                                .get(k as usize)
                                .map(|v| {
                                    v.payload
                                        .iter()
                                        .map(|t| {
                                            witness(
                                                self.p,
                                                &crate::exec::instantiate(t, args),
                                                depth + 1,
                                            )
                                        })
                                        .collect()
                                })
                                .unwrap_or_default(),
                        };
                        return data(self.p, *def, k, payload);
                    }
                }
                witness(self.p, ty, depth)
            }
        }
    }
}

/// The value JSON of a struct, wrapper, or enum value.
fn data(p: &Program, def: DefId, variant: u32, fields: Vec<Value>) -> Value {
    let Some(d) = p.defs.get(def.0 as usize) else {
        return Value::Null;
    };
    let mut m = Map::new();
    m.insert("$type".into(), Value::String(type_name(p, def)));
    match &d.kind {
        DefKind::Struct(s) => {
            for (f, v) in s.fields.iter().zip(fields) {
                m.insert(f.name.clone(), v);
            }
        }
        DefKind::Enum(e) => {
            if def == p.std_defs().unit {
                return json!({"$type": "Unit"});
            }
            let name = e
                .variants
                .get(variant as usize)
                .map(|v| v.name.clone())
                .unwrap_or_default();
            m.insert("$variant".into(), Value::String(name));
            if !fields.is_empty() {
                m.insert("$payload".into(), Value::Array(fields));
            }
        }
        DefKind::Wrapper(_) => {
            m.insert("$payload".into(), Value::Array(fields));
        }
        _ => return Value::Null,
    }
    Value::Object(m)
}

/// Some value of `ty` that satisfies its refinements and invariants, for a part of an input the
/// counterexample does not constrain.
pub fn witness(p: &Program, ty: &Ty, depth: u32) -> Value {
    if depth > MAX_DEPTH {
        return Value::Null;
    }
    match ty {
        Ty::Text => return Value::String(String::new()),
        Ty::Bytes => return json!({"$type": "Bytes", "hex": ""}),
        Ty::List(_) => return Value::Array(Vec::new()),
        Ty::Param(_) | Ty::Capability(_) | Ty::Gen(_) | Ty::Error => return Value::Null,
        _ => {}
    }
    let Some(root) = p.defs().find(|d| d.kind.as_fn().is_some()).map(|d| d.id) else {
        return Value::Null;
    };
    let mut shared = Shared::default();
    let mut path = Path::new(0);
    let v = {
        let mut x = Exec::new(p, &mut shared, root, Mode::Verify);
        x.fresh(&mut path, ty)
    };
    let cond = solver::Form::and(path.cond);
    match solver::solve(&cond, &shared.bools) {
        Outcome::Sat(model) => {
            // Pick the first variant of an enum and witness its payload.
            if let Sym::Opaque {
                ty: Ty::Adt(def, args),
                ..
            } = &v
            {
                if let Some(e) = p.def(*def).kind.as_enum() {
                    let payload = e
                        .variants
                        .first()
                        .map(|v| {
                            v.payload
                                .iter()
                                .map(|t| witness(p, &crate::exec::instantiate(t, args), depth + 1))
                                .collect()
                        })
                        .unwrap_or_default();
                    return data(p, *def, 0, payload);
                }
            }
            Renderer {
                p,
                shared: &shared,
                model: &model,
            }
            .at(&v, depth + 1)
        }
        _ => Value::Null,
    }
}
