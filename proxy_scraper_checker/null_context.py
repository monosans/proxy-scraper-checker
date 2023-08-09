from __future__ import annotations


class AsyncNullContext:
    __slots__ = ()

    async def __aenter__(self) -> None:
        pass

    async def __aexit__(self, *_: object) -> None:
        pass
