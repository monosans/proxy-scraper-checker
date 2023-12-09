from __future__ import annotations

from types import MappingProxyType

from aiohttp import hdrs

DEFAULT_CHECK_WEBSITE = "http://ip-api.com/json/?fields=8217"
HEADERS: MappingProxyType[str, str] = MappingProxyType({
    hdrs.USER_AGENT: (
        "Mozilla/5.0 (Windows NT 10.0; rv:120.0) Gecko/20100101 Firefox/120.0"
    )
})
