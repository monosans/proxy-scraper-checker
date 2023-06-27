from __future__ import annotations

import asyncio
import logging
import sys
from configparser import ConfigParser

import rich.traceback
from rich.console import Console
from rich.logging import RichHandler

from .proxy_scraper_checker import ProxyScraperChecker


def set_event_loop_policy() -> None:
    if sys.platform == "win32":
        asyncio.set_event_loop_policy(asyncio.WindowsSelectorEventLoopPolicy())
    elif sys.implementation.name == "cpython" and sys.platform in {
        "darwin",
        "linux",
    }:
        try:
            import uvloop
        except ImportError:
            pass
        else:
            uvloop.install()


def configure_logging(console: Console, *, debug: bool) -> None:
    rich.traceback.install(console=console)
    logging.basicConfig(
        level=logging.DEBUG if debug else logging.INFO,
        format="%(message)s",
        datefmt="%Y-%m-%d %H:%M:%S",
        handlers=(
            RichHandler(
                console=console,
                omit_repeated_times=False,
                show_path=False,
                rich_tracebacks=True,
            ),
        ),
    )


def get_config(file: str) -> ConfigParser:
    cfg = ConfigParser(interpolation=None)
    cfg.read(file, encoding="utf-8")
    return cfg


async def main() -> None:
    cfg = get_config("config.ini")

    console = Console()
    configure_logging(console, debug=cfg["General"].getboolean("Debug", False))

    await ProxyScraperChecker.from_configparser(cfg, console=console).run()


if __name__ == "__main__":
    set_event_loop_policy()
    asyncio.run(main())
