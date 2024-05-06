from __future__ import annotations

import ssl
from functools import lru_cache
from types import MappingProxyType

import certifi
from aiohttp import ClientResponse, DummyCookieJar, hdrs

from .utils import bytes_decode

HEADERS: MappingProxyType[str, str] = MappingProxyType({
    hdrs.USER_AGENT: (
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36"  # noqa: E501
    )
})
SSL_CONTEXT = ssl.create_default_context(cafile=certifi.where())


class NoCharsetHeaderError(Exception):
    pass


def fallback_charset_resolver(r: ClientResponse, b: bytes) -> str:  # noqa: ARG001
    raise NoCharsetHeaderError


@lru_cache(None)
def get_cookie_jar() -> DummyCookieJar:
    return DummyCookieJar()


def get_response_text(*, response: ClientResponse, content: bytes) -> str:
    try:
        return content.decode(response.get_encoding())
    except (NoCharsetHeaderError, UnicodeDecodeError):
        return bytes_decode(content)
