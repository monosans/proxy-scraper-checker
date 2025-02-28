from __future__ import annotations

import asyncio
import logging
from typing import TYPE_CHECKING

from proxy_scraper_checker import sort
from proxy_scraper_checker.incrementor import Incrementor

if TYPE_CHECKING:
    from collections.abc import Mapping

    from aiohttp_socks import ProxyType
    from rich.progress import Progress, TaskID

    from proxy_scraper_checker.proxy import Proxy
    from proxy_scraper_checker.settings import Settings
    from proxy_scraper_checker.storage import ProxyStorage

_logger = logging.getLogger(__name__)


async def check_one(
    *,
    incrementor: Incrementor,
    progress: Progress,
    progress_task: TaskID,
    proxy: Proxy,
    settings: Settings,
    storage: ProxyStorage,
) -> None:
    try:
        await proxy.check(settings=settings)
    except Exception as e:
        # Too many open files
        if isinstance(e, OSError) and e.errno == 24:  # noqa: PLR2004
            _logger.error("Please, set max_connections to lower value")

        _logger.debug(
            "%s.%s: %s", e.__class__.__module__, e.__class__.__qualname__, e
        )
        storage.remove(proxy)
    else:
        incrementor.increment()
    progress.update(
        task_id=progress_task,
        advance=1,
        successful_count=incrementor.get_value(),
    )


async def check_all(
    *,
    settings: Settings,
    storage: ProxyStorage,
    progress: Progress,
    proxies_count: Mapping[ProxyType, int],
) -> None:
    incrementors = {
        proto: Incrementor()
        for proto in sort.PROTOCOL_ORDER
        if proto in storage.enabled_protocols
    }
    progress_tasks = {
        proto: progress.add_task(
            description="",
            total=proxies_count[proto],
            module="Checker",
            protocol=proto.name,
            successful_count=0,
        )
        for proto in incrementors
    }
    await asyncio.gather(
        *(
            check_one(
                incrementor=incrementors[proxy.protocol],
                progress=progress,
                progress_task=progress_tasks[proxy.protocol],
                proxy=proxy,
                settings=settings,
                storage=storage,
            )
            for proxy in storage
        )
    )
