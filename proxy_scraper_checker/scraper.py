from __future__ import annotations

import asyncio
import logging
from pathlib import Path
from typing import TYPE_CHECKING

from aiohttp import ClientResponseError, ClientTimeout
from aiohttp_socks import ProxyType

from proxy_scraper_checker.incrementor import Incrementor
from proxy_scraper_checker.parsers import PROXY_REGEX
from proxy_scraper_checker.proxy import Proxy
from proxy_scraper_checker.utils import is_http_url

if TYPE_CHECKING:
    from aiohttp import ClientSession
    from rich.progress import Progress, TaskID

    from proxy_scraper_checker.settings import Settings
    from proxy_scraper_checker.storage import ProxyStorage

_logger = logging.getLogger(__name__)


async def scrape_one(
    *,
    incrementor: Incrementor,
    progress: Progress,
    progress_task: TaskID,
    proto: ProxyType,
    session: ClientSession,
    settings: Settings,
    source: str,
    storage: ProxyStorage,
    timeout: ClientTimeout,
) -> None:
    try:
        if is_http_url(source):
            async with session.get(source, timeout=timeout) as response:
                content = await response.read()
            text = content.decode(response.get_encoding(), errors="replace")
        else:
            text = await asyncio.to_thread(
                Path(source.removeprefix("file://")).read_text,
                encoding="utf-8",
                errors="replace",
            )
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
        incrementor.increment()
        proxies = tuple(PROXY_REGEX.finditer(text))
        if not proxies:
            _logger.warning("%s | No proxies found", source)
        elif (
            settings.proxies_per_source_limit
            and len(proxies) > settings.proxies_per_source_limit
        ):
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
    progress.update(
        task_id=progress_task,
        advance=1,
        successful_count=incrementor.get_value(),
    )


async def scrape_all(
    *,
    progress: Progress,
    session: ClientSession,
    settings: Settings,
    storage: ProxyStorage,
) -> None:
    incrementors = {proto: Incrementor() for proto in settings.sources}
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
    timeout = ClientTimeout(total=settings.source_timeout, connect=5)
    await asyncio.gather(
        *(
            scrape_one(
                incrementor=incrementors[proto],
                progress=progress,
                progress_task=progress_tasks[proto],
                proto=proto,
                session=session,
                settings=settings,
                source=source,
                storage=storage,
                timeout=timeout,
            )
            for proto, sources in settings.sources.items()
            for source in sources
        )
    )
