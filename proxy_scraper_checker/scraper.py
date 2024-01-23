from __future__ import annotations

import asyncio
import itertools
import logging

import aiofiles
import aiofiles.os
import aiofiles.ospath
from aiohttp import ClientSession, ClientTimeout
from aiohttp_socks import ProxyType
from rich.progress import Progress, TaskID

from .http import get_response_text
from .parsers import PROXY_REGEX
from .proxy import Proxy
from .settings import Settings
from .storage import ProxyStorage
from .utils import bytes_decode, is_url

logger = logging.getLogger(__name__)


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
        if is_url(source):
            async with session.get(source, timeout=timeout) as response:
                content = await response.read()
            text = get_response_text(response=response, content=content)
        else:
            response = None
            async with aiofiles.open(source, "rb") as f:
                content = await f.read()
            text = bytes_decode(content)
    except Exception as e:
        logger.warning(
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
            if response and response.status != 200:  # noqa: PLR2004
                logger.warning(
                    "%s | HTTP status code %d", source, response.status
                )
            else:
                logger.warning("%s | No proxies found", source)
        else:
            for proxy in itertools.chain((proxy,), proxies):  # noqa: B020
                try:
                    protocol = ProxyType[
                        "HTTP"
                        if (p := proxy.group("protocol").upper()) == "HTTPS"
                        else p
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
    progress.update(task, advance=1)


async def scrape_all(
    *,
    progress: Progress,
    session: ClientSession,
    settings: Settings,
    storage: ProxyStorage,
) -> None:
    tasks = {
        proto: progress.add_task(
            f"[yellow]Scraper [red]:: [green]{proto.name}", total=len(sources)
        )
        for proto, sources in settings.sources.items()
    }
    timeout = ClientTimeout(total=settings.source_timeout)
    coroutines = (
        scrape_one(
            progress=progress,
            proto=proto,
            session=session,
            source=source,
            storage=storage,
            task=tasks[proto],
            timeout=timeout,
        )
        for proto, sources in settings.sources.items()
        for source in sources
    )
    await asyncio.gather(*coroutines)
