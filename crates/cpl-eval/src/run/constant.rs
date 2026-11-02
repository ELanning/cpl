//! Constant-expression arguments of `cpl run` (`spec/toolchain.md` 7.2;
//! semantics S39 and 3.5).
//!
//! The argument is parsed with `cpl_syntax::parse_expr` and its AST is
//! decoded against the parameter type, directed by the type exactly as
//! value JSON is (toolchain 7.3): the expected type says which declaration
//! a constructor or struct literal must name, so no name resolution is
//! needed. A written qualifier must be a suffix of the declaration's path
//! (`Missing`, `StoreError.Missing`, `reader.pages.StoreError.Missing`;
//! `PageId`, `reader.pages.PageId`). A refinement-typed position is decoded
//! at its root base; its predicates are checked by the caller like those of
//! a JSON argument. Error pointers are the JSON pointers of the same
//! positions in the argument's value JSON (`/1/$payload/0`).

use super::{RunError, ARGUMENT_FILE, USAGE_BAD_ARGUMENT};
use crate::json::ArgError;
use crate::types::{instantiate, strip};
use crate::value::Value;
use cpl_diag::{Category, Diagnostic, Span};
use cpl_hir::{Program, Ty};
use cpl_syntax::ast::{self, ExprKind as K, LiteralKind, UnaryOp};
use num_bigint::BigInt;
use serde_json::json;

/// Decode argument `index`, the constant expression `src`, against `ty`.
pub(super) fn decode(p: &Program, src: &str, ty: &Ty, index: usize) -> Result<Value, RunError> {
    let parsed = cpl_syntax::parse_expr(src, ARGUMENT_FILE);
    let Some(expr) = parsed.value else {
        let diagnostic = parsed.diagnostics.into_iter().next().unwrap_or_else(|| {
            Diagnostic::error(
                USAGE_BAD_ARGUMENT,
                Category::Usage,
                format!("argument {index} does not parse"),
            )
        });
        return Err(RunError::ArgNotConstant { index, diagnostic });
    };
    let mut decoder = Decoder {
        p,
        pointer: Vec::new(),
    };
    decoder.decode(&expr, ty).map_err(|problem| match problem {
        Problem::NotConstant { what, span } => RunError::ArgNotConstant {
            index,
            diagnostic: Diagnostic::error(
                USAGE_BAD_ARGUMENT,
                Category::Usage,
                format!("argument {index} is not a constant expression: {what}"),
            )
            .with_span(span)
            .with_data("argument", json!(index)),
        },
        Problem::Type(error) => RunError::Arg { index, error },
    })
}

enum Problem {
    /// A form outside the constant-expression grammar.
    NotConstant { what: String, span: Span },
    /// A constant of the wrong type.
    Type(ArgError),
}

fn not_constant<T>(what: &str, span: Span) -> Result<T, Problem> {
    Err(Problem::NotConstant {
        what: what.to_string(),
        span,
    })
}

/// The written path of a constructor: `Name` or a chain of `Member`s over
/// names (`reader.pages.StoreError.Missing`).
fn ctor_path(e: &ast::Expr) -> Option<Vec<&ast::Name>> {
    match &e.kind {
        K::Name(n) => Some(vec![n]),
        K::Member(m) => {
            let mut path = ctor_path(&m.target)?;
            path.push(&m.member);
            Some(path)
        }
        _ => None,
    }
}

fn is_upper(name: &ast::Name) -> bool {
    name.text.starts_with(|c: char| c.is_ascii_uppercase())
}

/// True when the written qualifier is a suffix of the declaration's path.
fn is_suffix(qualifier: &[&ast::Name], full: &[String]) -> bool {
    qualifier.len() <= full.len()
        && qualifier
            .iter()
            .zip(&full[full.len() - qualifier.len()..])
            .all(|(q, f)| q.text == *f)
}

struct Decoder<'p> {
    p: &'p Program,
    pointer: Vec<String>,
}

impl Decoder<'_> {
    fn pointer(&self) -> String {
        let mut s = String::new();
        for segment in &self.pointer {
            s.push('/');
            s.push_str(&segment.replace('~', "~0").replace('/', "~1"));
        }
        s
    }

    fn error(&self, message: String) -> Problem {
        Problem::Type(ArgError {
            pointer: self.pointer(),
            message,
        })
    }

    fn mismatch(&self, ty: &Ty, found: &str) -> Problem {
        self.error(format!(
            "expected `{}`, found {found}",
            self.p.display_ty(ty)
        ))
    }

    fn at<T>(
        &mut self,
        segment: impl Into<String>,
        f: impl FnOnce(&mut Self) -> Result<T, Problem>,
    ) -> Result<T, Problem> {
        self.pointer.push(segment.into());
        let result = f(self);
        self.pointer.pop();
        result
    }

    fn decode(&mut self, e: &ast::Expr, ty: &Ty) -> Result<Value, Problem> {
        let ty = &match ty {
            Ty::Refined(..) => strip(self.p, ty),
            other => other.clone(),
        };
        if *ty == Ty::Bytes {
            return Err(self.error(
                "a `Bytes` value has no constant-expression form; pass it as --arg {\"$type\": \"Bytes\", \"hex\": \"...\"}"
                    .to_string(),
            ));
        }
        match &e.kind {
            K::Paren(inner) => self.decode(inner, ty),
            K::Literal(l) => self.literal(l, ty),
            K::Unary(u) => self.unary(u, ty),
            K::List(l) => self.list(l, ty),
            K::Struct(s) => self.struct_literal(s, ty),
            K::Name(_) | K::Member(_) => match ctor_path(e) {
                Some(path) if path.last().is_some_and(|n| is_upper(n)) => {
                    self.ctor(&path, None, ty)
                }
                _ => not_constant(
                    "a name or member access is not a constant expression; write the value",
                    e.span,
                ),
            },
            K::Call(c) => self.call(c, ty, e.span),
            K::Hole(_) => not_constant("a hole is not a value", e.span),
            K::Forall(_) | K::Parallel(_) => {
                not_constant("a `forall` expression is evaluated, not written", e.span)
            }
            K::Binary(_) => {
                not_constant("an operator expression is evaluated, not written", e.span)
            }
            K::Propagate(_) => not_constant("a `?` expression", e.span),
            K::If(_) => not_constant("an `if` expression", e.span),
            K::Match(_) => not_constant("a `match` expression", e.span),
        }
    }

    fn literal(&self, l: &ast::Literal, ty: &Ty) -> Result<Value, Problem> {
        match (&l.kind, ty) {
            (LiteralKind::Int(digits), Ty::Int) => digits
                .parse::<BigInt>()
                .map(Value::Int)
                .map_err(|_| self.error(format!("`{digits}` is not a decimal integer"))),
            (LiteralKind::Text(t), Ty::Text) => Ok(Value::text(t)),
            (LiteralKind::Bool(b), Ty::Bool) => Ok(Value::Bool(*b)),
            (LiteralKind::Int(_), _) => Err(self.mismatch(ty, "an Int literal")),
            (LiteralKind::Text(_), _) => Err(self.mismatch(ty, "a Text literal")),
            (LiteralKind::Bool(_), _) => Err(self.mismatch(ty, "a Bool literal")),
        }
    }

    fn unary(&mut self, u: &ast::UnaryExpr, ty: &Ty) -> Result<Value, Problem> {
        match (u.op, ty) {
            (UnaryOp::Neg, Ty::Int) => match &self.decode(&u.operand, &Ty::Int)? {
                Value::Int(n) => Ok(Value::Int(-n)),
                other => Err(self.error(format!("cannot negate {}", other.kind_name()))),
            },
            (UnaryOp::Not, Ty::Bool) => match &self.decode(&u.operand, &Ty::Bool)? {
                Value::Bool(b) => Ok(Value::Bool(!b)),
                other => Err(self.error(format!("cannot apply `not` to {}", other.kind_name()))),
            },
            (UnaryOp::Neg, _) => Err(self.mismatch(ty, "a negation (`-`)")),
            (UnaryOp::Not, _) => Err(self.mismatch(ty, "a `not` expression")),
        }
    }

    fn list(&mut self, l: &ast::ListLiteral, ty: &Ty) -> Result<Value, Problem> {
        let Ty::List(elem) = ty else {
            return Err(self.mismatch(ty, "a list"));
        };
        let mut out = Vec::with_capacity(l.elements.len());
        for (i, el) in l.elements.iter().enumerate() {
            out.push(self.at(i.to_string(), |d| d.decode(&el.expr, elem))?);
        }
        Ok(Value::list(out))
    }

    fn call(&mut self, c: &ast::CallExpr, ty: &Ty, span: Span) -> Result<Value, Problem> {
        let Some(path) = ctor_path(&c.callee) else {
            return not_constant("a call is not a constant expression", span);
        };
        if !path.last().is_some_and(|n| is_upper(n)) {
            return not_constant("a function call is not a constant expression", span);
        }
        if c.generic_args.is_some() {
            return not_constant(
                "type arguments are not part of a constant expression; the parameter type fixes them",
                span,
            );
        }
        if let Some(u) = c.args.using.first() {
            return not_constant("a `using` argument is not a value", u.span);
        }
        let args: Vec<&ast::Expr> = c.args.positional.iter().map(|a| &a.expr).collect();
        self.ctor(&path, Some(&args), ty)
    }

    /// A constructor application or a nullary constructor, resolved against
    /// the expected type: a variant of the expected enum, or the expected
    /// wrapper.
    fn ctor(
        &mut self,
        path: &[&ast::Name],
        args: Option<&[&ast::Expr]>,
        ty: &Ty,
    ) -> Result<Value, Problem> {
        let p = self.p;
        let (name, qualifier) = path.split_last().expect("a constructor path is not empty");
        let written: Vec<&str> = path.iter().map(|n| n.text.as_str()).collect();
        let written = written.join(".");
        let found = format!("the constructor `{written}`");
        match ty {
            Ty::Adt(def, targs) => {
                let d = p.def(*def);
                let Some(en) = d.kind.as_enum() else {
                    return Err(self.mismatch(ty, &found));
                };
                let mut full = p.module(d.module).path.clone();
                full.push(d.name.display());
                let index = en.variants.iter().position(|v| v.name == name.text);
                let Some(index) = index.filter(|_| is_suffix(qualifier, &full)) else {
                    return Err(self.mismatch(ty, &found));
                };
                let variant = &en.variants[index];
                let given = args.map_or(0, <[_]>::len);
                if given != variant.payload.len() || (args.is_none() && !variant.payload.is_empty())
                {
                    return Err(self.error(format!(
                        "the variant `{}` takes {} payload value(s), found {given}",
                        variant.name,
                        variant.payload.len()
                    )));
                }
                let mut payload = Vec::with_capacity(given);
                for (k, (arg, pty)) in args
                    .unwrap_or_default()
                    .iter()
                    .zip(&variant.payload)
                    .enumerate()
                {
                    let t = instantiate(pty, targs);
                    payload
                        .push(self.at("$payload", |d| d.at(k.to_string(), |d| d.decode(arg, &t)))?);
                }
                Ok(Value::new_enum(*def, index as u32, payload))
            }
            Ty::Wrapper(def) => {
                let d = p.def(*def);
                let (Some(w), true) = (
                    d.kind.as_wrapper(),
                    d.name.display() == name.text && is_suffix(qualifier, &p.module(d.module).path),
                ) else {
                    return Err(self.mismatch(ty, &found));
                };
                match args {
                    Some([inner]) => {
                        let inner_ty = w.inner.clone();
                        let v =
                            self.at("$payload", |d| d.at("0", |d| d.decode(inner, &inner_ty)))?;
                        Ok(Value::wrap(*def, v))
                    }
                    _ => Err(self.error(format!(
                        "the wrapper `{}` takes exactly one value: write {}(...)",
                        name.text, name.text
                    ))),
                }
            }
            _ => Err(self.mismatch(ty, &found)),
        }
    }

    /// `S { field: value, ... }` without a spread, naming the expected
    /// struct; every field exactly once.
    fn struct_literal(&mut self, s: &ast::StructLiteral, ty: &Ty) -> Result<Value, Problem> {
        if let Some(base) = &s.entries.spread {
            return not_constant(
                "a struct literal argument cannot spread a base value (`..`); list every field",
                base.span,
            );
        }
        let p = self.p;
        let found = format!("the struct literal `{} {{ ... }}`", s.path.dotted());
        let Ty::Adt(def, targs) = ty else {
            return Err(self.mismatch(ty, &found));
        };
        let d = p.def(*def);
        let Some(sd) = d.kind.as_struct() else {
            return Err(self.mismatch(ty, &found));
        };
        let module = &p.module(d.module).path;
        let modules: Vec<&ast::Name> = s.path.modules.iter().collect();
        if s.path.name.text != d.name.display() || !is_suffix(&modules, module) {
            return Err(self.mismatch(ty, &found));
        }
        let tag = d.name.display();
        for (i, init) in s.entries.fields.iter().enumerate() {
            if !sd.fields.iter().any(|f| f.name == init.name.text) {
                return Err(self.error(format!("`{tag}` has no field `{}`", init.name.text)));
            }
            if s.entries.fields[..i]
                .iter()
                .any(|other| other.name.text == init.name.text)
            {
                return Err(self.error(format!("the field `{}` is given twice", init.name.text)));
            }
        }
        let mut fields = Vec::with_capacity(sd.fields.len());
        for field in &sd.fields {
            let Some(init) = s.entries.fields.iter().find(|i| i.name.text == field.name) else {
                return Err(self.error(format!("missing field `{}` of `{tag}`", field.name)));
            };
            let fty = instantiate(&field.ty, targs);
            fields.push(self.at(field.name.clone(), |d| d.decode(&init.value, &fty))?);
        }
        Ok(Value::new_struct(*def, fields))
    }
}
