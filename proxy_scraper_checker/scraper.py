from __future__ import annotations

import asyncio
import logging
from pathlib import Path
from typing import TYPE_CHECKING

from aiohttp import ClientResponseError, ClientTimeout
from aiohttp_socks import ProxyType

from proxy_scraper_checker.counter import IncrInt
from proxy_scraper_checker.http import get_response_text
from proxy_scraper_checker.parsers import PROXY_REGEX
from proxy_scraper_checker.proxy import Proxy
from proxy_scraper_checker.utils import bytes_decode, is_http_url

if TYPE_CHECKING:
    from aiohttp import ClientSession
    from rich.progress import Progress, TaskID

    from proxy_scraper_checker.settings import Settings
    from proxy_scraper_checker.storage import ProxyStorage

_logger = logging.getLogger(__name__)


async def scrape_one(
    *,
    counter: IncrInt,
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
            content = await asyncio.to_thread(
                Path(source.removeprefix("file://")).read_bytes
            )
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
        counter.incr()
        proxies = PROXY_REGEX.findall(text)
        if not proxies:
            _logger.warning("%s | No proxies found", source)
        # Ignore too big sources
        elif len(proxies) <= 100_000:  # noqa: PLR2004
            _logger.warning(
                "%s has too many proxies (%d), skipping", source, len(proxies)
            )
        else:
            for proxy in proxies:
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
    progress.update(task_id=task, advance=1, successful_count=counter.value)


async def scrape_all(
    *,
    progress: Progress,
    session: ClientSession,
    settings: Settings,
    storage: ProxyStorage,
) -> None:
    counters = {proto: IncrInt() for proto in settings.sources}
    progress_tasks = {
        proto: progress.add_task(
            description="",
            total=len(sources),
            module="Scraper",
            protocol=proto.name,
            successful_count=0,
        )
        for proto, sources in settings.sources.items()
    }
    timeout = ClientTimeout(total=settings.source_timeout)
    await asyncio.gather(
        *(
            scrape_one(
                counter=counters[proto],
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
