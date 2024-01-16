from __future__ import annotations

import sys

if sys.version_info < (3, 11):
    from typing_extensions import Any, Literal, Self
else:
    from typing import Any, Literal, Self

__all__ = ("Any", "Literal", "Self")
