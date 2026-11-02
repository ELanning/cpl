"""The pyshop tally: a running total per grant, bound in process from Python (spec/python.md 5.3)."""

import os

# the names of the grants whose close() ran, in order (the package's tests read it)
CLOSED = []


class _Tally:
    def __init__(self, binding):
        self.binding = binding
        self.n = 0

    def add(self, n):
        self.n += n
        return self.n

    def total(self):
        return self.n

    def interrupt(self):
        raise KeyboardInterrupt("the tally was interrupted")

    def close(self):
        CLOSED.append(self.binding)
        log = os.environ.get("PYSHOP_TALLY_LOG")
        if log:
            with open(log, "a", encoding="utf-8") as f:
                f.write("closed " + self.binding + "\n")


def create(cpl, binding):
    return _Tally(binding)
