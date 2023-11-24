from __future__ import annotations

import asyncio
from dataclasses import dataclass
from time import perf_counter
from types import MappingProxyType
from typing import Union

from aiohttp import ClientSession, ClientTimeout
from aiohttp.abc import AbstractCookieJar
from aiohttp_socks import ProxyConnector, ProxyType

from .null_context import AsyncNullContext

DEFAULT_CHECK_WEBSITE = "http://ip-api.com/json/?fields=8217"
HEADERS = MappingProxyType(
    {
        "User-Agent": (
            "Mozilla/5.0 (Windows NT 10.0; rv:120.0)"
            " Gecko/20100101 Firefox/120.0"
        )
    }
)


@dataclass(repr=False, unsafe_hash=True)
class Proxy:
    __slots__ = ("geolocation", "host", "is_anonymous", "port", "timeout")

    host: str
    port: int

    async def check(
        self,
        *,
        website: str,
        sem: Union[asyncio.Semaphore, AsyncNullContext],
        cookie_jar: AbstractCookieJar,
        proto: ProxyType,
        timeout: ClientTimeout,
    ) -> None:
        if website == "default":
            website = DEFAULT_CHECK_WEBSITE
        async with sem:
            start = perf_counter()
            connector = ProxyConnector(
                proxy_type=proto, host=self.host, port=self.port
            )
            async with ClientSession(
                connector=connector,
                cookie_jar=cookie_jar,
                timeout=timeout,
                headers=HEADERS,
            ) as session, session.get(
                website, raise_for_status=True
            ) as response:
                if website == DEFAULT_CHECK_WEBSITE:
                    await response.read()
        self.timeout = perf_counter() - start
        if website == DEFAULT_CHECK_WEBSITE:
            data = await response.json(content_type=None)
            self.is_anonymous = self.host != data["query"]
            self.geolocation = "|{}|{}|{}".format(
                data["country"], data["regionName"], data["city"]
            )

    def as_str(self, *, include_geolocation: bool) -> str:
        if include_geolocation:
            return f"{self.host}:{self.port}{self.geolocation}"
        return f"{self.host}:{self.port}"
