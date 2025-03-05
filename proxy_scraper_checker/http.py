from __future__ import annotations

import ssl
from functools import cache
from types import MappingProxyType

import certifi
from aiohttp import DummyCookieJar, hdrs

HEADERS: MappingProxyType[str, str] = MappingProxyType({
    hdrs.USER_AGENT: (
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36"  # noqa: E501
    )
})
SSL_CONTEXT = ssl.create_default_context(cafile=certifi.where())
SSL_CONTEXT.set_alpn_protocols(("http/1.1",))
get_cookie_jar = cache(DummyCookieJar)
