from __future__ import annotations

from pathlib import Path
from shutil import rmtree


class Folder:
    __slots__ = ("for_anonymous", "for_geolocation", "path")

    def __init__(self, *, path: Path, folder_name: str) -> None:
        self.path = path / folder_name
        self.for_anonymous = "anon" in folder_name
        self.for_geolocation = "geo" in folder_name

    def remove(self) -> None:
        try:
            rmtree(self.path)
        except FileNotFoundError:
            pass

    def create(self) -> None:
        self.path.mkdir(parents=True, exist_ok=True)
