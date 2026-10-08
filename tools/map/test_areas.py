"""Placement sets must honor seam membership, gaps and off-map identity."""
import copy
from pathlib import Path
import tempfile
import shutil
import tomllib
import unittest
from areas import Areas, UnresolvedArea, CampaignStateRequired
from generate_areas import materialize,write
from frontier import known_sea_cells, load_national_regions, section_members
import csv

MAP=Path(__file__).resolve().parents[2]/"data/map"

class AreaTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.lookup=Areas(MAP)
        cls.definitions=tomllib.loads((MAP/"area-definitions.toml").read_text())

    def test_section_sets_include_canonical_seam_members(self):
        d=self.lookup.members("map_d").hex_ids
        self.assertIn("C4233",d)
        self.assertNotIn("D4200",d)
        self.assertEqual(len(d),1261)
        self.assertEqual(len(self.lookup.members("map_d_or_e").hex_ids),2558)
        self.assertEqual(len(self.lookup.members("map_a_or_b").hex_ids),2966)

    def test_unresolved_regions_and_dynamic_facilities_fail_explicitly(self):
        # A future unresolved membership must fail, even after countries resolve.
        with tempfile.TemporaryDirectory() as tmp:
            folder=Path(tmp)
            for name in ["hexes.csv","aliases.csv","sections.toml"]:
                shutil.copyfile(MAP/name,folder/name)
            data=copy.deepcopy(self.lookup.data)
            area=next(a for a in data["areas"] if a["id"]=="libya")
            area.update(membership_status="unresolved",hex_ids=[],reason="Unreadable synthetic frontier")
            write(data,folder/"areas.toml")
            with self.assertRaises(UnresolvedArea):Areas(folder).members("libya")
        with self.assertRaises(CampaignStateRequired):self.lookup.members("any_air_facility")

    def test_frontier_countries_partition_only_non_sea_and_retain_unknown_members(self):
        grid=self.lookup.grid
        with (MAP/"coverage.csv").open(newline="",encoding="utf-8") as stream:
            sea=known_sea_cells(grid,csv.DictReader(stream))
        regions=load_national_regions(grid)
        for id,members in regions.items():
            self.assertEqual(set(self.lookup.members(id).hex_ids),members)
            self.assertTrue(self.lookup.areas[id]["requires_land"])
        self.assertFalse(regions["libya"]&regions["egypt"])
        self.assertEqual(regions["libya"]|regions["egypt"],set(grid.hexes)-sea)
        self.assertEqual(regions["map_c_libya"],section_members(grid,{"C"})&regions["libya"])
        self.assertEqual(regions["map_c_or_d_egypt"],section_members(grid,{"C","D"})&regions["egypt"])
        self.assertIn("C4221",regions["libya"])
        self.assertIn("C4122",regions["egypt"])
        self.assertIn("C3920",regions["libya"])
        self.assertIn("C3819",regions["egypt"])
        self.assertTrue(any(grid.hexes[h]["terrain"]=="unclassified" for h in regions["libya"]))

    def test_box_group_is_four_distinct_symbolic_locations(self):
        group=self.lookup.members("tripoli_tunisia_boxes")
        self.assertEqual(set(group.location_ids),{"box_tripoli","box_tripolitania","box_tunis","box_gabes"})
        self.assertEqual(group.hex_ids,())

    def test_shared_offmap_printed_reference_does_not_merge_facilities(self):
        self.assertNotEqual(self.lookup.members("offmap_deversoir"),self.lookup.members("offmap_kabrit"))
        with self.assertRaisesRegex(ValueError,"ambiguous"):self.lookup.printed_location("E(1833)")
        self.assertEqual(self.lookup.printed_location("E(3433)")["id"],"offmap_abu_seier")
        self.assertEqual(self.lookup.members("offmap_abu_seier").hex_ids,())
        self.assertIn("E3433",self.lookup.grid.hexes)

    def test_within_uses_grid_distance_and_never_creates_hexes(self):
        self.assertEqual(self.lookup.within("C4218",0),("C4218",))
        self.assertEqual(len(self.lookup.within("C4218",1)),7)
        self.assertEqual(self.lookup.within("D4200",0),("C4233",))
        with self.assertRaises(ValueError):self.lookup.within("C4218",-1)

    def test_cairo_resolves_complete_reviewed_city_group(self):
        self.assertEqual(set(self.lookup.members("cairo").hex_ids),{"E1930","E1931","E1829","E1830","E1730"})
        self.assertEqual(self.lookup.members("helwan").hex_ids,("E1430",))

    def test_benghazi_uses_map_city_group_not_nearby_markers(self):
        self.assertEqual(self.lookup.members("benghazi").hex_ids,("A4827",))
        area=next(a for a in self.definitions["areas"] if a["id"]=="benghazi")
        self.assertEqual(area["review_batch"],"benghazi-0001")
        self.assertFalse({"A4728","A4829","B4827"}&set(self.lookup.members("benghazi").hex_ids))

    def test_unknown_location_cannot_publish(self):
        defs=copy.deepcopy(self.definitions)
        next(a for a in defs["areas"] if a["id"]=="tripoli")["location_ids"]=["invented"]
        with self.assertRaisesRegex(ValueError,"Unknown location"):materialize(defs,self.lookup.grid)

    def test_generated_areas_reproduce_published_bytes(self):
        with tempfile.TemporaryDirectory() as tmp:
            out=Path(tmp)/"areas.toml";write(materialize(self.definitions,self.lookup.grid),out)
            self.assertEqual(out.read_bytes(),(MAP/"areas.toml").read_bytes())

if __name__ == "__main__":unittest.main()
