from __future__ import annotations

import os as _os

from .typing_compat import Any as _Any

# Monkeypatch os.link to make aiofiles work on Termux
if not hasattr(_os, "link"):

    def _link(*args: _Any, **kwargs: _Any) -> None:
        raise RuntimeError

    _os.link = _link
