from __future__ import annotations

from threading import Lock


class IncrInt:
    __slots__ = ("_lock", "_v")

    def __init__(self) -> None:
        self._lock = Lock()
        self._v = 0

    @property
    def value(self) -> int:
        return self._v

    def incr(self) -> None:
        with self._lock:
            self._v += 1
