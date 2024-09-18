from __future__ import annotations

import asyncio
import logging
import sys
from typing import TYPE_CHECKING

import aiofiles
import rich.traceback
from aiohttp import ClientSession, TCPConnector
from rich.console import Console
from rich.logging import RichHandler
from rich.progress import BarColumn, MofNCompleteColumn, Progress, TextColumn
from rich.table import Table

from . import checker, geodb, http, output, scraper, sort, utils
from .settings import Settings
from .storage import ProxyStorage

if sys.version_info >= (3, 11):
    try:
        import tomllib
    except ImportError:
        # Help users on older alphas
        if not TYPE_CHECKING:
            import tomli as tomllib
else:
    import tomli as tomllib

if TYPE_CHECKING:
    from typing import Callable, Coroutine, Mapping

    from aiohttp_socks import ProxyType
    from typing_extensions import Any, TypeVar

    T = TypeVar("T")

logger = logging.getLogger(__name__)


def get_async_run() -> Callable[[Coroutine[Any, Any, T]], T]:
    if sys.implementation.name == "cpython":
        try:
            import uvloop  # type: ignore[import-not-found, unused-ignore]  # noqa: PLC0415
        except ImportError:
            pass
        else:
            try:
                return uvloop.run  # type: ignore[no-any-return, unused-ignore]
            except AttributeError:
                uvloop.install()
                return asyncio.run

        try:
            import winloop  # type: ignore[import-not-found, unused-ignore]  # noqa: PLC0415
        except ImportError:
            pass
        else:
            try:
                return winloop.run  # type: ignore[no-any-return, unused-ignore]
            except AttributeError:
                winloop.install()
                return asyncio.run
    if sys.platform == "win32":
        asyncio.set_event_loop_policy(asyncio.WindowsSelectorEventLoopPolicy())
    return asyncio.run


async def read_config(file: str, /) -> dict[str, Any]:
    async with aiofiles.open(file, "rb") as f:
        content = await f.read()
    return tomllib.loads(utils.bytes_decode(content))


def configure_logging(*, console: Console, debug: bool) -> None:
    rich.traceback.install(
        console=console, width=None, extra_lines=0, word_wrap=True
    )
    logging.basicConfig(
        format="%(message)s",
        datefmt=logging.Formatter.default_time_format,
        level=logging.DEBUG if debug else logging.INFO,
        handlers=(
            RichHandler(
                console=console,
                omit_repeated_times=False,
                show_path=False,
                rich_tracebacks=True,
                tracebacks_extra_lines=0,
            ),
        ),
        force=True,
    )


def get_summary_table(
    *, before: Mapping[ProxyType, int], after: Mapping[ProxyType, int]
) -> Table:
    table = Table()
    table.add_column("Protocol", style="cyan")
    table.add_column("Working", style="magenta")
    table.add_column("Total", style="green")
    for proto in sort.PROTOCOL_ORDER:
        if (total := before.get(proto)) is not None:
            working = after.get(proto, 0)
            percentage = (working / total) if total else 0
            table.add_row(
                proto.name, f"{working} ({percentage:.1%})", str(total)
            )
    return table


async def main() -> None:
    cfg = await read_config("config.toml")
    console = Console()
    configure_logging(console=console, debug=cfg["debug"])
    should_save = False
    try:
        async with ClientSession(
            connector=TCPConnector(ssl=http.SSL_CONTEXT),
            headers=http.HEADERS,
            cookie_jar=http.get_cookie_jar(),
            raise_for_status=True,
            fallback_charset_resolver=http.fallback_charset_resolver,
        ) as session:
            settings = await Settings.from_mapping(cfg, session=session)
            storage = ProxyStorage(protocols=settings.sources)
            with Progress(
                TextColumn("[yellow]{task.fields[col1]}"),
                TextColumn("[red]::"),
                TextColumn("[green]{task.fields[col2]}"),
                BarColumn(),
                MofNCompleteColumn(),
                console=console,
                transient=True,
            ) as progress:
                scrape = scraper.scrape_all(
                    progress=progress,
                    session=session,
                    settings=settings,
                    storage=storage,
                )
                await (
                    asyncio.gather(
                        geodb.download_geodb(
                            progress=progress, session=session
                        ),
                        scrape,
                    )
                    if settings.enable_geolocation
                    else scrape
                )
                await session.close()
                count_before_checking = storage.get_count()
                should_save = True
                if settings.check_website:
                    await checker.check_all(
                        settings=settings,
                        storage=storage,
                        progress=progress,
                        proxies_count=count_before_checking,
                    )
    finally:
        if should_save:
            if settings.check_website:
                storage.remove_unchecked()
            count_after_checking = storage.get_count()
            console.print(
                get_summary_table(
                    before=count_before_checking, after=count_after_checking
                )
            )

            await output.save_proxies(storage=storage, settings=settings)

        logger.info(
            "Thank you for using https://github.com/monosans/proxy-scraper-checker"
        )


if __name__ == "__main__":
    get_async_run()(main())
