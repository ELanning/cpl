# ---- the worker main of a Python worker (spec/python.md Section 6) -------------------------------------------------
# Appended to the types module when it is a worker's source; the bootstrap executes it as the module __cpl_worker__.


def _worker_main():
    import importlib
    import os
    import platform

    # the channel moves to descriptors of its own; standard input reads nothing, standard output is standard error
    chan_in = os.dup(0)
    chan_out = os.dup(1)
    null = os.open(os.devnull, os.O_RDONLY)
    os.dup2(null, 0)
    os.close(null)
    os.dup2(2, 1)
    reader = os.fdopen(chan_in, "rb")
    writer = os.fdopen(chan_out, "wb")

    def send(obj):
        writer.write(_json.dumps(obj, ensure_ascii=False, separators=(",", ":")).encode("utf-8") + b"\n")
        writer.flush()

    def usage(code, message, **data):
        error = {"code": code, "category": "interop", "severity": "error", "message": message}
        error.update(data)
        send({"error": error})

    def invalid(pointer, detail):
        usage(
            "interop_json_invalid",
            "the request is not cpl-worker protocol 1: " + detail,
            pointer=pointer,
            detail=detail,
        )

    send(
        {
            "protocol": 1,
            "worker": "python",
            "program": PROGRAM,
            "runtime": {"python": platform.python_version()},
        }
    )
    module = __import__("sys").modules[__name__]
    adapter = None
    cap = None
    bound = None
    for line in reader:
        try:
            req = _json.loads(line)
        except ValueError:
            invalid("", "not JSON")
            continue
        if not isinstance(req, dict):
            invalid("", "a request is an object")
            continue
        op = req.get("op")
        if op == "bind":
            name = req.get("binding")
            if bound is not None:
                invalid("/binding", "a worker hosts one grant")
                continue
            entry = _BINDINGS.get(name) if isinstance(name, str) else None
            if entry is None:
                invalid("/binding", "no backend-python binding of this name")
                continue
            module_path, cap_def = entry
            try:
                mod = importlib.import_module(module_path)
                create = getattr(mod, "create")
                adapter = create(module, name)
            except Exception as exc:
                kind, message, _ = _failure(exc)
                usage(
                    "interop_bind_failed",
                    "the binding `%s` could not be granted" % name,
                    binding=name,
                    notes=[{"message": "%s: %s" % (_class_name(type(exc)), message)}],
                )
                continue
            bound = name
            cap = cap_def
            send({"handle": 0})
        elif op == "cap_call":
            if bound is None or req.get("handle") != 0:
                invalid("/handle", "no grant has this handle")
                continue
            index = req.get("operation")
            ops = _CAPS[cap][1]
            if not isinstance(index, int) or isinstance(index, bool) or not 0 <= index < len(ops):
                invalid("/operation", "the capability has no operation of this index")
                continue
            args = req.get("args")
            if not isinstance(args, list) or not all(isinstance(a, dict) and isinstance(a.get("nodes"), list) for a in args):
                invalid("/args", "the arguments are node lists")
                continue
            answer = _call_operation(adapter, cap, index, [a["nodes"] for a in args])
            if answer[0] == "ok":
                send({"ok": {"nodes": answer[1]}})
            else:
                _, kind, message, details = answer
                send({"foreign_error": {"kind": kind, "message": message, "details": details}})
        elif op == "release":
            if bound is None:
                invalid("/handle", "no grant has this handle")
                continue
            close = getattr(adapter, "close", None)
            if close is not None:
                try:
                    close()
                except Exception as exc:
                    print("cpl worker: close() of %s raised %r" % (bound, exc), file=__import__("sys").stderr)
            adapter = None
            bound = None
            send({"ok": None})
        elif op == "shutdown":
            send({"ok": None})
            return 0
        else:
            invalid("/op", "an unknown request")
    return 0


raise SystemExit(_worker_main())
