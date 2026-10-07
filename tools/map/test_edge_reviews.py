"""Manual edge evidence cannot collapse unknowns or silently rewrite old data."""
import csv
import json
from pathlib import Path
import tempfile
import unittest
from geometry import Grid
from layers import COVERAGE_FIELDS, LINE_FIELDS, SIDE_FIELDS, Layers, UnknownCoverage
from edge_reviews import load_edge_reviews, merge_reviews, preserve_rows, preserve_edge_masks
from line_reviews import load_line_reviews
from generate_grid import write_csv

MAP = Path(__file__).resolve().parents[2] / "data/map"

class EdgeReviewTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.folder = Path(self.temp.name)
        self.grid = Grid(MAP)
        self.rows = [dict(from_hex="C4020", to_hex="C4120", layer="line:road",
                         observed="present", high_side="", src=["land:8.33"], note="synthetic crossing"),
                     dict(from_hex="C4020", to_hex="C4120", layer="side:escarpment",
                         observed="present", high_side="C4120", src=["land:8.35"], note="synthetic oriented feature"),
                     dict(from_hex="C4020", to_hex="C4120", layer="side:wadi",
                         observed="absent", high_side="", src=["land:8.37"], note="synthetic full side"),
                     dict(from_hex="C4020", to_hex="C4120", layer="line:track",
                         observed="unresolved", high_side="", src=["land:8.37"], note="synthetic unreadable")]
        self.batch = dict(id="sample", coordinate_profile="vassal-2021", verification="visual_single_pass",
                          observer="synthetic", observed_on="2026-10-07", review_basis="synthetic tests",
                          source_image_sha256="image", build_file_sha256="build", terrain_key_sha256="key")
        self.save()

    def save(self):
        text = ["[batch]"] + [f"{k} = {json.dumps(v)}" for k,v in self.batch.items()]
        for row in self.rows:
            text += ["", "[[survey]]"] + [f"{k} = {json.dumps(v)}" for k,v in row.items()]
        (self.folder / "sample.toml").write_text("\n".join(text) + "\n")

    def load(self):
        return load_edge_reviews(self.folder, self.grid, "image", "build", "key")

    def test_resolved_only_and_direction_survives_canonical_order(self):
        lines, sides, masks = self.load()
        self.assertEqual(len(lines), 1)
        self.assertEqual(len(masks), 3)
        self.assertEqual(sides[0]["direction"], "NW")
        self.assertEqual(sides[0]["high_side"], "C4120")
        self.assertNotIn("line:track", [r["layer"] for r in masks])

    def test_source_and_missing_visual_basis_fail(self):
        with self.assertRaisesRegex(ValueError, "source identity"):
            load_edge_reviews(self.folder, self.grid, "new", "build", "key")
        self.batch["terrain_key_sha256"] = "wrong"
        self.save()
        with self.assertRaisesRegex(ValueError, "source identity"):
            self.load()
        self.batch["terrain_key_sha256"] = "key"
        self.batch["review_basis"] = ""
        self.save()
        with self.assertRaisesRegex(ValueError, "visual provenance"):
            self.load()

    def test_alias_nonadjacent_and_reversed_endpoints_fail(self):
        for a,b in [("D4200", "C4232"), ("C4020", "C4321"), ("C4120", "C4020")]:
            with self.subTest(a=a,b=b):
                self.rows = [dict(from_hex=a, to_hex=b, layer="line:road", observed="absent",
                                  high_side="", src=["land:8.37"], note="synthetic")]
                self.save()
                with self.assertRaises(ValueError):
                    self.load()

    def test_directional_high_side_required_only_for_present_oriented_feature(self):
        self.rows[1]["high_side"] = ""
        self.save()
        with self.assertRaisesRegex(ValueError, "high endpoint"):
            self.load()
        self.rows[1]["observed"] = "absent"
        self.rows[1]["high_side"] = "C4120"
        self.save()
        with self.assertRaisesRegex(ValueError, "Only present"):
            self.load()

    def test_duplicate_and_overlapping_masks_fail(self):
        lines, sides, masks = self.load()
        with self.assertRaisesRegex(ValueError, "Overlapping"):
            merge_reviews(lines, masks, [], [], masks)
        self.rows.append(dict(self.rows[0]))
        self.save()
        with self.assertRaisesRegex(ValueError, "Duplicate"):
            self.load()

    def test_invalid_label_or_empty_citation_cannot_create_absence(self):
        for field, value in [("observed", "probably_absent"), ("layer", "side:everything"), ("src", []), ("note", "")]:
            with self.subTest(field=field):
                previous = self.rows[0][field]
                self.rows[0][field] = value
                self.save()
                with self.assertRaises(ValueError):
                    self.load()
                self.rows[0][field] = previous

    def test_existing_high_side_and_masks_cannot_be_erased_or_replaced(self):
        _, sides, masks = self.load()
        path = self.folder / "sides.csv"
        write_csv(path, SIDE_FIELDS, sides)
        preserve_rows(path, sides, SIDE_FIELDS, ("hex_id", "neighbour_id", "feature"))
        changed = [dict(sides[0], high_side="C4020")]
        with self.assertRaisesRegex(ValueError, "erase or alter"):
            preserve_rows(path, changed, SIDE_FIELDS, ("hex_id", "neighbour_id", "feature"))
        with self.assertRaisesRegex(ValueError, "erase or alter"):
            preserve_rows(path, [], SIDE_FIELDS, ("hex_id", "neighbour_id", "feature"))
        path = self.folder / "coverage.csv"
        write_csv(path, COVERAGE_FIELDS, masks)
        with self.assertRaisesRegex(ValueError, "erase or alter"):
            preserve_rows(path, masks[:-1], COVERAGE_FIELDS, ("layer", "hex_id", "neighbour_id"))

    def test_published_strip_is_complete_only_on_its_two_edges(self):
        import tomllib
        meta = tomllib.loads((MAP / "edge-reviews/road-spine-0001.toml").read_text())["batch"]
        lines, sides, masks = load_edge_reviews(MAP/"edge-reviews", self.grid,
                         meta["source_image_sha256"], meta["build_file_sha256"], meta["terrain_key_sha256"])
        self.assertEqual(tuple(sum(r['review_batch']=='road-spine-0001' for r in values) for values in (lines,sides,masks)), (2,0,26))
        data = Layers(MAP)
        for a,b in [("C4220","C4120"), ("C4120","C4020")]:
            self.assertIsNotNone(data.feature("line", "road", a,b))
            for kind in ("track","railroad","unfinished_road","unfinished_railroad"):
                self.assertIsNone(data.feature("line",kind,a,b))
            for kind in ("escarpment","slope","ridge","wadi","all_sea","major_river","minor_river","border"):
                self.assertIsNone(data.feature("side",kind,a,b))
            with self.assertRaises(UnknownCoverage):
                data.feature("line","pipeline",a,b)
        with self.assertRaises(UnknownCoverage):
            data.feature("side","escarpment","C4220","C4121")

    def test_previously_covered_absence_cannot_silently_turn_positive(self):
        from generate_grid import write_csv
        row = dict(layer="line:road", hex_id="C4020", neighbour_id="C4120",
                   src="land:8.33", review_batch="sample")
        write_csv(self.folder / "coverage.csv", COVERAGE_FIELDS, [row])
        write_csv(self.folder / "line_features.csv", LINE_FIELDS, [])
        write_csv(self.folder / "hexsides.csv", SIDE_FIELDS, [])
        preserve_edge_masks(self.folder, [row], [], [])
        positive = dict(from_hex="C4020", to_hex="C4120", kind="road", src="land:8.33", review_batch="sample")
        with self.assertRaisesRegex(ValueError, "presence/absence"):
            preserve_edge_masks(self.folder, [row], [positive], [])

    def test_route_manifest_refuses_an_unsurveyed_edge_kind(self):
        import shutil
        from generate_strips import generate
        for name in ("sections.toml", "hexes.csv", "aliases.csv", "layers.toml", "coverage.csv", "line_features.csv", "hexsides.csv"):
            shutil.copyfile(MAP/name, self.folder/name)
        shutil.copytree(MAP/"edge-reviews", self.folder/"edge-reviews")
        with (self.folder/"coverage.csv").open(newline="") as f:
            masks = list(csv.DictReader(f))
        masks = [r for r in masks if (r["layer"],r["hex_id"],r["neighbour_id"]) != ("side:wadi","C4020","C4120")]
        write_csv(self.folder/"coverage.csv", COVERAGE_FIELDS, masks)
        with self.assertRaises(UnknownCoverage):
            generate(self.folder)
        self.assertFalse((self.folder/"strips.toml").exists())

    def test_published_completeness_note_keeps_control_and_pipeline_unknown(self):
        import tomllib
        strip = next(s for s in tomllib.loads((MAP / "strips.toml").read_text())["strips"] if s["id"] == "road-spine-0001")
        self.assertEqual(strip["route_hex_ids"], ["C4220", "C4120", "C4020"])
        self.assertEqual(strip["surveyed_route_edges"], [["C4120", "C4220"], ["C4020", "C4120"]])
        self.assertTrue(strip["route_map_layers_complete"])
        self.assertFalse(strip["control_halo_complete"])
        self.assertFalse(strip["pipeline_complete"])
        self.assertFalse(strip["unit_action_legality_verified"])

    def test_sollum_positive_has_source_high_side_and_ambiguous_playback_stays_unknown(self):
        data = Layers(MAP)
        side = data.feature("side", "escarpment", "C3922", "C3921")
        self.assertEqual(side["direction"], "E")
        self.assertEqual(side["high_side"], "C3921")
        self.assertIsNotNone(data.feature("line", "track", "C4020", "C3921"))
        for a,b in [("C3921","C4021"),("C4020","C4121")]:
            side = data.feature("side","escarpment",b,a)
            self.assertEqual(side["direction"], "NE")
            self.assertEqual(side["high_side"], a)
        self.assertIsNotNone(data.feature("line","track","C4020","C4121"))
        for family, kind in [("line", "railroad"), ("line", "unfinished_railroad"), ("side", "border")]:
            with self.subTest(family=family, kind=kind):
                with self.assertRaises(UnknownCoverage):
                    data.feature(family,kind,"C4020","C3921")
        with self.assertRaises(UnknownCoverage):
            data.feature("side","escarpment","C4020","C4021")

    def test_control_blockers_at_c3921_resolve_without_claiming_every_approach_complete(self):
        from geometry import DIRECTIONS
        data = Layers(MAP)
        for direction in DIRECTIONS:
            neighbor = data.grid.neighbour("C3921", direction)
            for kind in ("all_sea", "major_river", "escarpment"):
                data.feature("side",kind,"C3921",neighbor)
        # Keep the uncovered-approach check independent of later source surveys.
        import shutil
        fixture = self.folder / "uncovered-layers"
        fixture.mkdir()
        for name in ("sections.toml", "hexes.csv", "aliases.csv", "layers.toml"):
            shutil.copyfile(MAP / name, fixture / name)
        write_csv(fixture / "coverage.csv", COVERAGE_FIELDS, [])
        write_csv(fixture / "line_features.csv", LINE_FIELDS, [])
        write_csv(fixture / "hexsides.csv", SIDE_FIELDS, [])
        with self.assertRaises(UnknownCoverage):
            Layers(fixture).feature("line","road","C3921","C3820")
        import tomllib
        strip = next(s for s in tomllib.loads((MAP/"strips.toml").read_text(encoding="utf-8"))["strips"] if s["id"]=="sollum-control-0001")
        self.assertEqual(strip["route_hex_ids"], ["C3922", "C3921", "C4021"])
        self.assertFalse(strip["control_halo_complete"])
        self.assertFalse(strip["unit_action_legality_verified"])

if __name__ == "__main__":
    unittest.main()
