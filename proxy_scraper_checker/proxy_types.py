from __future__ import annotations
import enum

class ProxyType(enum.IntEnum):
    """Proxy protocol types with compatibility for aiohttp_socks values."""
    
    SOCKS4 = 1  # Same as aiohttp_socks.ProxyType.SOCKS4
    SOCKS5 = 2  # Same as aiohttp_socks.ProxyType.SOCKS5
    HTTP = 3    # Same as aiohttp_socks.ProxyType.HTTP
    HTTPS = 4   # New type for HTTPS proxies

    @classmethod
    def is_http_based(cls, proto: ProxyType) -> bool:
        """Check if the proxy protocol is HTTP-based."""
        return proto in (cls.HTTP, cls.HTTPS)

    def to_aiohttp_socks_type(self) -> int:
        """Convert to aiohttp_socks ProxyType value."""
        if self == ProxyType.HTTPS:
            return ProxyType.HTTP
        return self.value
