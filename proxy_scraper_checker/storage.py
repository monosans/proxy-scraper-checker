from __future__ import annotations

import itertools
from collections import Counter
from typing import TYPE_CHECKING, Callable, Dict, Iterator, List, Set, Tuple

from aiohttp_socks import ProxyType

from . import sort
from .proxy import Proxy

if TYPE_CHECKING:
    from _typeshed import SupportsRichComparison


class ProxyStorage:
    __slots__ = ("_proxies", "enabled_protocols")

    def __init__(self) -> None:
        self._proxies: Set[Proxy] = set()
        self.enabled_protocols: Set[ProxyType] = set()

    def add(self, proxy: Proxy, /) -> None:
        self.enabled_protocols.add(proxy.protocol)
        self._proxies.add(proxy)

    def remove(self, proxy: Proxy, /) -> None:
        self._proxies.remove(proxy)

    def get_grouped(self) -> Dict[ProxyType, Tuple[Proxy, ...]]:
        key = sort.protocol_sort_key
        d: Dict[ProxyType, Tuple[Proxy, ...]] = {
            proto: ()
            for proto in sort.PROTOCOL_ORDER
            if proto in self.enabled_protocols
        }
        for (_, proto), v in itertools.groupby(
            sorted(self._proxies, key=key), key=key
        ):
            d[proto] = tuple(v)
        return d

    def get_sorted(
        self,
        *,
        key: Callable[[Proxy], SupportsRichComparison] = sort.protocol_sort_key,
    ) -> List[Proxy]:
        return sorted(self._proxies, key=key)

    def get_count(self) -> Dict[ProxyType, int]:
        return dict(Counter(proxy.protocol for proxy in self._proxies))

    def __iter__(self) -> Iterator[Proxy]:
        return iter(self._proxies)
