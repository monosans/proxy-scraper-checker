from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from shutil import rmtree


@dataclass(frozen=True)
class Folder:
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
