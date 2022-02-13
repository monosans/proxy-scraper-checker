#!/usr/bin/env python3
# -*- coding: utf-8 -*-
import asyncio
import re
from pathlib import Path
from random import shuffle
from shutil import rmtree
from time import perf_counter
from typing import Callable, Dict, Iterable, List, Optional, Set, Tuple, Union

from aiohttp import ClientSession
from aiohttp_socks import ProxyConnector
from rich.console import Console
from rich.progress import (
    BarColumn,
    Progress,
    TaskID,
    TextColumn,
    TimeRemainingColumn,
)
from rich.table import Table

import config


class Proxy:
    def __init__(self, socket_address: str, ip: str) -> None:
        self.socket_address = socket_address
        self.ip = ip
        self.is_anonymous: Optional[bool] = None
        self.geolocation: str = "::None::None::None"
        self.timeout = float("inf")

    def update(self, info: Dict[str, str]) -> None:
        country = info.get("country") or None
        region = info.get("regionName") or None
        city = info.get("city") or None
        self.geolocation = f"::{country}::{region}::{city}"
        self.is_anonymous = self.ip != info.get("query")

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, Proxy):
            return NotImplemented
        return self.socket_address == other.socket_address

    def __hash__(self) -> int:
        return hash(("socket_address", self.socket_address))


class Folder:
    def __init__(self, folder_name: str, path: Path) -> None:
        self.folder = folder_name
        self.path = path / folder_name
        self.for_anonymous = "anon" in folder_name
        self.for_geolocation = "geo" in folder_name

    def remove(self) -> None:
        try:
            rmtree(self.path)
        except FileNotFoundError:
            pass

    def create(self) -> None:
        self.path.mkdir(parents=True, exist_ok=True)


class ProxyScraperChecker:
    """HTTP, SOCKS4, SOCKS5 proxies scraper and checker."""

    def __init__(
        self,
        *,
        timeout: float,
        max_connections: int,
        sort_by_speed: bool,
        save_path: str,
        proxies: bool,
        proxies_anonymous: bool,
        proxies_geolocation: bool,
        proxies_geolocation_anonymous: bool,
        http_sources: Optional[Iterable[str]],
        socks4_sources: Optional[Iterable[str]],
        socks5_sources: Optional[Iterable[str]],
        console: Optional[Console] = None,
    ) -> None:
        """
        Args:
            timeout (float): How many seconds to wait for the connection.
            max_connections (int): Maximum concurrent connections.
            sort_by_speed (bool): Set to False to sort proxies alphabetically.
            save_path (str): Path to the folder where the proxy folders will be
                saved.
        """
        path = Path(save_path)
        folders = (
            ("proxies", proxies),
            ("proxies_anonymous", proxies_anonymous),
            ("proxies_geolocation", proxies_geolocation),
            ("proxies_geolocation_anonymous", proxies_geolocation_anonymous),
        )
        self.folders = [
            Folder(folder, path) for folder, enabled in folders if enabled
        ]
        if not self.folders:
            raise ValueError("all folders are disabled in the config")

        octet = r"(?:\d|[1-9]\d|1\d{2}|2[0-4]\d|25[0-5])"
        port = (
            r"(?:\d|[1-9]\d{1,3}|[1-5]\d{4}|6[0-4]\d{3}"
            + r"|65[0-4]\d{2}|655[0-2]\d|6553[0-5])"
        )
        self.regex = re.compile(
            rf"(?:^|\D)(({octet}\.{octet}\.{octet}\.{octet}):{port})(?:\D|$)"
        )

        self.sort_by_speed = sort_by_speed
        self.timeout = timeout
        self.path = save_path
        self.sources = {
            proto: (sources,)
            if isinstance(sources, str)
            else frozenset(sources)
            for proto, sources in (
                ("http", http_sources),
                ("socks4", socks4_sources),
                ("socks5", socks5_sources),
            )
            if sources
        }
        self.proxies: Dict[str, Set[Proxy]] = {
            proto: set() for proto in self.sources
        }
        self.proxies_count = {proto: 0 for proto in self.sources}
        self.c = console or Console()
        self.sem = asyncio.Semaphore(max_connections)

    async def fetch_source(
        self,
        session: ClientSession,
        source: str,
        proto: str,
        progress: Progress,
        task: TaskID,
    ) -> None:
        """Get proxies from source.

        Args:
            source (str): Proxy list URL.
            proto (str): http/socks4/socks5.
        """
        try:
            async with session.get(source.strip(), timeout=15) as r:
                text = await r.text(encoding="utf-8")
        except Exception as e:
            self.c.print(f"{source}: {e}")
        else:
            for proxy in self.regex.finditer(text):
                self.proxies[proto].add(Proxy(proxy.group(1), proxy.group(2)))
        progress.update(task, advance=1)

    async def check_proxy(
        self, proxy: Proxy, proto: str, progress: Progress, task: TaskID
    ) -> None:
        """Check if proxy is alive."""
        try:
            async with self.sem:
                start = perf_counter()
                async with ClientSession(
                    connector=ProxyConnector.from_url(
                        f"{proto}://{proxy.socket_address}"
                    )
                ) as session:
                    async with session.get(
                        "http://ip-api.com/json/", timeout=self.timeout
                    ) as r:
                        res = (
                            None
                            if r.status in {403, 404, 429}
                            else await r.json()
                        )
        except Exception as e:
            # Too many open files
            if isinstance(e, OSError) and e.errno == 24:
                self.c.print(
                    "[red]Please, set MAX_CONNECTIONS to lower value."
                )

            self.proxies[proto].remove(proxy)
        else:
            proxy.timeout = perf_counter() - start
            if res:
                proxy.update(res)
        progress.update(task, advance=1)

    async def fetch_all_sources(self) -> None:
        with self._progress as progress:
            tasks = {
                proto: progress.add_task(
                    f"[yellow]Scraper [red]:: [green]{proto.upper()}",
                    total=len(sources),
                )
                for proto, sources in self.sources.items()
            }
            async with ClientSession() as session:
                coroutines = (
                    self.fetch_source(
                        session, source, proto, progress, tasks[proto]
                    )
                    for proto, sources in self.sources.items()
                    for source in sources
                )
                await asyncio.gather(*coroutines)

        # Remember total count so we could print it in the table
        for proto, proxies in self.proxies.items():
            self.proxies_count[proto] = len(proxies)

    async def check_all_proxies(self) -> None:
        with self._progress as progress:
            tasks = {
                proto: progress.add_task(
                    f"[yellow]Checker [red]:: [green]{proto.upper()}",
                    total=len(proxies),
                )
                for proto, proxies in self.proxies.items()
            }
            coroutines = [
                self.check_proxy(proxy, proto, progress, tasks[proto])
                for proto, proxies in self.proxies.items()
                for proxy in proxies
            ]
            shuffle(coroutines)
            await asyncio.gather(*coroutines)

    def save_proxies(self) -> None:
        """Delete old proxies and save new ones."""
        sorted_proxies = self.sorted_proxies.items()
        for folder in self.folders:
            folder.remove()
            folder.create()
            for proto, proxies in sorted_proxies:
                text = "\n".join(
                    "{}{}".format(
                        proxy.socket_address,
                        proxy.geolocation if folder.for_geolocation else "",
                    )
                    for proxy in proxies
                    if (proxy.is_anonymous if folder.for_anonymous else True)
                )
                (folder.path / f"{proto}.txt").write_text(
                    text, encoding="utf-8"
                )

    async def main(self) -> None:
        await self.fetch_all_sources()
        await self.check_all_proxies()

        table = Table()
        table.add_column("Protocol", style="cyan")
        table.add_column("Working", style="magenta")
        table.add_column("Total", style="green")
        for proto, proxies in self.proxies.items():
            working = len(proxies)
            total = self.proxies_count[proto]
            percentage = working / total * 100 if total else 0
            table.add_row(
                proto.upper(), f"{working} ({percentage:.1f}%)", str(total)
            )
        self.c.print(table)

        self.save_proxies()
        self.c.print(
            "[green]Proxy folders have been created in the "
            + (f"{self.path} folder" if self.path else "current directory")
            + ".\nThank you for using proxy-scraper-checker :)"
        )

    @property
    def sorted_proxies(self) -> Dict[str, List[Proxy]]:
        key = self._sorting_key
        return {
            proto: sorted(proxies, key=key)
            for proto, proxies in self.proxies.items()
        }

    @property
    def _sorting_key(
        self,
    ) -> Union[Callable[[Proxy], float], Callable[[Proxy], Tuple[int, ...]]]:
        if self.sort_by_speed:
            return lambda proxy: proxy.timeout
        return lambda proxy: tuple(
            map(int, proxy.socket_address.replace(":", ".").split("."))
        )

    @property
    def _progress(self) -> Progress:
        return Progress(
            TextColumn("[progress.description]{task.description}"),
            BarColumn(),
            TextColumn("[progress.percentage]{task.percentage:3.0f}%"),
            TextColumn("[blue][{task.completed}/{task.total}]"),
            TimeRemainingColumn(),
            console=self.c,
        )


async def main() -> None:
    await ProxyScraperChecker(
        timeout=config.TIMEOUT,
        max_connections=config.MAX_CONNECTIONS,
        sort_by_speed=config.SORT_BY_SPEED,
        save_path=config.SAVE_PATH,
        proxies=config.PROXIES,
        proxies_anonymous=config.PROXIES_ANONYMOUS,
        proxies_geolocation=config.PROXIES_GEOLOCATION,
        proxies_geolocation_anonymous=config.PROXIES_GEOLOCATION_ANONYMOUS,
        http_sources=config.HTTP_SOURCES
        if config.HTTP and config.HTTP_SOURCES
        else None,
        socks4_sources=config.SOCKS4_SOURCES
        if config.SOCKS4 and config.SOCKS4_SOURCES
        else None,
        socks5_sources=config.SOCKS5_SOURCES
        if config.SOCKS5 and config.SOCKS5_SOURCES
        else None,
    ).main()


if __name__ == "__main__":
    asyncio.run(main())
