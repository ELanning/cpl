//! Declarative bindings: the `.cbind.toml` files of backend-`c` bindings (`spec/cabi.md` 6.3, `spec/toolchain.md`
//! 11.1), read by the closed schema of cabi.md Section 6. The syntax half of `project_cbind_invalid`; the semantic
//! half (the operations and types against the capability) is `cpl_sema::cbind`.

use std::collections::BTreeMap;

/// A scalar binding type (cabi.md 6.3).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Scalar {
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    Int,
    Bool,
}

impl Scalar {
    fn parse(s: &str) -> Option<Scalar> {
        Some(match s {
            "int8" => Scalar::I8,
            "int16" => Scalar::I16,
            "int32" => Scalar::I32,
            "int64" => Scalar::I64,
            "uint8" => Scalar::U8,
            "uint16" => Scalar::U16,
            "uint32" => Scalar::U32,
            "uint64" => Scalar::U64,
            "int" => Scalar::Int,
            "bool" => Scalar::Bool,
            _ => return None,
        })
    }

    /// The C type.
    pub fn c_type(self) -> &'static str {
        match self {
            Scalar::I8 => "int8_t",
            Scalar::I16 => "int16_t",
            Scalar::I32 => "int32_t",
            Scalar::I64 => "int64_t",
            Scalar::U8 => "uint8_t",
            Scalar::U16 => "uint16_t",
            Scalar::U32 => "uint32_t",
            Scalar::U64 => "uint64_t",
            Scalar::Int => "int",
            Scalar::Bool => "int",
        }
    }

    /// The range of an integer scalar as (min, max) in i128; Bool is 0..1.
    pub fn range(self) -> (i128, i128) {
        match self {
            Scalar::I8 => (i8::MIN as i128, i8::MAX as i128),
            Scalar::I16 => (i16::MIN as i128, i16::MAX as i128),
            Scalar::I32 | Scalar::Int => (i32::MIN as i128, i32::MAX as i128),
            Scalar::I64 => (i64::MIN as i128, i64::MAX as i128),
            Scalar::U8 => (0, u8::MAX as i128),
            Scalar::U16 => (0, u16::MAX as i128),
            Scalar::U32 => (0, u32::MAX as i128),
            Scalar::U64 => (0, u64::MAX as i128),
            Scalar::Bool => (0, 1),
        }
    }
}

/// A binding type of an argument or an out-parameter (cabi.md 6.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BindType {
    Scalar(Scalar),
    BytesSpan,
    TextSpan,
    CStr,
    OwnedBytes,
    OwnedText,
    Struct(String),
}

impl BindType {
    fn parse(s: &str) -> Option<BindType> {
        if let Some(sc) = Scalar::parse(s) {
            return Some(BindType::Scalar(sc));
        }
        Some(match s {
            "bytes_span" => BindType::BytesSpan,
            "text_span" => BindType::TextSpan,
            "cstr" => BindType::CStr,
            "owned_bytes" => BindType::OwnedBytes,
            "owned_text" => BindType::OwnedText,
            _ => match s.strip_prefix("struct:") {
                Some(name) if is_ident(name) => BindType::Struct(name.to_string()),
                _ => return None,
            },
        })
    }
}

/// `[state]`: one opaque foreign object per grant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateSpec {
    pub c_type: String,
    pub create: String,
    pub destroy: String,
}

/// `[structs.<name>]`: an explicit C struct and its fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructSpec {
    pub c_type: String,
    /// (CPL field, C member, scalar), in the file's order
    pub fields: Vec<(String, String, Scalar)>,
}

/// One C argument of an operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArgSpec {
    State,
    Param { name: String, ty: BindType },
}

/// One out-parameter of an operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutSpec {
    pub name: String,
    pub ty: BindType,
    pub release: Option<String>,
}

/// `returns` of an operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReturnsSpec {
    /// a status: the listed values succeed
    Status {
        ty: Scalar,
        ok: Vec<i64>,
    },
    /// an infallible scalar result
    Result {
        ty: Scalar,
    },
    Void,
}

/// `[ops.<operation>]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpSpec {
    pub c: String,
    pub args: Vec<ArgSpec>,
    pub out: Vec<OutSpec>,
    pub returns: ReturnsSpec,
    /// (decimal status, ForeignErrorKind name), then `default`
    pub errors: Vec<(i64, String)>,
    pub default_error: Option<String>,
    pub message: Option<String>,
    pub result: Option<String>,
}

/// A declarative binding (`.cbind.toml`) read by the closed schema of `spec/cabi.md` Section 6.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CBindSpec {
    pub headers: Vec<String>,
    pub state: Option<StateSpec>,
    pub structs: BTreeMap<String, StructSpec>,
    pub ops: BTreeMap<String, OpSpec>,
}

/// Why a `.cbind.toml` is outside the closed schema: the fields of `project_cbind_invalid` (toolchain 11.1,
/// Appendix B). `key` is the dotted key concerned (`ops.thumbnail.args`), `line` its 1-based line when known,
/// `reason` one sentence.
#[derive(thiserror::Error, Clone, Debug, PartialEq, Eq)]
#[error("{key}: {reason}")]
pub struct CBindError {
    pub key: String,
    pub line: Option<u32>,
    pub reason: String,
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The keywords of C11, which name no function, type, or member.
const C_KEYWORDS: [&str; 44] = [
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
];

/// A C identifier: an identifier that is no C keyword.
fn is_c_ident(s: &str) -> bool {
    is_ident(s) && !C_KEYWORDS.contains(&s)
}

/// A C object pointer type: an identifier or `struct <identifier>`, then ` *`.
fn is_pointer_type(s: &str) -> bool {
    let Some(base) = s.strip_suffix(" *") else {
        return false;
    };
    is_c_type(base)
}

fn is_c_type(s: &str) -> bool {
    match s.strip_prefix("struct ") {
        Some(id) => is_c_ident(id),
        None => is_c_ident(s),
    }
}

/// The 1-based line of the dotted `key` in `text`: the line of the deepest table header (`[ops.thumbnail]`) among the
/// key's prefixes, or the first line after it that assigns the key's next segment (`errors = ...`).
fn line_of(text: &str, key: &str) -> Option<u32> {
    let parts: Vec<&str> = key.split('.').collect();
    let lines: Vec<&str> = text.lines().collect();
    let mut start = 0usize;
    let mut matched = 0usize;
    for n in (1..=parts.len()).rev() {
        let header = format!("[{}]", parts[..n].join("."));
        if let Some(i) = lines.iter().position(|l| l.trim() == header) {
            start = i;
            matched = n;
            break;
        }
    }
    if matched == parts.len() {
        return Some(start as u32 + 1);
    }
    let segment = parts[matched];
    let assigns = |l: &str| {
        let l = l.trim_start();
        let rest = l
            .strip_prefix(segment)
            .or_else(|| l.strip_prefix(&format!("\"{segment}\"")[..]));
        rest.is_some_and(|r| r.trim_start().starts_with('='))
    };
    lines
        .iter()
        .enumerate()
        .skip(start)
        .find(|(_, l)| assigns(l))
        .map(|(i, _)| i as u32 + 1)
        .or(if matched > 0 {
            Some(start as u32 + 1)
        } else {
            None
        })
}

struct Reader<'a> {
    text: &'a str,
}

impl Reader<'_> {
    fn err(&self, key: &str, reason: impl Into<String>) -> CBindError {
        let line = if key.is_empty() {
            None
        } else {
            line_of(self.text, key)
        };
        CBindError {
            key: key.to_string(),
            line,
            reason: reason.into(),
        }
    }

    fn closed(&self, t: &toml::Table, key: &str, allowed: &[&str]) -> Result<(), CBindError> {
        for k in t.keys() {
            if !allowed.contains(&k.as_str()) {
                let full = if key.is_empty() {
                    k.clone()
                } else {
                    format!("{key}.{k}")
                };
                return Err(self.err(&full, format!("`{k}` is not a key of the schema")));
            }
        }
        Ok(())
    }

    fn string(&self, t: &toml::Table, key: &str, name: &str) -> Result<Option<String>, CBindError> {
        match t.get(name) {
            None => Ok(None),
            Some(toml::Value::String(s)) => Ok(Some(s.clone())),
            Some(_) => Err(self.err(&format!("{key}.{name}"), "a string is expected")),
        }
    }

    /// A C function's name.
    fn ident(&self, t: &toml::Table, key: &str, name: &str) -> Result<Option<String>, CBindError> {
        match self.string(t, key, name)? {
            Some(s) if !is_c_ident(&s) => Err(self.err(
                &format!("{key}.{name}"),
                format!("`{s}` is not a C identifier"),
            )),
            other => Ok(other),
        }
    }

    /// The name of an out-parameter: a label of the file, never written into C.
    fn label(&self, t: &toml::Table, key: &str, name: &str) -> Result<Option<String>, CBindError> {
        match self.string(t, key, name)? {
            Some(s) if !is_ident(&s) => {
                Err(self.err(&format!("{key}.{name}"), format!("`{s}` is not a name")))
            }
            other => Ok(other),
        }
    }
}

/// Read a `.cbind.toml` by the closed schema of `spec/cabi.md` 6.3 (the syntax half of `project_cbind_invalid`; the
/// operations and types are checked against the capability by `cpl_sema::interop::plan`).
pub fn parse_cbind(text: &str) -> Result<CBindSpec, CBindError> {
    let r = Reader { text };
    let doc: toml::Table = toml::from_str(text).map_err(|e| CBindError {
        key: String::new(),
        line: e
            .span()
            .map(|s| text[..s.start.min(text.len())].matches('\n').count() as u32 + 1),
        reason: format!("the file is not TOML: {}", e.message()),
    })?;
    r.closed(&doc, "", &["library", "state", "structs", "ops"])?;
    // [library]
    let library = match doc.get("library") {
        Some(toml::Value::Table(t)) => t,
        _ => return Err(r.err("library", "[library] with `headers` is required")),
    };
    r.closed(library, "library", &["headers"])?;
    let headers: Vec<String> = match library.get("headers") {
        Some(toml::Value::Array(a)) if !a.is_empty() => {
            let mut v = Vec::new();
            for h in a {
                match h {
                    toml::Value::String(s)
                        if !s.is_empty()
                            && !s.contains(['"', '\n', '<', '>'])
                            && !s.starts_with('/') =>
                    {
                        v.push(s.clone())
                    }
                    _ => return Err(r.err("library.headers", "a header is a non-empty name")),
                }
            }
            v
        }
        _ => {
            return Err(r.err(
                "library.headers",
                "`headers` is a non-empty array of header names",
            ))
        }
    };
    // [state]
    let state = match doc.get("state") {
        None => None,
        Some(toml::Value::Table(t)) => {
            r.closed(t, "state", &["type", "create", "destroy"])?;
            let c_type = r
                .string(t, "state", "type")?
                .ok_or_else(|| r.err("state.type", "`type` is required"))?;
            if !is_pointer_type(&c_type) {
                return Err(r.err(
                    "state.type",
                    format!("`{c_type}` is not a C object pointer type"),
                ));
            }
            let create = r
                .ident(t, "state", "create")?
                .ok_or_else(|| r.err("state.create", "`create` is required"))?;
            let destroy = r
                .ident(t, "state", "destroy")?
                .ok_or_else(|| r.err("state.destroy", "`destroy` is required"))?;
            Some(StateSpec {
                c_type,
                create,
                destroy,
            })
        }
        Some(_) => return Err(r.err("state", "[state] is a table")),
    };
    // [structs.<name>]
    let mut structs = BTreeMap::new();
    if let Some(v) = doc.get("structs") {
        let toml::Value::Table(all) = v else {
            return Err(r.err("structs", "[structs.<name>] are tables"));
        };
        for (name, sv) in all {
            let key = format!("structs.{name}");
            let toml::Value::Table(t) = sv else {
                return Err(r.err(&key, "a struct mapping is a table"));
            };
            let c_type = r
                .string(t, &key, "c_type")?
                .ok_or_else(|| r.err(&format!("{key}.c_type"), "`c_type` is required"))?;
            if !is_c_type(&c_type) {
                return Err(r.err(
                    &format!("{key}.c_type"),
                    format!("`{c_type}` is not a C struct type"),
                ));
            }
            let mut fields = Vec::new();
            for (field, fv) in t {
                if field == "c_type" {
                    continue;
                }
                let toml::Value::String(s) = fv else {
                    return Err(r.err(
                        &format!("{key}.{field}"),
                        "a field is \"<member>: <scalar>\"",
                    ));
                };
                let Some((member, ty)) = s.split_once(':') else {
                    return Err(r.err(
                        &format!("{key}.{field}"),
                        "a field is \"<member>: <scalar>\"",
                    ));
                };
                let (member, ty) = (member.trim(), ty.trim());
                let Some(sc) = Scalar::parse(ty).filter(|_| is_c_ident(member)) else {
                    return Err(r.err(
                        &format!("{key}.{field}"),
                        format!("`{s}` is not \"<member>: <scalar>\""),
                    ));
                };
                fields.push((field.clone(), member.to_string(), sc));
            }
            structs.insert(name.clone(), StructSpec { c_type, fields });
        }
    }
    // [ops.<operation>]
    let mut ops = BTreeMap::new();
    let Some(toml::Value::Table(all)) = doc.get("ops") else {
        return Err(r.err("ops", "[ops.<operation>] tables are required"));
    };
    for (name, ov) in all {
        let key = format!("ops.{name}");
        let toml::Value::Table(t) = ov else {
            return Err(r.err(&key, "an operation is a table"));
        };
        r.closed(
            t,
            &key,
            &["c", "args", "out", "returns", "errors", "message", "result"],
        )?;
        let c = r
            .ident(t, &key, "c")?
            .ok_or_else(|| r.err(&format!("{key}.c"), "`c` is required"))?;
        let mut args = Vec::new();
        match t.get("args") {
            Some(toml::Value::Array(a)) => {
                for item in a {
                    let toml::Value::String(s) = item else {
                        return Err(r.err(&format!("{key}.args"), "an argument is a string"));
                    };
                    if s == "state" {
                        if state.is_none() {
                            return Err(
                                r.err(&format!("{key}.args"), "`state` needs a [state] table")
                            );
                        }
                        args.push(ArgSpec::State);
                        continue;
                    }
                    let Some((p, ty)) = s.split_once(':') else {
                        return Err(r.err(
                            &format!("{key}.args"),
                            format!("`{s}` is not \"<parameter>: <type>\""),
                        ));
                    };
                    let (p, ty) = (p.trim(), ty.trim());
                    let bt = BindType::parse(ty).filter(|b| {
                        !matches!(b, BindType::OwnedBytes | BindType::OwnedText) && is_ident(p)
                    });
                    if let Some(BindType::Struct(sn)) = &bt {
                        if !structs.contains_key(sn) {
                            return Err(r.err(
                                &format!("{key}.args"),
                                format!("struct:{sn} names no [structs.{sn}] table"),
                            ));
                        }
                    }
                    match bt {
                        Some(bt) => args.push(ArgSpec::Param {
                            name: p.to_string(),
                            ty: bt,
                        }),
                        None => {
                            return Err(r.err(
                                &format!("{key}.args"),
                                format!("`{s}` has no argument binding type"),
                            ))
                        }
                    }
                }
            }
            _ => return Err(r.err(&format!("{key}.args"), "`args` is a required array")),
        }
        let mut out = Vec::new();
        if let Some(v) = t.get("out") {
            let toml::Value::Array(a) = v else {
                return Err(r.err(&format!("{key}.out"), "`out` is an array"));
            };
            for item in a {
                let toml::Value::Table(o) = item else {
                    return Err(r.err(&format!("{key}.out"), "an out-parameter is an inline table"));
                };
                r.closed(o, &format!("{key}.out"), &["name", "type", "release"])?;
                let oname = r
                    .label(o, &format!("{key}.out"), "name")?
                    .ok_or_else(|| r.err(&format!("{key}.out"), "`name` is required"))?;
                let ty = r
                    .string(o, &format!("{key}.out"), "type")?
                    .ok_or_else(|| r.err(&format!("{key}.out"), "`type` is required"))?;
                let bt = BindType::parse(&ty)
                    .filter(|b| {
                        !matches!(b, BindType::BytesSpan | BindType::TextSpan | BindType::CStr)
                    })
                    .ok_or_else(|| {
                        r.err(
                            &format!("{key}.out"),
                            format!("`{ty}` is no out binding type"),
                        )
                    })?;
                if let BindType::Struct(sn) = &bt {
                    if !structs.contains_key(sn) {
                        return Err(r.err(
                            &format!("{key}.out"),
                            format!("struct:{sn} names no [structs.{sn}] table"),
                        ));
                    }
                }
                if out.iter().any(|x: &OutSpec| x.name == oname) {
                    return Err(r.err(
                        &format!("{key}.out"),
                        format!("`{oname}` names two out-parameters"),
                    ));
                }
                let release = r.ident(o, &format!("{key}.out"), "release")?;
                let owned = matches!(bt, BindType::OwnedBytes | BindType::OwnedText);
                if owned != release.is_some() {
                    return Err(r.err(
                        &format!("{key}.out"),
                        "`release` is required for owned_bytes and owned_text and forbidden otherwise",
                    ));
                }
                out.push(OutSpec {
                    name: oname,
                    ty: bt,
                    release,
                });
            }
        }
        let returns = match t.get("returns") {
            Some(toml::Value::Table(rt)) => {
                r.closed(rt, &format!("{key}.returns"), &["type", "ok", "result"])?;
                let ty = r
                    .string(rt, &format!("{key}.returns"), "type")?
                    .unwrap_or_default();
                if ty == "void" {
                    if rt.len() != 1 {
                        return Err(
                            r.err(&format!("{key}.returns"), "a void return has no other key")
                        );
                    }
                    ReturnsSpec::Void
                } else {
                    let sc = Scalar::parse(&ty).ok_or_else(|| {
                        r.err(
                            &format!("{key}.returns.type"),
                            format!("`{ty}` is no scalar"),
                        )
                    })?;
                    match (rt.get("ok"), rt.get("result")) {
                        (Some(toml::Value::Array(oks)), None) => {
                            let mut ok = Vec::new();
                            for v in oks {
                                match v {
                                    toml::Value::Integer(n) if fits(sc, *n) => ok.push(*n),
                                    toml::Value::Integer(n) => {
                                        return Err(r.err(
                                            &format!("{key}.returns.ok"),
                                            format!("{n} is outside the status type {ty}"),
                                        ))
                                    }
                                    _ => {
                                        return Err(r.err(
                                            &format!("{key}.returns.ok"),
                                            "`ok` lists integers",
                                        ))
                                    }
                                }
                            }
                            if ok.is_empty() {
                                return Err(r.err(
                                    &format!("{key}.returns.ok"),
                                    "`ok` lists at least one status",
                                ));
                            }
                            ReturnsSpec::Status { ty: sc, ok }
                        }
                        (None, Some(toml::Value::Boolean(true))) => ReturnsSpec::Result { ty: sc },
                        _ => {
                            return Err(r.err(
                                &format!("{key}.returns"),
                                "`returns` has `ok = [...]`, `result = true`, or the type void",
                            ))
                        }
                    }
                }
            }
            _ => return Err(r.err(&format!("{key}.returns"), "`returns` is required")),
        };
        let mut errors = Vec::new();
        let mut default_error = None;
        match (t.get("errors"), &returns) {
            (Some(toml::Value::Table(et)), ReturnsSpec::Status { .. }) => {
                for (k, v) in et {
                    let toml::Value::String(kind) = v else {
                        return Err(r.err(&format!("{key}.errors"), "an error kind is a string"));
                    };
                    if !crate_foreign_kind(kind) {
                        return Err(r.err(
                            &format!("{key}.errors"),
                            format!("`{kind}` is no ForeignErrorKind"),
                        ));
                    }
                    if k == "default" {
                        default_error = Some(kind.clone());
                    } else {
                        let n: i64 = k.parse().map_err(|_| {
                            r.err(
                                &format!("{key}.errors"),
                                format!("`{k}` is not a decimal status"),
                            )
                        })?;
                        let ReturnsSpec::Status { ty, ok } = &returns else {
                            unreachable!()
                        };
                        if !fits(*ty, n) {
                            return Err(r.err(
                                &format!("{key}.errors"),
                                format!("{n} is outside the status type"),
                            ));
                        }
                        if ok.contains(&n) {
                            return Err(r.err(
                                &format!("{key}.errors"),
                                format!("{n} is also an `ok` status"),
                            ));
                        }
                        errors.push((n, kind.clone()));
                    }
                }
                if default_error.is_none() {
                    return Err(r.err(&format!("{key}.errors"), "`errors` requires `default`"));
                }
            }
            (None, ReturnsSpec::Status { .. }) => {
                return Err(r.err(&format!("{key}.errors"), "`errors` is required with `ok`"))
            }
            (Some(_), _) => {
                return Err(r.err(
                    &format!("{key}.errors"),
                    "`errors` is allowed only with `ok`",
                ))
            }
            (None, _) => {}
        }
        let message = r.ident(t, &key, "message")?;
        if message.is_some() && !matches!(returns, ReturnsSpec::Status { .. }) {
            return Err(r.err(
                &format!("{key}.message"),
                "`message` is allowed only with `ok`",
            ));
        }
        let result = r.label(t, &key, "result")?;
        if let Some(res) = &result {
            if !matches!(returns, ReturnsSpec::Status { .. }) {
                return Err(r.err(
                    &format!("{key}.result"),
                    "`result` is allowed only with `ok`",
                ));
            }
            if !out.iter().any(|o| &o.name == res) {
                return Err(r.err(
                    &format!("{key}.result"),
                    format!("`{res}` is no out-parameter"),
                ));
            }
        }
        ops.insert(
            name.clone(),
            OpSpec {
                c,
                args,
                out,
                returns,
                errors,
                default_error,
                message,
                result,
            },
        );
    }
    Ok(CBindSpec {
        headers,
        state,
        structs,
        ops,
    })
}

/// Whether the status `n` is a value of the scalar type `s`.
fn fits(s: Scalar, n: i64) -> bool {
    let (lo, hi) = s.range();
    (lo..=hi).contains(&(n as i128))
}

/// Whether `kind` names a variant of std.foreign.ForeignErrorKind.
fn crate_foreign_kind(kind: &str) -> bool {
    matches!(
        kind,
        "Exception"
            | "InvalidArgument"
            | "InvalidResult"
            | "Transport"
            | "Timeout"
            | "Cancelled"
            | "Unavailable"
    )
}
