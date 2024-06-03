from __future__ import annotations

from aiohttp_socks import ProxyType

from .proxy import Proxy

PROTOCOL_ORDER = (ProxyType.HTTP, ProxyType.SOCKS4, ProxyType.SOCKS5)


def protocol_sort_key(proxy: Proxy, /) -> tuple[int, ProxyType]:
    return (PROTOCOL_ORDER.index(proxy.protocol), proxy.protocol)


def natural_sort_key(proxy: Proxy, /) -> tuple[int, ...]:
    return (proxy.protocol.value, *map(int, proxy.host.split(".")), proxy.port)


def timeout_sort_key(proxy: Proxy, /) -> float:
    return proxy.timeout
