from __future__ import annotations

from typing import TYPE_CHECKING, Tuple

from aiohttp_socks import ProxyType

if TYPE_CHECKING:
    from .proxy import Proxy

PROTOCOL_ORDER = (ProxyType.HTTP, ProxyType.SOCKS4, ProxyType.SOCKS5)


def protocol_sort_key(proxy: Proxy, /) -> Tuple[int, ProxyType]:
    return (PROTOCOL_ORDER.index(proxy.protocol), proxy.protocol)


def natural_sort_key(proxy: Proxy, /) -> Tuple[int, ...]:
    return (proxy.protocol.value, *map(int, proxy.host.split(".")), proxy.port)


def timeout_sort_key(proxy: Proxy, /) -> float:
    return proxy.timeout
