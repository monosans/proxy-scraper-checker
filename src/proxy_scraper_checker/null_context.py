from __future__ import annotations


class NullContext:
    __slots__ = ()

    def __enter__(self) -> None:
        pass

    def __exit__(self, *_: object) -> None:
        pass

    async def __aenter__(self) -> None:
        pass

    async def __aexit__(self, *_: object) -> None:
        pass
