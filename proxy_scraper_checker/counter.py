from __future__ import annotations


class IncrInt:
    __slots__ = ("_v",)

    def __init__(self) -> None:
        self._v = 0

    @property
    def value(self) -> int:
        return self._v

    def incr(self) -> None:
        self._v += 1
