from __future__ import annotations

import asyncio
import os
from pathlib import Path
from typing import Union
from urllib.parse import urlparse

import aiofiles.os
import aiofiles.ospath
import charset_normalizer

IS_DOCKER = os.getenv("IS_DOCKER") == "1"


def is_url(value: str, /) -> bool:
    parsed_url = urlparse(value)
    return bool(parsed_url.scheme and parsed_url.netloc)


def bytes_decode(value: bytes, /) -> str:
    return str(charset_normalizer.from_bytes(value)[0])


async def create_or_check_dir(path: Union[Path, str], /, *, mode: int) -> None:
    try:
        await aiofiles.os.makedirs(path)
    except FileExistsError:
        access_task = asyncio.create_task(aiofiles.os.access(path, mode))
        if not await aiofiles.ospath.isdir(path):
            msg = f"{path} is not a directory"
            raise ValueError(msg) from None
        if not await access_task:
            msg = f"{path} is not accessible"
            raise ValueError(msg) from None
