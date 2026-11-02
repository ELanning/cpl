//! The input sweeps of suite D5 (toolchain 12.6): argument values drawn by the sweep rules from a ChaCha20 stream, on
//! the distributions of stdlib 8.3 that the property runner draws from.

use crate::interp::Interpreter;
use crate::test::gen::{self, Gen, Stream};
use crate::value::Value;
use crate::EvalConfig;
use cpl_hir::{DefKind, Program, Ty};

/// A stream of sweep draws.
pub struct Sweep {
    stream: Stream,
}

/// Why a draw gave no value: the type is no data type, or a refinement rejected 100 draws in a row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Undrawable {
    NotData(String),
    Rejected(String),
}

impl Sweep {
    /// A stream keyed by `key` (toolchain 12.6: `CPL_DIFF_SEED` and the function's identity).
    pub fn new(key: [u8; 32]) -> Sweep {
        Sweep {
            stream: Stream::new(key),
        }
    }

    /// A value of `ty` at `size` by the sweep rules: a type's distribution of stdlib 8.3 when it has one (`Int`,
    /// `Bool`, `Text`, `List<T>`, `Option<T>`, `Positive`); otherwise a struct field by field in declaration order, an
    /// enum by a variant drawn with `below(n)` over its `n` variants and then its payloads in order, a wrapper by its
    /// inner type, `Unit` as `Unit`, `Bytes` as the bytes of a `gen.list(gen.range(0, 255))` draw, and any other
    /// refinement by its base, a draw that its chain rejects being drawn again (100 rejections: undrawable).
    pub fn draw(&mut self, p: &Program, ty: &Ty, size: u32) -> Result<Value, Undrawable> {
        match ty {
            Ty::Int => Ok(gen::draw(&mut self.stream, &Gen::Int, size, p)),
            Ty::Bool => Ok(gen::draw(&mut self.stream, &Gen::Bool, size, p)),
            Ty::Text => Ok(gen::draw(&mut self.stream, &Gen::Text, size, p)),
            Ty::Bytes => {
                let g = Gen::List(Box::new(Gen::Range(0.into(), 255.into())));
                let drawn = gen::draw(&mut self.stream, &g, size, p);
                let Value::List(items) = &drawn else {
                    unreachable!("a list draw")
                };
                let bytes: Vec<u8> = items
                    .iter()
                    .map(|v| match v {
                        Value::Int(n) => u8::try_from(n.clone()).unwrap_or(0),
                        _ => 0,
                    })
                    .collect();
                Ok(Value::bytes(&bytes))
            }
            Ty::List(elem) => {
                let len = self.stream.below_u64(u64::from(size) + 1);
                let mut items = Vec::with_capacity(len as usize);
                for _ in 0..len {
                    items.push(self.draw(p, elem, size)?);
                }
                Ok(Value::list(items))
            }
            Ty::Adt(d, args) if *d == p.std_defs.option && args.len() == 1 => {
                if self.stream.word().is_multiple_of(4) {
                    Ok(Value::none(p))
                } else {
                    let v = self.draw(p, &args[0], size)?;
                    Ok(Value::some(p, v))
                }
            }
            Ty::Adt(d, args) => match &p.def(*d).kind {
                DefKind::Struct(s) => {
                    let mut fields = Vec::with_capacity(s.fields.len());
                    for f in &s.fields {
                        fields.push(self.draw(p, &crate::types::instantiate(&f.ty, args), size)?);
                    }
                    Ok(Value::Struct {
                        def: *d,
                        fields: fields.into(),
                    })
                }
                // an enum without variants has no values
                DefKind::Enum(e) if e.variants.is_empty() => {
                    Err(Undrawable::Rejected(p.display_ty(ty)))
                }
                DefKind::Enum(e) => {
                    let n = e.variants.len() as u64;
                    let variant = self.stream.below_u64(n) as usize;
                    let mut payload = Vec::new();
                    for t in &e.variants[variant].payload {
                        payload.push(self.draw(p, &crate::types::instantiate(t, args), size)?);
                    }
                    Ok(Value::Enum {
                        def: *d,
                        variant: variant as u32,
                        payload: payload.into(),
                    })
                }
                _ => Err(Undrawable::NotData(p.display_ty(ty))),
            },
            Ty::Wrapper(d) => match &p.def(*d).kind {
                DefKind::Wrapper(w) => {
                    let inner = self.draw(p, &w.inner, size)?;
                    Ok(Value::Wrapper {
                        def: *d,
                        inner: inner.into(),
                    })
                }
                _ => Err(Undrawable::NotData(p.display_ty(ty))),
            },
            Ty::Refined(class, d) => {
                // Positive has its own distribution (gen.positive_int): the refinement class of std.gen.Positive
                if class_of(p, p.std_defs.positive) == Some(*class) {
                    return Ok(gen::draw(&mut self.stream, &Gen::PositiveInt, size, p));
                }
                let base = match &p.def(*d).kind {
                    DefKind::Refinement(r) => r.base.clone(),
                    _ => return Err(Undrawable::NotData(p.display_ty(ty))),
                };
                let mut checker = Interpreter::new(p, EvalConfig::default());
                for _ in 0..100 {
                    let v = self.draw(p, &base, size)?;
                    if checker.satisfies_refinement(*d, &v).unwrap_or(false) {
                        return Ok(v);
                    }
                }
                Err(Undrawable::Rejected(p.display_ty(ty)))
            }
            _ => Err(Undrawable::NotData(p.display_ty(ty))),
        }
    }
}

/// The refinement class of a refinement declaration.
fn class_of(p: &Program, d: cpl_hir::DefId) -> Option<cpl_hir::RefinementClass> {
    p.refinement_classes
        .iter()
        .position(|c| c.members.contains(&d))
        .map(|i| cpl_hir::RefinementClass(i as u32))
}
