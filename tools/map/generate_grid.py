"""Extract discrete grid data from CNA_SOURCES, never source artwork or text.

Zone polygons stay in memory only. SVG uses integer axial data, with new regular
hexagons. Grid enumeration is independent of terrain and image pixels.
"""
from __future__ import annotations
import argparse
import csv
from dataclasses import dataclass
import hashlib
import math
import os
from pathlib import Path
import xml.etree.ElementTree as ET

IMAGE = "CNA Map Vassal Mitch Guthrie 2021.png"
PROFILE = "vassal-2021"
SRC = "land:4.1"

@dataclass(frozen=True)
class SourceHex:
    hex_id: str
    first: int
    second: int
    q: int
    r: int
    x: int
    y: int


def contains(polygon, x, y):
    """Even/odd ray crossing, with left/bottom half-open boundaries."""
    inside = False
    for (ax, ay), (bx, by) in zip(polygon, polygon[1:] + polygon[:1]):
        if (ay > y) != (by > y) and x < ax + (y - ay) * (bx - ax) / (by - ay):
            inside = not inside
    return inside


def extract(sources: Path):
    xml_path = sources / "vassal/extracted/buildFile.xml"
    xml = xml_path.read_bytes()
    root = ET.fromstring(xml)
    boards = [b for b in root.iter() if b.tag.endswith(".Board") and b.get("image") == IMAGE]
    if len(boards) != 1:
        raise ValueError("Expected one main map board")
    zones = {z.get("name"): z for z in boards[0].iter() if z.tag.endswith(".Zone")}
    records, sections = [], []
    for letter in "ABCDE":
        zone = zones[f"Map {letter}"]
        grid = next(g for g in zone if g.tag.endswith(".HexGrid"))
        numbering = next(iter(grid))
        if any(numbering.get(k) != v for k, v in
               {"first": "H", "hDescend": "false", "vDescend": "true", "hType": "N", "vType": "N"}.items()):
            raise ValueError("Unsupported numbering configuration")
        if grid.get("sideways") != "true":
            raise ValueError("Expected sideways VASSAL grid")
        dx, dy = float(grid.get("dx")), float(grid.get("dy"))
        x0, y0 = int(grid.get("x0")), int(grid.get("y0"))
        hoff, voff = int(numbering.get("hOff")), int(numbering.get("vOff"))
        stagger = numbering.get("stagger") == "true"
        polygon = [tuple(map(int, p.split(","))) for p in zone.get("path").split(";")]
        xmin, xmax = min(p[0] for p in polygon), max(p[0] for p in polygon)
        ymin, ymax = min(p[1] for p in polygon), max(p[1] for p in polygon)
        max_rows = math.floor((ymax - ymin) / dx + 0.5)
        shift = math.floor((x0 + 16) / dx + 0.5)
        north_constant = max_rows + hoff + shift
        if north_constant != 63:
            raise ValueError("North axis no longer matches calibrated profile")
        # Recover integer east offset from one even whole-map row (r=32).
        i = 32 - shift
        j = -voff - (i % 2 if stagger else 0)
        east_offset = math.floor((y0 + dy * (j + (i % 2) / 2) - 10) / dy + 0.5)
        section_records = []
        for i in range(math.floor((ymin - x0) / dx) - 1, math.ceil((ymax - x0) / dx) + 1):
            y = x0 + int(i * dx)
            for j in range(math.floor((xmin - y0) / dy) - 1, math.ceil((xmax - y0) / dy) + 1):
                x = y0 + int(dy * (j + (i % 2) / 2))
                if not contains(polygon, x, y):
                    continue
                first, second = max_rows - i + hoff, j + voff + (i % 2 if stagger else 0)
                if not (0 <= first <= 99 and 0 <= second <= 99):
                    raise ValueError("Non-four-digit map location")
                r = north_constant - first
                q = second + east_offset - r // 2
                # Pixel projection differences reflect registration, never topology.
                if abs(x - (10 + dy * (q + r / 2))) > 15 or abs(y - (-16 + dx * r)) > 3:
                    raise ValueError(f"Unexpected grid registration drift: {letter}{first:02}{second:02}")
                section_records.append(SourceHex(f"{letter}{first:02}{second:02}", first, second, q, r, x, y))
        records.extend(section_records)
        sections.append(dict(id=letter, north_axis_constant=north_constant, east_offset=east_offset,
                             first_min=min(h.first for h in section_records), first_max=max(h.first for h in section_records),
                             second_min=min(h.second for h in section_records), second_max=max(h.second for h in section_records),
                             hex_count=len(section_records), dx=dx, dy=dy, x0=x0, y0=y0,
                             max_rows=max_rows, h_off=hoff, v_off=voff, stagger=stagger))
    groups = {}
    for h in records:
        groups.setdefault((h.q, h.r), []).append(h)
    canonical, aliases = [], []
    for group in groups.values():
        # Prefer a positive column to the adjacent section's boundary column 00.
        # Remaining ties use west-to-east letter order, a stable display policy.
        group.sort(key=lambda h: (h.second == 0, h.hex_id))
        keeper = group[0]
        canonical.append(keeper)
        for alias in group[1:]:
            if abs(alias.x - keeper.x) > 15 or abs(alias.y - keeper.y) > 3:
                raise ValueError("Candidate alias does not match the same scan hex")
            aliases.append(dict(alias_id=alias.hex_id, hex_id=keeper.hex_id, src=SRC))
    canonical.sort(key=lambda h: h.hex_id)
    aliases.sort(key=lambda a: a["alias_id"])
    for section in sections:
        section["canonical_hex_count"] = sum(h.hex_id[0] == section["id"] for h in canonical)
        section["alias_count"] = sum(a["alias_id"][0] == section["id"] for a in aliases)
    records = canonical
    # Publish semantic boxes only; omit artwork geometry and UI tracking regions.
    boxes = sorted(name.strip() for name in zones if "Holding Box" in name or name in
                   {"Tunis", "Gabes", "Tripoli", "Tripolitania", "Tunis-Gabes", "Gabes-Tripoli",
                    "Tripoli-Tripolitania", "Tripolitania-Nofilia"})
    return records, sections, boxes, hashlib.sha256(xml).hexdigest(), aliases


def write_csv(path, fields, rows):
    with path.open("w", newline="", encoding="utf-8") as f:
        writer = csv.DictWriter(f, fields, lineterminator="\n")
        writer.writeheader()
        writer.writerows(rows)


def write_grid(sources: Path, output: Path):
    records, sections, boxes, digest, aliases = extract(sources)
    output = output.resolve()
    if output.is_relative_to(sources.resolve()):
        raise ValueError("Output must not be inside read-only source directory")
    output.mkdir(parents=True, exist_ok=True)
    hex_path = output / "hexes.csv"
    existing = {}
    if hex_path.exists():
        with hex_path.open(newline="") as f:
            existing = {r["hex_id"]: r for r in csv.DictReader(f)}
        if set(existing) != {h.hex_id for h in records}:
            raise ValueError("Grid changed; migrate existing classification records explicitly")
    rows = []
    for h in records:
        old = existing.get(h.hex_id, {})
        rows.append(dict(hex_id=h.hex_id, section=h.hex_id[0], printed_first=h.first,
                         printed_second=h.second, q=h.q, r=h.r,
                         terrain=old.get("terrain", "unclassified"), flags=old.get("flags", ""),
                         src=old.get("src", SRC).replace(";vassal:2021:map-grid", "")))
    write_csv(hex_path, list(rows[0]), rows)
    alias_path = output / "aliases.csv"
    write_csv(alias_path, ["alias_id", "hex_id", "src"], aliases)
    lines = ["schema_version = 1", f'coordinate_profile = "{PROFILE}"',
             'original_printed_seams = "unresolved"', 'orientation = "pointy-top"',
             'src = ["land:4.1"]', 'geometry_source = "vassal/extracted/buildFile.xml"', f'build_file_sha256 = "{digest}"', ""]
    for s in sections:
        lines.append("[[sections]]")
        for key, value in s.items():
            encoded = str(value).lower() if isinstance(value, bool) else f'"{value}"' if isinstance(value, str) else str(value)
            lines.append(f"{key} = {encoded}")
        lines.append("")
    for box in boxes:
        lines.extend(["[[off_map_boxes]]", f'name = "{box}"', 'hex_id = ""',
                      'src = ["land:8.8"]', ""])
    (output / "sections.toml").write_text("\n".join(lines), encoding="utf-8")
    write_preview(rows, output / "grid-preview.svg")
    print(f"{len(records)} canonical hexes, {len(aliases)} observed aliases; sections " + ", ".join(f'{s["id"]}={s["hex_count"]}' for s in sections))


def write_preview(rows, path):
    # New regular pointy-top geometry constructed solely from axial records.
    radius = 10
    centers = [(math.sqrt(3) * radius * (int(h["q"]) + int(h["r"]) / 2),
                1.5 * radius * int(h["r"])) for h in rows]
    xmin, xmax = min(x for x, y in centers) - 12, max(x for x, y in centers) + 12
    ymin, ymax = min(y for x, y in centers) - 12, max(y for x, y in centers) + 12
    colors = dict(zip("ABCDE", ["#dce4ee", "#dcebe3", "#eee5d7", "#eadfee", "#e8e9d7"]))
    out = [f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{xmin:.2f} {ymin:.2f} {xmax-xmin:.2f} {ymax-ymin:.2f}">',
           '<title>CNA grid geometry, VASSAL 2021 coordinate profile</title>',
           '<desc>Regular hexagons generated from axial records. Section colors are not terrain.</desc>']
    for h, (x, y) in zip(rows, centers):
        points = " ".join(f'{x+radius*math.cos(math.radians(a)):.2f},{y+radius*math.sin(math.radians(a)):.2f}'
                          for a in [30, 90, 150, 210, 270, 330])
        out.append(f'<polygon points="{points}" fill="{colors[h["section"]]}" stroke="#687582" stroke-width="0.35"><title>{h["hex_id"]}: grid geometry</title></polygon>')
    out.append("</svg>")
    path.write_text("\n".join(out) + "\n", encoding="utf-8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=Path(__file__).resolve().parents[2] / "data/map")
    args = parser.parse_args()
    source_env = os.environ.get("CNA_SOURCES")
    if not source_env:
        parser.error("Set CNA_SOURCES to the read-only local source directory")
    write_grid(Path(source_env), args.output)
