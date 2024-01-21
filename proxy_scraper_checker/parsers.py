from __future__ import annotations

import re

PROXY_REGEX = re.compile(
    r"(?:^|[^\.\/\d:@A-Za-z])(?:(?P<protocol>https?|socks[45]):\/\/)?(?:(?P<username>[\dA-Za-z]*):(?P<password>[\dA-Za-z]*)@)?(?P<host>(?:\d|[1-9]\d|1\d{2}|2[0-4]\d|25[0-5])(?:\.(?:\d|[1-9]\d|1\d{2}|2[0-4]\d|25[0-5])){3}):(?P<port>\d|[1-9]\d{1,3}|[1-5]\d{4}|6[0-4]\d{3}|65[0-4]\d{2}|655[0-2]\d|6553[0-5])(?=[^\.\/\d:@A-Za-z]|$)",
    flags=re.MULTILINE | re.IGNORECASE,
)
IPV4_REGEX = re.compile(
    r"^\s*(?P<host>(?:\d|[1-9]\d|1\d{2}|2[0-4]\d|25[0-5])(?:\.(?:\d|[1-9]\d|1\d{2}|2[0-4]\d|25[0-5])){3})(?::(?:\d|[1-9]\d{1,3}|[1-5]\d{4}|6[0-4]\d{3}|65[0-4]\d{2}|655[0-2]\d|6553[0-5]))?\s*$"
)


def parse_ipv4(s: str, /) -> str:
    """Parse ip from ip:port."""
    match = IPV4_REGEX.match(s)
    if not match:
        raise ValueError
    return match.group("host")
