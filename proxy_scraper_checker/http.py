from __future__ import annotations

import ssl
from functools import cache
from types import MappingProxyType
from typing import TYPE_CHECKING

import certifi
from aiohttp import DummyCookieJar, hdrs

from proxy_scraper_checker.utils import bytes_decode

if TYPE_CHECKING:
    from typing import NoReturn

    from aiohttp import ClientResponse

HEADERS: MappingProxyType[str, str] = MappingProxyType({
    hdrs.USER_AGENT: (
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36"  # noqa: E501
    )
})
SSL_CONTEXT = ssl.create_default_context(cafile=certifi.where())
SSL_CONTEXT.set_alpn_protocols(("http/1.1",))


class NoCharsetHeaderError(Exception):
    pass


def fallback_charset_resolver(_r: ClientResponse, _b: bytes) -> NoReturn:
    raise NoCharsetHeaderError


@cache
def get_cookie_jar() -> DummyCookieJar:
    return DummyCookieJar()


def get_response_text(*, response: ClientResponse, content: bytes) -> str:
    try:
        return content.decode(response.get_encoding())
    except (NoCharsetHeaderError, UnicodeDecodeError):
        return bytes_decode(content)
