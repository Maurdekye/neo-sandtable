"""A gap or a path around a coast endpoint must refuse national regions."""
import unittest

from frontier import known_sea_cells, national_regions, partition_section, section_members


class StripGrid:
    """Two columns with three rows; all cells geometric, terrain unknown."""
    def __init__(self):
        self.hexes = {f"C{r:02d}{c:02d}": {"terrain": "unclassified"}
                      for r in range(1, 4) for c in (1, 2)}
        self.aliases = {}

    def canonical(self, name):
        if name not in self.hexes:
            raise ValueError("Unknown hex")
        return name

    def neighbour(self, name, direction):
        row, col = int(name[1:3]), int(name[3:])
        offset = {"E": (0, 1), "W": (0, -1), "SE": (1, 0),
                  "NW": (-1, 0)}.get(direction)
        if offset is None:
            return None
        other = f"C{row+offset[0]:02d}{col+offset[1]:02d}"
        return other if other in self.hexes else None


def side(row):
    return dict(from_hex=f"C{row:02d}01", to_hex=f"C{row:02d}02",
                src=["land:8.37"], reason="Synthetic test incidence")


class FrontierTests(unittest.TestCase):
    def test_complete_cut_partitions_unknown_terrain_geometrically(self):
        grid = StripGrid()
        west, east = partition_section(grid, [side(r) for r in (1, 2, 3)],
                                       ["C0201"], ["C0202"])
        self.assertEqual(west, {"C0101", "C0201", "C0301"})
        self.assertEqual(east, {"C0102", "C0202", "C0302"})
        regions = national_regions(grid, dict(trace_status="complete",
            sides=[side(r) for r in (1, 2, 3)],
            west_seeds=["C0201"], east_seeds=["C0202"]))
        self.assertEqual(regions["map_c_libya"], west)
        self.assertEqual(regions["map_c_or_d_egypt"], east)

    def test_gap_in_middle_or_at_coast_endpoint_refuses(self):
        for rows in [(1, 3), (2, 3)]:
            with self.subTest(rows=rows), self.assertRaisesRegex(ValueError, "gap"):
                partition_section(StripGrid(), [side(r) for r in rows],
                                  ["C0201"], ["C0202"])

    def test_duplicate_or_uncited_sides_refuse(self):
        with self.assertRaisesRegex(ValueError, "Duplicate"):
            partition_section(StripGrid(), [side(1), side(1)], ["C0201"], ["C0202"])
        invalid = side(1)
        invalid["src"] = []
        with self.assertRaisesRegex(ValueError, "citation"):
            partition_section(StripGrid(), [invalid], ["C0201"], ["C0202"])

    def test_section_members_retains_seam_alias_membership(self):
        grid = StripGrid()
        grid.hexes["B0133"] = {"terrain": "unclassified"}
        grid.aliases["C0100"] = "B0133"
        self.assertIn("B0133", section_members(grid, {"C"}))

    def test_known_sea_only_exclusion_closes_coastal_end(self):
        grid = StripGrid()
        coverage = []
        for name in ["C0101", "C0102"]:
            grid.hexes[name] = {"terrain": "sea", "flags": "sea"}
            coverage += [dict(layer=layer, hex_id=name) for layer in ("terrain", "coastal")]
        sea = known_sea_cells(grid, coverage)
        self.assertEqual(sea, {"C0101", "C0102"})
        evidence = dict(trace_status="complete", sides=[side(2), side(3)],
                        west_seeds=["C0201"], east_seeds=["C0202"])
        regions = national_regions(grid, evidence, sea)
        self.assertEqual(regions["libya"], {"C0201", "C0301"})
        self.assertEqual(regions["egypt"], {"C0202", "C0302"})
        # Missing coastal-domain coverage cannot silently create a Sea exclusion.
        coverage = [x for x in coverage if not (x["hex_id"] == "C0101" and x["layer"] == "coastal")]
        self.assertNotIn("C0101", known_sea_cells(grid, coverage))

    def test_unknown_northern_bypass_still_refuses(self):
        with self.assertRaisesRegex(ValueError, "gap"):
            partition_section(StripGrid(), [side(2), side(3)],
                              ["C0201"], ["C0202"], {"C0301"})

    def test_source_orientation_disagreement_refuses_without_reversing_it(self):
        sides=[side(r) for r in (1,2,3)]
        sides[1].update(libya_side="C0202",egypt_side="C0201")
        with self.assertRaisesRegex(ValueError,"source side orientation"):
            national_regions(StripGrid(),dict(trace_status="complete",sides=sides,
                west_seeds=["C0201"],east_seeds=["C0202"]))


if __name__ == "__main__":
    unittest.main()
