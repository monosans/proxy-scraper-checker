from __future__ import annotations

import asyncio
import logging
import stat
from shutil import rmtree
from typing import TYPE_CHECKING

import maxminddb
import orjson

from proxy_scraper_checker import fs, sort
from proxy_scraper_checker.geodb import GEODB_PATH
from proxy_scraper_checker.utils import is_docker

if TYPE_CHECKING:
    from collections.abc import Sequence

    from proxy_scraper_checker.proxy import Proxy
    from proxy_scraper_checker.settings import Settings
    from proxy_scraper_checker.storage import ProxyStorage

_logger = logging.getLogger(__name__)


def _create_proxy_list_str(
    *, anonymous_only: bool, include_protocol: bool, proxies: Sequence[Proxy]
) -> str:
    return "\n".join(
        proxy.as_str(include_protocol=include_protocol)
        for proxy in proxies
        if not anonymous_only
        or (proxy.exit_ip is not None and proxy.host != proxy.exit_ip)
    )


async def save_proxies(*, settings: Settings, storage: ProxyStorage) -> None:
    if settings.output_json:
        if settings.enable_geolocation:
            await fs.add_permission(GEODB_PATH, stat.S_IRUSR)
            mmdb: maxminddb.Reader | None = await asyncio.to_thread(
                maxminddb.open_database, GEODB_PATH
            )
        else:
            mmdb = None
        try:
            proxy_dicts = [
                {
                    "protocol": proxy.protocol.name.lower(),
                    "username": proxy.username,
                    "password": proxy.password,
                    "host": proxy.host,
                    "port": proxy.port,
                    "exit_ip": proxy.exit_ip,
                    "timeout": round(proxy.timeout, 2)
                    if proxy.timeout is not None
                    else None,
                    "geolocation": await asyncio.to_thread(
                        mmdb.get, proxy.exit_ip
                    )
                    if mmdb is not None and proxy.exit_ip is not None
                    else None,
                }
                for proxy in sorted(storage, key=sort.timeout_sort_key)
            ]
            for path, orjson_option in (
                (settings.output_path / "proxies.json", orjson.OPT_SORT_KEYS),
                (
                    settings.output_path / "proxies_pretty.json",
                    orjson.OPT_INDENT_2 | orjson.OPT_SORT_KEYS,
                ),
            ):
                await asyncio.to_thread(path.unlink, missing_ok=True)
                await asyncio.to_thread(
                    path.write_bytes,
                    orjson.dumps(proxy_dicts, option=orjson_option),
                )
        finally:
            if mmdb is not None:
                await asyncio.to_thread(mmdb.close)
    if settings.output_txt:
        sorted_proxies = sorted(storage, key=settings.get_sorting_key())
        grouped_proxies = tuple(
            (k, sorted(v, key=settings.get_sorting_key()))
            for k, v in storage.get_grouped().items()
        )
        for folder, anonymous_only in (
            (settings.output_path / "proxies", False),
            (settings.output_path / "proxies_anonymous", True),
        ):
            try:
                await asyncio.to_thread(rmtree, folder)
            except FileNotFoundError:
                pass
            await asyncio.to_thread(folder.mkdir)
            text = _create_proxy_list_str(
                proxies=sorted_proxies,
                anonymous_only=anonymous_only,
                include_protocol=True,
            )
            await asyncio.to_thread(
                (folder / "all.txt").write_text, text, encoding="utf-8"
            )
            for proto, proxies in grouped_proxies:
                text = _create_proxy_list_str(
                    proxies=proxies,
                    anonymous_only=anonymous_only,
                    include_protocol=False,
                )
                await asyncio.to_thread(
                    (folder / f"{proto.name.lower()}.txt").write_text,
                    text,
                    encoding="utf-8",
                )
    if await asyncio.to_thread(is_docker):
        _logger.info(
            "Proxies have been saved to ./out (%s in container)",
            await asyncio.to_thread(settings.output_path.absolute),
        )
    else:
        _logger.info(
            "Proxies have been saved to %s",
            await asyncio.to_thread(settings.output_path.absolute),
        )
