"""Source-independent layer queries. Uncovered edges cannot mean no feature."""
import csv
from pathlib import Path
import tomllib
from geometry import Grid, DIRECTIONS

LINE_KINDS = {"road", "unfinished_road", "track", "railroad", "unfinished_railroad", "pipeline"}
SIDE_KINDS = {"escarpment", "slope", "ridge", "wadi", "major_river", "minor_river", "border", "all_sea"}
EDGE_LAYERS = {"line:" + k for k in LINE_KINDS} | {"side:" + k for k in SIDE_KINDS}
CELL_LAYERS = {"terrain", "coastal"}

class UnknownCoverage(ValueError):
    pass

def edge_key(grid, a, b):
    if grid.canonical(a) != a or grid.canonical(b) != b:
        raise ValueError("Published endpoints must be canonical")
    if not any(grid.neighbour(a, d) == b for d in DIRECTIONS):
        raise ValueError("Feature endpoints must be adjacent grid cells")
    return tuple(sorted((a, b)))

def read_csv(path, fields):
    with path.open(newline="", encoding="utf-8") as f:
        reader = csv.DictReader(f)
        if reader.fieldnames != fields:
            raise ValueError(f"Unexpected schema: {path.name}")
        rows = list(reader)
    if any(None in r or any(v is None for v in r.values()) for r in rows):
        raise ValueError("Malformed CSV row")
    return rows

COVERAGE_FIELDS = ["layer", "hex_id", "neighbour_id", "src", "review_batch"]
LINE_FIELDS = ["from_hex", "to_hex", "kind", "src", "review_batch"]
SIDE_FIELDS = ["hex_id", "direction", "neighbour_id", "feature", "high_side", "src", "review_batch"]

class Layers:
    def __init__(self, folder: Path):
        self.grid = Grid(folder)
        self.metadata = tomllib.loads((folder / "layers.toml").read_text(encoding="utf-8"))
        if (self.metadata["schema_version"] != 1 or
                self.metadata["coordinate_profile"] != self.grid.metadata["coordinate_profile"] or
                self.metadata["build_file_sha256"] != self.grid.metadata["build_file_sha256"] or
                set(self.metadata["line_kinds"]) != LINE_KINDS or
                set(self.metadata["hexside_kinds"]) != SIDE_KINDS):
            raise ValueError("Layer/grid schema or provenance differs")
        self.coverage = set()
        for row in read_csv(folder / "coverage.csv", COVERAGE_FIELDS):
            layer, a, b = row["layer"], row["hex_id"], row["neighbour_id"]
            self._evidence(row)
            if layer in EDGE_LAYERS:
                if edge_key(self.grid, a, b) != (a, b):
                    raise ValueError("Coverage edge must use sorted canonical endpoints")
            elif layer in CELL_LAYERS:
                if b or self.grid.canonical(a) != a:
                    raise ValueError("Cell coverage needs one canonical hex")
                h = self.grid.hexes[a]
                if layer == "terrain" and h["terrain"] == "unclassified":
                    raise ValueError("Cannot cover unreadable terrain")
                if layer == "coastal" and not {"land", "sea", "coastal"}.intersection(h["flags"].split("|")):
                    raise ValueError("Coastal coverage needs observed surface domain")
            else:
                raise ValueError("Unknown coverage layer")
            key = (layer, a, b)
            if key in self.coverage:
                raise ValueError("Duplicate coverage key")
            self.coverage.add(key)
        self.lines, self.sides = {}, {}
        for row in read_csv(folder / "line_features.csv", LINE_FIELDS):
            self._evidence(row)
            a, b, kind = row["from_hex"], row["to_hex"], row["kind"]
            if kind not in LINE_KINDS or edge_key(self.grid, a, b) != (a, b):
                raise ValueError("Invalid line feature")
            self._insert(self.lines, "line:" + kind, a, b, row)
        for row in read_csv(folder / "hexsides.csv", SIDE_FIELDS):
            self._evidence(row)
            a, b, kind = row["hex_id"], row["neighbour_id"], row["feature"]
            if kind not in SIDE_KINDS or edge_key(self.grid, a, b) != (a, b):
                raise ValueError("Invalid hexside feature")
            if row["direction"] not in DIRECTIONS or self.grid.neighbour(a, row["direction"]) != b:
                raise ValueError("Hexside direction disagrees with neighbour")
            high = row["high_side"]
            if (kind in {"slope", "escarpment"} and high not in {a, b}) or (kind not in {"slope", "escarpment"} and high):
                raise ValueError("Invalid or missing high-side orientation")
            self._insert(self.sides, "side:" + kind, a, b, row)

    @staticmethod
    def _evidence(row):
        if not row["src"] or not row["review_batch"]:
            raise ValueError("Coverage/features need citation and review batch")

    def _insert(self, records, layer, a, b, row):
        key = (layer, a, b)
        if key not in self.coverage:
            raise ValueError("Feature lacks matching layer coverage")
        if key in records:
            raise ValueError("Duplicate feature on one edge")
        records[key] = row

    def feature(self, family, kind, a, b):
        # Accept display aliases at the query boundary; publication is canonical.
        a, b = edge_key(self.grid, self.grid.canonical(a), self.grid.canonical(b))
        layer = family + ":" + kind
        if layer not in EDGE_LAYERS:
            raise ValueError("Unknown feature layer")
        key = (layer, a, b)
        if key not in self.coverage:
            raise UnknownCoverage(f"Unsurveyed {layer} edge {a}/{b}")
        return (self.lines if family == "line" else self.sides).get(key)
