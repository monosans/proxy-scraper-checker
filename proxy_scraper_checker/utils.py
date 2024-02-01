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


async def check_writable(path: Union[Path, str], /) -> None:
    if not await aiofiles.os.access(path, os.W_OK):
        msg = f"{path} is not writable"
        raise ValueError(msg)
