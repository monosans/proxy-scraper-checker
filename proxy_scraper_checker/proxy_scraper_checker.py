from __future__ import annotations

import asyncio
import logging
import re
from configparser import ConfigParser
from pathlib import Path
from random import shuffle
from typing import (
    Callable,
    Dict,
    FrozenSet,
    Iterable,
    List,
    Optional,
    Set,
    Tuple,
    Union,
)

from aiohttp import ClientSession, ClientTimeout, DummyCookieJar
from aiohttp_socks import ProxyType
from rich.console import Console
from rich.progress import (
    BarColumn,
    MofNCompleteColumn,
    Progress,
    TaskID,
    TextColumn,
)
from rich.table import Table
from typing_extensions import Self

from . import sort, validators
from .constants import DEFAULT_CHECK_WEBSITE, HEADERS
from .folder import Folder
from .null_context import AsyncNullContext
from .proxy import Proxy

logger = logging.getLogger(__name__)


class ProxyScraperChecker:
    """HTTP, SOCKS4, SOCKS5 proxies scraper and checker."""

    __slots__ = (
        "check_website",
        "console",
        "cookie_jar",
        "folders",
        "geolocation_enabled",
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
        check_website: str,
        sort_by_speed: bool,
        save_path: Path,
        folders: Iterable[Folder],
        sources: Dict[ProxyType, Optional[str]],
        console: Optional[Console] = None,
    ) -> None:
        """HTTP, SOCKS4, SOCKS5 proxies scraper and checker.

        Args:
            timeout: The number of seconds to wait for a proxied request.
                The higher the number, the longer the check will take and the
                more proxies you get.
            source_timeout: The number of seconds to wait for the proxies to be
                downloaded from the source.
            max_connections: Maximum concurrent connections.
                Windows supports maximum of 512.
                On *nix operating systems, this restriction is much looser.
                The limit on *nix can be seen with the command `ulimit -Hn`.
                Don't be in a hurry to set high values.
                Make sure you have enough RAM first, gradually increasing the
                default value.
                If set to 0, the maximum value available for your OS will be
                used.
            check_website: URL to which to send a request to check the proxy.
                If not equal to 'default', it will not be possible to determine
                the anonymity and geolocation of the proxies.
            sort_by_speed: Set to False to sort proxies alphabetically.
            save_path: Path to the folder where the proxy folders will be
                saved.
                Leave empty to save the proxies to the current directory.
        """
        validators.timeout(timeout)
        self.timeout = ClientTimeout(total=timeout, sock_connect=float("inf"))

        validators.source_timeout(source_timeout)
        self.source_timeout = source_timeout

        max_conn = validators.max_connections(max_connections)
        self.sem: Union[asyncio.Semaphore, AsyncNullContext] = (
            asyncio.Semaphore(max_conn) if max_conn else AsyncNullContext()
        )

        self.check_website = check_website
        self.sort_by_speed = sort_by_speed
        self.path = save_path
        self.folders = folders

        if self.check_website == "default":
            self.check_website = DEFAULT_CHECK_WEBSITE

        if self.check_website == DEFAULT_CHECK_WEBSITE:
            validators.folders(self.folders)
        else:
            validators.check_website(check_website)
            logger.info(
                "CheckWebsite is not 'default', "
                "so it will not be possible to determine "
                "the anonymity and geolocation of the proxies"
            )
            for folder in self.folders:
                folder.is_enabled = (
                    not folder.for_anonymous and not folder.for_geolocation
                )

        self.geolocation_enabled = any(
            self.check_website == DEFAULT_CHECK_WEBSITE
            and folder.is_enabled
            and folder.for_geolocation
            for folder in self.folders
        )

        self.sources: Dict[ProxyType, FrozenSet[str]] = {
            proto: frozenset(filter(None, sources.splitlines()))
            for proto, sources in sources.items()
            if sources
        }
        validators.sources(self.sources)
        self.proxies: Dict[ProxyType, Set[Proxy]] = {
            proto: set() for proto in self.sources
        }

        self.console = console or Console()
        self.cookie_jar = DummyCookieJar()
        self.regex = re.compile(
            r"(?:^|\D)?("
            r"(?:[1-9]|[1-9]\d|1\d{2}|2[0-4]\d|25[0-5])"  # 1-255
            + r"\.(?:\d|[1-9]\d|1\d{2}|2[0-4]\d|25[0-5])" * 3  # 0-255
            + r"):"
            + (
                r"(\d|[1-9]\d{1,3}|[1-5]\d{4}|6[0-4]\d{3}"
                r"|65[0-4]\d{2}|655[0-2]\d|6553[0-5])"
            )  # 0-65535
            + r"(?:\D|$)"
        )

    @classmethod
    def from_configparser(
        cls, cfg: ConfigParser, *, console: Optional[Console] = None
    ) -> Self:
        general = cfg["General"]
        folders = cfg["Folders"]
        http = cfg["HTTP"]
        socks4 = cfg["SOCKS4"]
        socks5 = cfg["SOCKS5"]
        save_path = Path(general.get("SavePath", ""))
        return cls(
            timeout=general.getfloat("Timeout", 5),
            source_timeout=general.getfloat("SourceTimeout", 15),
            max_connections=general.getint("MaxConnections", 512),
            check_website=general.get("CheckWebsite", "default"),
            sort_by_speed=general.getboolean("SortBySpeed", True),
            save_path=save_path,
            folders=(
                Folder(
                    path=save_path / "proxies",
                    is_enabled=folders.getboolean("proxies", True),
                    for_anonymous=False,
                    for_geolocation=False,
                ),
                Folder(
                    path=save_path / "proxies_anonymous",
                    is_enabled=folders.getboolean("proxies_anonymous", True),
                    for_anonymous=True,
                    for_geolocation=False,
                ),
                Folder(
                    path=save_path / "proxies_geolocation",
                    is_enabled=folders.getboolean("proxies_geolocation", True),
                    for_anonymous=False,
                    for_geolocation=True,
                ),
                Folder(
                    path=save_path / "proxies_geolocation_anonymous",
                    is_enabled=folders.getboolean(
                        "proxies_geolocation_anonymous", True
                    ),
                    for_anonymous=True,
                    for_geolocation=True,
                ),
            ),
            sources={
                ProxyType.HTTP: (
                    http.get("Sources")
                    if http.getboolean("Enabled", True)
                    else None
                ),
                ProxyType.SOCKS4: (
                    socks4.get("Sources")
                    if socks4.getboolean("Enabled", True)
                    else None
                ),
                ProxyType.SOCKS5: (
                    socks5.get("Sources")
                    if socks5.getboolean("Enabled", True)
                    else None
                ),
            },
            console=console,
        )

    async def fetch_source(
        self,
        *,
        session: ClientSession,
        source: str,
        proto: ProxyType,
        progress: Progress,
        task: TaskID,
    ) -> None:
        """Get proxies from source.

        Args:
            source: Proxy list URL.
        """
        try:
            async with session.get(source) as response:
                await response.read()
            text = await response.text()
        except asyncio.TimeoutError:
            logger.warning("%s | Timed out", source)
        except Exception as e:
            e_str = str(e)
            args: Tuple[object, ...] = (
                (
                    "%s | %s.%s (%s)",
                    source,
                    e.__class__.__module__,
                    e.__class__.__qualname__,
                    e_str,
                )
                if e_str
                else (
                    "%s | %s.%s",
                    source,
                    e.__class__.__module__,
                    e.__class__.__qualname__,
                )
            )
            logger.error(*args)
        else:
            proxies = self.regex.finditer(text)
            try:
                proxy = next(proxies)
            except StopIteration:
                args = (
                    ("%s | No proxies found", source)
                    if response.status == 200  # noqa: PLR2004
                    else ("%s | HTTP status code %d", source, response.status)
                )
                logger.warning(*args)
            else:
                proxies_set = self.proxies[proto]
                proxies_set.add(
                    Proxy(host=proxy.group(1), port=int(proxy.group(2)))
                )
                for proxy in proxies:
                    proxies_set.add(
                        Proxy(host=proxy.group(1), port=int(proxy.group(2)))
                    )
        progress.update(task, advance=1)

    async def check_proxy(
        self,
        *,
        proxy: Proxy,
        proto: ProxyType,
        progress: Progress,
        task: TaskID,
    ) -> None:
        """Check if proxy is alive."""
        try:
            await proxy.check(
                website=self.check_website,
                sem=self.sem,
                cookie_jar=self.cookie_jar,
                proto=proto,
                timeout=self.timeout,
                set_geolocation=self.geolocation_enabled,
            )
        except Exception as e:
            # Too many open files
            if isinstance(e, OSError) and e.errno == 24:  # noqa: PLR2004
                logger.error("Please, set MaxConnections to lower value.")

            logger.debug(
                "%s.%s | %s",
                e.__class__.__module__,
                e.__class__.__qualname__,
                e,
            )
            self.proxies[proto].remove(proxy)
        progress.update(task, advance=1)

    async def fetch_all_sources(self, progress: Progress) -> None:
        tasks = {
            proto: progress.add_task(
                f"[yellow]Scraper [red]:: [green]{proto.name}",
                total=len(sources),
            )
            for proto, sources in self.sources.items()
        }
        async with ClientSession(
            headers=HEADERS,
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

        self.proxies_count = {
            proto: len(proxies) for proto, proxies in self.proxies.items()
        }

    async def check_all_proxies(self, progress: Progress) -> None:
        tasks = {
            proto: progress.add_task(
                f"[yellow]Checker [red]:: [green]{proto.name}",
                total=len(proxies),
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
        for folder in self.folders:
            folder.remove()
        for folder in self.folders:
            if not folder.is_enabled:
                continue
            folder.create()
            for proto, proxies in sorted_proxies:
                text = "\n".join(
                    proxy.as_str(include_geolocation=folder.for_geolocation)
                    for proxy in proxies
                    if (proxy.is_anonymous if folder.for_anonymous else True)
                )
                file = folder.path / f"{proto.name.lower()}.txt"
                file.write_text(text, encoding="utf-8")
        logger.info(
            "Proxy folders have been created in the %s folder.",
            self.path.resolve(),
        )

    async def run(self) -> None:
        with self._get_progress_bar() as progress:
            await self.fetch_all_sources(progress)
            await self.check_all_proxies(progress)

        table = self._get_results_table()
        self.console.print(table)

        self.save_proxies()

        logger.info(
            "Thank you for using "
            "https://github.com/monosans/proxy-scraper-checker :)"
        )

    def get_sorted_proxies(self) -> Dict[ProxyType, List[Proxy]]:
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
            table.add_row(
                proto.name, f"{working} ({percentage:.1%})", str(total)
            )
        return table

    def _get_progress_bar(self) -> Progress:
        return Progress(
            TextColumn("[progress.description]{task.description}"),
            BarColumn(),
            MofNCompleteColumn(),
            console=self.console,
        )
