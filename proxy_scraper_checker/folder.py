from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from shutil import rmtree


@dataclass(repr=False, eq=False)
class Folder:
    __slots__ = ("path", "is_enabled", "for_anonymous", "for_geolocation")

    path: Path
    is_enabled: bool
    for_anonymous: bool
    for_geolocation: bool

    def remove(self) -> None:
        try:
            rmtree(self.path)
        except FileNotFoundError:
            pass

    def create(self) -> None:
        self.path.mkdir(parents=True, exist_ok=True)
