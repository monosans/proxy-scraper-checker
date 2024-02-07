from __future__ import annotations

import os as _os

# Monkeypatch os.link to make aiofiles work on Termux
if not hasattr(_os, "link"):
    from .typing_compat import Any as _Any

    def _link(*args: _Any, **kwargs: _Any) -> None:  # noqa: ARG001
        raise RuntimeError

    _os.link = _link
