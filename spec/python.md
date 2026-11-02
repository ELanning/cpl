# CPL Python package, Python adapters, and the worker transport, edition 0.1 (Milestone 3)

This document specifies Milestone 3 of `spec/toolchain.md` Section 13: the
Python package that `cpl build --target python` generates (a CPython
limited-API extension in C over the package's C API, a pure-Python wrapper,
and type stubs), Python adapters for capabilities, the worker transport that
runs an adapter in a process of its own (with which `isolation = "worker"`
becomes legal for C bindings too), and the test fixtures through which `cpl
test` gives tests capability handles. It builds on `spec/cabi.md`
(Milestone 2) and comes after it in the precedence order of
`spec/README.md`: where this document and an earlier one disagree, the
earlier one wins unless this document amends it explicitly (the amendments
are listed in Section 0.2). Decisions are numbered `P1`..`P11`; their
stable ids are D-328 to D-338 (`spec/decisions.md`), followed by those of
the amendments it makes elsewhere: `grammar.md` D74 (D-339),
`semantics.md` S82 and S83 (D-340, D-341), and `stdlib.md` S48 (D-342).

Design references: the interoperability table (design lines 435-439), the
Python section (lines 443-451), foreign boundary semantics and value
mappings (lines 473-508), the adapter example and its manifest binding
(lines 510-559), the export example's Python usage (lines 561-594), and
the Milestone 3 acceptance evidence (line 646: "Python imports CPL; CPL
calls a Python adapter; conversion, exception, and worker failure cases
pass").

## 0. Scope

### 0.1 Terms

| Term | Meaning |
| - | - |
| *package* | the importable Python package that `cpl build --target python` writes (`reader_cpl`), Section 1 |
| *extension* | the package's native module `_native`, a CPython limited-API extension compiled from generated C (Section 2) |
| *types module* | the package's pure-Python module `_types`, the classes and codecs generated from the program's declarations (Section 3) |
| *host* | the process that grants a binding: a Python process using the package, a C host of `spec/cabi.md`, a native executable, or the toolchain (`cpl run`, `cpl test`, the native engine's driver) |
| *Python adapter* | a Python module named by a `backend = "python"` binding (Section 5) |
| *worker* | a child process that hosts one granted adapter and speaks cpl-worker protocol 1 with its host (Section 6) |
| *worker client* | the side of the host that starts a worker and talks to it: the C runtime's (`cpl_rt_worker.c`) in native hosts, and `cpl_cabi::worker` in the evaluator |
| *fixture* | a binding that `cpl test` grants to a test or property for its `using` parameters (Section 8) |

### 0.2 Amendments to earlier documents

| Document | Amendment |
| - | - |
| `toolchain.md` T68 | the project owner confirmed the "one native core" reading on 2026-10-30 |
| `toolchain.md` 7.2 | `cpl build --target python` (Section 1); `--python` |
| `toolchain.md` 11.1 | `[interop.python]`; `[interop.c]` legal with the `python` or `node` target; `backend = "python"` active; `isolation = "worker"` legal for backend `c`; `timeout_ms`; `[test.fixtures]` (Sections 1, 5, 6, 8) |
| `toolchain.md` 11.2 | `targets.python` and the Python adapter digests (Section 9) |
| `toolchain.md` 8.2 | `fixtures`, `assumptions`, and `environment` of test receipts are populated (Section 8) |
| `grammar.md` | `test_decl` and `property_decl` take `using` parameters and an `effects` clause (D74) |
| `semantics.md` S30 | tests and properties may declare `using` parameters, bound to fixtures (S83) |
| `semantics.md` S78, S80 | `cpl run` binds `backend = "python"` bindings; a worker is an adapter host; a malformed worker answer is `binding_protocol` (S82) |
| `stdlib.md` S45 | the situations of Python adapters and workers (S48) |
| `cabi.md` 3.8, 6.1 | the driver's `workers` request and `run_body` with handles; C bindings with `isolation = "worker"` (Section 6.6) |

## 1. `cpl build --target python`

`cpl build --target python` requires `"python"` in `[interop] targets`
(else `build_target_not_enabled`, as for `c`) and builds, in the steps of
`spec/cabi.md` 1.6 up to the link: the program is emitted and compiled
exactly as for the C target (the same export plan, prefix, ABI major,
profile, sanitizers, and release policy), and instead of linking a shared
library, the build links the extension (Section 2) from those objects and
writes the package beside it.

| Input | Default | Rule |
| - | - | - |
| `[interop.python] package` | `<project name>_cpl` | the import name: `^[a-z][a-z0-9_]*$`, not a Python keyword or soft keyword, not a module of the Python standard library (`sys.stdlib_module_names` of 3.10, kept as a table in `cpl-project`); else `project_python_option_invalid` (`key` `"package"`) |
| `[interop.python] version` | `"0.1.0"` | the distribution version: `^[0-9]+(\.[0-9]+)*$` (a PEP 440 release segment); else `project_python_option_invalid` |
| `[interop.python] paths` | `["."]` | project-relative directories put first on the Python path of the workers that the toolchain starts (Section 6.5), and the search path for the adapter digests of Section 9; each must exist (`project_binding_path_missing`, `key` `"interop.python.paths"`) |
| `--python <PATH>`, else `CPL_PYTHON`, else `python3` on `PATH` | | the interpreter whose `sysconfig` gives the headers and whose version is recorded; it must be CPython 3.10 or later with `Python.h` (else `build_python_unavailable`, E, with `interpreter` and `reason`) |

The output root (`--out`, default `<project>/.cpl/build/python`):

| Path | Contents |
| - | - |
| `<package>/__init__.py` | the public API (Section 3.1) |
| `<package>/__init__.pyi` | its type stubs (Section 3.6) |
| `<package>/py.typed` | the PEP 561 marker (empty) |
| `<package>/_types.py` | the types module (Section 3.2) |
| `<package>/_native.abi3.so` | the extension (Section 2) |
| `<package>/<prefix>-worker` | the C adapter host of the package's backend-`c` bindings with `isolation = "worker"` (Section 6.6), present only when one exists |
| `dist/<package>-<version>-cp310-abi3-linux_<arch>.whl` | a wheel of the package directory (Section 1.1) |
| `src/`, `include/`, `obj/` | the C build of `spec/cabi.md` 1.6, plus `src/<prefix>_python.c` |
| `build.json` | toolchain 7.2's `BuildOutput` with `target` `"python"` and a `python` object: `interpreter`, `version`, `package`, `distribution_version`, `tag` (`cp310-abi3-linux_<arch>`) |

### 1.1 The wheel

A wheel (PEP 427) is a zip archive holding the package directory and
`<package>-<version>.dist-info/` with `METADATA` (`Metadata-Version: 2.1`,
`Name`, `Version`, `Summary` from `[project] description` when present,
`Requires-Python: >=3.10`), `WHEEL` (`Wheel-Version: 1.0`, `Generator: cpl
<version>`, `Root-Is-Purelib: false`, `Tag: cp310-abi3-linux_<arch>`), and
`RECORD` (one `path,sha256=<urlsafe base64 without padding>,<size>` line per
file, sorted by path, and `RECORD,,` last). The toolchain writes it itself:
entries are stored without compression, in path order, every timestamp is
1980-01-01 00:00:00, and the permissions are 0644 (0755 for the worker
executable), so the wheel is byte-identical for one compiler and flag set
(toolchain 4513's determinism row). The tag is `linux_<arch>`, not a
`manylinux` tag: the build does not audit the C library versions it links.

Decision: [D-328] P1. `cpl build --target python` emits and compiles the
program as the C target does and links, instead of a shared library, one
CPython limited-API extension that statically contains the whole native
core, beside a generated pure-Python package and its stubs, and writes a
deterministic wheel of them. Rationale: the design asks for a "generated
CPython extension and Python package" (line 437) wrapping "the same native
CPL core" (line 38, T68); one self-contained extension per package keeps
the runtime copy private (two CPL packages coexist, `spec/cabi.md` 1.6),
needs no shared library at run time, and the limited API makes one wheel
serve every CPython from 3.10 on. Rejected alternatives: a wrapper over the
C package's shared library through `ctypes` (no type safety at the
boundary, and the library must be found at run time); an extension per
CPython minor version (a build per interpreter for no benefit, since the
limited API covers every function the wrapper uses); requiring setuptools
or pip to build (an ecosystem tool in the build path that the lock cannot
pin).

## 2. The extension

The extension is `src/<prefix>_python.c`, generated, compiled with the
flags of the program's units plus `-DPy_LIMITED_API=0x030A0000` and the
interpreter's include directory, and linked with the program's objects
(no executable `main`) into `<package>/_native.abi3.so` with `-shared`,
`-Wl,-z,now`, `-Wl,--gc-sections`, `-Wl,-Bsymbolic`, and a version script
that exports `PyInit__native` alone; it does not link `libpython`, whose
symbols the interpreter provides at load time (so `-Wl,-z,defs` is left
out). It uses only the limited API of CPython 3.10 and the package's own C
API (`spec/cabi.md` Sections 4-5), plus one hidden function of the API
unit, `<prefix>__python_workers` (Section 6.5).

**Module state.** At import the extension opens one runtime with
`CPL_RUNTIME_OPTIONS_INIT`; `configure()` (Section 3.1) closes it and opens
another with the given options, refusing while values or calls are live
(`interop_runtime_in_use`, `interop_runtime_busy`, raised as
`CplUsageError`). The runtime is closed with `CPL_CLOSE_FORCE` when the
module is freed.

**Calls.** The pure-Python wrapper of an export calls `_native.call(index,
arguments, capabilities, time_budget_ms, cancel)`: `arguments` holds one
node-list text (`{"nodes": [...]}`, `spec/cabi.md` 3.8) per value parameter,
produced by the types module (Section 3.3); the extension builds each value
with the runtime's node-list reader (the API unit's hidden
`<prefix>__value_from_nodes`), passes spans and machine forms where the
export's form requires them (`spec/cabi.md` 5.4), releases the GIL
(`PyEval_SaveThread`), calls `<prefix>_<export>`, takes the GIL back, and
returns one of `("ok", nodes)`, `("err", nodes)` (the node-list text of the
`Ok` value or the `Err` payload, written by the hidden
`<prefix>__value_to_nodes`), `("fault", diagnostic)`, `("usage",
diagnostic)` (the S49 JSON text of the error object), or `("cancelled",
None)`; the wrapper turns all but the first into the exceptions of
Section 4. The export's own entry check (semantics S81) decides whether an
argument is a value of its parameter's type, so a value the types module
let through is still checked. Every handle the extension made is released
before it returns. Capability arguments are Section 5's.

Decision: [D-329] P2. Values cross between Python and the native core as
the node lists of cpl-driver protocol 1, encoded and decoded iteratively in
generated Python and read and written in C by the runtime's node-list
codec, the export's entry check deciding what enters CPL; and the GIL is
released for the whole native call. Rationale: node lists are flat, so
neither side recurses on a value's depth (Python's `json` module and its
recursion limit never see a deep value), they carry an `Int` as decimal
text, which Python converts in chunks however long it is (the interpreter
refuses `str` of an `int` above 4300 digits), and they are the codec the
workers of Section 6 already speak, so the package and the worker share one
generated codec; the extension stays a thin layer whose only Python objects
are texts and tuples (the place where reference-counting errors would
otherwise concentrate); releasing the GIL lets other Python threads run
while CPL evaluates, which the thread-safe runtime allows (`spec/cabi.md`
C21). Rejected alternatives: value JSON (recursive `json.loads` and
`json.dumps` limit the depth of a value, and the decimal of a long `Int`
meets the interpreter's digit limit); converting Python objects to handles
in C (a second decoder with its own reference counting); holding the GIL
during calls (one CPL call would stall every Python thread).

## 3. The package

### 3.1 Public API

`<package>/__init__.py` exports, by name:

| Name | Meaning |
| - | - |
| one function per export | named by the export's C symbol without `<prefix>_` (`paginate_ints`); positional value parameters in declaration order, then the capability parameters, then the keyword-only `_time_budget_ms` and `_cancel` (Section 4.1) |
| one class per nominal type | Section 3.2 |
| `Some`, `NOTHING`, `Ok`, `Err` | the tags of `Option` and `Result` |
| `bind_<binding>()` | one per binding of the project whose backend is `c` or `python` (Section 5.3) |
| `configure(*, max_call_depth=10000, max_heap_bytes=0)` | reopens the runtime (Section 2) |
| `library_info()` | a `dict` of `<prefix>_library_info()`'s members |
| `Capability`, `CancelToken` | the classes of a granted capability (Section 5) and of a cancellation token |
| `CplError` and its subclasses | Section 4 |
| `__version__` | `[interop.python] version` |

A name that is a Python keyword or soft keyword gets a trailing `_`
(`pass_`); the stubs show the result.

### 3.2 Values

| CPL type | Python, in arguments | Python, in results |
| - | - | - |
| `Int` | `int` (a `bool` is refused) | `int` |
| `Bool` | `bool` | `bool` |
| `Text` | `str` without unpaired surrogates | `str` |
| `Bytes` | `bytes`, `bytearray`, or `memoryview` | `bytes` |
| `Unit` | `None` | `None` |
| `List<T>` | `list` or `tuple` | `list` |
| a struct | an instance of its class | an instance of its class |
| a nominal wrapper | an instance of its class (one field, `value`) | an instance of its class |
| an enum (`std.foreign.ForeignErrorKind` included) | an instance of one of its variant classes | the same |
| `Option<T>` | `Some(x)` or `NOTHING` | `Some(x)` or `NOTHING` |
| `Result<T, E>` | `Ok(x)` or `Err(e)` | `Ok(x)` or `Err(e)`, except an export's own return (Section 4) |
| a refinement | its base type; the predicate is checked by the export (`spec/semantics.md` S81) | its base type |
| a capability | Section 5 | never a result |

The types module defines, for every nominal declaration of the program
(project and standard library, generic ones included):

- a **struct** as a frozen dataclass with slots, its fields in declaration
  order and named as declared (a keyword gets a trailing `_`);
- a **wrapper** as a frozen dataclass with one field `value`;
- an **enum** as a base class with one frozen dataclass per variant,
  reachable as an attribute of the base (`StoreError.Missing`), its payload
  fields named `_0`, `_1`, ... (so `match` and `__match_args__` work), and
  for a variant without payload one instance, the attribute itself
  (`StoreError.Unavailable`, `ForeignErrorKind.Timeout`);
- `Option` and `Result` as the tag classes `Some` (field `value`), the one
  instance `NOTHING`, `Ok` (`value`), and `Err` (`error`), so that nested
  options stay distinguishable (design line 504) and no Python `None`
  stands for a CPL value other than `Unit`.

A class is named by its declaration's name when no other nominal
declaration of the program has the same name, else by its module path and
name joined with `_` (`reader_pages_Page`); the standard library's
`Option`, `Result`, and `Unit` have no class. Instances compare by value
(`==`) and hash; `repr` shows the class and fields.

Decision: [D-330] P3. CPL values map to Python values as the design's table
gives (line 495), with generated frozen dataclasses for structs, wrappers,
and enum variants, explicit tags for `Option` and `Result`, `None` for
`Unit` alone, and strict acceptance of arguments (a `bool` is not an `Int`,
a `str` is not `Bytes`, an unpaired surrogate is refused). Rationale:
immutable CPL values become immutable Python values that compare by value;
explicit tags keep `Some(NOTHING)` distinct from `NOTHING` (line 504);
refusing `bool` for `Int` and unpaired surrogates (line 506) is the
design's "validate dynamic inputs" (line 481). Rejected alternatives:
`None` for `NOTHING` (nested options collapse); `dict`s for structs (no
type to check and no attribute access); accepting `bool` as `Int` (`True`
would silently become `1`).

### 3.3 The codec

The types module holds one codec, the node lists of `spec/cabi.md` 3.8,
generated from type descriptors (one per type that a parameter, a result,
or a capability operation of the program uses, fully instantiated):
`encode(x, T)` is type-directed and raises `CplTypeError` or
`CplValueError` (Section 4) with the RFC 6901 pointer of the offending part
of the argument (`/items/3`, the parameter's name first); `decode(nodes)`
is directed by the node tags and the program's `DefId`s, which the module
maps to its classes. Both walk the value with an explicit stack, so a value
of any depth converts; an `Int` crosses as its decimal, converted in
chunks of 1000 digits, so the interpreter's limit on `int` and `str`
conversions (`sys.get_int_max_str_digits`) never applies.

### 3.4 Strictness and pointers

An argument's check walks the value in value order and stops at the first
part that is not a value of its declared type. The exception's
`pointer` is that part's RFC 6901 position, rooted at the parameter's
name (`/items/2`, `/page/text`), and its message is `argument <name> is
not a value of <expected_type> at <pointer>: found <python type name>`
(`<expected_type>` rendered as `spec/semantics.md` 3.3 renders types, with
` at <pointer>` left out at the parameter itself).

### 3.5 Packaging conventions

The package's modules import only the Python standard library and the
extension. `__init__.py` re-exports the types module's classes and tags,
so `from reader_cpl import Page, Some` works.

### 3.6 Stubs

`__init__.pyi` declares every name of Section 3.1 with types: the classes
with their fields (`id: PageId`), enums as the union of their variant
classes, `Option[T]` and `Result[T, E]` as aliases of `Some[T] | Nothing`
and `Ok[T] | Err[E]` (generic classes), `List<T>` as `list[T]` in
results and `Sequence[T]` in arguments, a capability parameter as a
`Protocol` with one method per operation, and each function's return type
(the `Ok` payload of an export returning `Result`). It passes `mypy
--strict` with no error, which the pytest suite checks when `mypy` is
installed.

## 4. Errors and exceptions

| Exception | Bases | Raised when |
| - | - | - |
| `CplError` | `Exception` | the base of everything below; carries `diagnostic` (the S49 `dict`, or `None`) |
| `CplTypeError` | `CplError`, `TypeError` | an argument's Python type does not fit its CPL type (Section 3.4); `pointer` |
| `CplValueError` | `CplError`, `ValueError` | an argument has a fitting type but no CPL value (a `str` with an unpaired surrogate); `pointer` |
| `CplFault` | `CplError` | the call ended in an execution fault (`CPL_FAULT`, semantics 6.7): `code` (`contract_violation`, ...), `message`, `diagnostic` byte-for-byte the S49 object of `cpl run` (semantics S75) |
| `CplArgumentError` | `CplFault`, `ValueError` | the fault is the `refinement_violation` of an argument (semantics S81: it has `parameter`): the export's input check |
| `CplResultError` | `CplError` | the export returned `Err(e)`: `error` is the decoded `e` |
| `<E>Exception` | `CplResultError` | one per nominal error type `E` of an exported `Result` return (`StoreErrorException`, `ConstraintErrorException`, `ForeignErrorException`), named after `E`'s class |
| `CplUsageError` | `CplError` | the package was used wrongly or a C API usage error occurred: a released `Capability`, a closed runtime, `configure` while calls run, an `interop_*` status other than the ones above (`code`, `diagnostic`) |

An export whose return type is `Result<T, E>` returns `T`'s value or
raises `E`'s exception class (`CplResultError` itself when `E` is not
nominal); any other export returns its value. An `interop_*` usage error
that names an argument is raised as `CplTypeError` (the argument codes of
`spec/cabi.md` 4.9 but `interop_int_out_of_range`) or `CplValueError`
(`interop_int_out_of_range`, `interop_utf8_invalid`).

### 4.1 Budgets and cancellation

`_time_budget_ms` (an `int`, default 0, none) becomes the call's
`time_budget_ms`, and `_cancel` (a `CancelToken`) its `cancel`; a budget
exceeded is the fault `resource_exhaustion` (`time`) and a requested token
the C status `CPL_E_CANCELLED`, raised as `CplFault` with `code`
`"cancelled"` and no diagnostic. `CancelToken.cancel()` may be called from
any thread.

Decision: [D-331] P4. A CPL `Result` returned by an export becomes the
`Ok` value or a raised exception generated per error type, faults become
`CplFault` carrying the evaluator's diagnostic, and argument failures
become exceptions that are also `TypeError` or `ValueError`. Rationale:
the design gives Python "value or typed exception at the wrapper" for
`Result` (line 502) and "invalid size values become a typed exception in
Python" (line 594); generated subclasses let a caller catch one error type,
and the standard bases let Python code that catches `TypeError` and
`ValueError` keep working. Rejected alternatives: returning `Ok`/`Err` from
exports (every call site would unpack); one exception class for every
failure (a caller could not tell a refused argument from a contract
violation).

### 4.2 Threading

The extension's functions may be called from any thread. A call releases
the GIL while CPL runs (P2), so calls on several threads run concurrently;
a capability implemented in Python (Section 5) takes the GIL for its
callback (`PyGILState_Ensure`). Free-threaded CPython builds are not
supported (the limited API of 3.10 does not cover them). A call that a
signal interrupts is not interrupted: `KeyboardInterrupt` is raised when
the call returns, and a long call is ended by a budget or a token.

## 5. Capabilities from Python

### 5.1 Python objects as capabilities

A capability parameter accepts a `Capability` (Section 5.3) or any Python
object that has one attribute per operation of the capability, named as
the operation. For such an object the extension makes a capability with
`cpl_cap_new` for the duration of the call (released when the export
returns). Its operations follow the adapter contract of Section 5.2.

### 5.2 The adapter contract

An operation's implementation is called with its arguments as Python
values (Section 3.2) in declaration order, on a thread that holds the GIL,
and:

- **returns** a value of the operation's declared return type, `Ok(...)`
  and `Err(...)` included for a `Result` (as a C adapter's
  `cpl_adapter_ok`, `spec/cabi.md` 6.2): the binding layer converts it,
  and a value that is not of the declared type is the `InvalidResult`
  failure of S48 (with a foreign-error return type) or the fault
  `binding_result_ill_typed` (without one, semantics S80); the runtime then
  validates refinements (semantics S77) and evaluates `ensures`;
- **or raises**: a raised exception is a foreign failure (S48) whose kind is
  the exception's attribute `cpl_foreign_kind` when it holds a variant name
  of `ForeignErrorKind` (`"Unavailable"`), else `Exception`; whose message
  is `str(exception)`, or the class's qualified name when that is empty;
  and whose details are `foreign_type` (the class's `module.qualname`,
  `builtins.` left out) followed by the pairs of the exception's attribute
  `cpl_foreign_details` (a sequence of pairs of `str`), in order. With an
  operation whose return type is not a foreign-error return type, the
  failure is the fault `binding_error_unrepresentable` (semantics S80).
  `BaseException`s that are not `Exception`s (`KeyboardInterrupt`,
  `SystemExit`) are not caught by an in-process binding layer: the
  callback returns a status the callback contract does not allow, so CPL
  sees the fault `binding_protocol` (semantics S80), and the extension
  raises the `BaseException` again once the export has returned, in place
  of that fault. In a worker such an exception ends the worker, which is
  the `Transport` failure of Section 6.4.

### 5.3 Python adapters and `bind_<binding>()`

A binding with `backend = "python"` names a module (`adapter =
"reader_adapters.images"`, a dotted path of Python identifiers, else
`project_binding_adapter_invalid`). The module defines `create(cpl,
binding)`, called once per grant with the types module and the binding's
name, which returns the adapter object; the object implements the
capability's operations by the contract of Section 5.2 and may define
`close()`, called once when the grant's last reference is released.
`create` receives the types module so that an adapter builds `Ok`, `Err`,
and the program's classes without importing the package by name, which is
what lets the same adapter run in process and in a worker. An import of the
module, a missing or failing `create`, or an exception from it fails the
grant: `interop_bind_failed` (`binding`, and a note with the exception's
class and message).

`bind_<binding>()` grants a binding of the project in the Python process:

- `backend = "python"` with `isolation = "in_process"`: imports the module
  in this process and wraps the adapter object;
- `backend = "python"` with `isolation = "worker"` (the default): starts a
  worker (Section 6) with `sys.executable`, the Python path of this process
  (`sys.path`) as `PYTHONPATH`, and this process's environment;
- `backend = "c"`: the compiled-in adapter (`<prefix>_bind_<binding>`),
  in process or in the C adapter host of Section 6.6;
- `backend = "node"`: `CplUsageError` (the Node adapters of Milestone 4
  run in a Node host or worker, not from Python).

It returns a `Capability` (a context manager; `close()` releases the
host's reference, after which using it raises `CplUsageError`; the
garbage collector releases a forgotten one). The grant fails with
`CplUsageError` carrying the `interop_bind_failed` diagnostic.

Decision: [D-332] P5. A Python adapter is a module with `create(cpl,
binding)` returning an object with one method per operation, which returns
the operation's full declared return type and reports failures by
raising; the generated types module is passed in. Rationale: the design's
adapter is "project-supplied, not a CPL built-in" (line 557) and its module
initialization must happen inside the selected execution boundary (line
559), which a module imported by the host or by the worker gives; passing
the types module makes one adapter work in process and in a worker without
knowing the package name; returning the full value mirrors the C adapter
contract, so a `Result` payload that is itself a `Result` is never
ambiguous; raising is how Python code fails, and the two attributes let an
adapter choose the kind (`Unavailable`, `Timeout`) and add details without
a CPL import. Rejected alternatives: module-level functions per operation
(no per-grant state); returning bare payloads for `Result` operations
(ambiguous for nested `Result`s, and unlike C); requiring a CPL-specific
exception class (an adapter would import the generated package, whose name
differs per project).

## 6. The worker transport

A worker hosts one granted adapter in a process of its own and talks to
its host over one socket (design line 449: "Worker mode provides a separate
process and a place to apply host resource and access policy; process
separation alone is not a complete sandbox").

### 6.1 Channel, start, and banner

The worker client creates a connected `AF_UNIX` stream socket pair and
starts the worker with one end as its standard input and standard output;
the worker's standard error is the host's. The worker immediately moves
the channel to descriptors of its own, points standard input at
`/dev/null` and standard output at standard error, so that output printed
by an adapter never reaches the channel or the host's standard output. The
client writes with `MSG_NOSIGNAL`, so a dead worker never raises `SIGPIPE`
in the host.

| Worker kind | Command |
| - | - |
| `python` | `<python> -c <BOOT>`, the client then writes the worker's source as `<length in bytes>\n<source>` (P7) |
| `c` | the C adapter host of Section 6.6 |
| `node` | Milestone 4 (`spec/node.md`) |

The worker's first line is its banner, `{"protocol": 1, "worker":
"<kind>", "program": "<64 hex>", "runtime": {...}}`, with, for a Python
worker, the digest of its types module (the blake3 of the module's text
without its `PROGRAM` line, which the client computed when it generated the
source) and `"runtime": {"python": "<platform.python_version()>"}`, and for
a C adapter host the program digest of `spec/cabi.md` 3.8; a client refuses
another protocol, kind, or program.

### 6.2 cpl-worker protocol 1

Framing, the value encoding (`V`: node lists), and the foreign failure
`F` are cpl-driver protocol 1's (`spec/cabi.md` 3.8); one request at a
time.

| Request | Response |
| - | - |
| `{"op": "bind", "binding": "<name>"}` | `{"handle": 0}`: `create` ran (Section 5.3); or `{"error": E}` with `interop_bind_failed` and its note |
| `{"op": "cap_call", "handle": 0, "operation": i, "args": [V...]}` | `{"ok": V}` (the converted result, before validation), `{"foreign_error": F}` (Section 5.2), or `{"error": E}` |
| `{"op": "release", "handle": 0}` | `{"ok": null}`: `close()` ran when defined (an exception from it is ignored and printed to standard error) |
| `{"op": "shutdown"}` | `{"ok": null}`; the worker then exits 0 |

A worker hosts exactly one grant, so its handle is always 0. `F` carries
the details after the runtime's three keys (`adapter`, `capability`,
`operation`), which the client prepends (stdlib.md S45).

### 6.3 One worker per grant

Each grant of a worker binding starts one worker; the capability's last
release sends `release` and `shutdown`, waits up to 5 s for the exit, then
kills the process (`SIGKILL`) and reaps it. Calls on one worker are
serialized by the client (one in flight); the operations of one grant may
be called concurrently from several threads of the host (`spec/cabi.md`
4.4) and then wait their turn.

### 6.4 Failures

| Situation | Result |
| - | - |
| the command cannot be started, the banner does not arrive within 60 s (or `timeout_ms` when set), or it names another protocol, kind, or program | the grant fails: `interop_bind_failed` with a note naming the cause |
| `bind` answers an error | the grant fails with that error |
| the worker exits, or its channel closes, during a call | the foreign failure `Transport` with stdlib.md S45's message `the adapter host for <binding> exited with <status>` (S48) |
| no answer within the binding's `timeout_ms` (Section 6.5) | the client kills the worker; the foreign failure `Timeout`, message `the adapter host for <binding> did not answer within <n> ms` |
| the call's cancellation token is requested while it waits | the client kills the worker; the foreign failure `Cancelled`, message `the call of <operation> was cancelled` |
| an answer that is not JSON, not one of the responses above, or carries a value that is not a node list | the client kills the worker; the fault `binding_protocol` (semantics S82) |
| a call after any of the four above | the foreign failure `Transport` with the same message, at once: a worker is never restarted, since restarting would rerun the adapter's initialization; the host grants again to get a new one |

A foreign failure becomes `Err(ForeignError)` or the fault
`binding_error_unrepresentable` by the operation's return type, as every
binding's (semantics S80); `Transport` without a foreign-error return type
is the fault `binding_host_failed`. The worker's own conversion failures
(Section 5.2) arrive as `F` with kind `InvalidResult`.

### 6.5 Launch configuration

| Host | Python worker command | Environment |
| - | - | - |
| the package (Section 5.3) | `sys.executable` | the process's, with `PYTHONPATH` = `os.pathsep.join(sys.path)` |
| `cpl run`, `cpl test` (both engines, Section 7) | `--python`, else `CPL_PYTHON`, else `python3` | `cpl`'s, with `PYTHONPATH` = the absolute `[interop.python] paths`, then the inherited `PYTHONPATH` |
| a C host or a native executable | `CPL_PYTHON`, else `python3` | the process's |

The driver receives the toolchain's configuration in its `workers` request
(`spec/cabi.md` 3.8); the package passes its own through the API unit's
hidden `<prefix>__python_workers(interpreter, pythonpath)`. Every program
with a `backend = "python"` binding embeds its worker's source in its
tables, so no file has to be found at run time. `[bindings.<name>]
timeout_ms` (a positive integer; a non-positive one, or one on a backend-`c`
binding with `isolation = "in_process"`, is
`project_binding_timeout_invalid`) bounds every call's answer; absent, a
call waits as long as the worker runs.

### 6.6 C bindings in workers

From Milestone 3 a backend-`c` binding may have `isolation = "worker"`
(the manifest default). Its adapter then runs in the package's C adapter
host: an executable built from the program's objects, the backend-`c`
adapters, the runtime, and the runtime's worker main, which speaks
cpl-worker protocol 1 with kind `c` and grants with the adapter's `init`
exactly as `<prefix>_bind_<binding>` does in process. `cpl build` writes it
to `bin/<prefix>-worker` (C target) and `<package>/<prefix>-worker` (Python
target). A host finds it by the environment variable `CPL_C_WORKER`, else
beside the library or extension that contains the program (found with
`dladdr`, `../bin/` for a shared library); the driver and a native
executable re-execute themselves with the environment variable
`CPL_WORKER_MODE=1`, which makes their `main` the worker main.

Decision: [D-333] P6. A worker hosts one grant in a child process that
speaks cpl-worker protocol 1, driver protocol 1's framing, values, and
foreign failures with four requests, over a socket that replaces its
standard input and output; its source travels over the channel at start;
a worker that fails is never restarted. Rationale: the design's managed
worker (lines 436, 449, 455) needs a process boundary and a transport, and
reusing protocol 1 gives both clients (the C runtime's and the evaluator's)
the node-list codecs and failure records they already have; one process per
grant makes a grant's lifetime and failures its own; moving the channel off
standard output protects it from adapters that print; shipping the source
over the channel means no worker file must be installed or found; and
since a worker failure may leave an external action half done (line 489,
"must not automatically retry"), the client never restarts it. Rejected
alternatives: one worker shared by every grant (one adapter's crash would
fail the others); restarting a dead worker transparently (a hidden retry
of initialization); value JSON on the worker channel (recursive parsers on
deep values, and a second codec beside the node lists).

Decision: [D-334] P7. A Python worker is started as `<python> -c <BOOT>`,
where `BOOT` is a fixed bootstrap that reads `<length>\n<source>` from the
channel and executes it, and the worker's source is generated per program
and embedded in the program's tables. Rationale: no file is installed or
located at run time in any host, and the source is exactly the program's
(its banner names the program digest). Rejected alternatives: a worker
module of the package (a C host or `cpl run` has no package to import);
writing the source to a temporary file (files to clean up and to protect).


Decision: [D-335] P8. A backend-`c` binding may run its adapter in a
worker: the package's C adapter host, an executable of the program's
objects and adapters that speaks cpl-worker protocol 1, found by
`CPL_C_WORKER` or beside the library, and re-executed from the driver and
from native executables. Rationale: the Milestone 3 row of toolchain 13
makes `isolation = "worker"` legal for C bindings with the transport, and a
crash or memory error in a C library then ends the worker, not the host
(design line 485, "worker execution can contain failures"); the adapter
host is built from the same objects as the in-process grant, so both run
the same adapter. Rejected alternatives: forking the host without exec (a
fork of a multi-threaded host may deadlock in the child); one worker
executable for every package (it would not contain the program's
adapters).

## 7. `cpl run` and the native engine with Python bindings

From Milestone 3 `cpl run` binds `backend = "python"` bindings
(`spec/semantics.md` S78, amended by S82): each `using` parameter whose
capability's binding has that backend is granted in a worker, whatever its
`isolation` (the toolchain hosts no Python in process). The evaluator's
worker client is `cpl_cabi::worker::WorkerAdapter`, a `CapabilityImpl`
like `CAdapter` (`spec/cabi.md` 7.2): it starts the worker, binds,
forwards each operation as `cap_call`, rebuilds the answer, and leaves
validation and `ensures` to the evaluator. The native engine's driver
receives `{"op": "workers", "python": "<path>", "env": {"PYTHONPATH":
"..."}}` before its first `bind`, and its `bind` of a Python binding
starts the same worker with the runtime's client. Both engines therefore
run the same adapter in the same kind of process, which the differential
tests compare (Section 10).

A `cpl run` envelope's `bindings` entry for a Python binding has `backend`
`"python"` and `inputs` the adapter digest of Section 9.

Decision: [D-336] P9. Outside a Python process every Python binding runs in
a worker, whose interpreter and Python path each host configures: the
package passes its own interpreter and `sys.path`, the toolchain passes
`--python`, `CPL_PYTHON`, or `python3` with the project's `[interop.python]
paths`, and a C host or native executable uses `CPL_PYTHON` and its own
environment. Rationale: a C host, a native executable, and the toolchain
host no Python in process (the toolchain loads no foreign code at all,
`spec/cabi.md` C36), so the managed worker of the design (line 436) is the
one way for them to call Python; the host that grants the binding is the
one that knows which interpreter and path to use. Rejected alternatives:
embedding CPython in the C runtime (a `libpython` dependency for every C
package, and the toolchain would load foreign code); refusing Python
bindings outside Python (no evidence from `cpl run` that CPL calls Python,
and none comparable with the evaluator).

## 8. Test fixtures

`spec/semantics.md` S83 lets a test or property declare `using`
parameters (`grammar.md` D74):

```text
test "a stored page loads" (using db: PageRead)
    effects { db.get }
{
    assert load_text(PageId("p1"), using db) == Ok("hello")
}
```

`[test.fixtures]` in `cpl.toml` maps a capability's qualified path to the
binding that supplies it to tests (`"reader.pages.PageRead" =
"page_store"`); the binding must exist (`project_fixture_binding_unknown`)
and have that capability (`project_fixture_capability_mismatch`), and a
test's `using` parameter whose capability has no fixture is the error
`test_fixture_missing` (category `test`, at the parameter). `cpl test`
grants a fixture's binding once per execution of a test, and once per
property for all of its cases and shrinking, before the body runs, in
parameter order, and releases it after; a grant that fails fails the test
with `test_fixture_failed` (`binding`, the grant's diagnostic as a note).

The receipt's `fixtures` lists one `{capability, binding, digest}` per
fixture granted in the run, sorted by capability path, with `digest` the
binding's input digest (Section 9); `assumptions` gains one assumption of
kind `foreign_adapter` per fixture (`symbol` the capability, `description`
`fixture <binding>`); and `environment` gains `python` with the version
from the banner of a Python worker when the run granted one.

Decision: [D-337] P10. `[test.fixtures]` maps a capability to the binding
that supplies it to tests; `cpl test` grants it once per test execution and
once per property, and records each granted fixture, its assumption, and
the adapter runtime's version in the receipt. Rationale: "service-dependent
tests must declare fixtures or environment assumptions" (design line 600)
and a test receipt "includes seeds, fixtures, and environment inputs"
(line 261); the manifest is where a project already requests bindings
(line 559), and a fresh grant per test keeps one test's adapter state from
leaking into the next while a property's cases share one, as they share a
seed. Rejected alternatives: fixtures named in the test's source (the
source would name a binding, an operational choice the manifest owns);
one grant for the whole run (state would leak between tests); a grant per
property case (thousands of worker starts for one property).

## 9. Lock and digests

A Python binding's *input digest* is the blake3 of the adapter's module
path, then, when the module resolves under one of `[interop.python] paths`
(`a/b.py` or `a/b/__init__.py`, the first path that has one), the path and
content of every `.py` file in that module's directory tree (sorted), so an
edit of a project-local adapter changes it. A C binding's input digest is
the digest of its `c_input_digests` (toolchain 11.2). `cpl build --target
python --lock` writes `targets.python` (`package`, `version`, `api`,
`compiler_path`, `compiler_digest`, `flags_digest`, `runtime_digest`,
`python_headers_digest`, the digest of the interpreter headers that the
extension unit's depfile lists) and, for each Python binding,
`adapters.<binding> = { backend = "python", module, digest }`; `--release`
compares them as for C (`build_lock_mismatch` with the differing keys).

Decision: [D-338] P11. A Python binding's input digest covers its module
path and, for a module under the project's `[interop.python] paths`, the
module's source tree; it identifies the adapter in run envelopes, receipt
fixtures, and the lock. Rationale: the design locks "adapters" (line 487)
and invalidates evidence when "a foreign dependency or adapter change[s]"
(line 602); a project-local adapter is source the project controls, while
an installed module is pinned by the ecosystem's own lockfile (line 600),
which the module path names. Rejected alternatives: digesting every module
the adapter imports (the whole installed environment would be an input);
no digest for Python adapters (an edited adapter would leave receipts
valid).

## 10. Tests

| Area | Tests | What they establish |
| - | - | - |
| worker transport | `crates/cpl-cabi/tests/workers.rs` | both worker clients against generated Python workers: every request, every failure row of 6.4 (a crash mid-call, a timeout, a garbled answer, a start failure, a wrong program digest, a call after death), no zombie left, `SIGPIPE` never raised |
| `cpl run` with Python bindings | `crates/cpl-cli/tests/python_run.rs` | the fixture `pyshop` with `--engine eval`, `native`, and `both`: equal envelopes after the S75 normalization, every kind of result and failure |
| the package | `bindings/python/` (pytest), run by `crates/cpl-cli/tests/python_package.rs` | conversion of every kind in both directions, strictness and pointers, typed exceptions, faults equal to `cpl run`'s diagnostics, Python objects as capabilities, in-process and worker adapters, worker failure cases, threads, budgets and cancellation, the stubs under `mypy --strict` when installed, the wheel installing into a virtual environment when `venv` is available |
| fixtures | `crates/cpl-cli/tests/fixtures.rs` | `cpl test` with C and Python fixtures on both engines, receipts' `fixtures`, `assumptions`, and `environment` |
| build | `crates/cpl-cli/tests/python_build.rs` | the package layout, `build.json`, the stub golden, wheel determinism, `--lock` and `--release` |

`CPL_REQUIRE_PYTHON=1` turns a missing interpreter, `Python.h`, or pytest
into a failure instead of a skip, as `CPL_REQUIRE_CC` does for the C
compiler.

### 10.1 Milestone 3 acceptance checklist

1. **Gates.** The four workspace gates are clean with `CPL_REQUIRE_CC=1`,
   `CPL_REQUIRE_SANITIZERS=1`, `CPL_REQUIRE_TSAN=1`, and
   `CPL_REQUIRE_PYTHON=1`; every Milestone 1 and 2 test and golden passes.
2. **Python imports CPL.** `cpl build --target python` on `pyshop` exits 0;
   the pytest suite passes against the package directory and against the
   installed wheel.
3. **CPL calls a Python adapter.** `cpl run` of a `pyshop` function bound to
   a Python adapter returns the same envelope on `eval`, `native`, and
   `both`, in-process and worker adapters alike from Python.
4. **Conversion and exceptions.** Every row of Sections 3.2 and 4 is
   exercised from Python; adapter exceptions become `ForeignError` values
   with S48's kinds, messages, and details.
5. **Worker failures.** Every row of Section 6.4 is exercised by both
   clients with no zombie process left.
6. **Fixtures.** `cpl test` on `pyshop` grants C and Python fixtures on
   both engines and writes receipts with `fixtures`, `assumptions`, and
   `environment`.
7. **Specifications.** P1-P11, `grammar.md` D74, semantics S82-S83, and
   stdlib S48 are recorded and indexed in `spec/decisions.md`.
