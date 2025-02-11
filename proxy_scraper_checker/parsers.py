from __future__ import annotations

import re

PROXY_REGEX = re.compile(
    r"(?:^|[^\dA-Za-z])(?:(?P<protocol>https?|socks[45]):\/\/)?(?:(?P<username>[^\s:@]+):(?P<password>[^\s:@]+)@)?(?P<host>(?:[\-\.\dA-Za-z]+|(?:\d|[1-9]\d|1\d{2}|2[0-4]\d|25[0-5])(?:\.(?:\d|[1-9]\d|1\d{2}|2[0-4]\d|25[0-5])){3})):(?P<port>\d|[1-9]\d{1,3}|[1-5]\d{4}|6[0-4]\d{3}|65[0-4]\d{2}|655[0-2]\d|6553[0-5])(?=[^\dA-Za-z]|$)",
    flags=re.MULTILINE | re.IGNORECASE,
)
IPV4_REGEX = re.compile(
    r"^(?:[0-9:A-Fa-f]+,)?\s*(?P<host>(?:\d|[1-9]\d|1\d{2}|2[0-4]\d|25[0-5])(?:\.(?:\d|[1-9]\d|1\d{2}|2[0-4]\d|25[0-5])){3})(?::(?:\d|[1-9]\d{1,3}|[1-5]\d{4}|6[0-4]\d{3}|65[0-4]\d{2}|655[0-2]\d|6553[0-5]))?\s*$"
)


def parse_ipv4(s: str, /) -> str:
    """Parse ip from ip:port."""
    match = IPV4_REGEX.match(s)
    if not match:
        raise ValueError
    return match.group("host")
