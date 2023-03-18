from __future__ import annotations

import asyncio
import logging
import sys
from typing import Any, Iterable, Optional
from urllib.parse import urlparse

from .folder import Folder

logger = logging.getLogger(__name__)


def timeout(value: float) -> None:
    if value <= 0:
        msg = "Timeout must be positive"
        raise ValueError(msg)


def source_timeout(value: float) -> None:
    if value <= 0:
        msg = "SourceTimeout must be positive"
        raise ValueError(msg)


def max_connections(value: int) -> Optional[int]:
    if value < 0:
        msg = "MaxConnections must be non-negative"
        raise ValueError(msg)
    max_supported = _get_supported_max_connections()
    if not value:
        logger.info("Using %d as MaxConnections value", max_supported or 0)
        return max_supported
    if not max_supported or value <= max_supported:
        return value
    logger.warning(
        (
            "MaxConnections value is too high. "
            "Your OS supports a maximum of %d. "
            "The config value will be ignored and %d will be used."
        ),
        max_supported,
        max_supported,
    )
    return max_supported


def _get_supported_max_connections() -> Optional[int]:
    if sys.platform == "win32":
        if isinstance(
            asyncio.get_event_loop_policy(), asyncio.WindowsSelectorEventLoopPolicy
        ):
            return 512
        return None
    import resource

    soft_limit, hard_limit = resource.getrlimit(resource.RLIMIT_NOFILE)
    logger.debug(
        "MaxConnections soft limit = %d, hard limit = %d, infinity = %d",
        soft_limit,
        hard_limit,
        resource.RLIM_INFINITY,
    )
    if soft_limit != hard_limit:
        try:
            resource.setrlimit(resource.RLIMIT_NOFILE, (hard_limit, hard_limit))
        except ValueError as e:
            logger.warning("Failed setting MaxConnections: %s", e)
        else:
            soft_limit = hard_limit
    if soft_limit == resource.RLIM_INFINITY:
        return None
    return soft_limit


def check_website(value: str) -> None:
    parsed_url = urlparse(value)
    if not parsed_url.scheme or not parsed_url.netloc:
        msg = f"invalid CheckWebsite URL: {value}"
        raise ValueError(msg)


def folders(value: Iterable[Folder]) -> None:
    if not any(folder for folder in value if folder.is_enabled):
        msg = "all folders are disabled in the config"
        raise ValueError(msg)


def sources(value: Any) -> None:
    if not value:
        msg = "proxy sources list is empty"
        raise ValueError(msg)
