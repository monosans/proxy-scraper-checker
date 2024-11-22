from __future__ import annotations

import json
from io import StringIO
from time import perf_counter
from typing import TYPE_CHECKING

import attrs
from aiohttp import ClientSession
from aiohttp_socks import ProxyConnector

from proxy_scraper_checker.http import (
    HEADERS,
    SSL_CONTEXT,
    fallback_charset_resolver,
    get_cookie_jar,
    get_response_text,
)
from proxy_scraper_checker.parsers import parse_ipv4
from proxy_scraper_checker.settings import CheckWebsiteType

if TYPE_CHECKING:
    from aiohttp_socks import ProxyType

    from proxy_scraper_checker.settings import Settings


@attrs.define(
    repr=False,
    unsafe_hash=True,
    weakref_slot=False,
    kw_only=True,
    getstate_setstate=False,
    match_args=False,
)
class Proxy:
    protocol: ProxyType
    host: str
    port: int
    username: str | None
    password: str | None
    timeout: float | None = attrs.field(default=None, init=False, eq=False)
    exit_ip: str | None = attrs.field(default=None, init=False, eq=False)

    @property
    def is_checked(self) -> bool:
        return self.timeout is not None

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
            async with (
                ClientSession(
                    connector=connector,
                    headers=HEADERS,
                    cookie_jar=get_cookie_jar(),
                    raise_for_status=True,
                    timeout=settings.timeout,
                    fallback_charset_resolver=fallback_charset_resolver,
                ) as session,
                session.get(
                    settings.check_website,
                    headers=settings.check_website_type.headers,
                ) as response,
            ):
                content = await response.read()
        self.timeout = perf_counter() - start
        if settings.check_website_type == CheckWebsiteType.HTTPBIN_IP:
            r = json.loads(
                get_response_text(response=response, content=content)
            )
            self.exit_ip = parse_ipv4(r["origin"])
        elif settings.check_website_type == CheckWebsiteType.PLAIN_IP:
            self.exit_ip = parse_ipv4(
                get_response_text(response=response, content=content)
            )
        else:
            self.exit_ip = None

    def as_str(self, *, include_protocol: bool) -> str:
        with StringIO() as buf:
            if include_protocol:
                buf.write(f"{self.protocol.name.lower()}://")
            if self.username is not None and self.password is not None:
                buf.write(f"{self.username}:{self.password}@")
            buf.write(f"{self.host}:{self.port}")
            return buf.getvalue()
