from __future__ import annotations

import asyncio
from time import perf_counter

from aiohttp import ClientSession, ClientTimeout
from aiohttp.abc import AbstractCookieJar
from aiohttp_socks import ProxyConnector, ProxyType

from .constants import USER_AGENT


class Proxy:
    __slots__ = ("geolocation", "host", "is_anonymous", "port", "timeout")

    def __init__(self, *, host: str, port: int) -> None:
        self.host = host
        self.port = port

    @property
    def default_check_website(self) -> str:
        return "http://ip-api.com/json/?fields=8217"

    async def check(
        self,
        *,
        website: str,
        sem: asyncio.Semaphore,
        cookie_jar: AbstractCookieJar,
        proto: ProxyType,
        timeout: ClientTimeout,
    ) -> None:
        check_website = (
            self.default_check_website if website == "default" else website
        )
        async with sem:
            start = perf_counter()
            async with self.get_connector(proto) as connector, ClientSession(
                connector=connector,
                cookie_jar=cookie_jar,
                timeout=timeout,
                headers={"User-Agent": USER_AGENT},
            ) as session, session.get(
                check_website, raise_for_status=True
            ) as response:
                if website == "default":
                    await response.read()
        self.timeout = perf_counter() - start
        if website == "default":
            data = await response.json()
            self.is_anonymous = self.host != data["query"]
            self.geolocation = "|{}|{}|{}".format(
                data["country"], data["regionName"], data["city"]
            )

    def get_connector(self, proto: ProxyType) -> ProxyConnector:
        return ProxyConnector(proxy_type=proto, host=self.host, port=self.port)

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
