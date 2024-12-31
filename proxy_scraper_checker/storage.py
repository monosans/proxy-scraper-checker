from __future__ import annotations

import itertools
from collections import Counter
from typing import TYPE_CHECKING

from proxy_scraper_checker import sort

if TYPE_CHECKING:
    from collections.abc import Iterable, Iterator

    from proxy_scraper_checker.proxy import Proxy
    from proxy_scraper_checker.proxy_types import ProxyType


class ProxyStorage:
    __slots__ = ("_proxies", "enabled_protocols")

    def __init__(self, *, protocols: Iterable[ProxyType]) -> None:
        self.enabled_protocols = set(protocols)
        self._proxies: set[Proxy] = set()

    def add(self, proxy: Proxy, /) -> None:
        self.enabled_protocols.add(proxy.protocol)
        self._proxies.add(proxy)

    def remove(self, proxy: Proxy, /) -> None:
        self._proxies.remove(proxy)

    def get_grouped(self) -> dict[ProxyType, tuple[Proxy, ...]]:
        key = sort.protocol_sort_key
        return {
            **{
                proto: ()
                for proto in sort.PROTOCOL_ORDER
                if proto in self.enabled_protocols
            },
            **{
                proto: tuple(v)
                for (_, proto), v in itertools.groupby(
                    sorted(self, key=key), key=key
                )
            },
        }

    def get_count(self) -> dict[ProxyType, int]:
        return {
            **{
                proto: 0
                for proto in sort.PROTOCOL_ORDER
                if proto in self.enabled_protocols
            },
            **Counter(proxy.protocol for proxy in self),
        }

    def remove_unchecked(self) -> None:
        for p in self._proxies.copy():
            if not p.is_checked:
                self._proxies.remove(p)

    def __iter__(self) -> Iterator[Proxy]:
        return iter(self._proxies)
