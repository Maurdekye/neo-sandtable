"""Replay direct source reviews of line and hexside observations.

A review is explicit per kind. Unresolved observations supply no coverage;
there are no wildcard negatives or implied layers from terrain/endpoint flags.
"""
from pathlib import Path
import tomllib
from geometry import DIRECTIONS
from layers import EDGE_LAYERS, edge_key


def load_edge_reviews(folder, grid, image_hash, build_hash, key_hash):
    lines, sides, coverage = [], [], []
    seen, batches = set(), set()
    for path in sorted(Path(folder).glob("*.toml")):
        review = tomllib.loads(path.read_text(encoding="utf-8"))
        batch = review["batch"]
        if (batch.get("verification") != "visual_single_pass" or
                batch.get("coordinate_profile") != "vassal-2021" or
                not batch.get("observer", "").strip() or
                not batch.get("observed_on", "").strip() or
                not batch.get("review_basis", "").strip()):
            raise ValueError("Direct edge review needs visual provenance")
        if (batch.get("source_image_sha256") != image_hash or
                batch.get("build_file_sha256") != build_hash or
                batch.get("terrain_key_sha256") != key_hash):
            raise ValueError("Direct edge review source identity changed")
        batch_id = batch["id"]
        if batch_id != path.stem or batch_id in batches:
            raise ValueError("Invalid or duplicate direct review batch identity")
        batches.add(batch_id)
        for row in review.get("survey", []):
            a, b, layer = row["from_hex"], row["to_hex"], row["layer"]
            if a >= b or edge_key(grid, a, b) != (a, b):
                raise ValueError("Review endpoints must be sorted canonical neighbors")
            if layer not in EDGE_LAYERS:
                raise ValueError("Unknown direct review layer")
            identity = (layer, a, b)
            if identity in seen:
                raise ValueError("Duplicate direct edge review; amendment required")
            seen.add(identity)
            status = row["observed"]
            if status not in {"present", "absent", "unresolved"}:
                raise ValueError("Invalid direct observation")
            if not row.get("note", "").strip():
                raise ValueError("Direct observation needs an inspection note")
            src = row["src"]
            if (not isinstance(src, list) or not src or
                    any(not isinstance(s, str) or ":" not in s for s in src)):
                raise ValueError("Direct observation needs cited source cases")
            family, kind = layer.split(":")
            high = row.get("high_side", "")
            directional = family == "side" and kind in {"slope", "escarpment"}
            if status == "present" and directional:
                if high not in {a, b}:
                    raise ValueError("Directional observation needs its high endpoint")
            elif high:
                raise ValueError("Only present directional features have a high side")
            if status == "unresolved":
                continue
            common = dict(src=";".join(src), review_batch=batch_id)
            coverage.append(dict(layer=layer, hex_id=a, neighbour_id=b, **common))
            if status == "absent":
                continue
            if family == "line":
                lines.append(dict(from_hex=a, to_hex=b, kind=kind, **common))
            else:
                direction = next(d for d in DIRECTIONS if grid.neighbour(a, d) == b)
                sides.append(dict(hex_id=a, direction=direction, neighbour_id=b,
                                  feature=kind, high_side=high, **common))
    return (sorted(lines, key=lambda r: (r["from_hex"], r["to_hex"], r["kind"])),
            sorted(sides, key=lambda r: (r["hex_id"], r["neighbour_id"], r["feature"])),
            sorted(coverage, key=lambda r: (r["layer"], r["hex_id"], r["neighbour_id"])))


def merge_reviews(pilot_lines, pilot_coverage, direct_lines, direct_sides, direct_coverage):
    """No overlapping or conflicting masks may silently amend earlier evidence."""
    seen = set()
    for row in pilot_coverage + direct_coverage:
        key = (row["layer"], row["hex_id"], row["neighbour_id"])
        if key in seen:
            raise ValueError("Overlapping accepted edge reviews; amendment required")
        seen.add(key)
    return (sorted(pilot_lines + direct_lines,
                   key=lambda r: (r["from_hex"], r["to_hex"], r["kind"])),
            direct_sides,
            sorted(pilot_coverage + direct_coverage,
                   key=lambda r: (r["layer"], r["hex_id"], r["neighbour_id"])))


def preserve_rows(path, expected, fields, key_fields):
    """Source replay may add evidence but cannot alter/remove published rows."""
    import csv
    if not path.exists():
        return
    by_key = {tuple(r[k] for k in key_fields): r for r in expected}
    with path.open(newline="", encoding="utf-8") as stream:
        reader = csv.DictReader(stream)
        if reader.fieldnames != fields:
            raise ValueError("Unexpected preserved layer schema")
        for row in reader:
            key = tuple(row[k] for k in key_fields)
            if by_key.get(key) != row:
                raise ValueError("Cannot erase or alter existing edge evidence; amendment required")


def preserve_edge_masks(folder, expected_masks, lines, sides):
    """An old covered negative cannot silently become a positive either."""
    import csv
    from layers import COVERAGE_FIELDS
    path = folder / "coverage.csv"
    if not path.exists():
        return
    expected = {(r["layer"], r["hex_id"], r["neighbour_id"]): r for r in expected_masks}
    positive = {("line:"+r["kind"], r["from_hex"], r["to_hex"]) for r in lines}
    positive |= {("side:"+r["feature"], r["hex_id"], r["neighbour_id"]) for r in sides}
    previous_positive = set()
    for name, family, a_key, kind_key in (("line_features.csv", "line", "from_hex", "kind"),
                                        ("hexsides.csv", "side", "hex_id", "feature")):
        old_path = folder / name
        if old_path.exists():
            with old_path.open(newline="", encoding="utf-8") as stream:
                for row in csv.DictReader(stream):
                    b_key = "to_hex" if family == "line" else "neighbour_id"
                    previous_positive.add((family+":"+row[kind_key], row[a_key], row[b_key]))
    with path.open(newline="", encoding="utf-8") as stream:
        reader = csv.DictReader(stream)
        if reader.fieldnames != COVERAGE_FIELDS:
            raise ValueError("Unexpected preserved coverage schema")
        for row in reader:
            if not row["layer"].startswith(("line:", "side:")):
                continue
            key = (row["layer"], row["hex_id"], row["neighbour_id"])
            if (expected.get(key) != row or
                    (key in previous_positive) != (key in positive)):
                raise ValueError("Cannot alter old edge presence/absence or mask; amendment required")
