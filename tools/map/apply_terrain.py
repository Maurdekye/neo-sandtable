"""Replay reviewed terrain decisions, checking exact local source identity.

Python 3.11+ stdlib. No image is written or copied into the repository. Raster
proposals alone are never sufficient: each applied record has a visual review.
"""
from __future__ import annotations
import argparse
from collections import Counter
import csv
import hashlib
import os
from pathlib import Path
import tomllib
from generate_grid import extract, IMAGE, write_csv
from render_terrain import render


def sha256(path):
    with path.open("rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


def load_reviews(folder, records, source_hash, grid_hash, tec):
    permitted = {r["id"] for r in tec["row"] if r["group"] == "hex_terrain"}
    permitted.add("sea")
    by_id = {h.hex_id: h for h in records}
    decisions = {}
    for path in sorted(folder.glob("*.toml")):
        review = tomllib.loads(path.read_text(encoding="utf-8"))
        batch = review["batch"]
        if batch["source_image_sha256"] != source_hash or batch["build_file_sha256"] != grid_hash:
            raise ValueError(f"Source changed; re-review {path.name}")
        if batch["coordinate_profile"] != "vassal-2021" or batch["verification"] != "visual_single_pass":
            raise ValueError("Unsupported review provenance")
        if not batch.get("observer") or not batch.get("observed_on"):
            raise ValueError("Review needs observer and date")
        for entry in review["hex"]:
            name = entry["hex_id"]
            if name not in by_id or name in decisions:
                raise ValueError(f"Invalid or duplicated review id: {name}")
            status = entry["status"]
            if status not in {"accepted", "deferred"}:
                raise ValueError("Review must accept or defer each cell")
            if status == "accepted" and entry["terrain"] not in permitted:
                raise ValueError(f"Unknown TEC terrain: {entry['terrain']}")
            if status == "deferred" and entry["terrain"] != "unclassified":
                raise ValueError("Deferred cell must stay unclassified")
            if status == "deferred" and not entry.get("note"):
                raise ValueError("Deferred cell needs reason")
            if "land:8.37" not in entry["src"]:
                raise ValueError("Terrain record lacks TEC citation")
            flags = set(entry["flags"])
            if not flags <= {"land", "sea", "coastal"}:
                raise ValueError("Unknown terrain-review flag")
            if "land" in flags and "sea" in flags:
                raise ValueError("Conflicting water-domain flags")
            if status == "accepted" and entry["terrain"] == "sea" and flags != {"sea"}:
                raise ValueError("Pure sea record must have only sea flag")
            if status == "accepted" and entry["terrain"] != "sea" and "land" not in flags:
                raise ValueError("Accepted land terrain needs land flag")
            decisions[name] = entry
    return decisions


def apply(sources, output, reviews, tec_path):
    output = output.resolve()
    if output.is_relative_to(sources.resolve()):
        raise ValueError("Cannot write inside sources")
    records, _, _, grid_hash, _ = extract(sources)
    tec = tomllib.loads(tec_path.read_text(encoding="utf-8"))
    decisions = load_reviews(reviews, records,
                             sha256(sources / "vassal/extracted/images" / IMAGE), grid_hash, tec)
    with (output / "hexes.csv").open(newline="", encoding="utf-8") as f:
        reader = csv.DictReader(f)
        fields = reader.fieldnames
        rows = list(reader)
    if {r["hex_id"] for r in rows} != {h.hex_id for h in records}:
        raise ValueError("Published grid differs from source; regenerate geometry first")
    for row in rows:
        name = row["hex_id"]
        if row["terrain"] != "unclassified" and name not in decisions:
            raise ValueError(f"Unreviewed existing classification would be lost: {name}")
        entry = decisions.get(name)
        row["terrain"] = entry["terrain"] if entry else "unclassified"
        row["flags"] = "|".join(entry["flags"]) if entry else ""
        row["src"] = ";".join(dict.fromkeys(["land:4.1"] + entry["src"])) if entry else "land:4.1"
    write_csv(output / "hexes.csv", fields, rows)
    render(rows, output / "terrain-preview.svg")
    print("Published terrain counts:", dict(Counter(r["terrain"] for r in rows)))
    print("Accepted reviews:", sum(d["status"] == "accepted" for d in decisions.values()),
          "deferred:", sum(d["status"] == "deferred" for d in decisions.values()))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    repo = Path(__file__).resolve().parents[2]
    parser.add_argument("--output", type=Path, default=repo / "data/map")
    parser.add_argument("--reviews", type=Path, default=repo / "data/map/reviews")
    parser.add_argument("--tec", type=Path, default=repo / "data/tables/land/8.37-terrain-effects.toml")
    args = parser.parse_args()
    if not os.environ.get("CNA_SOURCES"):
        parser.error("Set CNA_SOURCES")
    apply(Path(os.environ["CNA_SOURCES"]), args.output, args.reviews, args.tec)
