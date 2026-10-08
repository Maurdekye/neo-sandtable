"""Geometric national regions from a complete, cited frontier cut.

Only source-verified Sea is excluded; unknown terrain remains geometric membership.
An incomplete frontier never creates a country membership by guesswork.
"""
from collections import deque
import csv
import tomllib

from geometry import DIRECTIONS


def section_members(grid, sections):
    """Include canonical cells reached through printed section aliases."""
    return {name for name in grid.hexes if name[0] in sections} | {
        target for alias, target in grid.aliases.items() if alias[0] in sections
    }


def known_sea_cells(grid, coverage):
    masks = {(row["layer"], row["hex_id"]) for row in coverage}
    return {name for name, row in grid.hexes.items()
            if row["terrain"] == "sea" and row["flags"] == "sea"
            and ("terrain", name) in masks and ("coastal", name) in masks}


def load_national_regions(grid):
    folder = grid.folder
    evidence = tomllib.loads((folder / "national-frontier.toml").read_text(encoding="utf-8"))
    layers = tomllib.loads((folder / "layers.toml").read_text(encoding="utf-8"))
    if (evidence.get("schema_version") != 1 or
            evidence.get("coordinate_profile") != "vassal-2021" or
            evidence.get("build_file_sha256") != grid.metadata["build_file_sha256"] or
            evidence.get("source_image_sha256") != layers["source_image_sha256"] or
            evidence.get("partition_policy") != "exclude_verified_sea_include_unknown"):
        raise ValueError("Frontier provenance or partition policy differs")
    if not evidence.get("observer") or not evidence.get("observed_on"):
        raise ValueError("Frontier needs source observation provenance")
    sides = evidence.get("sides", [])
    if ([s.get("sequence") for s in sides] != list(range(1, len(sides) + 1)) or
            sum(s.get("extent") == "full_printed_side" for s in sides) !=
            evidence.get("full_printed_side_count") or
            sum(s.get("extent") == "printed_to_shore_remaining_water" for s in sides) !=
            evidence.get("printed_to_shore_side_count") or
            any(s.get("extent") not in {"full_printed_side", "printed_to_shore_remaining_water"}
                or s.get("observed") != "present"
                or {s.get("libya_side"), s.get("egypt_side")} !=
                {s.get("from_hex"), s.get("to_hex")} for s in sides)):
        raise ValueError("Frontier extent, sequence or source orientation is incomplete")
    with (folder / "coverage.csv").open(newline="", encoding="utf-8") as stream:
        sea = known_sea_cells(grid, csv.DictReader(stream))
    return national_regions(grid, evidence, sea)


def partition_section(grid, sides, west_seeds, east_seeds, known_sea=()):
    section = section_members(grid, {"C"})
    cells = section - set(known_sea)
    blocked = set()
    for side in sides:
        a, b = side["from_hex"], side["to_hex"]
        if a != grid.canonical(a) or b != grid.canonical(b):
            raise ValueError("Frontier identities must be canonical")
        if a not in section or b not in section:
            raise ValueError("Frontier side is outside section C")
        if b not in {grid.neighbour(a, direction) for direction in DIRECTIONS}:
            raise ValueError("Frontier endpoints are not adjacent")
        if not side.get("src") or not side.get("reason"):
            raise ValueError("Frontier side lacks citation or incidence evidence")
        pair = tuple(sorted((a, b)))
        if pair in blocked:
            raise ValueError("Duplicate frontier side")
        blocked.add(pair)
    if not blocked or not west_seeds or not east_seeds:
        raise ValueError("Frontier and both source-grounded seed sets are required")

    def fill(seeds):
        if any(seed != grid.canonical(seed) or seed not in cells for seed in seeds):
            raise ValueError("Country seed is not a canonical section-C cell")
        reached = set(seeds)
        queue = deque(sorted(reached))
        while queue:
            a = queue.popleft()
            for direction in DIRECTIONS:
                b = grid.neighbour(a, direction)
                if b in cells and b not in reached and tuple(sorted((a, b))) not in blocked:
                    reached.add(b)
                    queue.append(b)
        return reached

    west, east = fill(west_seeds), fill(east_seeds)
    if west & east:
        raise ValueError("Frontier gap: west and east remain connected")
    if west | east != cells:
        raise ValueError("Frontier leaves an unassigned section-C component")
    if any(a in cells and b in cells and (a in west) == (b in west)
           for a, b in blocked):
        raise ValueError("Frontier includes a side within one country")
    return west, east


def national_regions(grid, evidence, known_sea=()):
    if evidence.get("trace_status") != "complete":
        raise ValueError("National frontier trace is unresolved")
    west, east = partition_section(
        grid, evidence["sides"], evidence["west_seeds"], evidence["east_seeds"], known_sea
    )
    for side in evidence["sides"]:
        if side.get("libya_side") not in west or side.get("egypt_side") not in east:
            # The lower-level cut validator can use synthetic rows without orientation.
            if "libya_side" in side or "egypt_side" in side:
                raise ValueError("Country fill disagrees with source side orientation")
    libya = (section_members(grid, {"A", "B"}) | west) - set(known_sea)
    egypt = (section_members(grid, {"D", "E"}) | east) - set(known_sea)
    if libya & egypt or libya | egypt != set(grid.hexes) - set(known_sea):
        raise ValueError("Country regions do not partition the canonical whole grid")
    c = section_members(grid, {"C"})
    cd = section_members(grid, {"C", "D"})
    return {"libya": libya, "egypt": egypt,
            "map_c_libya": c & libya, "map_c_or_d_egypt": cd & egypt}
