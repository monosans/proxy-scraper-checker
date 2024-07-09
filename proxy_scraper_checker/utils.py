from __future__ import annotations

import asyncio
import functools
import os
from typing import TYPE_CHECKING
from urllib.parse import urlparse

import charset_normalizer
from typing_extensions import ParamSpec, TypeVar

if TYPE_CHECKING:
    from typing import Callable

T = TypeVar("T")
P = ParamSpec("P")

IS_DOCKER = os.getenv("IS_DOCKER") == "1"


def is_http_url(value: str, /) -> bool:
    parsed_url = urlparse(value)
    return bool(parsed_url.scheme in {"http", "https"} and parsed_url.netloc)


def bytes_decode(value: bytes, /) -> str:
    return str(charset_normalizer.from_bytes(value)[0])


def asyncify(f: Callable[P, T], /) -> Callable[P, asyncio.Future[T]]:
    def wrapper(*args: P.args, **kwargs: P.kwargs) -> asyncio.Future[T]:
        return asyncio.get_running_loop().run_in_executor(
            None, functools.partial(f, *args, **kwargs)
        )

    return functools.update_wrapper(wrapper, f)
