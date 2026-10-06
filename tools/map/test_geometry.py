"""Source-free topology and validation checks; run with unittest discovery."""
import tempfile
import unittest
from pathlib import Path
from geometry import Grid, DIRECTIONS, distance

MAP = Path(__file__).resolve().parents[2] / "data/map"

class GeometryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.grid = Grid(MAP)

    def test_every_hex_round_trips_and_has_unique_position(self):
        for name in self.grid.hexes:
            with self.subTest(name=name):
                self.assertEqual(self.grid.from_axial(*self.grid.to_axial(name)), name)

    def test_scenario_locations(self):
        # scen:60.3-60.4: Tobruk, Bardia, deployments, Matruh, Alexandria.
        for name, expected in {"C4807": (66, 15), "C4321": (77, 20),
                               "C4218": (74, 21), "C4120": (75, 22),
                               "D3714": (100, 26), "E3613": (132, 27)}.items():
            self.assertEqual(self.grid.to_axial(name), expected)

    def test_neighbours_reciprocal_and_distance_one(self):
        opposite = dict(zip(DIRECTIONS, ["W", "NW", "NE", "E", "SE", "SW"]))
        seam_counts = {a+b: 0 for a,b in zip("ABCD", "BCDE")}
        for name in self.grid.hexes:
            for direction in DIRECTIONS:
                other = self.grid.neighbour(name, direction)
                if other:
                    self.assertEqual(self.grid.neighbour(other, opposite[direction]), name)
                    self.assertEqual(distance(self.grid.to_axial(name), self.grid.to_axial(other)), 1)
                    pair = name[0]+other[0]
                    if pair in seam_counts:
                        seam_counts[pair] += 1
        self.assertTrue(all(c > 0 for c in seam_counts.values()), seam_counts)

    def test_observed_aliases_and_range_example(self):
        self.assertEqual(len(self.grid.aliases), 32)
        for alias, target in self.grid.aliases.items():
            self.assertEqual(self.grid.to_axial(alias), self.grid.to_axial(target))
            self.assertEqual(self.grid.from_axial(*self.grid.to_axial(alias)), target)
        # airlog:34.11: same first axis, eastward flight of 32 hexes.
        self.assertEqual(distance(self.grid.to_axial("C3101"),
                                  self.grid.to_axial("C3133")), 32)
        # land:15.35: adjacent wadi/slope example.
        self.assertEqual(distance(self.grid.to_axial("B5810"),
                                  self.grid.to_axial("B5809")), 1)

    def test_invalid_and_missing_ids_fail(self):
        for name in ["C9999", "Z4218", "c4218", "C421", "C42X8", " C4218", "D0000"]:
            with self.assertRaises(ValueError):
                self.grid.to_axial(name)
        with self.assertRaises(ValueError):
            self.grid.from_axial(-999, -999)

    def test_explicit_alias_resolves_to_canonical(self):
        # Synthetic alias tests lookup plumbing, not an invented original-map alias.
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            for name in ["sections.toml", "hexes.csv"]:
                (folder / name).write_bytes((MAP / name).read_bytes())
            (folder / "aliases.csv").write_text("alias_id,hex_id,src\nC9999,C4321,test:synthetic\n")
            grid = Grid(folder)
            self.assertEqual(grid.canonical("C9999"), "C4321")
            self.assertEqual(grid.to_axial("C9999"), (77, 20))

if __name__ == "__main__":
    unittest.main()
