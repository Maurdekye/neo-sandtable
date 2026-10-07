"""Generate the GT1-6 digitization envelope, never a legal movement boundary."""
import json
from pathlib import Path
from geometry import Grid, DIRECTIONS

BOUNDS = [
    ("C", 20, 22, 30, 44), ("C", 23, 33, 30, 43),
    ("D", 0, 10, 28, 42), ("D", 11, 16, 25, 40),
    ("D", 17, 24, 20, 36), ("D", 25, 33, 20, 35),
    ("E", 0, 7, 23, 36), ("E", 8, 14, 25, 39),
]

def generate(folder):
    grid = Grid(folder)
    ids = set()
    parts = []
    for section, lo, hi, north_lo, north_hi in BOUNDS:
        members = {grid.canonical(n) for n in list(grid.hexes) + list(grid.aliases)
                   if n[0] == section and lo <= int(n[3:]) <= hi
                   and north_lo <= int(n[1:3]) <= north_hi}
        ids.update(members)
        parts.append(dict(section=section, second_min=lo, second_max=hi,
                          first_min=north_lo, first_max=north_hi,
                          member_count=len(members)))
    internal = set()
    boundary = set()
    for name in ids:
        for direction in DIRECTIONS:
            neighbour = grid.neighbour(name, direction)
            if neighbour is not None:
                pair = tuple(sorted((name, neighbour)))
                (internal if neighbour in ids else boundary).add(pair)
    for anchor in ("C4321", "C4022", "D3714", "E3613", "E3714"):
        if anchor not in ids:
            raise ValueError("Priority corridor lost anchor: " + anchor)
    document = dict(schema_version=1, id="graziani-gt1-6-corridor",
                    status="proposed_digitization_corridor",
                    coordinate_profile="vassal-2021",
                    build_file_sha256=grid.metadata["build_file_sha256"],
                    canonical_count=len(ids), internal_edge_count=len(internal),
                    boundary_edge_count=len(boundary), scenario_rule=False,
                    src=["scen:60.22", "land:4.1"], hex_ids=sorted(ids))
    lines = [f"{key} = {json.dumps(value)}" for key, value in document.items()]
    for part in parts:
        lines += ["", "[[bounds]]"] + [f"{k} = {json.dumps(v)}" for k, v in part.items()]
    (folder / "graziani-corridor.toml").write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
    print(f"Corridor: {len(ids)} cells, {len(internal)} internal + {len(boundary)} crossing edges")

if __name__ == "__main__":
    generate(Path(__file__).resolve().parents[2] / "data/map")
