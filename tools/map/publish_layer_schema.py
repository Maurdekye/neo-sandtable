"""Replay movement-layer schema and accepted masks; never invent edge absence.

Source hashes and terrain reviews are rechecked. Refuses to erase edge data.
"""
import os
import json
from pathlib import Path
import tomllib
from apply_terrain import load_reviews, sha256
from generate_grid import extract, IMAGE, write_csv
from geometry import Grid, DIRECTIONS
from line_reviews import load_line_reviews
from edge_reviews import load_edge_reviews, merge_reviews, preserve_rows, preserve_edge_masks
from generate_strips import generate as generate_strips
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
    line_features,line_coverage=load_line_reviews(folder/"line-reviews",grid,image_hash,build_hash)
    direct_lines, direct_sides, direct_coverage = load_edge_reviews(
        folder / "edge-reviews", grid, image_hash, build_hash,
        sha256(sources / "vassal/extracted/images/TEC.png"))
    line_features, side_features, edge_coverage = merge_reviews(
        line_features, line_coverage, direct_lines, direct_sides, direct_coverage)
    preserve_rows(folder / "line_features.csv", line_features, LINE_FIELDS,
                  ("from_hex", "to_hex", "kind"))
    preserve_rows(folder / "hexsides.csv", side_features, SIDE_FIELDS,
                  ("hex_id", "neighbour_id", "feature"))
    preserve_edge_masks(folder, edge_coverage, line_features, side_features)
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
    coverage.extend(edge_coverage)
    write_csv(folder / "coverage.csv", COVERAGE_FIELDS, sorted(coverage, key=lambda r:(r["layer"],r["hex_id"],r["neighbour_id"])))
    write_csv(folder / "line_features.csv", LINE_FIELDS, line_features)
    write_csv(folder / "hexsides.csv", SIDE_FIELDS, side_features)
    metadata = dict(schema_version=1, coordinate_profile="vassal-2021", build_file_sha256=build_hash,
                    source_image_sha256=image_hash, line_kinds=sorted(LINE_KINDS), hexside_kinds=sorted(SIDE_KINDS),
                    cell_layers=["terrain", "coastal"], edge_coverage="per_feature_kind",
                    unknown_policy="outside_mask_unknown", verification="visual_single_observer_partial_edge_strips")
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
    generate_strips(folder)
    print(f"Schema1: {len(coverage)-len(edge_coverage)} cell masks; {len(edge_coverage)} edge-kind masks, {len(line_features)} lines/{len(side_features)} hexside features. Window {len(ids)} hexes/{len(edges)} internal edges.")

if __name__ == "__main__":
    if not os.environ.get("CNA_SOURCES"):
        raise SystemExit("Set CNA_SOURCES")
    publish(Path(__file__).resolve().parents[2], Path(os.environ["CNA_SOURCES"]))
