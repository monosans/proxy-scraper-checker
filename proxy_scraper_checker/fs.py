from __future__ import annotations

import asyncio
import logging
from typing import TYPE_CHECKING

import platformdirs

if TYPE_CHECKING:
    from pathlib import Path

_logger = logging.getLogger(__name__)
CACHE_PATH = platformdirs.user_cache_path("proxy_scraper_checker")


async def add_permission(
    path: Path, permission: int, /, *, missing_ok: bool = False
) -> None:
    try:
        current_permissions = (await asyncio.to_thread(path.stat)).st_mode
        new_permissions = current_permissions | permission
        if current_permissions != new_permissions:
            await asyncio.to_thread(path.chmod, new_permissions)
            _logger.info(
                "Changed permissions of %s from %o to %o",
                path,
                current_permissions,
                new_permissions,
            )
    except FileNotFoundError:
        if not missing_ok:
            raise


async def create_or_fix_dir(path: Path, /, *, permission: int) -> None:
    try:
        await asyncio.to_thread(path.mkdir, parents=True)
    except FileExistsError:
        if not await asyncio.to_thread(path.is_dir):
            msg = f"{path} is not a directory"
            raise ValueError(msg) from None
        await add_permission(path, permission)
