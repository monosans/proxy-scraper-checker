from __future__ import annotations

import json
import logging
from shutil import rmtree
from typing import Sequence, Union

import aiofiles
import aiofiles.os
import aiofiles.ospath
import maxminddb

from . import sort
from .geodb import GEODB_PATH
from .null_context import NullContext
from .proxy import Proxy
from .settings import Settings
from .storage import ProxyStorage
from .utils import IS_DOCKER

logger = logging.getLogger(__name__)


def _create_proxy_list_str(
    *, anonymous_only: bool, include_protocol: bool, proxies: Sequence[Proxy]
) -> str:
    return "\n".join(
        proxy.as_str(include_protocol=include_protocol)
        for proxy in proxies
        if not anonymous_only
        or (proxy.exit_ip is not None and proxy.host != proxy.exit_ip)
    )


@aiofiles.ospath.wrap
def save_proxies(*, settings: Settings, storage: ProxyStorage) -> None:
    if settings.output_json:
        mmdb: Union[maxminddb.Reader, NullContext] = (
            maxminddb.open_database(GEODB_PATH)
            if settings.enable_geolocation
            else NullContext()
        )
        settings.output_path.mkdir(parents=True, exist_ok=True)
        with mmdb as mmdb_reader:
            proxy_dicts = [
                {
                    "protocol": proxy.protocol.name.lower(),
                    "username": proxy.username,
                    "password": proxy.password,
                    "host": proxy.host,
                    "port": proxy.port,
                    "exit_ip": proxy.exit_ip,
                    "timeout": round(proxy.timeout, 2),
                    "geolocation": mmdb_reader.get(proxy.exit_ip)
                    if mmdb_reader is not None and proxy.exit_ip is not None
                    else None,
                }
                for proxy in sorted(storage, key=sort.timeout_sort_key)
            ]
            with (settings.output_path / "proxies.json").open(
                "w", encoding="utf-8"
            ) as f:
                json.dump(
                    proxy_dicts, f, ensure_ascii=False, separators=(",", ":")
                )
            with (settings.output_path / "proxies_pretty.json").open(
                "w", encoding="utf-8"
            ) as f:
                json.dump(proxy_dicts, f, ensure_ascii=False, indent="\t")
    if settings.output_txt:
        sorted_proxies = sorted(storage, key=settings.sorting_key)
        grouped_proxies = tuple(
            (k, sorted(v, key=settings.sorting_key))
            for k, v in storage.get_grouped().items()
        )
        for folder, anonymous_only in (
            (settings.output_path / "proxies", False),
            (settings.output_path / "proxies_anonymous", True),
        ):
            try:
                rmtree(folder)
            except FileNotFoundError:
                pass
            folder.mkdir(parents=True, exist_ok=True)
            text = _create_proxy_list_str(
                proxies=sorted_proxies,
                anonymous_only=anonymous_only,
                include_protocol=True,
            )
            (folder / "all.txt").write_text(text, encoding="utf-8")
            for proto, proxies in grouped_proxies:
                text = _create_proxy_list_str(
                    proxies=proxies,
                    anonymous_only=anonymous_only,
                    include_protocol=False,
                )
                (folder / f"{proto.name.lower()}.txt").write_text(
                    text, encoding="utf-8"
                )
    if IS_DOCKER:
        logger.info(
            "Proxies have been saved to ./out (%s in container)",
            settings.output_path.absolute(),
        )
    else:
        logger.info(
            "Proxies have been saved to %s", settings.output_path.absolute()
        )
