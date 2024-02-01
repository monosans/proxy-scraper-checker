from __future__ import annotations

import logging
from pathlib import Path
from typing import Optional

import aiofiles
import aiofiles.ospath
from aiohttp import ClientResponse, ClientSession, hdrs
from rich.progress import Progress, TaskID

from . import cache
from .utils import IS_DOCKER, bytes_decode

logger = logging.getLogger(__name__)

GEODB_URL = "https://raw.githubusercontent.com/P3TERX/GeoLite.mmdb/download/GeoLite2-City.mmdb"
GEODB_PATH = Path(cache.DIR, "geolocation_database.mmdb")
GEODB_ETAG_PATH = GEODB_PATH.with_suffix(".mmdb.etag")


async def _read_etag() -> Optional[str]:
    try:
        async with aiofiles.open(GEODB_ETAG_PATH, "rb") as etag_file:
            content = await etag_file.read()
    except FileNotFoundError:
        return None
    return bytes_decode(content)


async def _save_etag(etag: str, /) -> None:
    async with aiofiles.open(
        GEODB_ETAG_PATH, "w", encoding="utf-8"
    ) as etag_file:
        await etag_file.write(etag)


async def _save_geodb(
    *, progress: Progress, response: ClientResponse, task: TaskID
) -> None:
    async with aiofiles.open(GEODB_PATH, "wb") as geodb:
        async for chunk in response.content.iter_any():
            await geodb.write(chunk)
            progress.advance(task_id=task, advance=len(chunk))


async def download_geodb(*, progress: Progress, session: ClientSession) -> None:
    headers = (
        {hdrs.IF_NONE_MATCH: current_etag}
        if await aiofiles.ospath.exists(GEODB_PATH)
        and (current_etag := await _read_etag())
        else None
    )

    async with session.get(GEODB_URL, headers=headers) as response:
        if response.status == 304:  # noqa: PLR2004
            logger.info(
                "Latest geolocation database is already cached at %s",
                GEODB_PATH,
            )
            return
        await cache.READY_EVENT.wait()
        await _save_geodb(
            progress=progress,
            response=response,
            task=progress.add_task(
                description="",
                total=response.content_length,
                col1="Downloader",
                col2="GeoDB",
            ),
        )
    logger.info(
        "Downloaded geolocation database to %s",
        "proxy_scraper_checker_cache Docker volume"
        if IS_DOCKER
        else GEODB_PATH,
    )

    if etag := response.headers.get(hdrs.ETAG):
        await _save_etag(etag)
