from __future__ import annotations

import asyncio
import logging
import stat
from typing import TYPE_CHECKING

from aiohttp import hdrs

from proxy_scraper_checker import fs
from proxy_scraper_checker.utils import bytes_decode, is_docker

if TYPE_CHECKING:
    from aiohttp import ClientResponse, ClientSession
    from rich.progress import Progress, TaskID

_logger = logging.getLogger(__name__)

GEODB_URL = "https://raw.githubusercontent.com/P3TERX/GeoLite.mmdb/download/GeoLite2-City.mmdb"
GEODB_PATH = fs.CACHE_PATH / "geolocation_database.mmdb"
GEODB_ETAG_PATH = GEODB_PATH.with_suffix(".mmdb.etag")


async def _read_etag() -> str | None:
    try:
        await fs.add_permission(GEODB_ETAG_PATH, stat.S_IRUSR)
        content = await asyncio.to_thread(GEODB_ETAG_PATH.read_bytes)
    except FileNotFoundError:
        return None
    return bytes_decode(content)


async def _remove_etag() -> None:
    return await asyncio.to_thread(GEODB_ETAG_PATH.unlink, missing_ok=True)


async def _save_etag(etag: str, /) -> None:
    await fs.add_permission(GEODB_ETAG_PATH, stat.S_IWUSR, missing_ok=True)
    await asyncio.to_thread(GEODB_ETAG_PATH.write_text, etag, encoding="utf-8")


async def _save_geodb(
    *, progress: Progress, response: ClientResponse, task: TaskID
) -> None:
    await fs.add_permission(GEODB_PATH, stat.S_IWUSR, missing_ok=True)
    geodb = await asyncio.to_thread(GEODB_PATH.open, "wb")
    try:
        async for chunk in response.content.iter_any():
            await asyncio.to_thread(geodb.write, chunk)
            progress.advance(task_id=task, advance=len(chunk))
    finally:
        await asyncio.to_thread(geodb.close)
    progress.update(task_id=task, successful_count="\N{CHECK MARK}")


async def download_geodb(*, progress: Progress, session: ClientSession) -> None:
    headers = (
        {hdrs.IF_NONE_MATCH: current_etag}
        if await asyncio.to_thread(GEODB_PATH.exists)
        and (current_etag := await _read_etag())
        else None
    )

    async with session.get(GEODB_URL, headers=headers) as response:
        if response.status == 304:  # noqa: PLR2004
            _logger.info(
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
                module="Downloader",
                protocol="GeoDB",
                successful_count="\N{HORIZONTAL ELLIPSIS}",
            ),
        )

    if await asyncio.to_thread(is_docker):
        _logger.info(
            "Downloaded geolocation database to proxy_scraper_checker_cache "
            "Docker volume (%s in container)",
            GEODB_PATH,
        )
    else:
        _logger.info("Downloaded geolocation database to %s", GEODB_PATH)

    if etag := response.headers.get(hdrs.ETAG):
        await _save_etag(etag)
    else:
        await _remove_etag()
