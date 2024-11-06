from __future__ import annotations

import asyncio
import itertools
import logging
from typing import TYPE_CHECKING

import aiofiles
from aiohttp import ClientResponseError, ClientTimeout
from aiohttp_socks import ProxyType

from .http import get_response_text
from .parsers import PROXY_REGEX
from .proxy import Proxy
from .utils import bytes_decode, is_http_url

if TYPE_CHECKING:
    from aiohttp import ClientSession
    from rich.progress import Progress, TaskID

    from .settings import Settings
    from .storage import ProxyStorage

_logger = logging.getLogger(__name__)


async def scrape_one(
    *,
    progress: Progress,
    proto: ProxyType,
    session: ClientSession,
    source: str,
    storage: ProxyStorage,
    task: TaskID,
    timeout: ClientTimeout,
) -> None:
    try:
        if is_http_url(source):
            async with session.get(source, timeout=timeout) as response:
                content = await response.read()
            text = get_response_text(response=response, content=content)
        else:
            async with aiofiles.open(source, "rb") as f:
                content = await f.read()
            text = bytes_decode(content)
    except ClientResponseError as e:
        _logger.warning(
            "%s | HTTP status code %d: %s", source, e.status, e.message
        )
    except Exception as e:
        _logger.warning(
            "%s | %s.%s: %s",
            source,
            e.__class__.__module__,
            e.__class__.__qualname__,
            e,
        )
    else:
        proxies = PROXY_REGEX.finditer(text)
        try:
            proxy = next(proxies)
        except StopIteration:
            _logger.warning("%s | No proxies found", source)
        else:
            for proxy in itertools.chain((proxy,), proxies):  # noqa: B020
                try:
                    protocol = ProxyType[
                        proxy.group("protocol").upper().rstrip("S")
                    ]
                except AttributeError:
                    protocol = proto
                storage.add(
                    Proxy(
                        protocol=protocol,
                        host=proxy.group("host"),
                        port=int(proxy.group("port")),
                        username=proxy.group("username"),
                        password=proxy.group("password"),
                    )
                )
    progress.advance(task_id=task, advance=1)


async def scrape_all(
    *,
    progress: Progress,
    session: ClientSession,
    settings: Settings,
    storage: ProxyStorage,
) -> None:
    progress_tasks = {
        proto: progress.add_task(
            description="", total=len(sources), col1="Scraper", col2=proto.name
        )
        for proto, sources in settings.sources.items()
    }
    timeout = ClientTimeout(total=settings.source_timeout)
    await asyncio.gather(
        *(
            scrape_one(
                progress=progress,
                proto=proto,
                session=session,
                source=source,
                storage=storage,
                task=progress_tasks[proto],
                timeout=timeout,
            )
            for proto, sources in settings.sources.items()
            for source in sources
        )
    )
