from __future__ import annotations

import logging
from pathlib import Path

import platformdirs

logger = logging.getLogger(__name__)
CACHE_PATH = platformdirs.user_cache_path("proxy_scraper_checker")


def add_permission(path: Path, permission: int, /) -> None:
    current_permissions = path.stat().st_mode
    new_permissions = current_permissions | permission
    if current_permissions != new_permissions:
        path.chmod(new_permissions)
        logger.info(
            "Changed permissions of %s from %o to %o",
            path,
            current_permissions,
            new_permissions,
        )


def maybe_add_permission(path: Path, permission: int, /) -> None:
    try:
        add_permission(path, permission)
    except FileNotFoundError:
        pass


def create_or_fix_dir(path: Path, /, *, permissions: int) -> None:
    try:
        path.mkdir(parents=True)
    except FileExistsError:
        if not path.is_dir():
            msg = f"{path} is not a directory"
            raise ValueError(msg) from None
        add_permission(path, permissions)
