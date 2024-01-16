from __future__ import annotations

from io import StringIO
from time import perf_counter
from typing import Optional

import attrs
from aiohttp import ClientSession
from aiohttp_socks import ProxyConnector, ProxyType

from .http import (
    HEADERS,
    SSL_CONTEXT,
    fallback_charset_resolver,
    get_cookie_jar,
)
from .parsers import parse_ipv4
from .settings import CheckWebsiteType, Settings


@attrs.define(
    repr=False,
    unsafe_hash=True,
    weakref_slot=False,
    kw_only=True,
    eq=False,
    getstate_setstate=False,
    match_args=False,
)
class Proxy:
    protocol: ProxyType
    host: str
    port: int
    username: Optional[str]
    password: Optional[str]
    timeout: float = attrs.field(hash=False, init=False)
    exit_ip: str = attrs.field(hash=False, init=False)

    async def check(self, *, settings: Settings) -> None:
        async with settings.semaphore:
            start = perf_counter()
            connector = ProxyConnector(
                proxy_type=self.protocol,
                host=self.host,
                port=self.port,
                username=self.username,
                password=self.password,
                ssl=SSL_CONTEXT,
            )
            async with ClientSession(
                connector=connector,
                headers=HEADERS,
                cookie_jar=get_cookie_jar(),
                timeout=settings.timeout,
                fallback_charset_resolver=fallback_charset_resolver,
            ) as session, session.get(
                settings.check_website, raise_for_status=True
            ) as response:
                await response.read()
        self.timeout = perf_counter() - start
        if settings.check_website_type == CheckWebsiteType.HTTPBIN_IP:
            r = await response.json(content_type=None)
            self.exit_ip = r["origin"]
        elif settings.check_website_type == CheckWebsiteType.PLAIN_IP:
            self.exit_ip = parse_ipv4(await response.text())

    def as_str(self, *, include_protocol: bool) -> str:
        with StringIO() as buf:
            if include_protocol:
                buf.write(f"{self.protocol.name.lower()}://")
            if self.username is not None and self.password is not None:
                buf.write(f"{self.username}:{self.password}@")
            buf.write(f"{self.host}:{self.port}")
            return buf.getvalue()
