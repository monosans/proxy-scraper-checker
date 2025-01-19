from __future__ import annotations

from functools import cache
from pathlib import Path
from urllib.parse import urlparse

import charset_normalizer

is_docker = cache(Path("/.dockerenv").exists)


def is_http_url(value: str, /) -> bool:
    parsed_url = urlparse(value)
    return bool(parsed_url.scheme in {"http", "https"} and parsed_url.netloc)


def bytes_decode(value: bytes, /) -> str:
    return str(charset_normalizer.from_bytes(value)[0])
