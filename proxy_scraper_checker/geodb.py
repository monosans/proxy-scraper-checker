from __future__ import annotations

import logging
import stat
from typing import TYPE_CHECKING

import aiofiles
from aiohttp import hdrs

from . import fs
from .utils import IS_DOCKER, asyncify, bytes_decode

if TYPE_CHECKING:
    import asyncio

    from aiohttp import ClientResponse, ClientSession
    from rich.progress import Progress, TaskID

logger = logging.getLogger(__name__)

GEODB_URL = "https://raw.githubusercontent.com/P3TERX/GeoLite.mmdb/download/GeoLite2-City.mmdb"
GEODB_PATH = fs.CACHE_PATH / "geolocation_database.mmdb"
GEODB_ETAG_PATH = GEODB_PATH.with_suffix(".mmdb.etag")


async def _read_etag() -> str | None:
    try:
        await fs.async_add_permission(GEODB_ETAG_PATH, stat.S_IRUSR)
        async with aiofiles.open(GEODB_ETAG_PATH, "rb") as etag_file:
            content = await etag_file.read()
    except FileNotFoundError:
        return None
    return bytes_decode(content)


def _remove_etag() -> asyncio.Future[None]:
    return asyncify(GEODB_ETAG_PATH.unlink)(missing_ok=True)


async def _save_etag(etag: str, /) -> None:
    await fs.async_add_permission(
        GEODB_ETAG_PATH, stat.S_IWUSR, missing_ok=True
    )
    async with aiofiles.open(
        GEODB_ETAG_PATH, "w", encoding="utf-8"
    ) as etag_file:
        await etag_file.write(etag)


async def _save_geodb(
    *, progress: Progress, response: ClientResponse, task: TaskID
) -> None:
    await fs.async_add_permission(GEODB_PATH, stat.S_IWUSR, missing_ok=True)
    async with aiofiles.open(GEODB_PATH, "wb") as geodb:
        async for chunk in response.content.iter_any():
            await geodb.write(chunk)
            progress.advance(task_id=task, advance=len(chunk))


async def download_geodb(*, progress: Progress, session: ClientSession) -> None:
    headers = (
        {hdrs.IF_NONE_MATCH: current_etag}
        if await asyncify(GEODB_PATH.exists)()
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

    if IS_DOCKER:
        logger.info(
            "Downloaded geolocation database to proxy_scraper_checker_cache "
            "Docker volume (%s in container)",
            GEODB_PATH,
        )
    else:
        logger.info("Downloaded geolocation database to %s", GEODB_PATH)

    if etag := response.headers.get(hdrs.ETAG):
        await _save_etag(etag)
    else:
        await _remove_etag()
