from __future__ import annotations

import os
from pathlib import Path
from typing import Union
from urllib.parse import urlparse

import aiofiles.os
import charset_normalizer

IS_DOCKER = os.getenv("IS_DOCKER") == "1"


def is_url(value: str, /) -> bool:
    parsed_url = urlparse(value)
    return bool(parsed_url.scheme and parsed_url.netloc)


def bytes_decode(value: bytes, /) -> str:
    return str(charset_normalizer.from_bytes(value)[0])


async def check_access(path: Union[Path, str], /, *, mode: int) -> None:
    if not await aiofiles.os.access(path, mode):
        msg = f"{path} is not accessible"
        raise ValueError(msg)
