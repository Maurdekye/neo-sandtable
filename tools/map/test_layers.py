"""Movement-facing checks: unknown must never collapse into verified absence."""
import csv
import shutil
import tempfile
import unittest
from pathlib import Path
import tomllib
from generate_grid import write_csv
from layers import Layers, UnknownCoverage, COVERAGE_FIELDS, LINE_FIELDS, SIDE_FIELDS

MAP = Path(__file__).resolve().parents[2] / "data/map"

class LayerTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.folder = Path(self.tmp.name)
        for name in ["sections.toml","hexes.csv","aliases.csv","layers.toml"]:
            shutil.copyfile(MAP/name,self.folder/name)
        write_csv(self.folder/"coverage.csv",COVERAGE_FIELDS,[])
        write_csv(self.folder/"line_features.csv",LINE_FIELDS,[])
        write_csv(self.folder/"hexsides.csv",SIDE_FIELDS,[])

    def cover(self, layer, a="C4218", b="C4219"):
        write_csv(self.folder/"coverage.csv",COVERAGE_FIELDS,[dict(layer=layer,hex_id=a,neighbour_id=b,src="land:8.37",review_batch="synthetic-test")])

    def test_uncovered_edge_is_unknown_both_directions(self):
        data=Layers(self.folder)
        for a,b in [("C4218","C4219"),("C4219","C4218")]:
            with self.assertRaises(UnknownCoverage): data.feature("line","road",a,b)

    def test_absence_is_only_for_surveyed_feature_kind(self):
        self.cover("line:road")
        data=Layers(self.folder)
        self.assertIsNone(data.feature("line","road","C4219","C4218"))
        with self.assertRaises(UnknownCoverage): data.feature("line","track","C4218","C4219")
        with self.assertRaises(UnknownCoverage): data.feature("side","escarpment","C4218","C4219")

    def test_present_road_requires_matching_coverage(self):
        row=dict(from_hex="C4218",to_hex="C4219",kind="road",src="land:8.33",review_batch="synthetic-test")
        write_csv(self.folder/"line_features.csv",LINE_FIELDS,[row])
        with self.assertRaisesRegex(ValueError,"lacks matching"): Layers(self.folder)
        self.cover("line:road")
        self.assertEqual(Layers(self.folder).feature("line","road","C4219","C4218")["kind"],"road")

    def test_alias_queries_use_one_seam_edge(self):
        self.cover("line:road","C4232","C4233")
        self.assertIsNone(Layers(self.folder).feature("line","road","D4200","C4232"))
        self.cover("line:road","C4232","D4200")
        with self.assertRaisesRegex(ValueError,"canonical"): Layers(self.folder)

    def test_direction_and_high_side_do_not_flip_on_reverse_query(self):
        self.cover("side:escarpment")
        row=dict(hex_id="C4218",direction="E",neighbour_id="C4219",feature="escarpment",high_side="C4219",src="land:8.35",review_batch="synthetic-test")
        write_csv(self.folder/"hexsides.csv",SIDE_FIELDS,[row])
        data=Layers(self.folder)
        self.assertEqual(data.feature("side","escarpment","C4219","C4218")["high_side"],"C4219")
        row["direction"]="W"
        write_csv(self.folder/"hexsides.csv",SIDE_FIELDS,[row])
        with self.assertRaisesRegex(ValueError,"direction disagrees"): Layers(self.folder)
        row["direction"]="E";row["high_side"]=""
        write_csv(self.folder/"hexsides.csv",SIDE_FIELDS,[row])
        with self.assertRaisesRegex(ValueError,"high-side"): Layers(self.folder)

    def test_nonadjacent_and_duplicate_coverage_rejected(self):
        self.cover("line:road","C4218","C4221")
        with self.assertRaisesRegex(ValueError,"adjacent"): Layers(self.folder)
        self.cover("line:road")
        with (self.folder/"coverage.csv").open(newline="") as f: rows=list(csv.DictReader(f))
        write_csv(self.folder/"coverage.csv",COVERAGE_FIELDS,rows+rows)
        with self.assertRaisesRegex(ValueError,"Duplicate coverage"): Layers(self.folder)

    def test_unreadable_fragment_has_coast_but_no_terrain_mask(self):
        # The unreadable fragment is a fixture, not a permanent public deferral.
        with (self.folder / "hexes.csv").open(newline="") as f:
            reader = csv.DictReader(f)
            fields = reader.fieldnames
            rows = list(reader)
        fragment = next(row for row in rows if row["hex_id"] == "C4026")
        fragment["terrain"] = "unclassified"
        fragment["flags"] = "coastal|land|sea"
        write_csv(self.folder / "hexes.csv", fields, rows)
        self.cover("coastal", "C4026", "")
        unreadable = Layers(self.folder)
        self.assertIn(("coastal","C4026",""),unreadable.coverage)
        self.assertNotIn(("terrain","C4026",""),unreadable.coverage)
        data=Layers(MAP)
        self.assertEqual({a for layer,a,b in data.coverage if layer=="terrain"},
                         {h for h,r in data.grid.hexes.items() if r["terrain"]!="unclassified"})
        self.assertEqual({a for layer,a,b in data.coverage if layer=="coastal"},
                         {h for h,r in data.grid.hexes.items()
                          if {"land","sea","coastal"}.intersection(r["flags"].split("|"))}
                         - {"E3413", "E3414", "E3514", "E3614"})
        with self.assertRaises(UnknownCoverage): Layers(self.folder).feature("side","escarpment","C4026","C4025")
        with self.assertRaises(UnknownCoverage): Layers(self.folder).feature("line","road","C4026","C4025")

    def test_work_window_is_approved_not_a_scenario_restriction(self):
        window=tomllib.loads((MAP/"graziani-window.toml").read_text())
        self.assertEqual(window["status"],"approved_digitization_window")
        self.assertFalse(window["scenario_rule"])
        self.assertEqual(window["canonical_count"],2933)
        self.assertEqual(len(set(window["hex_ids"])),2933)
        self.assertTrue({"C4807","C4321","C4022","D3714","E3613","E3714","C4233","D1233"} <= set(window["hex_ids"]))

    def test_owner_land_classes_keep_unresolved_water_uncovered(self):
        data = Layers(MAP)
        for name, terrain in [("E3413", "swamp"), ("E3414", "swamp"),
                              ("E3514", "clear"), ("E3614", "delta")]:
            with self.subTest(hex_id=name):
                self.assertEqual(data.grid.hexes[name]["terrain"], terrain)
                self.assertIn(("terrain", name, ""), data.coverage)
                self.assertNotIn(("coastal", name, ""), data.coverage)
                self.assertNotIn("coastal", data.grid.hexes[name]["flags"].split("|"))
        self.assertIn(("coastal", "E3713", ""), data.coverage)

if __name__ == "__main__": unittest.main()
