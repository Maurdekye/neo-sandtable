"""Placement sets must honor seam membership, gaps and off-map identity."""
import copy
from pathlib import Path
import tempfile
import tomllib
import unittest
from areas import Areas, UnresolvedArea, CampaignStateRequired
from generate_areas import materialize,write

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
        for id in ["libya","egypt","map_c_libya","map_c_or_d_egypt"]:
            with self.assertRaises(UnresolvedArea):self.lookup.members(id)
        with self.assertRaises(CampaignStateRequired):self.lookup.members("any_air_facility")

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
