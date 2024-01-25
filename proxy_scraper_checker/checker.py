from __future__ import annotations

import asyncio
import logging
from typing import Mapping

from aiohttp_socks import ProxyType
from rich.progress import Progress, TaskID

from . import sort
from .proxy import Proxy
from .settings import Settings
from .storage import ProxyStorage

logger = logging.getLogger(__name__)


async def check_one(
    *,
    progress: Progress,
    proxy: Proxy,
    settings: Settings,
    storage: ProxyStorage,
    task: TaskID,
) -> None:
    try:
        await proxy.check(settings=settings)
    except Exception as e:
        # Too many open files
        if isinstance(e, OSError) and e.errno == 24:  # noqa: PLR2004
            logger.error("Please, set max_connections to lower value")

        logger.debug(
            "%s.%s: %s", e.__class__.__module__, e.__class__.__qualname__, e
        )
        storage.remove(proxy)
    progress.update(task, advance=1)


async def check_all(
    *,
    settings: Settings,
    storage: ProxyStorage,
    progress: Progress,
    proxies_count: Mapping[ProxyType, int],
) -> None:
    tasks = {
        proto: progress.add_task(
            f"[yellow]Checker [red]:: [green]{proto.name}",
            total=proxies_count[proto],
        )
        for proto in sort.PROTOCOL_ORDER
        if proto in storage.enabled_protocols
    }
    await asyncio.gather(
        *(
            check_one(
                progress=progress,
                proxy=proxy,
                settings=settings,
                storage=storage,
                task=tasks[proxy.protocol],
            )
            for proxy in storage
        )
    )
