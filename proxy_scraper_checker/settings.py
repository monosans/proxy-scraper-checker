from __future__ import annotations

import asyncio
import enum
import json
import logging
import math
import stat
import sys
from pathlib import Path
from types import MappingProxyType
from typing import TYPE_CHECKING
from urllib.parse import urlparse

import attrs
import platformdirs
from aiohttp import ClientTimeout, hdrs
from aiohttp_socks import ProxyType

from . import fs, sort
from .http import get_response_text
from .null_context import NullContext
from .parsers import parse_ipv4
from .utils import IS_DOCKER

if TYPE_CHECKING:
    from typing import Callable, Iterable, Mapping

    from aiohttp import ClientSession
    from typing_extensions import Any, Literal, Self

    from .proxy import Proxy

logger = logging.getLogger(__name__)


def _get_supported_max_connections() -> int | None:
    if sys.platform == "win32":
        if isinstance(
            asyncio.get_event_loop_policy(),
            asyncio.WindowsSelectorEventLoopPolicy,
        ):
            return 512
        return None
    import resource  # type: ignore[unreachable, unused-ignore]  # noqa: PLC0415

    soft_limit, hard_limit = resource.getrlimit(resource.RLIMIT_NOFILE)
    logger.debug(
        "max_connections: soft limit = %d, hard limit = %d, infinity = %d",
        soft_limit,
        hard_limit,
        resource.RLIM_INFINITY,
    )
    if soft_limit != hard_limit:
        try:
            resource.setrlimit(resource.RLIMIT_NOFILE, (hard_limit, hard_limit))
        except ValueError as e:
            logger.warning("Failed setting max_connections: %s", e)
        else:
            soft_limit = hard_limit
    if soft_limit == resource.RLIM_INFINITY:
        return None
    return soft_limit


def _get_max_connections(value: int, /) -> int | None:
    if value < 0:
        msg = "max_connections must be non-negative"
        raise ValueError(msg)
    max_supported = _get_supported_max_connections()
    if not value:
        logger.info("Using %d as max_connections value", max_supported or 0)
        return max_supported
    if not max_supported or value <= max_supported:
        return value
    logger.warning(
        "max_connections value is too high for your OS. "
        "The config value will be ignored and %d will be used.%s",
        max_supported,
        " To make max_connections unlimited, install the winloop library."
        if sys.version_info >= (3, 9)
        and sys.platform in {"cygwin", "win32"}
        and sys.implementation.name == "cpython"
        else "",
    )
    return max_supported


def _semaphore_converter(value: int, /) -> asyncio.Semaphore | NullContext:
    v = _get_max_connections(value)
    return asyncio.Semaphore(v) if v else NullContext()


def _timeout_converter(value: float, /) -> ClientTimeout:
    return ClientTimeout(total=value, sock_connect=math.inf)


def _sources_converter(
    value: Mapping[ProxyType, Iterable[str] | None], /
) -> dict[ProxyType, frozenset[str]]:
    return {
        proxy_type: frozenset(sources)
        for proxy_type, sources in value.items()
        if sources is not None
    }


class CheckWebsiteType(enum.Enum):
    UNKNOWN = enum.auto(), False, False, None
    PLAIN_IP = enum.auto(), True, True, None
    """https://checkip.amazonaws.com"""
    HTTPBIN_IP = (
        enum.auto(),
        True,
        True,
        MappingProxyType({hdrs.ACCEPT: "application/json"}),
    )
    """https://httpbin.org/ip"""

    def __init__(
        self,
        _: object,
        supports_geolocation: bool,  # noqa: FBT001
        supports_anonymity: bool,  # noqa: FBT001
        headers: Mapping[str, str] | None,
        /,
    ) -> None:
        self.supports_geolocation = supports_geolocation
        self.supports_anonymity = supports_anonymity
        self.headers = headers

    def __new__(cls, value: object, *_: Any) -> Self:
        member = object.__new__(cls)
        member._value_ = value
        return member


async def _get_check_website_type_and_real_ip(
    *, check_website: str, session: ClientSession
) -> (
    tuple[Literal[CheckWebsiteType.UNKNOWN], None]
    | tuple[
        Literal[CheckWebsiteType.PLAIN_IP, CheckWebsiteType.HTTPBIN_IP], str
    ]
):
    if not check_website:
        return CheckWebsiteType.UNKNOWN, None
    try:
        async with session.get(check_website) as response:
            content = await response.read()
        text = get_response_text(response=response, content=content)
    except Exception:
        logger.exception(
            "Error when opening check_website without proxy, it will be "
            "impossible to determine anonymity and geolocation of proxies"
        )
        return CheckWebsiteType.UNKNOWN, None
    try:
        js = json.loads(text)
    except json.JSONDecodeError:
        try:
            return CheckWebsiteType.PLAIN_IP, parse_ipv4(text)
        except ValueError:
            pass
    else:
        try:
            return CheckWebsiteType.HTTPBIN_IP, parse_ipv4(js["origin"])
        except (KeyError, TypeError, ValueError):
            pass
    logger.warning(
        "Check_website is not httpbin and does not return plain ip, so it will"
        " be impossible to determine the anonymity and geolocation of proxies"
    )
    return CheckWebsiteType.UNKNOWN, None


@attrs.define(
    repr=False,
    weakref_slot=False,
    kw_only=True,
    eq=False,
    getstate_setstate=False,
    match_args=False,
)
class Settings:
    check_website: str = attrs.field(
        validator=attrs.validators.instance_of(str)
    )
    check_website_type: CheckWebsiteType = attrs.field(
        validator=attrs.validators.instance_of(CheckWebsiteType)
    )
    enable_geolocation: bool = attrs.field(
        validator=attrs.validators.instance_of(bool)
    )
    output_json: bool = attrs.field(
        validator=attrs.validators.instance_of(bool)
    )
    output_path: Path = attrs.field(converter=Path)
    output_txt: bool = attrs.field(validator=attrs.validators.instance_of(bool))
    real_ip: str | None = attrs.field(
        validator=attrs.validators.optional(attrs.validators.instance_of(str))
    )
    semaphore: asyncio.Semaphore | NullContext = attrs.field(
        converter=_semaphore_converter
    )
    sort_by_speed: bool = attrs.field(
        validator=attrs.validators.instance_of(bool)
    )
    source_timeout: float = attrs.field(validator=attrs.validators.gt(0))
    sources: dict[ProxyType, frozenset[str]] = attrs.field(
        validator=attrs.validators.and_(
            attrs.validators.instance_of(dict),
            attrs.validators.min_len(1),
            attrs.validators.deep_mapping(
                attrs.validators.instance_of(ProxyType),
                attrs.validators.and_(
                    attrs.validators.min_len(1),
                    attrs.validators.deep_iterable(
                        attrs.validators.and_(
                            attrs.validators.instance_of(str),
                            attrs.validators.min_len(1),
                        )
                    ),
                ),
            ),
        ),
        converter=_sources_converter,
    )
    timeout: ClientTimeout = attrs.field(converter=_timeout_converter)

    @property
    def sorting_key(
        self,
    ) -> Callable[[Proxy], float] | Callable[[Proxy], tuple[int, ...]]:
        return (
            sort.timeout_sort_key
            if self.sort_by_speed
            else sort.natural_sort_key
        )

    def __attrs_post_init__(self) -> None:
        if not self.output_json and not self.output_txt:
            msg = "both json and txt outputs are disabled"
            raise ValueError(msg)

        if not self.output_json and self.enable_geolocation:
            msg = "geolocation can not be enabled if json output is disabled"
            raise ValueError(msg)

        if not self.check_website and self.sort_by_speed:
            logger.warning(
                "Proxy checking is disabled, so sorting by speed is not"
                " possible. Alphabetical sorting will be used instead."
            )
            self.sort_by_speed = False

    @check_website.validator
    def _validate_check_website(
        self,
        attribute: attrs.Attribute[str],  # noqa: ARG002
        value: str,
        /,
    ) -> None:
        if value:
            parsed_url = urlparse(value)
            if (
                parsed_url.scheme not in {"http", "https"}
                or not parsed_url.netloc
            ):
                msg = f"invalid check_website: {value}"
                raise ValueError(msg)

            if parsed_url.scheme == "http":
                logger.warning(
                    "check_website uses the http protocol. "
                    "It is recommended to use https for correct checking."
                )

    @timeout.validator
    def _validate_timeout(
        self,
        attribute: attrs.Attribute[str],  # noqa: ARG002
        value: float,  # noqa: ARG002
        /,
    ) -> None:
        if self.timeout.total is None or self.timeout.total <= 0:
            msg = "timeout must be positive"
            raise ValueError(msg)

    @classmethod
    async def from_mapping(
        cls, cfg: Mapping[str, Any], /, *, session: ClientSession
    ) -> Self:
        output_path = (
            platformdirs.user_data_path("proxy_scraper_checker")
            if IS_DOCKER
            else Path(cfg["output"]["path"])
        )

        output_path_future = fs.async_create_or_fix_dir(
            output_path, permission=stat.S_IXUSR | stat.S_IWUSR
        )

        check_website_type, real_ip = await _get_check_website_type_and_real_ip(
            check_website=cfg["check_website"], session=session
        )

        enable_geolocation = (
            cfg["enable_geolocation"]
            and check_website_type.supports_geolocation
        )

        if enable_geolocation:
            await fs.async_create_or_fix_dir(
                fs.CACHE_PATH,
                permission=stat.S_IRUSR | stat.S_IXUSR | stat.S_IWUSR,
            )

        await output_path_future

        return cls(
            check_website=cfg["check_website"],
            check_website_type=check_website_type,
            enable_geolocation=enable_geolocation,
            output_json=cfg["output"]["json"],
            output_path=output_path,
            output_txt=cfg["output"]["txt"],
            real_ip=real_ip,
            semaphore=cfg["max_connections"],
            sort_by_speed=cfg["sort_by_speed"],
            source_timeout=cfg["source_timeout"],
            sources={
                ProxyType.HTTP: (
                    cfg["http"]["sources"] if cfg["http"]["enabled"] else None
                ),
                ProxyType.SOCKS4: (
                    cfg["socks4"]["sources"]
                    if cfg["socks4"]["enabled"]
                    else None
                ),
                ProxyType.SOCKS5: (
                    cfg["socks5"]["sources"]
                    if cfg["socks5"]["enabled"]
                    else None
                ),
            },
            timeout=cfg["timeout"],
        )
