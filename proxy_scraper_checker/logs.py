from __future__ import annotations

import logging
import logging.handlers
import queue
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from typing import Any


def configure() -> logging.handlers.QueueListener:
    log_queue: queue.Queue[Any] = queue.Queue()

    logging.root.addHandler(logging.handlers.QueueHandler(log_queue))
    logging.root.setLevel(logging.INFO)

    # Start logging before importing rich for the first time
    import rich.traceback  # noqa: PLC0415
    from rich.logging import RichHandler  # noqa: PLC0415

    rich.traceback.install(width=80, extra_lines=0, word_wrap=True)
    stream_handler = RichHandler(
        omit_repeated_times=False,
        show_path=False,
        rich_tracebacks=True,
        tracebacks_width=80,
        tracebacks_extra_lines=0,
        tracebacks_word_wrap=True,
        log_time_format=logging.Formatter.default_time_format,
    )

    return logging.handlers.QueueListener(log_queue, stream_handler)
