from __future__ import annotations

from functools import cache
from pathlib import Path
from urllib.parse import urlparse

is_docker = cache(Path("/.dockerenv").exists)


def is_http_url(value: str, /) -> bool:
    parsed_url = urlparse(value)
    return bool(parsed_url.scheme in {"http", "https"} and parsed_url.netloc)
