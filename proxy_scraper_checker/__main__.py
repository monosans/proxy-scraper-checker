from __future__ import annotations

from asyncio import set_event_loop_policy
from logging import basicConfig, DEBUG, INFO
from sys import platform, implementation
from configparser import ConfigParser

from rich.traceback import install as rich_install
from rich.console import Console
from rich.logging import RichHandler

from .proxy_scraper_checker import ProxyScraperChecker


def set_event_loop_policy_local() -> None:
    if platform == "win32":
        from asyncio import WindowsSelectorEventLoopPolicy
        set_event_loop_policy(WindowsSelectorEventLoopPolicy())
    elif implementation.name == "cpython" and platform in {"darwin", "linux"}:
        try:
            from uvloop import install
        except ImportError:
            pass
        else:
            install()


def configure_logging(console: Console, *, debug: bool) -> None:
    rich_install(console=console)
    basicConfig(
        level=DEBUG if debug else INFO,
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
    set_event_loop_policy_local()
    asyncio.run(main())
