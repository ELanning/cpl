# ---- the fixed part of the types module (spec/python.md Sections 3 and 5) ----------------------------------------
# The program's classes and the tables _DEFS, _TYPES, _CAPS follow it; the functions below read those globals when they
# run. Nothing here imports anything but the standard library.

import dataclasses as _dc
import json as _json


class Nothing:
    """The tag of `Option`'s `None` (spec/python.md 3.2): one instance, `NOTHING`."""

    __slots__ = ()

    def __repr__(self):
        return "NOTHING"

    def __eq__(self, other):
        return isinstance(other, Nothing)

    def __hash__(self):
        return hash("cpl.NOTHING")

    def __reduce__(self):
        return (_nothing, ())


NOTHING = Nothing()


def _nothing():
    return NOTHING


@_dc.dataclass(frozen=True, slots=True)
class Some:
    """The tag of `Option`'s `Some(value)`."""

    value: object

    def __repr__(self):
        return f"Some({self.value!r})"


@_dc.dataclass(frozen=True, slots=True)
class Ok:
    """The tag of `Result`'s `Ok(value)`."""

    value: object

    def __repr__(self):
        return f"Ok({self.value!r})"


@_dc.dataclass(frozen=True, slots=True)
class Err:
    """The tag of `Result`'s `Err(error)`."""

    error: object

    def __repr__(self):
        return f"Err({self.error!r})"


class ForeignFailure(Exception):
    """An exception an adapter may raise to report a foreign failure of a chosen kind (spec/python.md 5.2): `kind` is
    the name of a `ForeignErrorKind` variant, `details` pairs of strings appended after the runtime's keys."""

    def __init__(self, kind, message, details=()):
        super().__init__(message)
        self.cpl_foreign_kind = kind
        self.cpl_foreign_details = tuple(details)


# ---- Int: decimal text in chunks, so that no conversion meets sys.get_int_max_str_digits ----------------------------

_CHUNK = 1000
_CHUNK_POW = 10**_CHUNK
_DIRECT = 10**4000


def _int_to_dec(n):
    if -_DIRECT < n < _DIRECT:
        return str(n)
    neg = n < 0
    if neg:
        n = -n
    parts = []
    while n:
        n, r = divmod(n, _CHUNK_POW)
        parts.append(r)
    out = str(parts[-1]) + "".join("%0*d" % (_CHUNK, x) for x in reversed(parts[:-1]))
    return "-" + out if neg else out


def _dec_to_int(s):
    neg = s.startswith("-")
    digits = s[1:] if neg else s
    if len(digits) <= 4000:
        v = int(digits)
    else:
        v = 0
        for i in range(0, len(digits), _CHUNK):
            chunk = digits[i : i + _CHUNK]
            v = v * (_CHUNK_POW if len(chunk) == _CHUNK else 10 ** len(chunk)) + int(chunk)
    return -v if neg else v


# ---- the codec: node lists of cpl-driver protocol 1 (spec/cabi.md 3.8), iterative both ways --------------------------


class _ConvError(Exception):
    """A value that is not of its declared type: `pointer` (RFC 6901), `expected` (the type's display text), `found`
    (the Python type's name), `value_error` (the type fits but the value has no CPL counterpart)."""

    def __init__(self, pointer, expected, found, value_error=False, reason=None):
        super().__init__(pointer)
        self.pointer = pointer
        self.expected = expected
        self.found = found
        self.value_error = value_error
        self.reason = reason


def _found(x):
    return "None" if x is None else type(x).__qualname__


def encode(x, t, root=""):
    """The node list of the Python value `x` at the type descriptor `t`, as the list of nodes (root last); raises
    _ConvError at the first part of `x`, in value order, that is not a value of its declared type."""
    T = _TYPES
    nodes = []
    results = []
    stack = [(0, x, t, root)]
    while stack:
        op, a, b, c = stack.pop()
        if op == 1:
            kids = results[len(results) - b :] if b else []
            if b:
                del results[len(results) - b :]
            results.append(len(nodes))
            nodes.append(a + kids)
            continue
        x, ti, ptr = a, b, c
        d = T[ti]
        k = d[0]
        if k == "refined":
            stack.append((0, x, d[2], ptr))
            continue
        if k == "int":
            if isinstance(x, bool) or not isinstance(x, int):
                raise _ConvError(ptr, d[1], _found(x))
            results.append(len(nodes))
            nodes.append(["i", _int_to_dec(int(x))])
        elif k == "bool":
            if x is not True and x is not False:
                raise _ConvError(ptr, d[1], _found(x))
            results.append(len(nodes))
            nodes.append(["t"] if x else ["f"])
        elif k == "text":
            if not isinstance(x, str):
                raise _ConvError(ptr, d[1], _found(x))
            try:
                x.encode("utf-8")
            except UnicodeEncodeError:
                raise _ConvError(ptr, d[1], "str", True, "an unpaired surrogate") from None
            results.append(len(nodes))
            nodes.append(["s", str(x)])
        elif k == "bytes":
            if not isinstance(x, (bytes, bytearray, memoryview)):
                raise _ConvError(ptr, d[1], _found(x))
            results.append(len(nodes))
            nodes.append(["y", bytes(x).hex()])
        elif k == "unit":
            if x is not None:
                raise _ConvError(ptr, d[1], _found(x))
            results.append(len(nodes))
            nodes.append(["e", _UNIT, 0])
        elif k == "list":
            if not isinstance(x, (list, tuple)):
                raise _ConvError(ptr, d[1], _found(x))
            stack.append((1, ["l"], len(x), None))
            for i in range(len(x) - 1, -1, -1):
                stack.append((0, x[i], d[2], ptr + "/" + str(i)))
        elif k == "option":
            info = _DEFS[_OPTION]
            if isinstance(x, Nothing):
                results.append(len(nodes))
                nodes.append(["e", _OPTION, info[1]])
            elif isinstance(x, Some):
                stack.append((1, ["e", _OPTION, info[2]], 1, None))
                stack.append((0, x.value, d[2], ptr + "/$payload/0"))
            else:
                raise _ConvError(ptr, d[1], _found(x))
        elif k == "result":
            info = _DEFS[_RESULT]
            if isinstance(x, Ok):
                stack.append((1, ["e", _RESULT, info[1]], 1, None))
                stack.append((0, x.value, d[2], ptr + "/$payload/0"))
            elif isinstance(x, Err):
                stack.append((1, ["e", _RESULT, info[2]], 1, None))
                stack.append((0, x.error, d[3], ptr + "/$payload/0"))
            else:
                raise _ConvError(ptr, d[1], _found(x))
        elif k == "struct":
            cls = _DEFS[d[2]][1]
            if not isinstance(x, cls):
                raise _ConvError(ptr, d[1], _found(x))
            fields = d[3]
            stack.append((1, ["r", d[2]], len(fields), None))
            for pyname, name, fti in reversed(fields):
                stack.append((0, getattr(x, pyname), fti, ptr + "/" + name))
        elif k == "enum":
            info = _DEFS[d[2]]
            vi = getattr(type(x), "_cpl_variant", None)
            if not isinstance(x, info[1]) or vi is None:
                raise _ConvError(ptr, d[1], _found(x))
            payload = d[3][vi]
            stack.append((1, ["e", d[2], vi], len(payload), None))
            for i in range(len(payload) - 1, -1, -1):
                stack.append((0, getattr(x, "_" + str(i)), payload[i], ptr + "/$payload/" + str(i)))
        elif k == "wrapper":
            cls = _DEFS[d[2]][1]
            if not isinstance(x, cls):
                raise _ConvError(ptr, d[1], _found(x))
            stack.append((1, ["w", d[2]], 1, None))
            stack.append((0, x.value, d[3], ptr + "/$payload/0"))
        else:
            raise _ConvError(ptr, d[1], _found(x))
    return nodes


def decode(nodes):
    """The Python value of a node list (its nodes, root last), built bottom-up."""
    built = []
    for n in nodes:
        tag = n[0]
        if tag == "i":
            v = _dec_to_int(n[1])
        elif tag == "t":
            v = True
        elif tag == "f":
            v = False
        elif tag == "s":
            v = n[1]
        elif tag == "y":
            v = bytes.fromhex(n[1])
        elif tag == "l":
            v = [built[k] for k in n[1:]]
        elif tag == "r":
            v = _DEFS[n[1]][1](*[built[k] for k in n[2:]])
        elif tag == "e":
            info = _DEFS[n[1]]
            vi = n[2]
            kind = info[0]
            if kind == "unit":
                v = None
            elif kind == "option":
                v = NOTHING if vi == info[1] else Some(built[n[3]])
            elif kind == "result":
                v = Ok(built[n[3]]) if vi == info[1] else Err(built[n[3]])
            else:
                cls = info[2][vi]
                v = cls(*[built[k] for k in n[3:]]) if info[3][vi] else cls
        elif tag == "w":
            v = _DEFS[n[1]][1](built[n[2]])
        else:
            raise ValueError("a node of an unknown tag: " + repr(tag))
        built.append(v)
    return built[-1]


def _nodes_text(nodes):
    return _json.dumps({"nodes": nodes}, ensure_ascii=False, separators=(",", ":"))


def _decode_text(text):
    return decode(_json.loads(text)["nodes"])


# ---- foreign failures (spec/stdlib.md S48) ----------------------------------------------------------------------------

_KINDS = ("Exception", "InvalidArgument", "InvalidResult", "Transport", "Timeout", "Cancelled", "Unavailable")


def _class_name(cls):
    # the module's own ForeignFailure lives in a module named differently in process and in a worker
    if cls is ForeignFailure:
        return "ForeignFailure"
    module = getattr(cls, "__module__", None) or "builtins"
    name = getattr(cls, "__qualname__", None) or cls.__name__
    return name if module == "builtins" else module + "." + name


def _failure(exc):
    """(kind, message, details) of an exception an adapter raised."""
    kind = getattr(exc, "cpl_foreign_kind", None)
    if kind not in _KINDS:
        kind = "Exception"
    message = str(exc)
    if not message:
        message = _class_name(type(exc))
    details = [["foreign_type", _class_name(type(exc))]]
    for pair in getattr(exc, "cpl_foreign_details", ()) or ():
        try:
            key, value = pair
        except (TypeError, ValueError):
            continue
        if isinstance(key, str) and isinstance(value, str):
            details.append([key, value])
    return kind, message, details


def _invalid_result(operation, e):
    at = "" if e.pointer == "" else " at " + e.pointer
    message = "the result of %s is not a value of %s%s" % (operation, e.expected, at)
    return "InvalidResult", message, [["pointer", e.pointer], ["expected_type", e.expected]]


def _call_operation(adapter, cap, op_index, args_nodes):
    """Runs one operation of a Python adapter object: ("ok", nodes) or ("foreign", kind, message, details)."""
    pyname, ret, qualified = _CAPS[cap][1][op_index]
    args = [decode(nodes) for nodes in args_nodes]
    try:
        method = getattr(adapter, pyname)
        result = method(*args)
    except Exception as exc:  # BaseException propagates (spec/python.md 5.2)
        return ("foreign",) + _failure(exc)
    try:
        return ("ok", encode(result, ret))
    except _ConvError as e:
        return ("foreign",) + _invalid_result(qualified, e)


def _dispatch(adapter, cap, op_index, texts):
    """The extension's entry for an operation of a Python capability in process (spec/python.md 5.1): node-list
    texts in; ("ok", nodes text) or ("foreign", kind index, message, details) out."""
    answer = _call_operation(adapter, cap, op_index, [_json.loads(t)["nodes"] for t in texts])
    if answer[0] == "ok":
        return ("ok", _nodes_text(answer[1]))
    _, kind, message, details = answer
    return ("foreign", _KINDS.index(kind), message, [(k, v) for k, v in details])
