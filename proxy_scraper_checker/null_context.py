from __future__ import annotations

from typing import Any


class AsyncNullContext:
    __slots__ = ()

    async def __aenter__(self) -> None:
        pass

    async def __aexit__(self, *_: Any) -> None:
        pass
