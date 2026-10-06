"""Checks the publication boundary: rejected inputs cannot become terrain data."""
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
import tomllib
from apply_terrain import load_reviews
from propose_terrain import decide

MAP = Path(__file__).resolve().parents[2] / "data/map"
TEC = Path(__file__).resolve().parents[2] / "data/tables/land/8.37-terrain-effects.toml"

class TerrainTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.review = tomllib.loads((MAP / "reviews/graziani-0001.toml").read_text())
        cls.tec = tomllib.loads(TEC.read_text())
        cls.records = [SimpleNamespace(hex_id=h["hex_id"]) for h in cls.review["hex"]]
        cls.batch = cls.review["batch"]

    def load(self, folder, source_hash=None):
        return load_reviews(folder, self.records,
                            source_hash or self.batch["source_image_sha256"],
                            self.batch["build_file_sha256"], self.tec)

    def test_source_changed_requires_new_review(self):
        with self.assertRaisesRegex(ValueError, "Source changed"):
            self.load(MAP / "reviews", "0" * 64)

    def test_deferred_coastline_stays_unknown(self):
        decisions = self.load(MAP / "reviews")
        deferred = [h for h in decisions.values() if h["status"] == "deferred"]
        self.assertEqual(len(deferred), 11)
        self.assertTrue(all(h["terrain"] == "unclassified" and h["flags"] == ["coastal"] for h in deferred))
        self.assertEqual(sum(h["status"] == "accepted" for h in decisions.values()), 62)

    def test_duplicate_review_cannot_override_silently(self):
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            data = (MAP / "reviews/graziani-0001.toml").read_bytes()
            (folder / "a.toml").write_bytes(data)
            (folder / "b.toml").write_bytes(data)
            with self.assertRaisesRegex(ValueError, "duplicated review id"):
                self.load(folder)

    def test_unknown_class_cannot_be_published(self):
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            data = (MAP / "reviews/graziani-0001.toml").read_text().replace('terrain = "clear"', 'terrain = "invented"', 1)
            (folder / "a.toml").write_text(data)
            with self.assertRaisesRegex(ValueError, "Unknown TEC terrain"):
                self.load(folder)

    def test_contour_color_causes_abstention(self):
        # Ochre splashes of a hexside symbol must not be called mountain terrain.
        features = dict(clear=.49, rough=0, mountain=.23, sea=0)
        self.assertEqual(decide(features, 0, .102), "unclassified")

    def test_published_records_have_review_and_citations(self):
        import csv
        decisions = self.load(MAP / "reviews")
        with (MAP / "hexes.csv").open(newline="") as f:
            rows = list(csv.DictReader(f))
        classified = [r for r in rows if r["terrain"] != "unclassified"]
        self.assertEqual(len(classified), 62)
        for row in classified:
            entry = decisions[row["hex_id"]]
            self.assertEqual(entry["status"], "accepted")
            self.assertEqual(row["terrain"], entry["terrain"])
            self.assertIn("land:8.37", row["src"].split(";"))

if __name__ == "__main__":
    unittest.main()
