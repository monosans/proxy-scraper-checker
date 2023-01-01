from __future__ import annotations

import asyncio
import logging
import sys

import rich.traceback
from rich.console import Console
from rich.logging import RichHandler


def install_uvloop() -> None:
    if sys.implementation.name == "cpython" and sys.platform in {
        "darwin",
        "linux",
    }:
        try:
            import uvloop
        except ImportError:
            pass
        else:
            uvloop.install()


def setup_logging(console: Console) -> None:
    rich.traceback.install(console=console)
    logging.basicConfig(
        level=logging.INFO,
        format="%(message)s",
        handlers=[
            RichHandler(console=console, show_path=False, rich_tracebacks=True)
        ],
    )


if __name__ == "__main__":
    install_uvloop()

    console = Console()
    setup_logging(console)

    from .proxy_scraper_checker import ProxyScraperChecker

    psc = ProxyScraperChecker.from_ini("config.ini", console=console)
    asyncio.run(psc.run())
