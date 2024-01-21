from __future__ import annotations

import ssl
from functools import lru_cache
from types import MappingProxyType

import certifi
from aiohttp import DummyCookieJar, hdrs

SSL_CONTEXT = ssl.create_default_context(cafile=certifi.where())
HEADERS: MappingProxyType[str, str] = MappingProxyType({
    hdrs.USER_AGENT: (
        "Mozilla/5.0 (Windows NT 10.0; rv:121.0) Gecko/20100101 Firefox/121.0"
    )
})


@lru_cache(None)
def get_cookie_jar() -> DummyCookieJar:
    return DummyCookieJar()
