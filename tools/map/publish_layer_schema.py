"""Publish initial movement-layer schema/masks; never invent edge absence.

Source hashes and terrain reviews are rechecked. Refuses to erase edge data.
"""
import os
import json
from pathlib import Path
import tomllib
from apply_terrain import load_reviews, sha256
from generate_grid import extract, IMAGE, write_csv
from geometry import Grid, DIRECTIONS
from layers import LINE_KINDS, SIDE_KINDS, COVERAGE_FIELDS, LINE_FIELDS, SIDE_FIELDS, Layers

def publish(repo, sources):
    folder = repo / "data/map"
    grid = Grid(folder)
    records, _, _, build_hash, _ = extract(sources)
    if build_hash != grid.metadata["build_file_sha256"] or {h.hex_id for h in records} != set(grid.hexes):
        raise ValueError("Source/grid identity differs")
    image_hash = sha256(sources / "vassal/extracted/images" / IMAGE)
    review_folder = folder / "reviews"
    tec = tomllib.loads((repo / "data/tables/land/8.37-terrain-effects.toml").read_text())
    decisions = load_reviews(review_folder, records, image_hash, build_hash, tec)
    batch_for = {}
    for path in sorted(review_folder.glob("*.toml")):
        review = tomllib.loads(path.read_text())
        for h in review["hex"]:
            batch_for[h["hex_id"]] = review["batch"]["id"]
    for file in ["line_features.csv", "hexsides.csv"]:
        if (folder / file).exists() and len((folder / file).read_text().splitlines()) > 1:
            raise ValueError("Initial-schema publisher cannot erase reviewed features")
    if (folder / "coverage.csv").exists():
        import csv
        with (folder / "coverage.csv").open(newline="") as f:
            if any(r["layer"] not in {"terrain", "coastal"} for r in csv.DictReader(f)):
                raise ValueError("Initial-schema publisher cannot erase edge coverage")
    coverage = []
    for name, entry in sorted(decisions.items()):
        h = grid.hexes[name]
        if h["terrain"] != entry["terrain"] or not set(entry["flags"]) <= set(h["flags"].split("|")):
            raise ValueError("Published surface differs from reviewed decisions")
        layers = ["terrain"] if entry["status"] == "accepted" else []
        if set(entry["flags"]) & {"land", "sea", "coastal"}:
            layers.append("coastal")
        for layer in layers:
            coverage.append(dict(layer=layer, hex_id=name, neighbour_id="",
                                 src=";".join(entry["src"]), review_batch=batch_for[name]))
    write_csv(folder / "coverage.csv", COVERAGE_FIELDS, sorted(coverage, key=lambda r:(r["layer"],r["hex_id"])))
    write_csv(folder / "line_features.csv", LINE_FIELDS, [])
    write_csv(folder / "hexsides.csv", SIDE_FIELDS, [])
    metadata = dict(schema_version=1, coordinate_profile="vassal-2021", build_file_sha256=build_hash,
                    source_image_sha256=image_hash, line_kinds=sorted(LINE_KINDS), hexside_kinds=sorted(SIDE_KINDS),
                    cell_layers=["terrain", "coastal"], edge_coverage="per_feature_kind",
                    unknown_policy="outside_mask_unknown", verification="surface_single_observer_edges_unreviewed")
    (folder / "layers.toml").write_text("\n".join(f"{k} = {json.dumps(v)}" for k,v in metadata.items())+"\n",encoding="utf-8",newline="\n")
    ids = set()
    parts = []
    for section, lo, hi in [("C",7,33),("D",0,33),("E",0,14)]:
        members = {grid.canonical(n) for n in list(grid.hexes)+list(grid.aliases)
                   if n[0] == section and lo <= int(n[3:]) <= hi}
        ids.update(members)
        parts.append(dict(section=section, second_min=lo, second_max=hi,
                          first_axis="all_source_contained", member_count=len(members)))
    edges = {tuple(sorted((n,b))) for n in ids for d in DIRECTIONS
             if (b:=grid.neighbour(n,d)) is not None and b in ids}
    boundary = {tuple(sorted((n,b))) for n in ids for d in DIRECTIONS
                if (b:=grid.neighbour(n,d)) is not None and b not in ids}
    window = dict(schema_version=1, id="graziani-priority", status="approved_digitization_window",
                  coordinate_profile="vassal-2021", build_file_sha256=build_hash,
                  canonical_count=len(ids), internal_edge_count=len(edges), boundary_edge_count=len(boundary),
                  scenario_rule=False, src=["scen:60.22"], hex_ids=sorted(ids))
    lines = [f"{k} = {json.dumps(v)}" for k,v in window.items()]
    for part in parts:
        lines += ["", "[[bounds]]"] + [f"{k} = {json.dumps(v)}" for k,v in part.items()]
    (folder / "graziani-window.toml").write_text("\n".join(lines)+"\n",encoding="utf-8",newline="\n")
    Layers(folder)
    print(f"Schema1: {len(coverage)} cell masks; zero surveyed edges. Window {len(ids)} hexes/{len(edges)} internal edges.")

if __name__ == "__main__":
    if not os.environ.get("CNA_SOURCES"):
        raise SystemExit("Set CNA_SOURCES")
    publish(Path(__file__).resolve().parents[2], Path(os.environ["CNA_SOURCES"]))
