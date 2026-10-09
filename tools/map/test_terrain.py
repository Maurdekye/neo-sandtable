"""Checks the publication boundary: rejected inputs cannot become terrain data."""
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
import tomllib
from apply_terrain import load_reviews, load_places, reviewed_cell_layers
from propose_terrain import decide

MAP = Path(__file__).resolve().parents[2] / "data/map"
TEC = Path(__file__).resolve().parents[2] / "data/tables/land/8.37-terrain-effects.toml"

class TerrainTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.review = tomllib.loads((MAP / "reviews/graziani-0001.toml").read_text())
        cls.tec = tomllib.loads(TEC.read_text())
        import csv
        with (MAP / "hexes.csv").open(newline="") as f:
            cls.records = [SimpleNamespace(hex_id=h["hex_id"]) for h in csv.DictReader(f)]
        cls.batch = cls.review["batch"]

    def load(self, folder, source_hash=None):
        return load_reviews(folder, self.records,
                            source_hash or self.batch["source_image_sha256"],
                            self.batch["build_file_sha256"], self.tec)

    def test_source_changed_requires_new_review(self):
        with self.assertRaisesRegex(ValueError, "Source changed"):
            self.load(MAP / "reviews", "0" * 64)

    def test_deferred_coastline_stays_unknown(self):
        # Replay the immutable deferral, independently of later reinspection.
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            for name in ["graziani-0001.toml", "graziani-0002.toml"]:
                (folder / name).write_bytes((MAP / "reviews" / name).read_bytes())
            deferred = self.load(folder)["C4026"]
            self.assertEqual(deferred["status"], "deferred")
            self.assertEqual(deferred["terrain"], "unclassified")
            self.assertEqual(deferred["flags"], ["coastal"])
        decisions = self.load(MAP / "reviews")
        self.assertEqual(decisions["C4221"]["terrain"], "rough")
        self.assertEqual(decisions["C4022"]["flags"], ["land", "coastal"])

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

    def test_amendment_cannot_target_unreviewed_cell(self):
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            (folder / "a.toml").write_bytes((MAP / "reviews/graziani-0001.toml").read_bytes())
            data = (MAP / "reviews/graziani-0002.toml").read_text().replace('hex_id = "C4221"', 'hex_id = "C2818"', 1)
            (folder / "b.toml").write_text(data)
            with self.assertRaisesRegex(ValueError, "Amendment must target"):
                self.load(folder)

    def test_one_review_amends_cells_from_distinct_prior_batches(self):
        # A single owner adjudication may resolve deferrals from several batches.
        header = '\n'.join(f'{k} = {v!r}' for k, v in self.batch.items()
                           if isinstance(v, str) and k != 'id')
        def review(batch, entries):
            return '[batch]\nid = ' + repr(batch) + '\n' + header + '\n' + entries
        def entry(name, target=None):
            result = '\n[[hex]]\nhex_id = ' + repr(name) + '\n'
            if target is not None:
                result += 'supersedes = ' + repr(target) + '\n'
            return result + 'status = "accepted"\nterrain = "clear"\nflags = ["land"]\nsrc = ["land:8.37"]\n'
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            (folder / 'a.toml').write_text(review('first', entry('C2818')))
            (folder / 'b.toml').write_text(review('second', entry('C2819')))
            valid = review('owner', entry('C2818', 'first') + entry('C2819', 'second'))
            (folder / 'c.toml').write_text(valid)
            self.assertEqual(set(self.load(folder)), {'C2818', 'C2819'})
            for bad, error in [
                (valid.replace("supersedes = 'second'", "supersedes = 'first'"), 'Amendment must target'),
                (valid.replace("supersedes = 'first'", "supersedes = 'future'"), 'must precede'),
                (valid.replace("supersedes = 'first'", "supersedes = ''"), 'nonempty prior batch'),
                (valid.replace("supersedes = 'first'\n", ''), 'duplicated review id'),
                (valid + entry('C2818', 'first'), 'duplicated review id'),
            ]:
                with self.subTest(error=error):
                    (folder / 'c.toml').write_text(bad)
                    with self.assertRaisesRegex(ValueError, error):
                        self.load(folder)

    def test_sea_and_coastal_flags_conflict(self):
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            data = (MAP / "reviews/graziani-0001.toml").read_text().replace('flags = ["sea"]', 'flags = ["sea", "coastal"]', 1)
            (folder / "a.toml").write_text(data)
            with self.assertRaisesRegex(ValueError, "Conflicting water-domain"):
                self.load(folder)

    def test_land_class_does_not_resolve_unknown_coastal_domain(self):
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            original = (MAP / 'reviews/graziani-0001.toml').read_text()
            land = original.replace('terrain = "clear"',
                                    'coastal_status = "unresolved"\nterrain = "clear"', 1)
            (folder / 'a.toml').write_text(land)
            unknown = next(entry for entry in self.load(folder).values()
                           if entry.get('coastal_status') == 'unresolved')
            self.assertEqual(unknown['terrain'], 'clear')
            self.assertEqual(reviewed_cell_layers(unknown), ['terrain'])
            legacy = self.load(MAP / 'reviews')['C4022']
            self.assertEqual(reviewed_cell_layers(legacy), ['terrain', 'coastal'])
            for value, error in [('"typo"', 'Unknown coastal review status'),
                                 ('false', 'Unknown coastal review status'),
                                 ('[]', 'Unknown coastal review status')]:
                (folder / 'a.toml').write_text(land.replace('"unresolved"', value))
                with self.subTest(value=value), self.assertRaisesRegex(ValueError, error):
                    self.load(folder)
            coastal = original.replace('flags = ["sea"]',
                                       'coastal_status = "unresolved"\nflags = ["sea"]', 1)
            (folder / 'a.toml').write_text(coastal)
            with self.assertRaisesRegex(ValueError, 'cannot assert sea or coastal'):
                self.load(folder)

    def test_reviewed_port_requires_coastal_hex(self):
        import csv
        with (MAP / "hexes.csv").open(newline="") as f:
            rows=list(csv.DictReader(f))
        next(r for r in rows if r["hex_id"]=="C4022")["flags"]="land"
        with self.assertRaisesRegex(ValueError, "requires a coastal"):
            load_places(MAP / "reviews", rows)
        places = tomllib.loads((MAP / "places.toml").read_text())["places"]
        ports = [(p["id"], p["hex_id"]) for p in places if p["type"]=="port"]
        self.assertIn(("port-sollum", "C4022"), ports)
        self.assertTrue(all("coastal" in next(r["flags"].split("|") for r in rows if r["hex_id"]==h) or h=="C4022" for _,h in ports))

    def test_combined_marker_does_not_infer_terrain_coast_or_extent(self):
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            text = 'hex = []\n[batch]\nid = "synthetic-marker"\n[[place]]\nid = "marker"\nname = "Synthetic"\nhex_id = "C3419"\ntype = "village_bir"\nsrc = ["land:8.37"]\nnote = "Synthetic combined marker only"\n'
            path = folder / "sample.toml"
            path.write_text(text, encoding="utf-8")
            rows = [dict(hex_id="C3419", terrain="unclassified", flags="")]
            places = load_places(folder, rows)
            self.assertEqual(places[0]["type"], "village_bir")
            self.assertNotIn("place_group", places[0])
            self.assertEqual(rows, [dict(hex_id="C3419", terrain="unclassified", flags="")])
            path.write_text(text + 'place_group = "invented-extent"\n', encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "does not establish place extent"):
                load_places(folder, rows)
            path.write_text(text.replace('land:8.37', 'scen:60.31'), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "TEC citation"):
                load_places(folder, rows)

    def test_marsh_and_city_corrections_are_reviewed(self):
        decisions=self.load(MAP/"reviews")
        self.assertEqual(decisions["D3315"]["proposed_terrain"],"clear")
        self.assertEqual(decisions["D3315"]["terrain"],"salt_marsh")
        self.assertEqual(decisions["D3414"]["terrain"],"salt_marsh")
        self.assertEqual(decisions["D3414"]["status"],"accepted")
        self.assertIn("interp:map-0003", decisions["D3414"]["src"])
        # Preserve the historical decision rather than rewriting its evidence.
        prior = tomllib.loads((MAP / "reviews/zz-map-terrain-gap-0009.toml").read_text())
        old = next(e for e in prior["hex"] if e["hex_id"] == "D3414")
        self.assertEqual((old["status"], old["terrain"]), ("deferred", "unclassified"))
        places=tomllib.loads((MAP/"places.toml").read_text())["places"]
        city={p["hex_id"] for p in places if p["type"]=="major_city"}
        self.assertTrue({"A4827","E1930","E1931","E1829","E1830","E1730"} <= city)
        self.assertTrue(all(decisions[h]["terrain"]=="major_city" for h in city))

    def test_owner29_terrain_and_water_provenance_stay_independent(self):
        decisions = self.load(MAP / "reviews")
        for name, terrain in [("E3413", "swamp"), ("E3414", "swamp"),
                              ("E3514", "clear"), ("E3614", "delta")]:
            with self.subTest(hex_id=name):
                self.assertEqual(decisions[name]["terrain"], terrain)
                self.assertEqual(reviewed_cell_layers(decisions[name]), ["terrain"])
                self.assertEqual(decisions[name]["coastal_status"], "unresolved")
        for name in ["D2516", "D2530"]:
            self.assertEqual(decisions[name]["terrain"], "rough")
            self.assertEqual(decisions[name]["minor_classes"], ["clear"])
        self.assertEqual(decisions["E3713"]["flags"], ["land", "coastal"])

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
        self.assertEqual({r["hex_id"] for r in classified}, {h for h,d in decisions.items() if d["status"]=="accepted"})
        for row in classified:
            entry = decisions[row["hex_id"]]
            self.assertEqual(entry["status"], "accepted")
            self.assertEqual(row["terrain"], entry["terrain"])
            self.assertIn("land:8.37", row["src"].split(";"))

if __name__ == "__main__":
    unittest.main()
