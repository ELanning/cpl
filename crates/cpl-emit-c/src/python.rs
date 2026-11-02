//! The Python side of Milestone 3 (`spec/python.md`): the types module of a program (its classes, type descriptors,
//! and the node-list codec, Section 3), a Python worker's source (Section 6), and the bootstrap that starts one.
//!
//! The fixed Python is `python/runtime.py` (the tags, the codec, the failure mapping) and `python/worker.py` (a
//! worker's main loop); this module generates the classes and tables that follow the fixed part and that it reads.

use cpl_hir::{DefId, DefKind, Program, Ty};
use cpl_project::{Manifest, PYTHON_KEYWORDS};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;

const RUNTIME: &str = include_str!("python/runtime.py");
const WORKER: &str = include_str!("python/worker.py");

/// The bootstrap of a Python worker (`spec/python.md` 6.1, P7), passed as `<python> -c BOOTSTRAP`: it reads
/// `<length>\n<source>` from standard input byte-exactly and executes the source as the module `__cpl_worker__`.
pub const BOOTSTRAP: &str = "import os,sys,types\n\
def _r(n):\n b=bytearray()\n while len(b)<n:\n  c=os.read(0,n-len(b))\n  if not c:sys.exit(70)\n  b+=c\n return bytes(b)\n\
h=bytearray()\n\
while not h.endswith(b'\\n'):\n c=os.read(0,1)\n if not c:sys.exit(70)\n h+=c\n\
s=_r(int(h))\n\
m=types.ModuleType('__cpl_worker__')\n\
sys.modules['__cpl_worker__']=m\n\
exec(compile(s,'<cpl-worker>','exec'),m.__dict__)\n";

/// The names of Python 3.10's `builtins`: a generated class never takes one, since the fixed part of the module
/// reads several of them (`Exception`, `isinstance`, ...) as globals when it runs.
const PYTHON_BUILTINS: &[&str] = &[
    "ArithmeticError",
    "AssertionError",
    "AttributeError",
    "BaseException",
    "BlockingIOError",
    "BrokenPipeError",
    "BufferError",
    "BytesWarning",
    "ChildProcessError",
    "ConnectionAbortedError",
    "ConnectionError",
    "ConnectionRefusedError",
    "ConnectionResetError",
    "DeprecationWarning",
    "EOFError",
    "Ellipsis",
    "EncodingWarning",
    "EnvironmentError",
    "Exception",
    "False",
    "FileExistsError",
    "FileNotFoundError",
    "FloatingPointError",
    "FutureWarning",
    "GeneratorExit",
    "IOError",
    "ImportError",
    "ImportWarning",
    "IndentationError",
    "IndexError",
    "InterruptedError",
    "IsADirectoryError",
    "KeyError",
    "KeyboardInterrupt",
    "LookupError",
    "MemoryError",
    "ModuleNotFoundError",
    "NameError",
    "None",
    "NotADirectoryError",
    "NotImplemented",
    "NotImplementedError",
    "OSError",
    "OverflowError",
    "PendingDeprecationWarning",
    "PermissionError",
    "ProcessLookupError",
    "RecursionError",
    "ReferenceError",
    "ResourceWarning",
    "RuntimeError",
    "RuntimeWarning",
    "StopAsyncIteration",
    "StopIteration",
    "SyntaxError",
    "SyntaxWarning",
    "SystemError",
    "SystemExit",
    "TabError",
    "TimeoutError",
    "True",
    "TypeError",
    "UnboundLocalError",
    "UnicodeDecodeError",
    "UnicodeEncodeError",
    "UnicodeError",
    "UnicodeTranslateError",
    "UnicodeWarning",
    "UserWarning",
    "ValueError",
    "Warning",
    "ZeroDivisionError",
    "__build_class__",
    "__debug__",
    "__doc__",
    "__import__",
    "__loader__",
    "__name__",
    "__package__",
    "__spec__",
    "abs",
    "aiter",
    "all",
    "anext",
    "any",
    "ascii",
    "bin",
    "bool",
    "breakpoint",
    "bytearray",
    "bytes",
    "callable",
    "chr",
    "classmethod",
    "compile",
    "complex",
    "copyright",
    "credits",
    "delattr",
    "dict",
    "dir",
    "divmod",
    "enumerate",
    "eval",
    "exec",
    "exit",
    "filter",
    "float",
    "format",
    "frozenset",
    "getattr",
    "globals",
    "hasattr",
    "hash",
    "help",
    "hex",
    "id",
    "input",
    "int",
    "isinstance",
    "issubclass",
    "iter",
    "len",
    "license",
    "list",
    "locals",
    "map",
    "max",
    "memoryview",
    "min",
    "next",
    "object",
    "oct",
    "open",
    "ord",
    "pow",
    "print",
    "property",
    "quit",
    "range",
    "repr",
    "reversed",
    "round",
    "set",
    "setattr",
    "slice",
    "sorted",
    "staticmethod",
    "str",
    "sum",
    "super",
    "tuple",
    "type",
    "vars",
    "zip",
];

/// The names the fixed part of the types module defines or reads.
const RESERVED: &[&str] = &[
    "Nothing",
    "NOTHING",
    "Some",
    "Ok",
    "Err",
    "ForeignFailure",
    "encode",
    "decode",
    "PROGRAM",
];

/// A name as a Python identifier that shadows nothing: a keyword, a builtin, or a reserved name gets a trailing `_`
/// (`spec/python.md` 3.1).
pub fn py_name(name: &str) -> String {
    if PYTHON_KEYWORDS.contains(&name)
        || PYTHON_BUILTINS.contains(&name)
        || RESERVED.contains(&name)
    {
        format!("{name}_")
    } else {
        name.to_string()
    }
}

/// A field, parameter, or operation name: keywords get a trailing `_`.
pub fn py_member(name: &str) -> String {
    if PYTHON_KEYWORDS.contains(&name) {
        format!("{name}_")
    } else {
        name.to_string()
    }
}

/// A Python string literal of `s`.
pub fn py_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                let _ = write!(out, "\\x{:02x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// One type descriptor of `_TYPES` (`spec/python.md` 3.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Desc {
    Int,
    Bool,
    Text,
    Bytes,
    Unit,
    List(usize),
    Option(usize),
    Result(usize, usize),
    /// The declaration and its fields: (Python name, CPL name, descriptor).
    Struct(DefId, Vec<(String, String, usize)>),
    /// The declaration and each variant's payload descriptors.
    Enum(DefId, Vec<Vec<usize>>),
    Wrapper(DefId, usize),
    Refined(usize),
    Capability(DefId),
    /// A type the codec does not describe (a generic parameter, a generator, an error): no value is of it.
    Opaque,
}

/// A class of the types module.
#[derive(Clone, Debug)]
pub struct PyClass {
    pub def: DefId,
    /// The Python name (`Page`, `reader_pages_Page`).
    pub name: String,
    /// The qualified CPL name (`reader.pages.Page`).
    pub path: String,
    pub kind: PyClassKind,
}

#[derive(Clone, Debug)]
pub enum PyClassKind {
    /// Fields: (Python name, CPL name, annotation).
    Struct(Vec<(String, String, String)>),
    Wrapper(String),
    /// Variants: (attribute name, CPL name, payload annotations).
    Enum(Vec<(String, String, Vec<String>)>),
}

/// One capability as the codec sees it: its qualified name and, per operation, (Python name, return descriptor,
/// qualified operation name, parameter names, parameter descriptors).
#[derive(Clone, Debug)]
pub struct PyCapability {
    pub def: DefId,
    pub path: String,
    pub ops: Vec<PyOp>,
}

#[derive(Clone, Debug)]
pub struct PyOp {
    pub name: String,
    pub py_name: String,
    pub qualified: String,
    pub params: Vec<(String, usize)>,
    pub ret: usize,
}

/// The program as the generated Python sees it.
pub struct PyModel {
    pub digest: String,
    pub classes: Vec<PyClass>,
    pub types: Vec<(Desc, String)>,
    index: HashMap<Ty, usize>,
    pub capabilities: Vec<PyCapability>,
    /// The backend-python bindings: (name, module path, capability).
    pub python_bindings: Vec<(String, String, DefId)>,
    pub unit: DefId,
    pub option: DefId,
    pub result: DefId,
    /// Variant indices: Option's (None, Some), Result's (Ok, Err).
    pub option_variants: (u32, u32),
    pub result_variants: (u32, u32),
    names: BTreeMap<DefId, String>,
}

fn variant_index(p: &Program, def: DefId, name: &str) -> u32 {
    match &p.def(def).kind {
        DefKind::Enum(e) => e
            .variants
            .iter()
            .position(|v| v.name == name)
            .map_or(0, |i| i as u32),
        _ => 0,
    }
}

impl PyModel {
    /// The model of a checked program: every nominal declaration's class, the descriptors of every type that an
    /// export's or a capability operation's signature uses, the capabilities, and the backend-python bindings.
    pub fn new(p: &Program, manifest: &Manifest, exports: &[DefId]) -> PyModel {
        let std = p.std_defs;
        let mut model = PyModel {
            digest: String::new(),
            classes: Vec::new(),
            types: Vec::new(),
            index: HashMap::new(),
            capabilities: Vec::new(),
            python_bindings: Vec::new(),
            unit: std.unit,
            option: std.option,
            result: std.result,
            option_variants: (
                variant_index(p, std.option, "None"),
                variant_index(p, std.option, "Some"),
            ),
            result_variants: (
                variant_index(p, std.result, "Ok"),
                variant_index(p, std.result, "Err"),
            ),
            names: BTreeMap::new(),
        };
        model.name_classes(p);
        for def in p.defs() {
            if def.id == std.unit || def.id == std.option || def.id == std.result {
                continue;
            }
            let path = p.display_path(def.id);
            let name = model.names.get(&def.id).cloned();
            let Some(name) = name else { continue };
            let kind = match &def.kind {
                DefKind::Struct(s) => PyClassKind::Struct(
                    s.fields
                        .iter()
                        .map(|f| {
                            (
                                py_member(&f.name),
                                f.name.clone(),
                                model.annotation(p, &f.ty, &s.generics),
                            )
                        })
                        .collect(),
                ),
                DefKind::Wrapper(w) => PyClassKind::Wrapper(model.annotation(p, &w.inner, &[])),
                DefKind::Enum(e) => PyClassKind::Enum(
                    e.variants
                        .iter()
                        .map(|v| {
                            (
                                // an attribute of its class shadows no builtin: only a keyword gets the `_`
                                py_member(&v.name),
                                v.name.clone(),
                                v.payload
                                    .iter()
                                    .map(|t| model.annotation(p, t, &e.generics))
                                    .collect(),
                            )
                        })
                        .collect(),
                ),
                _ => continue,
            };
            model.classes.push(PyClass {
                def: def.id,
                name,
                path,
                kind,
            });
        }
        // the descriptors: every export's signature, every capability's operations
        for &f in exports {
            if let DefKind::Fn(fd) = &p.def(f).kind {
                for param in fd.params.iter().chain(fd.using.iter()) {
                    model.intern(p, &param.ty, 0);
                }
                model.intern(p, &fd.ret, 0);
            }
        }
        for def in p.defs() {
            if let DefKind::Capability(c) = &def.kind {
                let path = p.display_path(def.id);
                let mut ops = Vec::new();
                for op in &c.ops {
                    let params = op
                        .params
                        .iter()
                        .map(|param| (param.name.clone(), model.intern(p, &param.ty, 0)))
                        .collect();
                    let ret = model.intern(p, &op.ret, 0);
                    ops.push(PyOp {
                        name: op.name.clone(),
                        py_name: py_member(&op.name),
                        qualified: format!("{path}.{}", op.name),
                        params,
                        ret,
                    });
                }
                model.capabilities.push(PyCapability {
                    def: def.id,
                    path,
                    ops,
                });
            }
        }
        for (name, binding) in &manifest.bindings {
            if binding.backend != "python" {
                continue;
            }
            if let Some(cap) = model
                .capabilities
                .iter()
                .find(|c| c.path == binding.capability)
            {
                model
                    .python_bindings
                    .push((name.clone(), binding.adapter.clone(), cap.def));
            }
        }
        // the module's own digest: of its text without the PROGRAM line (spec/python.md 6.1)
        model.digest = blake3::hash(model.module_text("").as_bytes())
            .to_hex()
            .to_string();
        model
    }

    /// Unique Python names for the nominal declarations (`spec/python.md` 3.2).
    fn name_classes(&mut self, p: &Program) {
        let std = p.std_defs;
        let nominal: Vec<DefId> = p
            .defs()
            .filter(|d| {
                matches!(
                    d.kind,
                    DefKind::Struct(_) | DefKind::Enum(_) | DefKind::Wrapper(_)
                )
            })
            .filter(|d| d.id != std.unit && d.id != std.option && d.id != std.result)
            .map(|d| d.id)
            .collect();
        let mut count: HashMap<String, usize> = HashMap::new();
        for &d in &nominal {
            *count.entry(p.def(d).name.display()).or_default() += 1;
        }
        let mut taken: BTreeSet<String> = BTreeSet::new();
        for &d in &nominal {
            let simple = p.def(d).name.display();
            let base = if count[&simple] == 1 {
                simple.clone()
            } else {
                let module = &p.module(p.def(d).module).path;
                format!("{}_{simple}", module.join("_"))
            };
            let mut name = py_name(&base);
            while !taken.insert(name.clone()) {
                name.push('_');
            }
            self.names.insert(d, name);
        }
    }

    /// The Python class name of a nominal declaration.
    pub fn class_name(&self, def: DefId) -> Option<&str> {
        self.names.get(&def).map(String::as_str)
    }

    /// A Python annotation of a CPL type (stubs and dataclass fields).
    pub fn annotation(&self, p: &Program, ty: &Ty, generics: &[String]) -> String {
        match ty {
            Ty::Int => "int".to_string(),
            Ty::Bool => "bool".to_string(),
            Ty::Text => "str".to_string(),
            Ty::Bytes => "bytes".to_string(),
            Ty::List(t) => format!("list[{}]", self.annotation(p, t, generics)),
            Ty::Adt(d, args) if *d == self.unit => "None".to_string(),
            Ty::Adt(d, args) if *d == self.option => {
                format!(
                    "Some[{}] | Nothing",
                    args.first()
                        .map_or("object".to_string(), |t| self.annotation(p, t, generics))
                )
            }
            Ty::Adt(d, args) if *d == self.result => format!(
                "Ok[{}] | Err[{}]",
                args.first()
                    .map_or("object".to_string(), |t| self.annotation(p, t, generics)),
                args.get(1)
                    .map_or("object".to_string(), |t| self.annotation(p, t, generics))
            ),
            Ty::Adt(d, _) | Ty::Wrapper(d) => self.class_name(*d).unwrap_or("object").to_string(),
            Ty::Refined(_, d) => match &p.def(*d).kind {
                DefKind::Refinement(r) => self.annotation(p, &r.base, generics),
                _ => "object".to_string(),
            },
            Ty::Param(i) => generics
                .get(*i as usize)
                .map_or("object".to_string(), |_| "object".to_string()),
            _ => "object".to_string(),
        }
    }

    /// The descriptor index of a type, interning it and every type it reaches (a recursive type refers to its own
    /// index).
    pub fn intern(&mut self, p: &Program, ty: &Ty, depth: u32) -> usize {
        if let Some(&i) = self.index.get(ty) {
            return i;
        }
        let i = self.types.len();
        self.types.push((Desc::Opaque, p.display_ty(ty)));
        self.index.insert(ty.clone(), i);
        if depth > 64 {
            return i;
        }
        let desc = match ty {
            Ty::Int => Desc::Int,
            Ty::Bool => Desc::Bool,
            Ty::Text => Desc::Text,
            Ty::Bytes => Desc::Bytes,
            Ty::List(t) => Desc::List(self.intern(p, t, depth + 1)),
            Ty::Adt(d, _) if *d == self.unit => Desc::Unit,
            Ty::Adt(d, args) if *d == self.option => match args.first() {
                Some(t) => Desc::Option(self.intern(p, t, depth + 1)),
                None => Desc::Opaque,
            },
            Ty::Adt(d, args) if *d == self.result => match (args.first(), args.get(1)) {
                (Some(t), Some(e)) => {
                    let ok = self.intern(p, t, depth + 1);
                    Desc::Result(ok, self.intern(p, e, depth + 1))
                }
                _ => Desc::Opaque,
            },
            Ty::Adt(d, args) => match &p.def(*d).kind {
                DefKind::Struct(s) => {
                    let mut fields = Vec::new();
                    for f in &s.fields {
                        let fty = cpl_eval::types::instantiate(&f.ty, args);
                        fields.push((
                            py_member(&f.name),
                            f.name.clone(),
                            self.intern(p, &fty, depth + 1),
                        ));
                    }
                    Desc::Struct(*d, fields)
                }
                DefKind::Enum(e) => {
                    let mut variants = Vec::new();
                    for v in &e.variants {
                        let mut payload = Vec::new();
                        for t in &v.payload {
                            let vty = cpl_eval::types::instantiate(t, args);
                            payload.push(self.intern(p, &vty, depth + 1));
                        }
                        variants.push(payload);
                    }
                    Desc::Enum(*d, variants)
                }
                _ => Desc::Opaque,
            },
            Ty::Wrapper(d) => match &p.def(*d).kind {
                DefKind::Wrapper(w) => Desc::Wrapper(*d, self.intern(p, &w.inner, depth + 1)),
                _ => Desc::Opaque,
            },
            Ty::Refined(_, d) => match &p.def(*d).kind {
                DefKind::Refinement(r) => Desc::Refined(self.intern(p, &r.base, depth + 1)),
                _ => Desc::Opaque,
            },
            Ty::Capability(d) => Desc::Capability(*d),
            _ => Desc::Opaque,
        };
        self.types[i].0 = desc;
        i
    }

    /// The descriptor index of a type already interned.
    pub fn index_of(&self, ty: &Ty) -> Option<usize> {
        self.index.get(ty).copied()
    }

    /// The types module (`<package>/_types.py`, and the head of a worker's source).
    pub fn types_module(&self) -> String {
        self.module_text(&format!("PROGRAM = {}\n", py_str(&self.digest)))
    }

    /// The module's text with `program_line` (the `PROGRAM` assignment, or nothing for the digest).
    fn module_text(&self, program_line: &str) -> String {
        let mut s = String::with_capacity(RUNTIME.len() + 16 * 1024);
        let _ = writeln!(
            s,
            "# Generated by cpl {}: the program's types and the node-list codec (spec/python.md Section 3). Do not\n# edit.\n",
            cpl_diag::version::TOOLCHAIN
        );
        s.push_str(RUNTIME);
        s.push('\n');
        s.push_str(program_line);
        s.push('\n');
        s.push_str("# ---- the program's classes (spec/python.md 3.2) ----\n");
        for c in &self.classes {
            s.push('\n');
            match &c.kind {
                PyClassKind::Struct(fields) => {
                    let _ = writeln!(
                        s,
                        "\n@_dc.dataclass(frozen=True, slots=True)\nclass {}:",
                        c.name
                    );
                    let _ = writeln!(s, "    {}", py_str(&c.path));
                    let _ = writeln!(s, "    _cpl_def = {}", c.def.0);
                    for (py, _, ann) in fields {
                        let _ = writeln!(s, "    {py}: {}", py_str(ann));
                    }
                }
                PyClassKind::Wrapper(ann) => {
                    let _ = writeln!(
                        s,
                        "\n@_dc.dataclass(frozen=True, slots=True)\nclass {}:",
                        c.name
                    );
                    let _ = writeln!(s, "    {}", py_str(&c.path));
                    let _ = writeln!(s, "    _cpl_def = {}", c.def.0);
                    let _ = writeln!(s, "    value: {}", py_str(ann));
                }
                PyClassKind::Enum(variants) => {
                    let _ = writeln!(s, "\nclass {}:", c.name);
                    let _ = writeln!(s, "    {}", py_str(&c.path));
                    let _ = writeln!(s, "    __slots__ = ()\n    _cpl_def = {}", c.def.0);
                    for (vi, (attr, cpl_name, payload)) in variants.iter().enumerate() {
                        let class = format!("_{}_{}", c.name, cpl_name);
                        let display = format!("{}.{}", c.name, attr);
                        let _ = writeln!(
                            s,
                            "\n\n@_dc.dataclass(frozen=True, slots=True, repr=False)\nclass {class}({}):",
                            c.name
                        );
                        let _ = writeln!(s, "    _cpl_variant = {vi}");
                        for (k, ann) in payload.iter().enumerate() {
                            let _ = writeln!(s, "    _{k}: {}", py_str(ann));
                        }
                        if payload.is_empty() {
                            let _ = writeln!(
                                s,
                                "\n    def __repr__(self):\n        return {}",
                                py_str(&display)
                            );
                        } else {
                            let fields: Vec<String> =
                                (0..payload.len()).map(|k| format!("self._{k}")).collect();
                            let _ = writeln!(
                                s,
                                "\n    def __repr__(self):\n        return {} + \"(\" + \", \".join(repr(x) for x in ({},)) + \")\"",
                                py_str(&display),
                                fields.join(", ")
                            );
                        }
                        let _ = writeln!(s, "\n\n{class}.__qualname__ = {}", py_str(&display));
                        if payload.is_empty() {
                            let _ = writeln!(s, "{}.{attr} = {class}()", c.name);
                        } else {
                            let _ = writeln!(s, "{}.{attr} = {class}", c.name);
                        }
                    }
                }
            }
        }
        s.push_str("\n\n# ---- the tables the codec reads (spec/python.md 3.3) ----\n\n");
        let _ = writeln!(s, "_UNIT = {}", self.unit.0);
        let _ = writeln!(s, "_OPTION = {}", self.option.0);
        let _ = writeln!(s, "_RESULT = {}", self.result.0);
        s.push_str("_DEFS = {\n");
        let _ = writeln!(s, "    {}: (\"unit\",),", self.unit.0);
        let _ = writeln!(
            s,
            "    {}: (\"option\", {}, {}),",
            self.option.0, self.option_variants.0, self.option_variants.1
        );
        let _ = writeln!(
            s,
            "    {}: (\"result\", {}, {}),",
            self.result.0, self.result_variants.0, self.result_variants.1
        );
        for c in &self.classes {
            match &c.kind {
                PyClassKind::Struct(_) => {
                    let _ = writeln!(s, "    {}: (\"struct\", {}),", c.def.0, c.name);
                }
                PyClassKind::Wrapper(_) => {
                    let _ = writeln!(s, "    {}: (\"wrapper\", {}),", c.def.0, c.name);
                }
                PyClassKind::Enum(variants) => {
                    let members: Vec<String> = variants
                        .iter()
                        .map(|(attr, _, _)| format!("{}.{attr}", c.name))
                        .collect();
                    let payloads: Vec<&str> = variants
                        .iter()
                        .map(|(_, _, p)| if p.is_empty() { "False" } else { "True" })
                        .collect();
                    let _ = writeln!(
                        s,
                        "    {}: (\"enum\", {}, ({},), ({},)),",
                        c.def.0,
                        c.name,
                        members.join(", "),
                        payloads.join(", ")
                    );
                }
            }
        }
        s.push_str("}\n_TYPES = [\n");
        for (i, (desc, display)) in self.types.iter().enumerate() {
            let d = py_str(display);
            let entry = match desc {
                Desc::Int => format!("(\"int\", {d})"),
                Desc::Bool => format!("(\"bool\", {d})"),
                Desc::Text => format!("(\"text\", {d})"),
                Desc::Bytes => format!("(\"bytes\", {d})"),
                Desc::Unit => format!("(\"unit\", {d})"),
                Desc::List(t) => format!("(\"list\", {d}, {t})"),
                Desc::Option(t) => format!("(\"option\", {d}, {t})"),
                Desc::Result(t, e) => format!("(\"result\", {d}, {t}, {e})"),
                Desc::Struct(def, fields) => {
                    let f: Vec<String> = fields
                        .iter()
                        .map(|(py, name, t)| format!("({}, {}, {t})", py_str(py), py_str(name)))
                        .collect();
                    format!(
                        "(\"struct\", {d}, {}, ({}{}))",
                        def.0,
                        f.join(", "),
                        if f.len() == 1 { "," } else { "" }
                    )
                }
                Desc::Enum(def, variants) => {
                    let v: Vec<String> = variants
                        .iter()
                        .map(|payload| {
                            let items: Vec<String> =
                                payload.iter().map(|t| t.to_string()).collect();
                            format!(
                                "({}{})",
                                items.join(", "),
                                if items.len() == 1 { "," } else { "" }
                            )
                        })
                        .collect();
                    format!(
                        "(\"enum\", {d}, {}, ({}{}))",
                        def.0,
                        v.join(", "),
                        if v.len() == 1 { "," } else { "" }
                    )
                }
                Desc::Wrapper(def, t) => format!("(\"wrapper\", {d}, {}, {t})", def.0),
                Desc::Refined(t) => format!("(\"refined\", {d}, {t})"),
                Desc::Capability(def) => format!("(\"cap\", {d}, {})", def.0),
                Desc::Opaque => format!("(\"opaque\", {d})"),
            };
            let _ = writeln!(s, "    {entry},  # {i}");
        }
        s.push_str("]\n_CAPS = {\n");
        for c in &self.capabilities {
            let ops: Vec<String> = c
                .ops
                .iter()
                .map(|op| {
                    format!(
                        "({}, {}, {})",
                        py_str(&op.py_name),
                        op.ret,
                        py_str(&op.qualified)
                    )
                })
                .collect();
            let _ = writeln!(
                s,
                "    {}: ({}, ({}{})),",
                c.def.0,
                py_str(&c.path),
                ops.join(", "),
                if ops.len() == 1 { "," } else { "" }
            );
        }
        s.push_str("}\n_BINDINGS = {\n");
        for (name, module, cap) in &self.python_bindings {
            let _ = writeln!(s, "    {}: ({}, {}),", py_str(name), py_str(module), cap.0);
        }
        s.push_str("}\n");
        s
    }

    /// A Python worker's source: the types module, then the worker main (`spec/python.md` 6.1).
    pub fn worker_source(&self) -> String {
        let mut s = self.types_module();
        s.push('\n');
        s.push_str(WORKER);
        s
    }
}

/// The extension's C unit (`src/<prefix>_python.c`, `spec/python.md` 2): the fixed template for the prefix.
pub fn extension_c(prefix: &str) -> String {
    include_str!("python/extension.c").replace("@PREFIX@", prefix)
}

/// The package's `__init__.py` (`spec/python.md` 3.1): the exceptions, the capability and token classes, one wrapper
/// per export, and one `bind_<binding>()` per backend-c and backend-python binding.
pub fn package_init(
    model: &PyModel,
    p: &Program,
    plan: &crate::InteropPlan,
    manifest: &Manifest,
) -> String {
    let models = crate::api::export_models(p, plan);
    let prefix = plan.prefix.as_str();
    let mut s = String::with_capacity(32 * 1024);
    let _ = writeln!(
        s,
        "\"\"\"The CPL package of the project {} (spec/python.md), generated by cpl {}. Do not edit.\"\"\"\n",
        manifest.project.name,
        cpl_diag::version::TOOLCHAIN
    );
    s.push_str("import importlib as _importlib\nimport json as _json\nimport os as _os\nimport sys as _sys\n\n");
    s.push_str("from . import _native, _types\nfrom ._types import NOTHING, Err, ForeignFailure, Nothing, Ok, Some\n");
    for c in &model.classes {
        let _ = writeln!(s, "from ._types import {}", c.name);
    }
    let _ = writeln!(
        s,
        "\n__version__ = {}\n",
        py_str(&manifest.python_version())
    );
    s.push_str(PACKAGE_FIXED);
    // the exception classes of the exported Result error types
    let errors = error_classes(model, p, plan);
    for e in errors.values() {
        let _ = writeln!(s, "\n\nclass {e}(CplResultError):\n    \"\"\"An exported function returned `Err` of this type (spec/python.md 4).\"\"\"");
    }
    s.push_str("\n\n# ---- the exports (spec/python.md 3.1) ----\n");
    for (index, m) in models.iter().enumerate() {
        let name = py_name(
            m.symbol
                .strip_prefix(&format!("{prefix}_"))
                .unwrap_or(&m.symbol),
        );
        let fd = p
            .def(m.function)
            .kind
            .as_fn()
            .expect("an export is a function");
        let mut params: Vec<String> = Vec::new();
        for prm in &fd.params {
            params.push(py_member(&prm.name));
        }
        for u in &fd.using {
            params.push(py_member(&u.name));
        }
        params.push("*".to_string());
        params.push("_time_budget_ms=0".to_string());
        params.push("_cancel=None".to_string());
        let _ = writeln!(s, "\n\ndef {name}({}):", params.join(", "));
        let mut sig: Vec<String> = fd
            .params
            .iter()
            .map(|prm| format!("{}: {}", prm.name, p.display_ty(&prm.ty)))
            .collect();
        sig.extend(
            fd.using
                .iter()
                .map(|u| format!("using {}: {}", u.name, p.display_ty(&u.ty))),
        );
        let _ = writeln!(
            s,
            "    {}",
            py_str(&format!(
                "{}({}) -> {}",
                p.display_path(m.function),
                sig.join(", "),
                p.display_ty(&fd.ret)
            ))
        );
        let args: Vec<String> = fd
            .params
            .iter()
            .map(|prm| {
                let t = model
                    .index_of(&prm.ty)
                    .expect("an export's parameter type is described");
                format!("_arg({}, {t}, {})", py_member(&prm.name), py_str(&prm.name))
            })
            .collect();
        let caps: Vec<String> = fd
            .using
            .iter()
            .map(|u| {
                let cap = match &u.ty {
                    Ty::Capability(d) => *d,
                    _ => DefId(u32::MAX),
                };
                format!(
                    "_cap({}, {}, {})",
                    py_member(&u.name),
                    cap.0,
                    py_str(&u.name)
                )
            })
            .collect();
        let err = match &m.err_ty {
            Ty::Adt(d, _) | Ty::Wrapper(d) if m.split => errors
                .get(d)
                .cloned()
                .unwrap_or_else(|| "CplResultError".to_string()),
            _ => "CplResultError".to_string(),
        };
        let _ = writeln!(
            s,
            "    return _finish(\n        _native.call({index}, ({}), ({}), _budget(_time_budget_ms), _token(_cancel)),\n        {err},\n    )",
            if args.is_empty() { String::new() } else { format!("{},", args.join(", ")) },
            if caps.is_empty() { String::new() } else { format!("{},", caps.join(", ")) },
        );
    }
    s.push_str("\n\n# ---- the bindings (spec/python.md 5.3) ----\n");
    for (gi, b) in plan.bindings.iter().enumerate() {
        let _ = writeln!(
            s,
            "\n\ndef bind_{}():\n    {}\n    return _grant({gi}, {})",
            b.name,
            py_str(&format!(
                "[bindings.{}]: backend c, adapter {}, {}.",
                b.name,
                b.adapter,
                if b.worker {
                    "in the C adapter host"
                } else {
                    "in process"
                }
            )),
            py_str(&b.name)
        );
    }
    for (i, b) in plan.python_bindings.iter().enumerate() {
        let gi = plan.bindings.len() + i;
        if b.worker {
            let _ = writeln!(
                s,
                "\n\ndef bind_{}():\n    {}\n    _workers()\n    return _grant({gi}, {})",
                b.name,
                py_str(&format!(
                    "[bindings.{}]: backend python, adapter module {}, in a worker.",
                    b.name, b.module
                )),
                py_str(&b.name)
            );
        } else {
            let _ = writeln!(
                s,
                "\n\ndef bind_{}():\n    {}\n    return _in_process({}, {}, {})",
                b.name,
                py_str(&format!(
                    "[bindings.{}]: backend python, adapter module {}, in process.",
                    b.name, b.module
                )),
                py_str(&b.name),
                py_str(&b.module),
                b.capability.0
            );
        }
    }
    s
}

/// The fixed part of `__init__.py`: the exceptions, `Capability`, `CancelToken`, and the helpers the generated
/// wrappers call.
const PACKAGE_FIXED: &str = r#"

class CplError(Exception):
    """The base of every exception of the package (spec/python.md 4); `diagnostic` is the S49 dict, or None."""

    def __init__(self, message, diagnostic=None):
        super().__init__(message)
        self.diagnostic = diagnostic


class CplTypeError(CplError, TypeError):
    """An argument's Python type does not fit its CPL type; `pointer` is its RFC 6901 position."""

    def __init__(self, message, pointer, diagnostic=None):
        super().__init__(message, diagnostic)
        self.pointer = pointer


class CplValueError(CplError, ValueError):
    """An argument has a fitting type but no CPL value; `pointer` is its RFC 6901 position."""

    def __init__(self, message, pointer, diagnostic=None):
        super().__init__(message, diagnostic)
        self.pointer = pointer


class CplFault(CplError):
    """The call ended in an execution fault: `code`, `message`, and `diagnostic`, the S49 object `cpl run` reports
    (None for a cancelled call, whose `code` is "cancelled")."""

    def __init__(self, diagnostic, code=None, message=None):
        if diagnostic is not None:
            code = diagnostic.get("code")
            message = diagnostic.get("message", "")
        super().__init__(message, diagnostic)
        self.code = code
        self.message = message


class CplArgumentError(CplFault, ValueError):
    """An argument's refined position does not hold (semantics S81: refinement_violation with a parameter)."""


class CplResultError(CplError):
    """An exported function returned `Err(error)`."""

    def __init__(self, error):
        super().__init__(repr(error))
        self.error = error


class CplUsageError(CplError):
    """The package was used wrongly, or the C API reported a usage error: `code` and `diagnostic`."""

    def __init__(self, diagnostic):
        super().__init__(diagnostic.get("message", ""), diagnostic)
        self.code = diagnostic.get("code")


def _usage(r):
    # an interop_* error naming an argument is CplValueError (out of range, invalid UTF-8) or CplTypeError (spec/python.md 4)
    d = _json.loads(r[1])
    code = d.get("code", "")
    pointer = d.get("pointer", "/" + d["parameter"] if isinstance(d.get("parameter"), str) else "")
    if code in ("interop_int_out_of_range", "interop_utf8_invalid"):
        return CplValueError(d.get("message", ""), pointer, d)
    if "parameter" in d:
        return CplTypeError(d.get("message", ""), pointer, d)
    return CplUsageError(d)


def _finish(r, error_class):
    tag = r[0]
    if tag == "ok":
        return _types._decode_text(r[1])
    if tag == "err":
        raise error_class(_types._decode_text(r[1]))
    if tag == "cancelled":
        raise CplFault(None, "cancelled", "the call was cancelled")
    if tag == "fault":
        d = _json.loads(r[1])
        if d.get("code") == "refinement_violation" and "parameter" in d:
            raise CplArgumentError(d)
        raise CplFault(d)
    raise _usage(r)


def _arg(x, t, name):
    try:
        return _types._nodes_text(_types.encode(x, t, "/" + name))
    except _types._ConvError as e:
        at = "" if e.pointer == "/" + name else " at " + e.pointer
        message = "argument %s is not a value of %s%s: found %s" % (name, e.expected, at, e.found)
        if e.value_error:
            if e.reason:
                message += " (" + e.reason + ")"
            raise CplValueError(message, e.pointer) from None
        raise CplTypeError(message, e.pointer) from None


def _cap(x, cap, name):
    text, ops = _types._CAPS[cap]
    if isinstance(x, Capability):
        return x._for_call(cap, text, len(ops), name)
    missing = [op[0] for op in ops if not callable(getattr(x, op[0], None))]
    if missing:
        raise CplTypeError(
            "argument %s is not a capability %s: it has no operation %s" % (name, text, missing[0]), "/" + name
        )
    return (x, text, len(ops), cap, None)


def _budget(ms):
    if isinstance(ms, bool) or not isinstance(ms, int) or ms < 0:
        raise CplTypeError("_time_budget_ms is a non-negative int", "/_time_budget_ms")
    return ms


def _token(cancel):
    if cancel is None:
        return None
    if not isinstance(cancel, CancelToken):
        raise CplTypeError("_cancel is a CancelToken", "/_cancel")
    return cancel._capsule


class CancelToken:
    """A cancellation token (spec/python.md 4.1): pass it as `_cancel=`; `cancel()` may be called from any thread."""

    def __init__(self):
        self._capsule = _native.cancel_new()

    def cancel(self):
        _native.cancel_request(self._capsule)


class Capability:
    """A granted binding (spec/python.md 5.3). `close()` releases it; using it afterwards raises CplUsageError."""

    def __init__(self, name, capsule=None, adapter=None, capability=None):
        self.binding = name
        self._capsule = capsule
        self._adapter = adapter
        self._capability = capability
        self._closed = False

    def _for_call(self, cap, text, nops, name):
        if self._closed:
            raise CplUsageError(
                {
                    "code": "interop_handle_released",
                    "message": "argument %s: the capability of the binding `%s` was closed" % (name, self.binding),
                }
            )
        if self._capsule is not None:
            return self._capsule
        if self._capability != cap:
            raise CplTypeError(
                "argument %s is not a capability %s: the binding `%s` grants another" % (name, text, self.binding),
                "/" + name,
            )
        return (self._adapter, text, nops, cap, self.binding)

    def close(self):
        if self._closed:
            return
        self._closed = True
        if self._capsule is not None:
            _native.release(self._capsule)
            self._capsule = None
        elif self._adapter is not None:
            close = getattr(self._adapter, "close", None)
            self._adapter = None
            if close is not None:
                close()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()
        return False

    def __del__(self):
        # a forgotten in-process grant: its adapter's close() runs when the object is collected (spec/python.md 5.3);
        # a granted capsule is released by its own destructor
        if self._adapter is not None and not self._closed:
            try:
                self.close()
            except Exception:
                pass

    def __repr__(self):
        return "<Capability %s%s>" % (self.binding, " closed" if self._closed else "")


def _workers():
    path = _os.pathsep.join(p for p in _sys.path if isinstance(p, str))
    _native.workers(_sys.executable or "python3", ("PYTHONPATH=" + path,))


def _grant(index, name):
    r = _native.grant(index)
    if r[0] != "ok":
        raise _usage(r)
    return Capability(name, capsule=r[1])


def _in_process(name, module, cap):
    try:
        adapter = _importlib.import_module(module).create(_types, name)
    except Exception as e:
        raise CplUsageError(
            {
                "code": "interop_bind_failed",
                "message": "the binding `%s` could not be granted" % name,
                "binding": name,
                "notes": [{"message": "%s: %s" % (type(e).__qualname__, e)}],
            }
        ) from e
    return Capability(name, adapter=adapter, capability=cap)


def configure(*, max_call_depth=10000, max_heap_bytes=0):
    """Reopens the package's runtime with these limits (spec/python.md 2); refused while values or calls are live."""
    r = _native.configure(max_call_depth, max_heap_bytes)
    if r[0] != "ok":
        raise _usage(r)


def library_info():
    """The package's library information (spec/cabi.md 4.2) as a dict."""
    return _native.info()


_native.set_dispatch(_types._dispatch)
"#;

/// The stub annotation of a CPL type in an argument position (`spec/python.md` 3.6): `List<T>` as `Sequence[T]`,
/// `Bytes` as any bytes-like object, a capability as a `Capability` or an object with its operations.
fn arg_annotation(
    model: &PyModel,
    p: &Program,
    ty: &Ty,
    protocols: &BTreeMap<DefId, String>,
) -> String {
    match ty {
        Ty::Bytes => "bytes | bytearray | memoryview".to_string(),
        Ty::List(t) => format!("Sequence[{}]", arg_annotation(model, p, t, protocols)),
        Ty::Adt(d, args) if *d == model.option => format!(
            "Some[{}] | Nothing",
            args.first()
                .map_or("object".to_string(), |t| arg_annotation(
                    model, p, t, protocols
                ))
        ),
        Ty::Adt(d, args) if *d == model.result => format!(
            "Ok[{}] | Err[{}]",
            args.first()
                .map_or("object".to_string(), |t| arg_annotation(
                    model, p, t, protocols
                )),
            args.get(1).map_or("object".to_string(), |t| arg_annotation(
                model, p, t, protocols
            ))
        ),
        Ty::Refined(_, d) => match &p.def(*d).kind {
            DefKind::Refinement(r) => arg_annotation(model, p, &r.base, protocols),
            _ => "object".to_string(),
        },
        Ty::Capability(d) => match protocols.get(d) {
            Some(proto) => format!("Capability | {proto}"),
            None => "Capability".to_string(),
        },
        _ => model.annotation(p, ty, &[]),
    }
}

/// The exception class names of the exported `Result` error types (`spec/python.md` 4): `<Class>Exception`, with a
/// trailing `_` while the name is taken by a class of the program.
fn error_classes(
    model: &PyModel,
    p: &Program,
    plan: &crate::InteropPlan,
) -> BTreeMap<DefId, String> {
    let taken: BTreeSet<&str> = model.classes.iter().map(|c| c.name.as_str()).collect();
    let mut out = BTreeMap::new();
    for m in crate::api::export_models(p, plan) {
        if !m.split {
            continue;
        }
        if let Ty::Adt(d, _) | Ty::Wrapper(d) = &m.err_ty {
            if let Some(c) = model.class_name(*d) {
                let mut name = format!("{c}Exception");
                while taken.contains(name.as_str()) {
                    name.push('_');
                }
                out.insert(*d, name);
            }
        }
    }
    out
}

/// The stub-only `Protocol` name of each capability (`_<Name>Protocol`, the module path joined in when two
/// capabilities share a name).
fn protocol_names(model: &PyModel, p: &Program) -> BTreeMap<DefId, String> {
    let mut count: HashMap<String, usize> = HashMap::new();
    for c in &model.capabilities {
        *count.entry(p.def(c.def).name.display()).or_default() += 1;
    }
    model
        .capabilities
        .iter()
        .map(|c| {
            let simple = p.def(c.def).name.display();
            let base = if count[&simple] == 1 {
                simple
            } else {
                c.path.replace('.', "_")
            };
            (c.def, format!("_{base}Protocol"))
        })
        .collect()
}

/// The package's `__init__.pyi` (`spec/python.md` 3.6): every public name with its type, for `mypy --strict`.
pub fn package_stub(
    model: &PyModel,
    p: &Program,
    plan: &crate::InteropPlan,
    manifest: &Manifest,
) -> String {
    let models = crate::api::export_models(p, plan);
    let prefix = plan.prefix.as_str();
    let protocols = protocol_names(model, p);
    let errors = error_classes(model, p, plan);
    let mut s = String::with_capacity(16 * 1024);
    let _ = writeln!(
        s,
        "# The type stubs of the CPL package of the project {} (spec/python.md 3.6), generated by cpl {}. Do not edit.\n",
        manifest.project.name,
        cpl_diag::version::TOOLCHAIN
    );
    s.push_str(STUB_FIXED);
    s.push_str("\n# ---- the program's classes (spec/python.md 3.2) ----\n");
    for c in &model.classes {
        match &c.kind {
            PyClassKind::Struct(fields) => {
                let _ = writeln!(s, "\n@dataclass(frozen=True)\nclass {}:", c.name);
                if fields.is_empty() {
                    s.push_str("    pass\n");
                }
                for (py, _, ann) in fields {
                    let _ = writeln!(s, "    {py}: {ann}");
                }
            }
            PyClassKind::Wrapper(ann) => {
                let _ = writeln!(
                    s,
                    "\n@dataclass(frozen=True)\nclass {}:\n    value: {ann}",
                    c.name
                );
            }
            PyClassKind::Enum(variants) => {
                let _ = writeln!(s, "\nclass {}:", c.name);
                if variants.is_empty() {
                    s.push_str("    pass\n");
                }
                for (attr, cpl_name, payload) in variants {
                    let class = format!("_{}_{}", c.name, cpl_name);
                    if payload.is_empty() {
                        let _ = writeln!(s, "    {attr}: ClassVar[{class}]");
                    } else {
                        let _ = writeln!(s, "    {attr} = {class}");
                    }
                }
                for (_, cpl_name, payload) in variants {
                    let class = format!("_{}_{}", c.name, cpl_name);
                    let _ = writeln!(
                        s,
                        "\n@final\n@dataclass(frozen=True)\nclass {class}({}):",
                        c.name
                    );
                    if payload.is_empty() {
                        s.push_str("    pass\n");
                    }
                    for (k, ann) in payload.iter().enumerate() {
                        let _ = writeln!(s, "    _{k}: {ann}");
                    }
                }
            }
        }
    }
    s.push_str("\n# ---- the capabilities' operations, for objects passed as capabilities (spec/python.md 5.1) ----\n");
    for c in &model.capabilities {
        let _ = writeln!(s, "\nclass {}(Protocol):", protocols[&c.def]);
        let ops = match &p.def(c.def).kind {
            DefKind::Capability(cd) => &cd.ops,
            _ => continue,
        };
        if ops.is_empty() {
            s.push_str("    pass\n");
        }
        for op in ops {
            let mut params: Vec<String> = vec!["self".to_string()];
            for prm in &op.params {
                params.push(format!(
                    "{}: {}",
                    py_member(&prm.name),
                    model.annotation(p, &prm.ty, &[])
                ));
            }
            if !op.params.is_empty() {
                params.push("/".to_string());
            }
            let _ = writeln!(
                s,
                "    def {}({}) -> {}: ...",
                py_member(&op.name),
                params.join(", "),
                arg_annotation(model, p, &op.ret, &protocols)
            );
        }
    }
    if !errors.is_empty() {
        s.push_str(
            "\n# ---- the exceptions of the exported Result error types (spec/python.md 4) ----\n",
        );
    }
    for (d, name) in &errors {
        let _ = writeln!(
            s,
            "\nclass {name}(CplResultError):\n    error: {}\n    def __init__(self, error: {}) -> None: ...",
            model.class_name(*d).unwrap_or("object"),
            model.class_name(*d).unwrap_or("object")
        );
    }
    s.push_str("\n# ---- the exports (spec/python.md 3.1) ----\n");
    for m in &models {
        let name = py_name(
            m.symbol
                .strip_prefix(&format!("{prefix}_"))
                .unwrap_or(&m.symbol),
        );
        let Some(fd) = p.def(m.function).kind.as_fn() else {
            continue;
        };
        let mut params: Vec<String> = Vec::new();
        for prm in fd.params.iter().chain(fd.using.iter()) {
            params.push(format!(
                "{}: {}",
                py_member(&prm.name),
                arg_annotation(model, p, &prm.ty, &protocols)
            ));
        }
        params.push("*".to_string());
        params.push("_time_budget_ms: int = ...".to_string());
        params.push("_cancel: CancelToken | None = ...".to_string());
        let ret = if m.split {
            model.annotation(p, &m.ok_ty, &[])
        } else {
            model.annotation(p, &fd.ret, &[])
        };
        let _ = writeln!(s, "\ndef {name}({}) -> {ret}: ...", params.join(", "));
    }
    s.push_str("\n# ---- the bindings (spec/python.md 5.3) ----\n");
    for b in &plan.bindings {
        let _ = writeln!(s, "\ndef bind_{}() -> Capability: ...", b.name);
    }
    for b in &plan.python_bindings {
        let _ = writeln!(s, "\ndef bind_{}() -> Capability: ...", b.name);
    }
    s
}

/// The fixed part of `__init__.pyi`.
const STUB_FIXED: &str = r#"from dataclasses import dataclass
from typing import Any, ClassVar, Final, Generic, Iterable, Protocol, Sequence, TypeVar, final

__version__: str

_T_co = TypeVar("_T_co", covariant=True)
_E_co = TypeVar("_E_co", covariant=True)


@final
class Nothing:
    def __eq__(self, other: object) -> bool: ...
    def __hash__(self) -> int: ...


NOTHING: Final[Nothing]


@final
@dataclass(frozen=True)
class Some(Generic[_T_co]):
    value: _T_co


@final
@dataclass(frozen=True)
class Ok(Generic[_T_co]):
    value: _T_co


@final
@dataclass(frozen=True)
class Err(Generic[_E_co]):
    error: _E_co


class ForeignFailure(Exception):
    cpl_foreign_kind: str
    cpl_foreign_details: tuple[tuple[str, str], ...]
    def __init__(self, kind: str, message: str, details: Iterable[tuple[str, str]] = ...) -> None: ...


class CplError(Exception):
    diagnostic: dict[str, Any] | None
    def __init__(self, message: str, diagnostic: dict[str, Any] | None = ...) -> None: ...


class CplTypeError(CplError, TypeError):
    pointer: str
    def __init__(self, message: str, pointer: str, diagnostic: dict[str, Any] | None = ...) -> None: ...


class CplValueError(CplError, ValueError):
    pointer: str
    def __init__(self, message: str, pointer: str, diagnostic: dict[str, Any] | None = ...) -> None: ...


class CplFault(CplError):
    code: str | None
    message: str | None
    def __init__(
        self, diagnostic: dict[str, Any] | None, code: str | None = ..., message: str | None = ...
    ) -> None: ...


class CplArgumentError(CplFault, ValueError): ...


class CplResultError(CplError):
    error: object
    def __init__(self, error: object) -> None: ...


class CplUsageError(CplError):
    code: str | None
    def __init__(self, diagnostic: dict[str, Any]) -> None: ...


class CancelToken:
    def __init__(self) -> None: ...
    def cancel(self) -> None: ...


class Capability:
    binding: str
    def close(self) -> None: ...
    def __enter__(self) -> Capability: ...
    def __exit__(self, *exc: object) -> None: ...


def configure(*, max_call_depth: int = ..., max_heap_bytes: int = ...) -> None: ...
def library_info() -> dict[str, int | str]: ...
"#;
