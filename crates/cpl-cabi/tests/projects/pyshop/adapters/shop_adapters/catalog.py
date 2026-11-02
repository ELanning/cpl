"""The pyshop catalog: a fixed in-memory catalog whose operations exercise every outcome of a Python adapter
(spec/python.md 5.2, 6.4; stdlib S48)."""

import os


class NotFound(Exception):
    """A sku the catalog does not have."""


class _Catalog:
    def __init__(self, cpl):
        self.cpl = cpl
        c = cpl
        self.items = {
            "b1": c.Item(
                sku=c.Sku("b1"),
                name="Dune",
                price=12,
                category=c.Category.Book,
                tags=["scifi", "classic"],
                thumbnail=b"\x89PNG\x00\xff",
                discontinued=False,
                note=c.NOTHING,
            ),
            "t1": c.Item(
                sku=c.Sku("t1"),
                name="Hammer \u00e9\U0001F528",
                price=0,
                category=c.Category.Tool("steel"),
                tags=[],
                thumbnail=b"",
                discontinued=True,
                note=c.Some("heavy"),
            ),
            "k1": c.Item(
                sku=c.Sku("k1"),
                name="Kit",
                price=30,
                category=c.Category.Bundle([c.Sku("b1"), c.Sku("t1")], 2),
                tags=["bundle"],
                thumbnail=b"kit",
                discontinued=False,
                note=c.Some(""),
            ),
        }

    def lookup(self, sku):
        item = self.items.get(sku.value)
        if item is None:
            raise NotFound("no item " + sku.value)
        return self.cpl.Ok(item)

    def count(self):
        return len(self.items)

    def fail(self, kind):
        if kind == "plain":
            raise ValueError("a plain failure")
        if kind == "empty":
            raise RuntimeError()
        if kind == "attr":
            e = KeyError("with attributes")
            e.cpl_foreign_kind = "Unavailable"
            e.cpl_foreign_details = [("region", "eu"), ("bad",), ("n", 3), ("retry_safe", "true")]
            raise e
        raise self.cpl.ForeignFailure(kind, "failure of kind " + kind, [("hint", "h")])

    def crash(self, code):
        os._exit(code)

    def crash_plain(self, code):
        os._exit(code)

    def wrong(self):
        return self.cpl.Ok("not an item")

    def wrong_plain(self):
        return 42

    def negative(self):
        return self.cpl.Ok(-5)

    def echo(self, item):
        return self.cpl.Ok(item)

    def big(self, digits):
        # 7 repeated `digits` times, without int(str) (sys.get_int_max_str_digits)
        return self.cpl.Ok(7 * (10**digits - 1) // 9)


def create(cpl, binding):
    return _Catalog(cpl)
