from __future__ import annotations

import asyncio

import aiofiles.os
import platformdirs

DIR = platformdirs.user_cache_dir("proxy_scraper_checker")
READY_EVENT = asyncio.Event()


async def create() -> None:
    await aiofiles.os.makedirs(DIR, exist_ok=True)
    READY_EVENT.set()
