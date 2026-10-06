# Map gaps

## Geometry and source authority

- **Original seams (`land:4.1`):** original 01xx/39xx overlap conflicts with the
  VASSAL 2021 numbering and roughly 33 horizontal columns. The proposed profile
  preserves scenario ids and actual module duplicates; it does not claim to
  resolve original sheet seam aliases. Do not mark original-map coverage complete.
- **1979 map unavailable:** sources currently include original rulebook page
  scans, not original map sheets. The local PNG is a 2021 re-rendering. Its map
  details may differ. If original sheets become available, audit each section's
  terrain, coastline/boundary cells and every hexside/facility using a stratified
  sample before asserting correspondence to 1979; retain discrepancy records.
- **Benghazi id (`scen:60.31`):** extracted text has `84827`. B4827 falls inland
  in section B and cannot be Benghazi. The re-rendered city lies in A4827.
  This likely concerns text extraction, but checking the page is needed before
  publishing a corrected place id. Geometry includes both valid hexes and does
  not silently rewrite scenario text.
- **Water mask:** zone-contained centers include a narrow sea fringe; offshore
  image hexes outside A-E zones (and Malta) are not in this grid. Surface-domain flags
  exist only for the 228 reviewed cells; the rest are unknown. Completeness refers only to the VASSAL A-E mask.
- **Edge artifacts:** A includes partial column-00 hexes; E includes column 34.
  Confirm their playability and any off-map entry meanings against rules data.
- **Boxes:** current inventory is names only. No distances, capacities, routes or
  attachment hexes are inferred from placement or names in the module UI.

## Classification and rendering

- Five review batches cover 228 distinct terrain cells: 227 classified (191 clear,
  22 rough, seven sea, two salt marsh, five major city); 6796 remain unknown.
- **C4026 coastal fragment:** map-0002 resolves coastal semantics. Ten original
  coastal deferrals are now classified by land substrate. C4026 retains the
  coastal flag and unknown terrain because its tiny land fragment is unreadable.
  Its automatic sea proposal is rejected; majority color cannot erase land.
- **Sollum port:** printed anchor and label identify C4022. Published place
  `port-sollum` has no verified capacity or other attributes. The town dot is in
  C4021; no town record is silently inferred from the port's hex.
- The proposal classifier covers only clear/rough/sea and abstains on ochre
  contour color. D3315 proves clear proposals can conceal salt marsh. Salt marsh
  and major-city cells are visually identified; automated recognition remains
  unvalidated. Desert/gravel/vegetation/mountain/delta/swamp are not yet published.
- Hexside features remain undigitized, including all_sea edges across bays.
  One port and five Cairo city records are published; other facilities remain pending.
- Base TEC vocabulary is pinned; feature extraction still needs local verification.
- `grid-preview.svg` remains a section-colored geometry preview;
  `terrain-preview.svg` adds the 227 reviewed terrain fills and generic place markers. Hexside and facility
  artwork remains pending.

## Scenario area memberships and off-map range

- Stable region IDs libya, egypt, map_c_libya and map_c_or_d_egypt are published,
  but exact national frontier and complete land masks are not yet verified.
  Their membership_status is unresolved; empty lists are not legal placement sets.
- Cairo city cells E1930/E1931/E1829/E1830/E1730 are now visually enumerated and
  published as a resolved area. Helwan E1430 remains separate.
- Six off-map facility identities follow scen:60.5 text and oob's page
  transcription. Their printed references are retained without assigning ordinary
  grid occupancy, flight distance or facility capacities. E(1833) identifies both
  Deversoir and Kabrit, so names/IDs must disambiguate. All range_status values
  remain unresolved pending local map/table audit.
- Tripolitania is a main off-map box for this scenario selector, not a newly
  inferred polygon of western Libya. Four main Tripoli/Tunisia boxes are pinned
  by land:8.81; transit boxes and route distances are not included in this group.
- Italy/Sicily/Crete identities do not establish availability at any game date.
  Crete's initial unavailability belongs to scenario state, not static map data.
- B5825, C4119, D3231, D3416, D3516, D3903 are valid canonical grid IDs. This
  membership check does not verify facility symbols or supply/owner attributes.

## Frontier symbol discrepancy (requires lead ruling)

The prominent blue ticked line in section C, near the Libya/Egypt labels,
appears graphically similar to the TEC unfinished-railroad key, while its long
north-south alignment could denote a frontier or fence in the 2021 re-rendering.
Neither name/geometry alone proves its feature identity. The TEC border example
uses a different appearance. No border feature or country division has been
published from this line. Local labeled section/whole-map inspection overviews
are in cartographer scratch runs/frontier, outside the repo. Ask the lead to
establish the intended source symbol/authority before national sets depend on it.

## Movement-layer completeness

Schema1 now pins lines, directional hexsides and per-kind masks. No edges have
been reviewed: empty line/hexside CSVs mean unknown everywhere. Terrain mask
covers227 cells; coastal-domain mask covers228. No place-absence mask exists.
The approved Graziani priority window contains2933 canonical cells and8547
internal edges, plus175 crossing edges; this is not a scenario boundary.
Semi-automatic line/hexside classification and random error auditing remain
pending. Existing palette proposals cannot certify edge absence or orientation.
