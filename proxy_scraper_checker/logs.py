from __future__ import annotations

import logging
import logging.handlers
import queue
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from typing import Any


def configure() -> logging.handlers.QueueListener:
    log_queue: queue.Queue[Any] = queue.Queue()

    logging.root.setLevel(logging.INFO)
    logging.root.addHandler(logging.handlers.QueueHandler(log_queue))

    # Start logging before importing rich for the first time
    from rich.logging import RichHandler  # noqa: PLC0415

    stream_handler = RichHandler(
        omit_repeated_times=False,
        show_path=False,
        log_time_format=logging.Formatter.default_time_format,
    )

    return logging.handlers.QueueListener(log_queue, stream_handler)
