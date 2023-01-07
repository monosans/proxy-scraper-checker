from __future__ import annotations

import asyncio
import logging
import re
import sys
from configparser import ConfigParser
from pathlib import Path
from random import shuffle
from typing import (
    Callable,
    Dict,
    List,
    Optional,
    Set,
    Tuple,
    Type,
    TypeVar,
    Union,
)

from aiohttp import ClientSession, ClientTimeout, DummyCookieJar
from rich.console import Console
from rich.progress import (
    BarColumn,
    MofNCompleteColumn,
    Progress,
    TaskID,
    TextColumn,
)
from rich.table import Table

from . import sort
from .folder import Folder
from .proxy import Proxy

logger = logging.getLogger(__name__)

TProxyScraperChecker = TypeVar(
    "TProxyScraperChecker", bound="ProxyScraperChecker"
)


def validate_max_connections(value: int) -> int:
    if sys.platform != "win32":
        import resource

        soft_limit, hard_limit = resource.getrlimit(resource.RLIMIT_NOFILE)
        if soft_limit < hard_limit:
            resource.setrlimit(
                resource.RLIMIT_NOFILE, (hard_limit, hard_limit)
            )
    elif value > 512 and isinstance(
        asyncio.get_event_loop_policy(), asyncio.WindowsSelectorEventLoopPolicy
    ):
        logger.warning(
            "MaxConnections value is too high. "
            + "Windows supports a maximum of 512. "
            + "The config value will be ignored and 512 will be used."
        )
        return 512
    return value


class ProxyScraperChecker:
    """HTTP, SOCKS4, SOCKS5 proxies scraper and checker."""

    __slots__ = (
        "all_folders",
        "console",
        "cookie_jar",
        "enabled_folders",
        "path",
        "proxies_count",
        "proxies",
        "regex",
        "sem",
        "sort_by_speed",
        "source_timeout",
        "sources",
        "timeout",
    )

    def __init__(
        self,
        *,
        timeout: float,
        source_timeout: float,
        max_connections: int,
        sort_by_speed: bool,
        save_path: str,
        proxies: bool,
        proxies_anonymous: bool,
        proxies_geolocation: bool,
        proxies_geolocation_anonymous: bool,
        http_sources: Optional[str],
        socks4_sources: Optional[str],
        socks5_sources: Optional[str],
        console: Optional[Console] = None,
    ) -> None:
        """HTTP, SOCKS4, SOCKS5 proxies scraper and checker.

        Args:
            timeout: The number of seconds to wait for a proxied request.
                The higher the number, the longer the check will take
                and the more proxies you get.
            source_timeout: The number of seconds to wait for the proxy
                to be downloaded from the source.
            max_connections: Maximum concurrent connections.
                Windows supports maximum of 512.
                On *nix operating systems, this restriction is much looser.
                The limit on *nix can be seen with the command ulimit -Hn.
            sort_by_speed: Set to False to sort proxies alphabetically.
            save_path: Path to the folder where the proxy folders will
                be saved. Leave empty to save the proxies to the current
                directory.
        """
        self.path = Path(save_path)
        folders_mapping = {
            "proxies": proxies,
            "proxies_anonymous": proxies_anonymous,
            "proxies_geolocation": proxies_geolocation,
            "proxies_geolocation_anonymous": proxies_geolocation_anonymous,
        }
        self.all_folders = tuple(
            Folder(path=self.path, folder_name=folder_name)
            for folder_name in folders_mapping
        )
        self.enabled_folders = tuple(
            folder
            for folder in self.all_folders
            if folders_mapping[folder.path.name]
        )
        if not self.enabled_folders:
            raise ValueError("all folders are disabled in the config")

        self.regex = re.compile(
            r"(?:^|\D)?("
            + r"(?:[1-9]|[1-9]\d|1\d{2}|2[0-4]\d|25[0-5])"  # 1-255
            + r"\.(?:\d|[1-9]\d|1\d{2}|2[0-4]\d|25[0-5])" * 3  # 0-255
            + r"):"
            + (
                r"(\d|[1-9]\d{1,3}|[1-5]\d{4}|6[0-4]\d{3}"
                + r"|65[0-4]\d{2}|655[0-2]\d|6553[0-5])"
            )  # 0-65535
            + r"(?:\D|$)"
        )

        self.sort_by_speed = sort_by_speed
        self.timeout = ClientTimeout(total=timeout, sock_connect=timeout)
        self.source_timeout = source_timeout
        self.sources = {
            proto: frozenset(filter(None, sources.splitlines()))
            for proto, sources in (
                ("HTTP", http_sources),
                ("SOCKS4", socks4_sources),
                ("SOCKS5", socks5_sources),
            )
            if sources
        }
        self.proxies: Dict[str, Set[Proxy]] = {
            proto: set() for proto in self.sources
        }
        self.proxies_count = {proto: 0 for proto in self.sources}
        self.cookie_jar = DummyCookieJar()
        self.console = console or Console()

        max_connections = validate_max_connections(max_connections)
        self.sem = asyncio.Semaphore(max_connections)

    @classmethod
    def from_ini(
        cls: Type[TProxyScraperChecker],
        file_name: str,
        *,
        console: Optional[Console] = None,
    ) -> TProxyScraperChecker:
        cfg = ConfigParser(interpolation=None)
        cfg.read(file_name, encoding="utf-8")
        general = cfg["General"]
        folders = cfg["Folders"]
        http = cfg["HTTP"]
        socks4 = cfg["SOCKS4"]
        socks5 = cfg["SOCKS5"]
        return cls(
            timeout=general.getfloat("Timeout", 5),
            source_timeout=general.getfloat("SourceTimeout", 15),
            max_connections=general.getint("MaxConnections", 512),
            sort_by_speed=general.getboolean("SortBySpeed", True),
            save_path=general.get("SavePath", ""),
            proxies=folders.getboolean("proxies", True),
            proxies_anonymous=folders.getboolean("proxies_anonymous", True),
            proxies_geolocation=folders.getboolean(
                "proxies_geolocation", True
            ),
            proxies_geolocation_anonymous=folders.getboolean(
                "proxies_geolocation_anonymous", True
            ),
            http_sources=http.get("Sources")
            if http.getboolean("Enabled", True)
            else None,
            socks4_sources=socks4.get("Sources")
            if socks4.getboolean("Enabled", True)
            else None,
            socks5_sources=socks5.get("Sources")
            if socks5.getboolean("Enabled", True)
            else None,
            console=console,
        )

    async def fetch_source(
        self,
        *,
        session: ClientSession,
        source: str,
        proto: str,
        progress: Progress,
        task: TaskID,
    ) -> None:
        """Get proxies from source.

        Args:
            source: Proxy list URL.
            proto: HTTP/SOCKS4/SOCKS5.
        """
        try:
            async with session.get(source) as response:
                status = response.status
                text = await response.text()
        except Exception as e:
            logger.error("%s | %s | %s", source, e.__class__.__qualname__, e)
        else:
            proxies = tuple(self.regex.finditer(text))
            if proxies:
                for proxy in proxies:
                    proxy_obj = Proxy(
                        host=proxy.group(1), port=int(proxy.group(2))
                    )
                    self.proxies[proto].add(proxy_obj)
            else:
                logger.warning(
                    "%s | No proxies found | HTTP status code %d",
                    source,
                    status,
                )
        progress.update(task, advance=1)

    async def check_proxy(
        self, *, proxy: Proxy, proto: str, progress: Progress, task: TaskID
    ) -> None:
        """Check if proxy is alive."""
        try:
            await proxy.check(
                sem=self.sem,
                cookie_jar=self.cookie_jar,
                proto=proto,
                timeout=self.timeout,
            )
        except Exception as e:
            # Too many open files
            if isinstance(e, OSError) and e.errno == 24:
                logger.error("Please, set MaxConnections to lower value.")

            self.proxies[proto].remove(proxy)
        progress.update(task, advance=1)

    async def fetch_all_sources(self, progress: Progress) -> None:
        tasks = {
            proto: progress.add_task(
                f"[yellow]Scraper [red]:: [green]{proto}", total=len(sources)
            )
            for proto, sources in self.sources.items()
        }
        headers = {
            "User-Agent": (
                "Mozilla/5.0 (Windows NT 10.0; rv:108.0)"
                + " Gecko/20100101 Firefox/108.0"
            )
        }
        async with ClientSession(
            headers=headers,
            cookie_jar=self.cookie_jar,
            timeout=ClientTimeout(total=self.source_timeout),
        ) as session:
            coroutines = (
                self.fetch_source(
                    session=session,
                    source=source,
                    proto=proto,
                    progress=progress,
                    task=tasks[proto],
                )
                for proto, sources in self.sources.items()
                for source in sources
            )
            await asyncio.gather(*coroutines)

        # Remember total count so we could print it in the table
        for proto, proxies in self.proxies.items():
            self.proxies_count[proto] = len(proxies)

    async def check_all_proxies(self, progress: Progress) -> None:
        tasks = {
            proto: progress.add_task(
                f"[yellow]Checker [red]:: [green]{proto}", total=len(proxies)
            )
            for proto, proxies in self.proxies.items()
        }
        coroutines = [
            self.check_proxy(
                proxy=proxy, proto=proto, progress=progress, task=tasks[proto]
            )
            for proto, proxies in self.proxies.items()
            for proxy in proxies
        ]
        shuffle(coroutines)
        await asyncio.gather(*coroutines)

    def save_proxies(self) -> None:
        """Delete old proxies and save new ones."""
        sorted_proxies = self.get_sorted_proxies().items()
        for folder in self.all_folders:
            folder.remove()
        for folder in self.enabled_folders:
            folder.create()
            for proto, proxies in sorted_proxies:
                text = "\n".join(
                    proxy.as_str(include_geolocation=folder.for_geolocation)
                    for proxy in proxies
                    if (proxy.is_anonymous if folder.for_anonymous else True)
                )
                file = folder.path / f"{proto.lower()}.txt"
                file.write_text(text, encoding="utf-8")

    async def run(self) -> None:
        with self._get_progress_bar() as progress:
            await self.fetch_all_sources(progress)
            await self.check_all_proxies(progress)

        table = self._get_results_table()
        self.console.print(table)

        self.save_proxies()
        logger.info(
            "Proxy folders have been created in the %s folder."
            + "\nThank you for using proxy-scraper-checker :)",
            self.path.resolve(),
        )

    def get_sorted_proxies(self) -> Dict[str, List[Proxy]]:
        key: Union[
            Callable[[Proxy], float], Callable[[Proxy], Tuple[int, ...]]
        ] = (
            sort.timeout_sort_key
            if self.sort_by_speed
            else sort.natural_sort_key
        )
        return {
            proto: sorted(proxies, key=key)
            for proto, proxies in self.proxies.items()
        }

    def _get_results_table(self) -> Table:
        table = Table()
        table.add_column("Protocol", style="cyan")
        table.add_column("Working", style="magenta")
        table.add_column("Total", style="green")
        for proto, proxies in self.proxies.items():
            working = len(proxies)
            total = self.proxies_count[proto]
            percentage = working / total if total else 0
            table.add_row(proto, f"{working} ({percentage:.1%})", str(total))
        return table

    def _get_progress_bar(self) -> Progress:
        return Progress(
            TextColumn("[progress.description]{task.description}"),
            BarColumn(),
            MofNCompleteColumn(),
            console=self.console,
        )
