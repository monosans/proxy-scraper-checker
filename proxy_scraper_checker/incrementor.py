from __future__ import annotations


class Incrementor:
    __slots__ = ("_value",)

    def __init__(self) -> None:
        self._value = 0

    def get_value(self) -> int:
        return self._value

    def increment(self) -> None:
        self._value += 1
