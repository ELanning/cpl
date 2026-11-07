//! The Java printer: sources for the JVM (`spec/hosts.md` 4), which Kotlin uses through its
//! Java interop the way it uses any Java library.
//!
//! A CPL module is a Java package under the base package; its functions are static methods of
//! one final class named after the module (`geo.shapes` → `base.geo.shapes.Shapes`). Structs
//! are records, enums are sealed interfaces with one record per variant (`Shape.Line(f0)`),
//! wrappers are records with one component `value`, and capabilities are interfaces the host
//! implements. Text is `String`, Bool is `boolean`; the runtime package `base.cpl` has the rest,
//! with Kotlin's operator names on `Int` (`plus`, `times`, `compareTo`, ...).
//!
//! Every function has an internal form `x_f(args..., cx)` that the program's own calls use; a
//! `pub fn` also has a host form `f(args...)` that makes the calling context, checks the
//! refinements and invariants of its arguments, and throws a fault as `Fault`, an unchecked
//! exception without a stack trace.

use crate::lir::*;
use crate::lower::Lowered;
use crate::{camel, erase, instantiate, pascal, std_type, Options, Package, StdType};
use cpl_hir::{DefId, DefKind, Program, Ty};
use num_traits::ToPrimitive;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// Whether a value entering at `ty` is checked at the boundary: its refinements and invariants,
/// and its texts, which a Go string or a Java string may hold invalid (not Unicode scalars).
fn needs_validation(p: &Program, ty: &Ty) -> bool {
    crate::needs_validation(p, ty) || crate::contains_text(p, ty)
}

/// Java's keywords and literals, and the names the generated code declares itself.
const RESERVED: &[&str] = &[
    "abstract",
    "assert",
    "boolean",
    "break",
    "byte",
    "case",
    "catch",
    "char",
    "class",
    "const",
    "continue",
    "default",
    "do",
    "double",
    "else",
    "enum",
    "extends",
    "final",
    "finally",
    "float",
    "for",
    "goto",
    "if",
    "implements",
    "import",
    "instanceof",
    "int",
    "interface",
    "long",
    "native",
    "new",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "short",
    "static",
    "strictfp",
    "super",
    "switch",
    "synchronized",
    "this",
    "throw",
    "throws",
    "transient",
    "try",
    "void",
    "volatile",
    "while",
    "true",
    "false",
    "null",
    "var",
    "yield",
    "record",
    "sealed",
    "permits",
    "_",
    "cx",
    "r",
    "java",
];

/// `Object`'s methods and the value methods: no record component or operation is named so.
const MEMBERS: &[&str] = &[
    "hashCode",
    "toString",
    "equals",
    "getClass",
    "notify",
    "notifyAll",
    "wait",
    "clone",
    "finalize",
    "writeText",
    "toJson",
];

pub fn emit(p: &Program, lowered: &Lowered, options: &Options) -> Package {
    let base = package_segment(&options.name);
    let rt = format!("{base}.cpl");
    let mut modules: BTreeMap<Vec<String>, Vec<DefId>> = BTreeMap::new();
    for d in p.defs() {
        if lowered.defs.contains(&d.id) && std_type(p, d.id).is_none() {
            modules
                .entry(p.module(d.module).path.clone())
                .or_default()
                .push(d.id);
        }
    }
    let mut packages = BTreeMap::new();
    let mut classes = BTreeMap::new();
    for (path, defs) in &modules {
        let mut segs: Vec<String> = path.iter().map(|s| package_segment(s)).collect();
        if segs.first().is_some_and(|s| s == "cpl") {
            segs[0] = "cpl_".into();
        }
        packages.insert(path.clone(), format!("{base}.{}", segs.join(".")));
        let mut class = upper_first(&pascal(path.last().map(String::as_str).unwrap_or("Module")));
        if class.is_empty() || !class.starts_with(|c: char| c.is_ascii_uppercase()) {
            class = format!("M{class}");
        }
        let taken = |c: &str| {
            defs.iter().any(|d| p.def(*d).name.display() == c) || c == "Support" || c == "Driver"
        };
        while taken(&class) {
            class.push_str("Module");
        }
        classes.insert(path.clone(), class);
    }
    let mut j = Printer {
        p,
        base: base.clone(),
        rt: rt.clone(),
        packages,
        classes,
        validators: BTreeMap::new(),
        pending: Vec::new(),
        decoders: BTreeMap::new(),
        pending_dec: Vec::new(),
        fn_ids: lowered
            .fns
            .iter()
            .enumerate()
            .map(|(i, f)| (f.def, i as u32))
            .collect(),
        counter: 0,
        breaks: BTreeSet::new(),
        exact: {
            let mut by_name: BTreeMap<(Vec<String>, String), Vec<DefId>> = BTreeMap::new();
            for f in &lowered.fns {
                let d = p.def(f.def);
                if d.is_pub {
                    by_name
                        .entry((p.module(d.module).path.clone(), member(&d.name.display())))
                        .or_default()
                        .push(f.def);
                }
            }
            by_name
                .into_values()
                .filter(|v| v.len() > 1)
                .flatten()
                .collect()
        },
        q: crate::java_q::JavaQ::new(p, &base, &rt),
    };
    // the generated names of the speculative forms' records and enums
    for d in j.q.named() {
        let n = j.fqn(d);
        j.q.gen.insert(d, n);
        match &p.def(d).kind {
            cpl_hir::DefKind::Enum(e) => {
                for v in 0..e.variants.len() as u32 {
                    let vn = format!("{}.{}", j.fqn(d), j.variant_name(d, v));
                    j.q.variants.insert((d, v), vn);
                }
            }
            cpl_hir::DefKind::Struct(s) => {
                for i in 0..s.fields.len() {
                    let f = j.field_name(d, i);
                    j.q.accessors.insert((d, i), f);
                }
            }
            _ => {}
        }
    }
    let mut files = BTreeMap::new();
    let src = |pkg: &str, class: &str| format!("src/{}/{class}.java", pkg.replace('.', "/"));
    for part in include_str!("runtime/java-runtime.txt")
        .split("//// FILE: ")
        .filter(|s| !s.trim().is_empty())
    {
        let (name, body) = part.split_once('\n').expect("a runtime file");
        files.insert(
            format!("src/{}/{}", rt.replace('.', "/"), name.trim()),
            format!(
                "// Generated by `cpl build --target java`: do not edit.\n{}",
                body.replace("__PKG__", &base)
            ),
        );
    }
    let fns: BTreeMap<DefId, &LFn> = lowered.fns.iter().map(|f| (f.def, f)).collect();
    let preds: BTreeMap<DefId, &LPred> = lowered.preds.iter().map(|q| (q.def, q)).collect();
    let mut exports = Vec::new();
    for (path, defs) in &modules {
        let pkg = j.packages[path].clone();
        let class = j.classes[path].clone();
        let header =
            format!("// Generated by `cpl build --target java`: do not edit.\npackage {pkg};\n\n");
        let mut fns_out = String::new();
        for d in defs {
            let def = p.def(*d);
            let name = def.name.display();
            match &def.kind {
                DefKind::Struct(_)
                | DefKind::Enum(_)
                | DefKind::Wrapper(_)
                | DefKind::Capability(_) => {
                    let mut out = header.clone();
                    j.type_decl(*d, &mut out);
                    files.insert(src(&pkg, &ident(&name)), out);
                    if let Some(pred) = preds.get(d) {
                        j.pred(pred, &mut fns_out);
                    }
                }
                DefKind::Refinement(_) => {
                    if let Some(pred) = preds.get(d) {
                        j.pred(pred, &mut fns_out);
                    }
                }
                DefKind::Fn(_) => {
                    if let Some(f) = fns.get(d) {
                        j.function(f, &mut fns_out);
                        // (a function taking a context handle is CPL's alone: a host starts none)
                        if def.is_pub && !def.kind.as_fn().is_some_and(cpl_hir::FnDef::takes_context) {
                            exports.push(p.display_path(*d));
                            j.host_function(f, &mut fns_out);
                        }
                    }
                }
                _ => {}
            }
        }
        if !fns_out.is_empty() {
            let out = format!(
                "{header}/** The functions of the CPL module {{@code {}}}. */\npublic final class {class} {{\n    private {class}() {{\n    }}\n\n{fns_out}}}\n",
                path.join(".")
            );
            files.insert(src(&pkg, &class), out);
        }
    }
    let mut support = String::new();
    // the records of the speculative forms
    support.push_str(&j.q.records(p));
    if options.driver {
        let mut driver = String::new();
        j.driver(lowered, &mut driver);
        files.insert(
            src(&base, "Driver"),
            format!("// Generated by `cpl build --target java`: do not edit.\npackage {base};\n\n/** The JSON driver of the differential tests (spec/hosts.md 4). */\npublic final class Driver {{\n{driver}}}\n"),
        );
    }
    let mut done = BTreeSet::new();
    loop {
        if let Some((key, ty)) = j.pending.pop() {
            if done.insert(format!("v {key}")) {
                j.validator(&key, &ty, &mut support);
            }
        } else if let Some((key, ty)) = j.pending_dec.pop() {
            if done.insert(format!("d {key}")) {
                j.decoder(&key, &ty, &mut support);
            }
        } else {
            break;
        }
    }
    files.insert(
        src(&base, "Support"),
        format!("// Generated by `cpl build --target java`: do not edit.\npackage {base};\n\n/** The boundary checks and value JSON decoders of the generated code: internal. */\npublic final class Support {{\n    private Support() {{\n    }}\n\n{support}}}\n"),
    );
    Package { files, exports }
}

/// A Java package segment.
fn package_segment(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() || s.starts_with(|c: char| c.is_ascii_digit()) {
        s.insert(0, 'p');
    }
    if RESERVED.contains(&s.as_str()) {
        s.push('_');
    }
    s
}

fn upper_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

pub(crate) fn ident(name: &str) -> String {
    if RESERVED.contains(&name) {
        format!("{name}_")
    } else {
        name.to_string()
    }
}

/// A method or component name: camelCase, safe.
fn member(name: &str) -> String {
    let c = camel(name);
    let c = if c.is_empty() { "f".to_string() } else { c };
    if RESERVED.contains(&c.as_str()) || MEMBERS.contains(&c.as_str()) {
        format!("{c}_")
    } else {
        c
    }
}

/// A Java string literal: ASCII printable characters as they are, everything else escaped
/// (octal below 0x80, `\uXXXX` above, which never makes a quote or line break).
pub(crate) fn java_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (' '..='~').contains(&c) => out.push(c),
            c if (c as u32) < 0x80 => {
                let _ = write!(out, "\\{:03o}", c as u32);
            }
            c => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    let _ = write!(out, "\\u{:04x}", unit);
                }
            }
        }
    }
    out.push('"');
    out
}

/// Documentation text safe in a Java comment.
fn doc_text(s: &str) -> String {
    s.replace('\\', "\\\\").replace("*/", "*&#47;")
}

fn generics_decl(generics: &[String]) -> String {
    if generics.is_empty() {
        return String::new();
    }
    format!(
        "<{}>",
        generics
            .iter()
            .map(|g| format!("G{g}"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

struct Printer<'p> {
    p: &'p Program,
    base: String,
    rt: String,
    packages: BTreeMap<Vec<String>, String>,
    classes: BTreeMap<Vec<String>, String>,
    validators: BTreeMap<String, String>,
    pending: Vec<(String, Ty)>,
    decoders: BTreeMap<String, String>,
    pending_dec: Vec<(String, Ty)>,
    fn_ids: BTreeMap<DefId, u32>,
    counter: usize,
    /// The loops with a reachable `break`, while their bodies are printed.
    breaks: BTreeSet<u32>,
    /// The `pub fn`s whose camelCase name another `pub fn` of their module shares: they keep
    /// their CPL names (`foo_bar` and `fooBar` would both be `fooBar`).
    exact: BTreeSet<DefId>,
    /// The functions with a speculative form (`java_q.rs`).
    q: crate::java_q::JavaQ,
}

impl<'p> Printer<'p> {
    fn fresh(&mut self, prefix: &str) -> String {
        self.counter += 1;
        format!("{prefix}{}", self.counter)
    }

    fn module_path(&self, d: DefId) -> &Vec<String> {
        &self.p.module(self.p.def(d).module).path
    }

    /// The fully qualified name of a type.
    fn fqn(&self, d: DefId) -> String {
        format!(
            "{}.{}",
            self.packages[self.module_path(d)],
            ident(&self.p.def(d).name.display())
        )
    }

    /// The functions class of the module of `d`, fully qualified.
    fn class_of(&self, d: DefId) -> String {
        let path = self.module_path(d);
        format!("{}.{}", self.packages[path], self.classes[path])
    }

    fn variant_name(&self, d: DefId, variant: u32) -> String {
        let e = self.p.def(d).kind.as_enum().expect("an enum");
        let v = ident(&e.variants[variant as usize].name);
        if v == self.p.def(d).name.display() {
            format!("{v}_")
        } else {
            v
        }
    }

    fn variant_ty(&self, d: DefId, variant: u32, ty: &Ty, generics: &[String]) -> String {
        let args = match ty {
            Ty::Adt(_, a) if !a.is_empty() => {
                format!(
                    "<{}>",
                    a.iter()
                        .map(|t| self.jty(t, generics, true))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
            _ => String::new(),
        };
        format!("{}.{}{args}", self.fqn(d), self.variant_name(d, variant))
    }

    fn field_name(&self, d: DefId, i: usize) -> String {
        let s = self.p.def(d).kind.as_struct().expect("a struct");
        let name = member(&s.fields[i].name);
        let earlier = (0..i).any(|j| member(&s.fields[j].name) == name);
        if earlier {
            format!("{name}_{i}")
        } else {
            name
        }
    }

    fn is_unit(&self, t: &Ty) -> bool {
        matches!(t, Ty::Adt(d, _) if *d == self.p.std_defs().unit)
    }

    /// The Java type of `t`; `boxed` for a type argument (`Boolean` for Bool).
    fn jty(&self, t: &Ty, generics: &[String], boxed: bool) -> String {
        let p = self.p;
        let rt = &self.rt;
        match t {
            Ty::Int => format!("{rt}.Int"),
            Ty::Bool if boxed => "java.lang.Boolean".into(),
            Ty::Bool => "boolean".into(),
            Ty::Text => "java.lang.String".into(),
            Ty::Bytes => format!("{rt}.Bytes"),
            Ty::List(e) => format!("{rt}.List<{}>", self.jty(e, generics, true)),
            Ty::Adt(d, args) => {
                let a: Vec<String> = args.iter().map(|x| self.jty(x, generics, true)).collect();
                match std_type(p, *d) {
                    Some(StdType::Unit) => format!("{rt}.Unit"),
                    Some(StdType::Option) => format!("{rt}.Option<{}>", a[0]),
                    Some(StdType::Result) => format!("{rt}.Result<{}, {}>", a[0], a[1]),
                    Some(StdType::ConstraintError) => format!("{rt}.ConstraintError"),
                    Some(StdType::ForeignError) => format!("{rt}.ForeignError"),
                    Some(StdType::ForeignErrorKind) => format!("{rt}.ForeignErrorKind"),
                    Some(StdType::ForeignDetail) => format!("{rt}.ForeignDetail"),
                    None if a.is_empty() => self.fqn(*d),
                    None => format!("{}<{}>", self.fqn(*d), a.join(", ")),
                }
            }
            Ty::Wrapper(d) | Ty::Capability(d) => self.fqn(*d),
            // the call stack's state machine: a shared cell
            Ty::Context(d) => format!("{}.Context<{}>", self.rt, self.fqn(*d)),
            Ty::Refined(..) => self.jty(&erase(p, t), generics, boxed),
            Ty::Param(i) => generics
                .get(*i as usize)
                .map(|g| format!("G{g}"))
                .unwrap_or_else(|| format!("{rt}.Unit")),
            Ty::Gen(_) | Ty::Error => format!("{rt}.Unit"),
        }
    }

    fn default_value(&self, t: &Ty) -> &'static str {
        if matches!(erase(self.p, t), Ty::Bool) {
            "false"
        } else {
            "null"
        }
    }

    // ---- declarations ----------------------------------------------------------------

    fn type_decl(&mut self, d: DefId, out: &mut String) {
        let p = self.p;
        let def = p.def(d);
        let name = ident(&def.name.display());
        let rt = self.rt.clone();
        let qualified = crate::lower::type_name(p, d);
        let doc = def
            .docs
            .as_ref()
            .and_then(|d| d.lines().next())
            .map(|l| format!(": {}", doc_text(l)))
            .unwrap_or_default();
        let to_json_map = |out: &mut String, entries: &[(String, String)]| {
            let _ = writeln!(out, "    @java.lang.Override\n    public java.lang.Object toJson() {{\n        java.util.Map<java.lang.String, java.lang.Object> m = new java.util.TreeMap<>();\n        m.put(\"$type\", {});", java_str(&qualified));
            for (k, v) in entries {
                let _ = writeln!(out, "        m.put({}, {rt}.Rt.toJson({v}));", java_str(k));
            }
            let _ = writeln!(out, "        return m;\n    }}");
        };
        match &def.kind {
            DefKind::Struct(s) => {
                let g = generics_decl(&s.generics);
                let comps: Vec<String> = s
                    .fields
                    .iter()
                    .enumerate()
                    .map(|(i, f)| {
                        format!(
                            "{} {}",
                            self.jty(&f.ty, &s.generics, false),
                            self.field_name(d, i)
                        )
                    })
                    .collect();
                let _ = writeln!(
                    out,
                    "/** The CPL struct {{@code {}}}{doc}. */\npublic record {name}{g}({}) implements {rt}.Value {{",
                    p.display_path(d),
                    comps.join(", ")
                );
                let bare = def.name.display();
                let _ = writeln!(out, "    @java.lang.Override\n    public java.lang.String toString() {{\n        return {rt}.Rt.valueText(this);\n    }}\n");
                let _ = writeln!(
                    out,
                    "    @java.lang.Override\n    public void writeText(java.lang.StringBuilder sb) {{"
                );
                if s.fields.is_empty() {
                    let _ = writeln!(
                        out,
                        "        sb.append({});",
                        java_str(&format!("{bare} {{}}"))
                    );
                } else {
                    for (i, f) in s.fields.iter().enumerate() {
                        let lead = if i == 0 {
                            format!("{bare} {{ {}: ", f.name)
                        } else {
                            format!(", {}: ", f.name)
                        };
                        let _ = writeln!(
                            out,
                            "        sb.append({});\n        {rt}.Rt.writeText(sb, {});",
                            java_str(&lead),
                            self.field_name(d, i)
                        );
                    }
                    let _ = writeln!(out, "        sb.append(\" }}\");");
                }
                let _ = writeln!(out, "    }}\n");
                let entries: Vec<(String, String)> = s
                    .fields
                    .iter()
                    .enumerate()
                    .map(|(i, f)| (f.name.clone(), self.field_name(d, i)))
                    .collect();
                to_json_map(out, &entries);
                let _ = writeln!(out, "}}");
            }
            DefKind::Enum(e) => {
                let g = generics_decl(&e.generics);
                let sealed = if e.variants.is_empty() { "" } else { "sealed " };
                let _ = writeln!(
                    out,
                    "/** The CPL enum {{@code {}}}{doc}. */\npublic {sealed}interface {name}{g} extends {rt}.Value {{",
                    p.display_path(d)
                );
                for (vi, v) in e.variants.iter().enumerate() {
                    let vn = self.variant_name(d, vi as u32);
                    let comps: Vec<String> = v
                        .payload
                        .iter()
                        .enumerate()
                        .map(|(i, t)| format!("{} f{i}", self.jty(t, &e.generics, false)))
                        .collect();
                    let _ = writeln!(
                        out,
                        "    record {vn}{g}({}) implements {name}{g} {{",
                        comps.join(", ")
                    );
                    let _ = writeln!(out, "        @java.lang.Override\n        public java.lang.String toString() {{\n            return {rt}.Rt.valueText(this);\n        }}\n");
                    let _ = writeln!(out, "        @java.lang.Override\n        public void writeText(java.lang.StringBuilder sb) {{");
                    if v.payload.is_empty() {
                        let _ = writeln!(out, "            sb.append({});", java_str(&v.name));
                    } else {
                        let _ = writeln!(
                            out,
                            "            sb.append({});",
                            java_str(&format!("{}(", v.name))
                        );
                        for i in 0..v.payload.len() {
                            if i > 0 {
                                let _ = writeln!(out, "            sb.append(\", \");");
                            }
                            let _ = writeln!(out, "            {rt}.Rt.writeText(sb, f{i});");
                        }
                        let _ = writeln!(out, "            sb.append(')');");
                    }
                    let payload = if v.payload.is_empty() {
                        "null".to_string()
                    } else {
                        format!(
                            "java.util.List.of({})",
                            (0..v.payload.len())
                                .map(|i| format!("{rt}.Rt.toJson(f{i})"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    };
                    let _ = writeln!(
                        out,
                        "        }}\n\n        @java.lang.Override\n        public java.lang.Object toJson() {{\n            return {rt}.Json.tagged({}, {}, {payload});\n        }}\n    }}\n",
                        java_str(&qualified),
                        java_str(&v.name)
                    );
                }
                let _ = writeln!(out, "}}");
            }
            DefKind::Wrapper(w) => {
                let inner = self.jty(&w.inner, &[], false);
                let bare = def.name.display();
                let _ = writeln!(
                    out,
                    "/** The CPL wrapper {{@code {}}}{doc}. */\npublic record {name}({inner} value) implements {rt}.Value {{\n    @java.lang.Override\n    public java.lang.String toString() {{\n        return {rt}.Rt.valueText(this);\n    }}\n\n    @java.lang.Override\n    public void writeText(java.lang.StringBuilder sb) {{\n        sb.append({});\n        {rt}.Rt.writeText(sb, value);\n        sb.append(')');\n    }}\n\n    @java.lang.Override\n    public java.lang.Object toJson() {{\n        return {rt}.Json.tagged({}, null, java.util.List.of({rt}.Rt.toJson(value)));\n    }}\n}}",
                    p.display_path(d),
                    java_str(&format!("{bare}(")),
                    java_str(&qualified)
                );
            }
            DefKind::Capability(c) => {
                let _ = writeln!(out, "/** The CPL capability {{@code {}}}{doc}: the host implements it. */\npublic interface {name} {{", p.display_path(d));
                for op in &c.ops {
                    let params: Vec<String> = op
                        .params
                        .iter()
                        .map(|q| format!("{} {}", self.jty(&q.ty, &[], false), ident(&q.name)))
                        .collect();
                    let ret = if self.is_unit(&op.ret) {
                        "void".to_string()
                    } else {
                        self.jty(&op.ret, &[], false)
                    };
                    let _ = writeln!(
                        out,
                        "    {ret} {}({});",
                        member(&op.name),
                        params.join(", ")
                    );
                }
                let _ = writeln!(out, "}}");
            }
            _ => {}
        }
    }

    // ---- functions -------------------------------------------------------------------

    fn params(&self, f: &LFn) -> Vec<String> {
        f.params
            .iter()
            .chain(&f.using)
            .map(|v| {
                format!(
                    "{} v{v}",
                    self.jty(&f.vars[*v as usize].ty, &f.generics, false)
                )
            })
            .collect()
    }

    fn function(&mut self, f: &LFn, out: &mut String) {
        let p = self.p;
        let name = ident(&p.def(f.def).name.display());
        let class = self.class_of(f.def);
        let g = generics_decl(&f.generics);
        let gsp = if g.is_empty() {
            String::new()
        } else {
            format!("{g} ")
        };
        let targs = if f.generics.is_empty() {
            String::new()
        } else {
            g.clone()
        };
        let ret = self.jty(&f.ret, &f.generics, false);
        let rt = self.rt.clone();
        let mut params = self.params(f);
        params.push(format!("{rt}.Cx cx"));
        let args: Vec<String> = f
            .params
            .iter()
            .chain(&f.using)
            .map(|v| format!("v{v}, "))
            .collect();
        let id = self.fn_ids[&f.def];
        let symbol = java_str(p.def(f.def).stable_id.as_str());
        self.breaks.clear();
        let mut w = String::new();
        let _ = writeln!(
            w,
            "    public static {gsp}{ret} x_{name}({}) {{",
            params.join(", ")
        );
        if self.q.has(f.def) {
            // the speculative form first (java_q.rs); on a bail the generic form below runs
            let names: Vec<String> = f.params.iter().map(|v| format!("v{v}")).collect();
            w.push_str(&self.q.wrapper(p, f.def, &names, id, &class, &name));
        }
        w.push_str("        cx.enter();\n");
        if f.measure.is_some() {
            let _ = writeln!(w, "        cx.measurePush({id});");
        }
        let mut fx = Fx {
            r: self,
            vars: &f.vars,
            generics: &f.generics,
            text_bufs: BTreeSet::new(),
        };
        let mut live = fx.stmts(&f.requires, 2, &mut w);
        if let (true, Some((stmts, value, clause))) = (live, &f.measure) {
            live = fx.stmts(stmts, 2, &mut w);
            if live {
                let v = fx.expr(value);
                let _ = writeln!(
                    w,
                    "        cx.measureSet({id}, {v}, {}, {symbol});",
                    java_str(clause)
                );
            }
        }
        if live {
            let _ = writeln!(
                w,
                "        {ret} r = {class}.{targs}b_{name}({}cx);",
                args.concat()
            );
            if !f.ensures.is_empty() {
                if let Some(r) = f.result {
                    let _ = writeln!(
                        w,
                        "        {} v{r} = r;",
                        fx.r.jty(&f.vars[r as usize].ty, &f.generics, false)
                    );
                }
                live = fx.stmts(&f.ensures, 2, &mut w);
            }
            if live {
                if f.measure.is_some() {
                    let _ = writeln!(w, "        cx.measurePop();");
                }
                let _ = writeln!(w, "        cx.leave();\n        return r;");
            }
        }
        let _ = writeln!(w, "    }}\n");
        let _ = writeln!(
            w,
            "    public static {gsp}{ret} b_{name}({}) {{",
            params.join(", ")
        );
        // the body's own text variables that it appends to (a parameter stays a String)
        let mut bufs = BTreeSet::new();
        crate::go::appended_texts(fx.r.p, &f.body, &f.vars, &mut bufs);
        let mut defined = BTreeSet::new();
        defined_vars(&f.body, &mut defined);
        fx.text_bufs = bufs.intersection(&defined).copied().collect();
        if fx.stmts(&f.body, 2, &mut w) {
            let _ = writeln!(
                w,
                "        throw new java.lang.IllegalStateException(\"unreachable\");"
            );
        }
        let _ = writeln!(w, "    }}\n");
        if self.q.has(f.def) {
            w.push_str(&self.q.functions(p, f.def, &|d| self.class_of(d)));
        }
        out.push_str(&w);
    }

    /// The host form of a `pub fn` (`spec/hosts.md` 4.3).
    fn host_function(&mut self, f: &LFn, out: &mut String) {
        let p = self.p;
        let def = p.def(f.def);
        let fd = def.kind.as_fn().expect("a function");
        let name = ident(&def.name.display());
        let host = if self.exact.contains(&f.def) {
            ident(&def.name.display())
        } else {
            member(&def.name.display())
        };
        let g = generics_decl(&f.generics);
        let gsp = if g.is_empty() {
            String::new()
        } else {
            format!("{g} ")
        };
        let ret = self.jty(&f.ret, &f.generics, false);
        let rt = self.rt.clone();
        let base_head = self.base.split('.').next().unwrap_or("").to_string();
        let pname = |n: &str| {
            let i = ident(n);
            if i == base_head {
                format!("{i}_")
            } else {
                i
            }
        };
        let params: Vec<String> = fd
            .params
            .iter()
            .chain(&fd.using)
            .map(|q| format!("{} {}", self.jty(&q.ty, &f.generics, false), pname(&q.name)))
            .collect();
        let doc = def
            .docs
            .as_ref()
            .and_then(|d| d.lines().next())
            .map(|l| format!(": {}", doc_text(l)))
            .unwrap_or_default();
        let symbol = java_str(def.stable_id.as_str());
        let _ = writeln!(
            out,
            "    /** The CPL function {{@code {}}}{doc}. @throws {rt}.Fault when the call faults */\n    public static {gsp}{ret} {host}({}) {{\n        {rt}.Cx cx = new {rt}.Cx();\n        try {{",
            p.display_path(f.def),
            params.join(", ")
        );
        for q in &fd.params {
            if !crate::has_type_param(&q.ty) && needs_validation(p, &q.ty) {
                let v = self.validator_name(&q.ty);
                let _ = writeln!(
                    out,
                    "            if (!{}.Support.{v}({}, cx)) {{\n                throw {rt}.Fault.refinement({}, {symbol});\n            }}",
                    self.base,
                    pname(&q.name),
                    java_str(&p.display_ty(&q.ty))
                );
            }
        }
        let args: Vec<String> = fd
            .params
            .iter()
            .chain(&fd.using)
            .map(|q| format!("{}, ", pname(&q.name)))
            .collect();
        let _ = writeln!(
            out,
            "            return x_{name}({}cx);\n        }} catch ({rt}.Fault $f) {{\n            throw cx.attribute($f);\n        }} catch (java.lang.NullPointerException $e) {{\n            throw cx.attribute({rt}.Fault.nullValue());\n        }} catch (java.lang.StackOverflowError $e) {{\n            throw {rt}.Fault.stack();\n        }}\n    }}\n",
            args.concat()
        );
    }

    fn pred(&mut self, q: &LPred, out: &mut String) {
        let name = ident(&self.p.def(q.def).name.display());
        let g = generics_decl(&q.generics);
        let gsp = if g.is_empty() {
            String::new()
        } else {
            format!("{g} ")
        };
        let self_ty = self.jty(&q.self_ty, &q.generics, false);
        let rt = self.rt.clone();
        self.breaks.clear();
        let _ = writeln!(
            out,
            "    public static {gsp}boolean pred_{name}({self_ty} v{}, {rt}.Cx cx) {{",
            q.self_var
        );
        let mut fx = Fx {
            r: self,
            vars: &q.vars,
            generics: &q.generics,
            text_bufs: BTreeSet::new(),
        };
        let mut body = String::new();
        if fx.stmts(&q.body, 2, &mut body) {
            let v = fx.expr(&q.value);
            let _ = writeln!(body, "        return {v};");
        }
        out.push_str(&body);
        out.push_str("    }\n\n");
    }

    // ---- validation and decoding at the boundary ------------------------------------

    fn validator_name(&mut self, ty: &Ty) -> String {
        let key = self.p.display_ty(ty);
        let n = self.validators.len();
        let name = self
            .validators
            .entry(key.clone())
            .or_insert_with(|| format!("valid_{n}"))
            .clone();
        self.pending.push((key, ty.clone()));
        name
    }

    fn validator(&mut self, key: &str, ty: &Ty, out: &mut String) {
        let p = self.p;
        let name = self.validators[key].clone();
        let t = self.jty(ty, &[], false);
        let rt = self.rt.clone();
        let mut body = String::new();
        match ty {
            Ty::Refined(_, d) => {
                if let Some(b) = p
                    .def(*d)
                    .kind
                    .as_refinement()
                    .map(|r| r.base.clone())
                    .filter(|b| needs_validation(p, b))
                {
                    let v = self.validator_name(&b);
                    let _ = writeln!(
                        body,
                        "        if (!{v}(x, cx)) {{\n            return false;\n        }}"
                    );
                }
                for r in crate::lower::check_order(p, *d) {
                    let _ = writeln!(
                        body,
                        "        if (!{}.pred_{}(x, cx)) {{\n            return false;\n        }}",
                        self.class_of(r),
                        ident(&p.def(r).name.display())
                    );
                }
            }
            Ty::List(e) => {
                let v = self.validator_name(e);
                let et = self.jty(e, &[], false);
                let _ = writeln!(body, "        for ({et} y : x) {{\n            if (!{v}(y, cx)) {{\n                return false;\n            }}\n        }}");
            }
            Ty::Wrapper(d) => {
                if let Some(w) = p.def(*d).kind.as_wrapper() {
                    let v = self.validator_name(&w.inner);
                    let _ = writeln!(body, "        if (!{v}(x.value(), cx)) {{\n            return false;\n        }}");
                }
            }
            Ty::Adt(d, args) => match (std_type(p, *d), &p.def(*d).kind) {
                (Some(StdType::Option), _) => {
                    if needs_validation(p, &args[0]) {
                        let v = self.validator_name(&args[0]);
                        let a = self.jty(&args[0], &[], true);
                        let _ = writeln!(body, "        if (x instanceof {rt}.Option.Some<{a}> s && !{v}(s.value(), cx)) {{\n            return false;\n        }}");
                    }
                }
                // the runtime's records hold texts a host may have made
                (Some(StdType::ForeignError), _) => {
                    let _ = writeln!(body, "        if (!{rt}.Rt.validText(x.message())) {{\n            return false;\n        }}\n        for ({rt}.ForeignDetail d : x.details()) {{\n            if (!{rt}.Rt.validText(d.key()) || !{rt}.Rt.validText(d.value())) {{\n                return false;\n            }}\n        }}");
                }
                (Some(StdType::ForeignDetail), _) => {
                    let _ = writeln!(body, "        if (!{rt}.Rt.validText(x.key()) || !{rt}.Rt.validText(x.value())) {{\n            return false;\n        }}");
                }
                (Some(StdType::ConstraintError), _) => {
                    let _ = writeln!(body, "        if (!{rt}.Rt.validText(x.typeName()) || !{rt}.Rt.validText(x.message())) {{\n            return false;\n        }}");
                }
                (Some(StdType::Result), _) => {
                    let (a, b) = (self.jty(&args[0], &[], true), self.jty(&args[1], &[], true));
                    if needs_validation(p, &args[0]) {
                        let v = self.validator_name(&args[0]);
                        let _ = writeln!(body, "        if (x instanceof {rt}.Result.Ok<{a}, {b}> s && !{v}(s.value(), cx)) {{\n            return false;\n        }}");
                    }
                    if needs_validation(p, &args[1]) {
                        let v = self.validator_name(&args[1]);
                        let _ = writeln!(body, "        if (x instanceof {rt}.Result.Err<{a}, {b}> s && !{v}(s.error(), cx)) {{\n            return false;\n        }}");
                    }
                }
                (None, DefKind::Struct(s)) => {
                    for (i, f) in s.fields.iter().enumerate() {
                        let ft = instantiate(&f.ty, args);
                        if needs_validation(p, &ft) {
                            let v = self.validator_name(&ft);
                            let _ = writeln!(body, "        if (!{v}(x.{}(), cx)) {{\n            return false;\n        }}", self.field_name(*d, i));
                        }
                    }
                    if s.invariant.is_some() {
                        let targs = if args.is_empty() {
                            String::new()
                        } else {
                            format!(
                                "<{}>",
                                args.iter()
                                    .map(|a| self.jty(a, &[], true))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )
                        };
                        let _ = writeln!(
                            body,
                            "        if (!{}.{targs}pred_{}(x, cx)) {{\n            return false;\n        }}",
                            self.class_of(*d),
                            ident(&p.def(*d).name.display())
                        );
                    }
                }
                (None, DefKind::Enum(e)) => {
                    for (vi, v) in e.variants.iter().enumerate() {
                        let tys: Vec<Ty> = v.payload.iter().map(|t| instantiate(t, args)).collect();
                        if !tys.iter().any(|t| needs_validation(p, t)) {
                            continue;
                        }
                        let vt = self.variant_ty(*d, vi as u32, ty, &[]);
                        let y = self.fresh("y");
                        let _ = writeln!(body, "        if (x instanceof {vt} {y}) {{");
                        for (i, t) in tys.iter().enumerate() {
                            if needs_validation(p, t) {
                                let vn = self.validator_name(t);
                                let _ = writeln!(body, "            if (!{vn}({y}.f{i}(), cx)) {{\n                return false;\n            }}");
                            }
                        }
                        let _ = writeln!(body, "        }}");
                    }
                }
                _ => {}
            },
            Ty::Text => {
                let _ = writeln!(
                    body,
                    "        if (!{rt}.Rt.validText(x)) {{\n            return false;\n        }}"
                );
            }
            _ => {}
        }
        let _ = writeln!(out, "    public static boolean {name}({t} x, {rt}.Cx cx) {{\n{body}        return true;\n    }}\n");
    }

    /// The decoder of value JSON at a concrete type, as a qualified method name.
    fn decoder_of(&mut self, ty: &Ty) -> String {
        let p = self.p;
        let rt = &self.rt;
        match ty {
            Ty::Int => format!("{rt}.Json.decInt"),
            Ty::Bool => format!("{rt}.Json.decBool"),
            Ty::Text => format!("{rt}.Json.decText"),
            Ty::Bytes => format!("{rt}.Json.decBytes"),
            Ty::Refined(..) => self.decoder_of(&erase(p, ty)),
            Ty::Adt(d, _) if std_type(p, *d) == Some(StdType::Unit) => format!("{rt}.Json.decUnit"),
            Ty::Adt(d, _) if std_type(p, *d) == Some(StdType::ConstraintError) => {
                format!("{rt}.Json.decConstraintError")
            }
            Ty::Adt(d, _) if std_type(p, *d) == Some(StdType::ForeignError) => {
                format!("{rt}.Json.decForeignError")
            }
            Ty::Adt(d, _) if std_type(p, *d) == Some(StdType::ForeignErrorKind) => {
                format!("{rt}.Json.decForeignErrorKind")
            }
            Ty::Adt(d, _) if std_type(p, *d) == Some(StdType::ForeignDetail) => {
                format!("{rt}.Json.decForeignDetail")
            }
            _ => {
                let key = p.display_ty(ty);
                let n = self.decoders.len();
                let name = self
                    .decoders
                    .entry(key.clone())
                    .or_insert_with(|| format!("dec_{n}"))
                    .clone();
                self.pending_dec.push((key, ty.clone()));
                format!("{}.Support.{name}", self.base)
            }
        }
    }

    fn method_ref(name: &str) -> String {
        match name.rfind('.') {
            Some(i) => format!("{}::{}", &name[..i], &name[i + 1..]),
            None => name.to_string(),
        }
    }

    fn decoder(&mut self, key: &str, ty: &Ty, out: &mut String) {
        let p = self.p;
        let name = self.decoders[key].clone();
        let t = self.jty(ty, &[], false);
        let rt = self.rt.clone();
        let _ = writeln!(out, "    public static {t} {name}(java.lang.Object j) {{");
        match ty {
            Ty::List(e) => {
                let de = Self::method_ref(&self.decoder_of(e));
                let _ = writeln!(out, "        return {rt}.Json.decList(j, {de});");
            }
            Ty::Wrapper(d) => {
                let inner = p
                    .def(*d)
                    .kind
                    .as_wrapper()
                    .map(|w| w.inner.clone())
                    .unwrap_or(Ty::Error);
                let de = self.decoder_of(&inner);
                let _ = writeln!(
                    out,
                    "        {rt}.Json.variant(j, {});\n        java.util.List<java.lang.Object> p = {rt}.Json.payload(j);\n        if (p.size() != 1) {{\n            throw new {rt}.Json.DecodeError(\"expected a wrapper\");\n        }}\n        return new {t}({de}(p.get(0)));",
                    java_str(&crate::lower::type_name(p, *d))
                );
            }
            Ty::Adt(d, args) => match (std_type(p, *d), &p.def(*d).kind) {
                (Some(StdType::Option), _) => {
                    let de = Self::method_ref(&self.decoder_of(&args[0]));
                    let _ = writeln!(out, "        return {rt}.Json.decOption(j, {de});");
                }
                (Some(StdType::Result), _) => {
                    let a = Self::method_ref(&self.decoder_of(&args[0]));
                    let b = Self::method_ref(&self.decoder_of(&args[1]));
                    let _ = writeln!(out, "        return {rt}.Json.decResult(j, {a}, {b});");
                }
                (None, DefKind::Struct(s)) => {
                    let _ = writeln!(
                        out,
                        "        {rt}.Json.expectType(j, {});",
                        java_str(&crate::lower::type_name(p, *d))
                    );
                    let mut fields = Vec::new();
                    for f in &s.fields {
                        let ft = instantiate(&f.ty, args);
                        let de = self.decoder_of(&ft);
                        fields.push(format!("{de}({rt}.Json.field(j, {}))", java_str(&f.name)));
                    }
                    let _ = writeln!(out, "        return new {t}({});", fields.join(", "));
                }
                (None, DefKind::Enum(e)) => {
                    let _ = writeln!(
                        out,
                        "        java.lang.String v = {rt}.Json.variant(j, {});\n        java.util.List<java.lang.Object> p = {rt}.Json.payload(j);",
                        java_str(&crate::lower::type_name(p, *d))
                    );
                    for (vi, v) in e.variants.iter().enumerate() {
                        let tys: Vec<Ty> = v.payload.iter().map(|x| instantiate(x, args)).collect();
                        let vt = self.variant_ty(*d, vi as u32, ty, &[]);
                        let mut xs = Vec::new();
                        for (i, pt) in tys.iter().enumerate() {
                            let de = self.decoder_of(pt);
                            xs.push(format!("{de}(p.get({i}))"));
                        }
                        let _ = writeln!(
                            out,
                            "        if (v.equals({}) && p.size() == {}) {{\n            return new {vt}({});\n        }}",
                            java_str(&v.name),
                            tys.len(),
                            xs.join(", ")
                        );
                    }
                    let _ = writeln!(
                        out,
                        "        throw new {rt}.Json.DecodeError(\"expected a {}\");",
                        p.def(*d).name.display()
                    );
                }
                _ => {
                    let _ = writeln!(
                        out,
                        "        throw new {rt}.Json.DecodeError(\"not decodable\");"
                    );
                }
            },
            _ => {
                let _ = writeln!(
                    out,
                    "        throw new {rt}.Json.DecodeError(\"not decodable\");"
                );
            }
        }
        let _ = writeln!(out, "    }}\n");
    }

    // ---- the JSON driver ---------------------------------------------------------------

    fn driver(&mut self, lowered: &Lowered, out: &mut String) {
        let p = self.p;
        let rt = self.rt.clone();
        let base = self.base.clone();
        let _ = writeln!(
            out,
            "    public static void main(java.lang.String[] args) throws java.lang.InterruptedException {{\n        java.lang.Thread t = new java.lang.Thread(null, () -> {rt}.Rt.serve(Driver::call), \"cpl-driver\", 1L << 30);\n        t.start();\n        t.join();\n    }}\n\n    static java.lang.Object call(java.lang.String function, java.util.List<java.lang.Object> args) {{\n        {rt}.Cx cx = new {rt}.Cx();\n        try {{\n            return dispatch(function, args, cx);\n        }} catch ({rt}.Fault $f) {{\n            throw cx.attribute($f);\n        }}\n    }}\n\n    static java.lang.Object dispatch(java.lang.String function, java.util.List<java.lang.Object> args, {rt}.Cx cx) {{\n        switch (function) {{"
        );
        for d in crate::driver_functions(p, lowered) {
            let fd = p.def(d).kind.as_fn().expect("a function");
            let _ = writeln!(
                out,
                "            case {}: {{\n                if (args.size() != {}) {{\n                    return {rt}.Rt.outside(\"arity\");\n                }}",
                java_str(&p.display_path(d)),
                fd.params.len()
            );
            let mut names = Vec::new();
            for (i, q) in fd.params.iter().enumerate() {
                let de = self.decoder_of(&q.ty);
                let t = self.jty(&q.ty, &[], false);
                let _ = writeln!(out, "                {t} a{i} = {de}(args.get({i}));");
                if needs_validation(p, &q.ty) {
                    let v = self.validator_name(&q.ty);
                    let _ = writeln!(out, "                if (!{base}.Support.{v}(a{i}, cx)) {{\n                    return {rt}.Rt.outside(\"refinement\");\n                }}");
                }
                names.push(format!("a{i}, "));
            }
            let _ = writeln!(
                out,
                "                return {rt}.Rt.answer({}.x_{}({}cx));\n            }}",
                self.class_of(d),
                ident(&p.def(d).name.display()),
                names.concat()
            );
        }
        let _ = writeln!(out, "            default:\n                return {rt}.Rt.outside(\"unknown function\");\n        }}\n    }}");
    }
}

/// The printing context of one function body.
struct Fx<'a, 'p> {
    r: &'a mut Printer<'p>,
    vars: &'a [VarDef],
    generics: &'a [String],
    /// The text variables appended to through a `cpl.TextBuf` (`v<var>` is then the buffer, read with `get()`), so
    /// that a loop of appends is linear where a `String` would be copied whole by each.
    text_bufs: BTreeSet<Var>,
}

/// The variables `stmts` defines with `Let` or `Decl`, at any depth.
fn defined_vars(stmts: &[LStmt], out: &mut BTreeSet<Var>) {
    for s in stmts {
        match s {
            LStmt::Let { var, .. } | LStmt::Decl { var } => {
                out.insert(*var);
            }
            LStmt::If { then, els, .. } => {
                defined_vars(then, out);
                defined_vars(els, out);
            }
            LStmt::Match { arms, .. } => {
                for a in arms {
                    defined_vars(&a.body, out);
                }
            }
            LStmt::Loop { body, .. } | LStmt::ForEach { body, .. } => defined_vars(body, out),
            _ => {}
        }
    }
}

impl Fx<'_, '_> {
    fn jty(&self, t: &Ty, boxed: bool) -> String {
        self.r.jty(t, self.generics, boxed)
    }

    fn var_ty(&self, v: Var) -> String {
        self.jty(&self.vars[v as usize].ty, false)
    }

    /// Print `stmts`; whether the block can complete normally (JLS 14.22). A statement that
    /// is unreachable in Java is a compile error, so none is printed.
    fn stmts(&mut self, stmts: &[LStmt], depth: usize, out: &mut String) -> bool {
        for s in stmts {
            if !self.stmt(s, depth, out) {
                return false;
            }
        }
        true
    }

    /// A capability operation returning Unit, a `void` method in Java.
    fn unit_cap_call(&mut self, e: &LExpr) -> Option<String> {
        if let LExpr::CapCall {
            handle,
            capability,
            op,
            args,
            ty,
        } = e
        {
            if self.r.is_unit(ty) {
                let opdef = &self
                    .r
                    .p
                    .def(*capability)
                    .kind
                    .as_capability()
                    .expect("a capability")
                    .ops[*op as usize];
                let h = self.expr(handle);
                let a: Vec<String> = args.iter().map(|x| self.expr(x)).collect();
                return Some(format!("({h}).{}({})", member(&opdef.name), a.join(", ")));
            }
        }
        None
    }

    fn stmt(&mut self, s: &LStmt, depth: usize, out: &mut String) -> bool {
        let pad = "    ".repeat(depth);
        let rt = self.r.rt.clone();
        match s {
            LStmt::ContextSet { handle, value } => {
                let h = self.expr(handle);
                let v = self.expr(value);
                let _ = writeln!(out, "{pad}({h}).set({v});");
                true
            }
            LStmt::Let { var, value } => {
                if let Some(call) = self.unit_cap_call(value) {
                    let _ = writeln!(out, "{pad}{call};\n{pad}{rt}.Unit v{var} = {rt}.Unit.UNIT;");
                    return true;
                }
                let e = self.expr(value);
                if self.text_bufs.contains(var) {
                    let _ = writeln!(out, "{pad}final {rt}.TextBuf v{var} = new {rt}.TextBuf({e});");
                    return true;
                }
                let _ = writeln!(out, "{pad}{} v{var} = {e};", self.var_ty(*var));
                true
            }
            LStmt::Decl { var } => {
                let d = self.r.default_value(&self.vars[*var as usize].ty);
                if self.text_bufs.contains(var) {
                    let _ = writeln!(out, "{pad}final {rt}.TextBuf v{var} = new {rt}.TextBuf({d});");
                    return true;
                }
                let _ = writeln!(out, "{pad}{} v{var} = {d};", self.var_ty(*var));
                true
            }
            LStmt::Assign { var, value } => {
                if self.text_bufs.contains(var) {
                    // x = x ++ a ++ b ...: every part first (each sees x unchanged), then appended to the buffer
                    if let Some(parts) = crate::go::append_chain(*var, value) {
                        let names: Vec<String> = parts
                            .iter()
                            .map(|part| {
                                let e = self.expr(part);
                                let n = self.r.fresh("ap");
                                let _ = writeln!(out, "{pad}final java.lang.String {n} = {e};");
                                n
                            })
                            .collect();
                        for n in names {
                            let _ = writeln!(out, "{pad}v{var}.append({n});");
                        }
                        return true;
                    }
                    let e = self.expr(value);
                    let _ = writeln!(out, "{pad}v{var}.set({e});");
                    return true;
                }
                let e = self.expr(value);
                let _ = writeln!(out, "{pad}v{var} = {e};");
                true
            }
            LStmt::Expr(e) => {
                if let Some(call) = self.unit_cap_call(e) {
                    let _ = writeln!(out, "{pad}{call};");
                    return true;
                }
                let e = self.expr(e);
                let _ = writeln!(out, "{pad}{rt}.Rt.discard({e});");
                true
            }
            LStmt::If { cond, then, els } => {
                let c = self.expr(cond);
                let _ = writeln!(out, "{pad}if ({c}) {{");
                let a = self.stmts(then, depth + 1, out);
                if els.is_empty() {
                    let _ = writeln!(out, "{pad}}}");
                    true
                } else {
                    let _ = writeln!(out, "{pad}}} else {{");
                    let b = self.stmts(els, depth + 1, out);
                    let _ = writeln!(out, "{pad}}}");
                    a || b
                }
            }
            LStmt::Match { scrutinee, arms } => {
                let label = self.r.fresh("m");
                let _ = writeln!(out, "{pad}{label}: {{");
                let mut broke = false;
                let mut reachable = true;
                for arm in arms {
                    if !reachable {
                        break;
                    }
                    let mut opens = 0;
                    let subject = if self.text_bufs.contains(scrutinee) {
                        format!("v{scrutinee}.get()")
                    } else {
                        format!("v{scrutinee}")
                    };
                    self.pattern(
                        &arm.pattern,
                        &subject,
                        depth + 1,
                        &mut opens,
                        out,
                    );
                    let inner = depth + 1 + opens;
                    if self.stmts(&arm.body, inner, out) {
                        let _ = writeln!(out, "{}break {label};", "    ".repeat(inner));
                        broke = true;
                    }
                    for k in (0..opens).rev() {
                        let _ = writeln!(out, "{}}}", "    ".repeat(depth + 1 + k));
                    }
                    if opens == 0 {
                        reachable = false;
                    }
                }
                if reachable {
                    let _ = writeln!(
                        out,
                        "{pad}    throw new java.lang.IllegalStateException(\"no arm matched\");"
                    );
                }
                let _ = writeln!(out, "{pad}}}");
                broke
            }
            LStmt::Loop { label, body } => {
                self.r.breaks.remove(label);
                let _ = writeln!(out, "{pad}l{label}: while (true) {{");
                self.stmts(body, depth + 1, out);
                let _ = writeln!(out, "{pad}}}");
                self.r.breaks.contains(label)
            }
            LStmt::ForEach {
                label,
                var,
                list,
                body,
            } => {
                let l = self.expr(list);
                let _ = writeln!(
                    out,
                    "{pad}l{label}: for ({} v{var} : {l}) {{",
                    self.var_ty(*var)
                );
                self.stmts(body, depth + 1, out);
                let _ = writeln!(out, "{pad}}}");
                true
            }
            LStmt::Break { label } => {
                let _ = writeln!(out, "{pad}break l{label};");
                self.r.breaks.insert(*label);
                false
            }
            LStmt::Return(e) => {
                let e = self.expr(e);
                let _ = writeln!(out, "{pad}return {e};");
                false
            }
            LStmt::Fault(f) => {
                let kind = f.clause_kind.map(java_str).unwrap_or_else(|| "null".into());
                let _ = writeln!(
                    out,
                    "{pad}throw new {rt}.Fault({}, {kind}, {}, {}, {});",
                    java_str(f.code),
                    java_str(&f.clause),
                    java_str(&f.symbol),
                    java_str(&f.message)
                );
                false
            }
            LStmt::Push { list, value } => {
                let e = self.expr(value);
                let _ = writeln!(out, "{pad}v{list} = v{list}.push({e});");
                true
            }
            LStmt::ClauseEnter { owner } => {
                let _ = writeln!(out, "{pad}cx.clauseEnter({});", java_str(owner));
                true
            }
            LStmt::ClauseLeave => {
                let _ = writeln!(out, "{pad}cx.clauseLeave();");
                true
            }
        }
    }

    /// Open the tests of `pat` against `subject`, binding its variables; each `if` opened
    /// counts in `opens`.
    fn pattern(
        &mut self,
        pat: &LPat,
        subject: &str,
        depth: usize,
        opens: &mut usize,
        out: &mut String,
    ) {
        let p = self.r.p;
        let rt = self.r.rt.clone();
        let pad = "    ".repeat(depth + *opens);
        match pat {
            LPat::Wild => {}
            LPat::Bind(v) => {
                let _ = writeln!(out, "{pad}{} v{v} = {subject};", self.var_ty(*v));
            }
            LPat::Int(n) => {
                let _ = writeln!(out, "{pad}if (({subject}).equals({})) {{", self.int_lit(n));
                *opens += 1;
            }
            LPat::Text(s) => {
                let _ = writeln!(out, "{pad}if (({subject}).equals({})) {{", java_str(s));
                *opens += 1;
            }
            LPat::Bool(b) => {
                let _ = writeln!(out, "{pad}if ({}{subject}) {{", if *b { "" } else { "!" });
                *opens += 1;
            }
            LPat::Unwrap { arg, .. } => {
                self.pattern(arg, &format!("({subject}).value()"), depth, opens, out);
            }
            LPat::Ctor {
                def,
                variant,
                args,
                ty,
            } => {
                let targs = match ty {
                    Ty::Adt(_, a) => a.iter().map(|t| self.jty(t, true)).collect::<Vec<_>>(),
                    _ => Vec::new(),
                };
                match std_type(p, *def) {
                    Some(StdType::Option) => {
                        if *variant == 0 {
                            let o = self.r.fresh("o");
                            let _ = writeln!(
                                out,
                                "{pad}if ({subject} instanceof {rt}.Option.Some<{}> {o}) {{",
                                targs[0]
                            );
                            *opens += 1;
                            self.pattern(&args[0], &format!("{o}.value()"), depth, opens, out);
                        } else {
                            let _ = writeln!(
                                out,
                                "{pad}if ({subject} instanceof {rt}.Option.None<{}>) {{",
                                targs[0]
                            );
                            *opens += 1;
                        }
                    }
                    Some(StdType::Result) => {
                        let o = self.r.fresh("o");
                        let (v, get) = if *variant == 0 {
                            ("Ok", "value")
                        } else {
                            ("Err", "error")
                        };
                        let _ = writeln!(
                            out,
                            "{pad}if ({subject} instanceof {rt}.Result.{v}<{}, {}> {o}) {{",
                            targs[0], targs[1]
                        );
                        *opens += 1;
                        self.pattern(&args[0], &format!("{o}.{get}()"), depth, opens, out);
                    }
                    Some(StdType::ForeignErrorKind) => {
                        let e = p.def(*def).kind.as_enum().expect("an enum");
                        let _ = writeln!(
                            out,
                            "{pad}if ({subject} == {rt}.ForeignErrorKind.{}) {{",
                            e.variants[*variant as usize].name
                        );
                        *opens += 1;
                    }
                    _ => {
                        let vt = self.r.variant_ty(*def, *variant, ty, self.generics);
                        let o = self.r.fresh("o");
                        let _ = writeln!(out, "{pad}if ({subject} instanceof {vt} {o}) {{");
                        *opens += 1;
                        for (i, a) in args.iter().enumerate() {
                            self.pattern(a, &format!("{o}.f{i}()"), depth, opens, out);
                        }
                    }
                }
            }
        }
    }

    fn int_lit(&self, n: &num_bigint::BigInt) -> String {
        let rt = &self.r.rt;
        match n.to_i64() {
            Some(v) => format!("{rt}.Int.of({v}L)"),
            None => format!("{rt}.Int.lit(\"{n}\")"),
        }
    }

    fn targs(&self, generics: &[Ty]) -> String {
        if generics.is_empty() {
            String::new()
        } else {
            format!(
                "<{}>",
                generics
                    .iter()
                    .map(|g| self.jty(g, true))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }

    fn expr(&mut self, e: &LExpr) -> String {
        let p = self.r.p;
        let rt = self.r.rt.clone();
        match e {
            LExpr::Var(v) if self.text_bufs.contains(v) => format!("v{v}.get()"),
            LExpr::Var(v) => format!("v{v}"),
            LExpr::Int(n) => self.int_lit(n),
            LExpr::Text(s) => java_str(s),
            LExpr::Bool(b) => b.to_string(),
            LExpr::Unit => format!("{rt}.Unit.UNIT"),
            LExpr::ContextNew(x) => format!("new {rt}.Context<>({})", self.expr(x)),
            LExpr::ContextGet(h) => format!("({}).get()", self.expr(h)),
            LExpr::Call {
                def,
                generics,
                args,
                using,
            } => {
                let mut a: Vec<String> = args.iter().chain(using).map(|x| self.expr(x)).collect();
                a.push("cx".into());
                format!(
                    "{}.{}x_{}({})",
                    self.r.class_of(*def),
                    self.targs(generics),
                    ident(&p.def(*def).name.display()),
                    a.join(", ")
                )
            }
            LExpr::Builtin { id, args, .. } => {
                let a: Vec<String> = args.iter().map(|x| self.expr(x)).collect();
                format!("{rt}.Rt.{}({})", camel(id), a.join(", "))
            }
            LExpr::Op { op, args, ty } => {
                let a: Vec<String> = args.iter().map(|x| self.expr(x)).collect();
                let bool_ty = matches!(erase(p, ty), Ty::Bool);
                match op {
                    Op::Add => format!("({}).plus({})", a[0], a[1]),
                    Op::Sub => format!("({}).minus({})", a[0], a[1]),
                    Op::Mul => format!("({}).times({})", a[0], a[1]),
                    Op::Neg => format!("({}).unaryMinus()", a[0]),
                    Op::Lt => format!("({}).lt({})", a[0], a[1]),
                    Op::Le => format!("({}).le({})", a[0], a[1]),
                    Op::Gt => format!("({}).gt({})", a[0], a[1]),
                    Op::Ge => format!("({}).ge({})", a[0], a[1]),
                    Op::Eq if bool_ty => format!("{rt}.Rt.eqBool({}, {})", a[0], a[1]),
                    Op::Ne if bool_ty => format!("!{rt}.Rt.eqBool({}, {})", a[0], a[1]),
                    Op::Eq => format!("({}).equals({})", a[0], a[1]),
                    Op::Ne => format!("!({}).equals({})", a[0], a[1]),
                    Op::Not => format!("!({})", a[0]),
                    Op::And => format!("({} && {})", a[0], a[1]),
                    Op::Or => format!("({} || {})", a[0], a[1]),
                    Op::Concat => match erase(p, ty) {
                        Ty::Text => format!("({} + {})", a[0], a[1]),
                        Ty::Bytes => format!("({}).concat({})", a[0], a[1]),
                        _ => format!("{rt}.List.concat({}, {})", a[0], a[1]),
                    },
                }
            }
            LExpr::Div { a, b, rem, symbol } => {
                let (a, b) = (self.expr(a), self.expr(b));
                format!(
                    "({a}).{}({b}, {})",
                    if *rem { "rem" } else { "div" },
                    java_str(symbol)
                )
            }
            LExpr::Ctor {
                def,
                variant,
                args,
                ty,
            } => {
                let a: Vec<String> = args.iter().map(|x| self.expr(x)).collect();
                let targs: Vec<String> = match ty {
                    Ty::Adt(_, t) => t.iter().map(|x| self.jty(x, true)).collect(),
                    _ => Vec::new(),
                };
                match std_type(p, *def) {
                    Some(StdType::Option) => {
                        if *variant == 0 {
                            format!("{rt}.Option.<{}>some({})", targs[0], a[0])
                        } else {
                            format!("{rt}.Option.<{}>none()", targs[0])
                        }
                    }
                    Some(StdType::Result) => {
                        let v = if *variant == 0 { "ok" } else { "err" };
                        format!("{rt}.Result.<{}, {}>{v}({})", targs[0], targs[1], a[0])
                    }
                    Some(StdType::Unit) => format!("{rt}.Unit.UNIT"),
                    Some(StdType::ForeignErrorKind) => {
                        let e = p.def(*def).kind.as_enum().expect("an enum");
                        format!(
                            "{rt}.ForeignErrorKind.{}",
                            e.variants[*variant as usize].name
                        )
                    }
                    _ => {
                        let vt = self.r.variant_ty(*def, *variant, ty, self.generics);
                        format!("new {vt}({})", a.join(", "))
                    }
                }
            }
            LExpr::Struct { fields, ty, .. } => {
                let a: Vec<String> = fields.iter().map(|x| self.expr(x)).collect();
                format!("new {}({})", self.jty(ty, false), a.join(", "))
            }
            LExpr::Wrap { def, arg } => {
                let a = self.expr(arg);
                format!("new {}({a})", self.r.fqn(*def))
            }
            LExpr::Field {
                base, def, index, ..
            } => {
                let b = self.expr(base);
                let f = if std_type(p, *def).is_some() {
                    member(
                        &p.def(*def).kind.as_struct().expect("a struct").fields[*index as usize]
                            .name,
                    )
                } else {
                    self.r.field_name(*def, *index as usize)
                };
                format!("({b}).{f}()")
            }
            LExpr::Payload {
                base,
                def,
                variant,
                index,
                ty,
            } => {
                let b = self.expr(base);
                let targs: Vec<String> = match ty {
                    Ty::Adt(_, t) => t.iter().map(|x| self.jty(x, true)).collect(),
                    _ => Vec::new(),
                };
                match std_type(p, *def) {
                    Some(StdType::Option) => {
                        format!("(({rt}.Option.Some<{}>) ({b})).value()", targs[0])
                    }
                    Some(StdType::Result) if *variant == 0 => format!(
                        "(({rt}.Result.Ok<{}, {}>) ({b})).value()",
                        targs[0], targs[1]
                    ),
                    Some(StdType::Result) => format!(
                        "(({rt}.Result.Err<{}, {}>) ({b})).error()",
                        targs[0], targs[1]
                    ),
                    _ => {
                        let vt = self.r.variant_ty(*def, *variant, ty, self.generics);
                        format!("(({vt}) ({b})).f{index}()")
                    }
                }
            }
            LExpr::IsVariant {
                base, def, variant, ..
            } => {
                let b = self.expr(base);
                match std_type(p, *def) {
                    Some(StdType::Option) => format!(
                        "(({b}) instanceof {rt}.Option.{}<?>)",
                        if *variant == 0 { "Some" } else { "None" }
                    ),
                    Some(StdType::Result) => format!(
                        "(({b}) instanceof {rt}.Result.{}<?, ?>)",
                        if *variant == 0 { "Ok" } else { "Err" }
                    ),
                    Some(StdType::ForeignErrorKind) => {
                        let e = p.def(*def).kind.as_enum().expect("an enum");
                        format!(
                            "(({b}) == {rt}.ForeignErrorKind.{})",
                            e.variants[*variant as usize].name
                        )
                    }
                    _ => {
                        let generic = p
                            .def(*def)
                            .kind
                            .as_enum()
                            .is_some_and(|e| !e.generics.is_empty());
                        let n = p.def(*def).kind.as_enum().map_or(0, |e| e.generics.len());
                        let wild = if generic {
                            format!("<{}>", vec!["?"; n].join(", "))
                        } else {
                            String::new()
                        };
                        format!(
                            "(({b}) instanceof {}.{}{wild})",
                            self.r.fqn(*def),
                            self.r.variant_name(*def, *variant)
                        )
                    }
                }
            }
            LExpr::Unwrap { base, .. } => {
                let b = self.expr(base);
                format!("({b}).value()")
            }
            LExpr::List { elem, items } => {
                let t = self.jty(elem, true);
                if items.is_empty() {
                    format!("{rt}.List.<{t}>empty()")
                } else {
                    let a: Vec<String> = items.iter().map(|x| self.expr(x)).collect();
                    format!("{rt}.List.<{t}>lit({})", a.join(", "))
                }
            }
            LExpr::CapCall {
                handle,
                capability,
                op,
                args,
                ..
            } => {
                let opdef = &p
                    .def(*capability)
                    .kind
                    .as_capability()
                    .expect("a capability")
                    .ops[*op as usize];
                let h = self.expr(handle);
                let a: Vec<String> = args.iter().map(|x| self.expr(x)).collect();
                format!("({h}).{}({})", member(&opdef.name), a.join(", "))
            }
            LExpr::Pred { def, generics, arg } => {
                let a = self.expr(arg);
                format!(
                    "{}.{}pred_{}({a}, cx)",
                    self.r.class_of(*def),
                    self.targs(generics),
                    ident(&p.def(*def).name.display())
                )
            }
            LExpr::Valid { ty, arg } => {
                let a = self.expr(arg);
                let v = self.r.validator_name(ty);
                format!("{}.Support.{v}({a}, cx)", self.r.base)
            }
            LExpr::CheckFailure {
                value,
                type_name,
                predicate,
                ty,
                ..
            } => {
                let v = self.expr(value);
                let ok = match ty {
                    Ty::Adt(_, a) if !a.is_empty() => self.jty(&a[0], true),
                    _ => format!("{rt}.Unit"),
                };
                format!(
                    "{rt}.Rt.<{ok}>checkFailure({v}, {}, {})",
                    java_str(type_name),
                    java_str(predicate)
                )
            }
        }
    }
}
