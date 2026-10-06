"""Source-independent lookup for the published grid. Python 3.11+."""
from __future__ import annotations
import csv
import re
from pathlib import Path
import tomllib

DIRECTIONS = {"E": (1, 0), "SE": (0, 1), "SW": (-1, 1),
              "W": (-1, 0), "NW": (0, -1), "NE": (1, -1)}

class Grid:
    def __init__(self, folder: Path):
        self.metadata = tomllib.loads((folder / "sections.toml").read_text())
        self.sections = {s["id"]: s for s in self.metadata["sections"]}
        with (folder / "hexes.csv").open(newline="") as f:
            self.hexes = {h["hex_id"]: h for h in csv.DictReader(f)}
        self.by_axial = {(int(h["q"]), int(h["r"])): name
                         for name, h in self.hexes.items()}
        if len(self.by_axial) != len(self.hexes):
            raise ValueError("Multiple canonical hexes at one axial coordinate")
        with (folder / "aliases.csv").open(newline="") as f:
            self.aliases = {a["alias_id"]: a["hex_id"] for a in csv.DictReader(f)}
        if any(a in self.hexes or h not in self.hexes for a, h in self.aliases.items()):
            raise ValueError("Invalid aliases")

    def canonical(self, name: str) -> str:
        if not re.fullmatch(r"[A-E][0-9]{4}", name):
            raise ValueError(f"Invalid hex id: {name}")
        name = self.aliases.get(name, name)
        if name not in self.hexes:
            raise ValueError(f"Hex outside published grid: {name}")
        return name

    def to_axial(self, name: str) -> tuple[int, int]:
        name = self.canonical(name)
        s = self.sections[name[0]]
        r = s["north_axis_constant"] - int(name[1:3])
        q = int(name[3:]) + s["east_offset"] - r // 2
        row = self.hexes[name]
        if (q, r) != (int(row["q"]), int(row["r"])):
            raise ValueError(f"Metadata disagrees with hex record: {name}")
        return q, r

    def from_axial(self, q: int, r: int) -> str:
        try:
            return self.by_axial[q, r]
        except KeyError as e:
            raise ValueError(f"Coordinate outside published grid: {(q, r)}") from e

    def neighbour(self, name: str, direction: str) -> str | None:
        q, r = self.to_axial(name)
        dq, dr = DIRECTIONS[direction]
        return self.by_axial.get((q + dq, r + dr))


def distance(a: tuple[int, int], b: tuple[int, int]) -> int:
    dq, dr = a[0] - b[0], a[1] - b[1]
    return max(abs(dq), abs(dr), abs(dq + dr))
