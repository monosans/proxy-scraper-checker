from __future__ import annotations

import asyncio
from time import perf_counter

from aiohttp import ClientSession
from aiohttp_socks import ProxyConnector


class Proxy:
    __slots__ = (
        "geolocation",
        "ip",
        "is_anonymous",
        "socket_address",
        "timeout",
    )

    def __init__(self, *, socket_address: str, ip: str) -> None:
        """
        Args:
            socket_address: ip:port
        """
        self.socket_address = socket_address
        self.ip = ip

    async def check(
        self, *, sem: asyncio.Semaphore, proto: str, timeout: float
    ) -> None:
        async with sem:
            proxy_url = f"{proto}://{self.socket_address}"
            start = perf_counter()
            async with ProxyConnector.from_url(proxy_url) as connector:
                async with ClientSession(connector=connector) as session:
                    async with session.get(
                        "http://ip-api.com/json/?fields=8217",
                        timeout=timeout,
                        raise_for_status=True,
                    ) as response:
                        data = await response.json()
        self.timeout = perf_counter() - start
        self.is_anonymous = self.ip != data["query"]
        self.geolocation = "|{}|{}|{}".format(
            data["country"], data["regionName"], data["city"]
        )

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, Proxy):
            return NotImplemented
        return self.socket_address == other.socket_address

    def __hash__(self) -> int:
        return hash(self.socket_address)
