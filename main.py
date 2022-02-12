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
        self.SOCKET_ADDRESS = socket_address
        self.IP = ip
        self.is_anonymous: Optional[bool] = None
        self.geolocation: str = "::None::None::None"
        self.timeout = float("inf")

    def update(self, info: Dict[str, str]) -> None:
        country = info.get("country") or None
        region = info.get("regionName") or None
        city = info.get("city") or None
        self.geolocation = f"::{country}::{region}::{city}"
        self.is_anonymous = self.IP != info.get("query")

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, Proxy):
            return NotImplemented
        return self.SOCKET_ADDRESS == other.SOCKET_ADDRESS

    def __hash__(self) -> int:
        return hash(("socket_address", self.SOCKET_ADDRESS))


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
            geolocation (bool): Add geolocation info for each proxy.
            anonymous (bool): Check if proxies are anonymous.
            save_path (str): Path to the folder where the proxy folders will be
                saved.
        """
        octet = r"(?:\d|[1-9]\d|1\d{2}|2[0-4]\d|25[0-5])"
        port = (
            r"(?:\d|[1-9]\d{1,3}|[1-5]\d{4}|6[0-4]\d{3}"
            + r"|65[0-4]\d{2}|655[0-2]\d|6553[0-5])"
        )
        self.REGEX = re.compile(
            rf"(?:^|\D)(({octet}\.{octet}\.{octet}\.{octet}):{port})(?:\D|$)"
        )
        self.sem = asyncio.Semaphore(max_connections)
        self.SORT_BY_SPEED = sort_by_speed
        self.FOLDERS = {
            "proxies": proxies,
            "proxies_anonymous": proxies_anonymous,
            "proxies_geolocation": proxies_geolocation,
            "proxies_geolocation_anonymous": proxies_geolocation_anonymous,
        }
        self.TIMEOUT = timeout
        self.PATH = save_path
        self.SOURCES = {
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
            proto: set() for proto in self.SOURCES
        }
        self.proxies_count = {proto: 0 for proto in self.SOURCES}
        self.c = console or Console()

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
            for proxy in self.REGEX.finditer(text):
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
                        f"{proto}://{proxy.SOCKET_ADDRESS}"
                    )
                ) as session:
                    async with session.get(
                        "http://ip-api.com/json/", timeout=self.TIMEOUT
                    ) as r:
                        res = (
                            None
                            if r.status in {400, 403, 404, 429, 503}
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
                for proto, sources in self.SOURCES.items()
            }
            async with ClientSession() as session:
                coroutines = (
                    self.fetch_source(
                        session, source, proto, progress, tasks[proto]
                    )
                    for proto, sources in self.SOURCES.items()
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
        path = Path(self.PATH)
        dirs = tuple(path / dir for dir in self.FOLDERS)
        for dir in dirs:
            try:
                rmtree(dir)
            except FileNotFoundError:
                pass
        sorted_proxies = self.sorted_proxies.items()
        for dir, folder in zip(dirs, self.FOLDERS):
            if not self.FOLDERS[folder]:
                continue
            dir.mkdir(parents=True, exist_ok=True)
            for proto, proxies in sorted_proxies:
                text = "\n".join(
                    "{}{}".format(
                        proxy.SOCKET_ADDRESS,
                        proxy.geolocation if "geolocation" in folder else "",
                    )
                    for proxy in proxies
                    if (proxy.is_anonymous if "anonymous" in folder else True)
                )
                (dir / f"{proto}.txt").write_text(text, encoding="utf-8")

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
            + (f"{self.PATH} folder" if self.PATH else "current directory")
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
    def _sorting_key(self) -> Callable[[Proxy], Union[float, Tuple[int, ...]]]:
        if self.SORT_BY_SPEED:
            return lambda proxy: proxy.timeout
        return lambda proxy: tuple(
            map(int, proxy.SOCKET_ADDRESS.replace(":", ".").split("."))
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
    if not (
        config.PROXIES
        or config.PROXIES_ANONYMOUS
        or config.PROXIES_GEOLOCATION
        or config.PROXIES_GEOLOCATION_ANONYMOUS
    ):
        raise ValueError("all folders are disabled in the config")
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
