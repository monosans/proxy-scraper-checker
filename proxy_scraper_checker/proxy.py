from __future__ import annotations

import asyncio
from time import perf_counter

from aiohttp import ClientSession, ClientTimeout
from aiohttp.abc import AbstractCookieJar
from aiohttp_socks import ProxyConnector, ProxyType


class Proxy:
    __slots__ = ("geolocation", "host", "is_anonymous", "port", "timeout")

    def __init__(self, *, host: str, port: int) -> None:
        self.host = host
        self.port = port

    async def check(
        self,
        *,
        sem: asyncio.Semaphore,
        cookie_jar: AbstractCookieJar,
        proto: str,
        timeout: ClientTimeout,
    ) -> None:
        async with sem:
            start = perf_counter()
            async with self.get_connector(proto) as connector:
                async with ClientSession(
                    connector=connector, cookie_jar=cookie_jar, timeout=timeout
                ) as session:
                    async with session.get(
                        "http://ip-api.com/json/?fields=8217",
                        raise_for_status=True,
                    ) as response:
                        data = await response.json()
        self.timeout = perf_counter() - start
        self.is_anonymous = self.host != data["query"]
        self.geolocation = "|{}|{}|{}".format(
            data["country"], data["regionName"], data["city"]
        )

    def get_connector(self, proto: str) -> ProxyConnector:
        return ProxyConnector(
            proxy_type=ProxyType[proto], host=self.host, port=self.port
        )

    def as_str(self, include_geolocation: bool) -> str:
        if include_geolocation:
            return f"{self.host}:{self.port}{self.geolocation}"
        return f"{self.host}:{self.port}"

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, Proxy):
            return NotImplemented
        return self.host == other.host and self.port == other.port

    def __hash__(self) -> int:
        return hash((self.host, self.port))
