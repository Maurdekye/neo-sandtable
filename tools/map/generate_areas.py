"""Generate exact section sets and symbolic locations; unresolved sets stay explicit."""
from __future__ import annotations
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import tomllib
from geometry import Grid


def materialize(definitions, grid):
    if definitions["schema_version"] != 1 or definitions["coordinate_profile"] != "vassal-2021":
        raise ValueError("Unsupported area definition schema/profile")
    locations = copy.deepcopy(definitions["locations"])
    location_ids = {p["id"] for p in locations}
    if len(location_ids) != len(locations):
        raise ValueError("Duplicate location identity")
    for location in locations:
        if not location["src"] or not location["off_map"] or location["range_status"] != "unresolved":
            raise ValueError("Location needs source and explicit range status")
        for field in ["reference_hex", "departure_hex"]:
            if field in location:
                grid.canonical(location[field])
    areas = []
    seen = set()
    for definition in definitions["areas"]:
        entry = copy.deepcopy(definition)
        if entry["id"] in seen or not entry["src"]:
            raise ValueError("Duplicate or unsourced area")
        seen.add(entry["id"])
        kind = entry["kind"]
        cells, places = set(), set()
        status = "resolved"
        if kind == "sections":
            sections = set(entry["sections"])
            if not sections or not sections <= set(grid.sections):
                raise ValueError("Unknown section selector")
            cells = {name for name in grid.hexes if name[0] in sections}
            cells.update(target for alias, target in grid.aliases.items() if alias[0] in sections)
        elif kind == "hexes":
            cells = {grid.canonical(name) for name in entry["hex_ids"]}
        elif kind == "locations":
            places = set(entry["location_ids"])
            if not places or not places <= location_ids:
                raise ValueError("Unknown location selector")
        elif kind in {"unresolved_region", "dynamic_facilities"}:
            if not entry.get("reason"):
                raise ValueError("Unresolved selector needs reason")
            status = "unresolved" if kind == "unresolved_region" else "requires_state"
        else:
            raise ValueError("Unknown area selector")
        if status == "resolved" and not (cells or places):
            raise ValueError("Resolved area cannot be empty")
        entry.update(membership_status=status, hex_ids=sorted(cells), location_ids=sorted(places))
        areas.append(entry)
    return dict(schema_version=1, coordinate_profile="vassal-2021", complete=False,
                build_file_sha256=grid.metadata["build_file_sha256"],
                definitions_source="data/map/area-definitions.toml",
                locations=sorted(locations,key=lambda p:p["id"]),
                areas=sorted(areas,key=lambda p:p["id"]))


def encode(value):
    return str(value).lower() if isinstance(value,bool) else json.dumps(value)


def write(data, path):
    lines = [f"{k} = {encode(v)}" for k,v in data.items() if k not in {"locations","areas"}]
    for group in ["locations", "areas"]:
        for entry in data[group]:
            lines += ["", f"[[{group}]]"]
            for key,value in entry.items():
                if isinstance(value,list) and len(value)>16:
                    lines += [f"{key} = ["]
                    lines += ["  " + ", ".join(encode(v) for v in value[i:i+12]) + "," for i in range(0,len(value),12)]
                    lines += ["]"]
                else:
                    lines += [f"{key} = {encode(value)}"]
    path.write_text("\n".join(lines)+"\n",encoding="utf-8",newline="\n")


def generate(sources, folder, definitions, output):
    if output.resolve().is_relative_to(sources.resolve()):
        raise ValueError("Cannot write in sources")
    grid = Grid(folder)
    digest = hashlib.sha256((sources / "vassal/extracted/buildFile.xml").read_bytes()).hexdigest()
    if digest != grid.metadata["build_file_sha256"]:
        raise ValueError("Grid source changed; regenerate and verify geometry first")
    data = materialize(tomllib.loads(definitions.read_text(encoding="utf-8")),grid)
    write(data,output)
    print(f"Generated {len(data['areas'])} areas and {len(data['locations'])} symbolic locations; complete=false")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    repo = Path(__file__).resolve().parents[2]
    parser.add_argument("--grid",type=Path,default=repo/"data/map")
    parser.add_argument("--definitions",type=Path,default=repo/"data/map/area-definitions.toml")
    parser.add_argument("--output",type=Path,default=repo/"data/map/areas.toml")
    args=parser.parse_args()
    if not os.environ.get("CNA_SOURCES"):
        parser.error("Set CNA_SOURCES")
    generate(Path(os.environ["CNA_SOURCES"]),args.grid,args.definitions,args.output)
