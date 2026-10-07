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
- **Benghazi:** adopted scen-0001 garrison correction is now map-confirmed:
  city building group A4827, independently checked with 40 halo cells and a
  shared-boundary enlargement. The city area contains only A4827. El Berca's
  A4728 village and Benina A4829 airfield do not enlarge it. Original-map
  comparison, port attributes and separate training/facility data remain pending.
- **Water mask:** zone-contained centers include a narrow sea fringe; offshore
  image hexes outside A-E zones (and Malta) are not in this grid. Surface-domain flags
  exist only for the 229 reviewed cells; the rest are unknown. Completeness refers only to the VASSAL A-E mask.
- **Edge artifacts:** A includes partial column-00 hexes; E includes column 34.
  Confirm their playability and any off-map entry meanings against rules data.
- **Boxes:** current inventory is names only. No distances, capacities, routes or
  attachment hexes are inferred from placement or names in the module UI.

## Classification and rendering

- Six review batches cover 229 distinct terrain cells: 228 classified (191 clear,
  22 rough, seven sea, two salt marsh, six major city); 6795 remain unknown.
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
  One port and six city records (Cairo five, Benghazi one) are published; other facilities remain pending.
- Base TEC vocabulary is pinned; feature extraction still needs local verification.
- `grid-preview.svg` remains a section-colored geometry preview;
  `terrain-preview.svg` adds the 228 reviewed terrain fills and generic place markers. Hexside and facility
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
covers228 cells; coastal-domain mask covers229. No place-absence mask exists.
The approved Graziani priority window contains2933 canonical cells and8547
internal edges, plus175 crossing edges; this is not a scenario boundary.
Semi-automatic line/hexside classification and random error auditing remain
pending. Existing palette proposals cannot certify edge absence or orientation.


## First line-pilot limitations

- Fifty-five kind/edge masks cover reviewed rows only; most of the approved
  window remains unknown. Do not extend negative decisions to adjacent edges.
- C4320/C4419 has a verified road crossing, but the nearby dashed segment lies
  nearly on the shared side. Track crossing attribution remains unresolved.
- Track false positives still occur at grid/contour strokes. Railroad positive
  accuracy is unmeasured on the separate strip; earlier calibration confused
  escarpments with railway ties.
- Unfinished road remains an always-abstaining proposal layer; eight reviewed
  absences do not establish a classifier. Unfinished railroad and pipeline have
  no coverage. All hexside layers, including directional escarpments, remain
  unreviewed. No movement cost or route absence may be inferred there.


## Malta inset

The area `malta` resolves to the symbolic location `offmap_malta`, outside the
African axial grid (airlog:44.11; scen:60.46). Individual airfield identities,
inset target hexes, Valetta port anchoring and transfer distances remain
unverified. The aggregate scenario facility allowance does not resolve those
geographic details or assign capacity to individual fields.


GT1-6 corridor narrowing does not resolve unknown movement edges. The
corridor-d-0001 surface batch adds no escarpment/slope high-side evidence,
wadis or railway/track/road absence masks. Connected playable strips need
both accepted terrain and every movement-relevant edge layer; even visually
clear terrain must not imply no escarpment. Full-corridor completeness and
population accuracy remain unverified.


First map-complete route segment: C4220-C4120-C4020 only (road-spine-0001).
The subsequent sollum-control-0001 branch publishes three directional
escarpments: C3921/C3922 and C3921/C4021 with C3921 high, plus
C4020/C4121 with C4020 high. The wider control halo and
pipeline layer remain partial/unknown, as do the next coast-road edges. Do not infer full-spine completeness or legal unit moves from this
two-edge strip. Some blue interrupted/tied lines resemble unfinished railroad
family in TEC.png; this does not identify every boundary-coincident block
or establish country membership. See the exact-side abstentions below.
Paired dashed Via Balbia strokes east of Sollum match unfinished road.
Specific crossing IDs and their other layers still require review.


Observed playback side C3921/C4020: a thin dashed track is verified, but
railroad, unfinished_railroad and border remain unknown. The gray tied
stroke and blue side overlay need additional source identification; do
not infer one from the other or from the board's dev-profile assumption.
C4020/C4021 contour identity/orientation remains unresolved. The confirmed
C4020/C4121 contour/track has other line kinds still unknown.
Three immediate blocker kinds are resolved around C3921, but other
neighbor-to-cell entry layers can still be unknown. No complete control
halo, full-profile move or national frontier is asserted.


Bardia road C4220/C4320 and C4320/C4321: finished road is confirmed, but
brown contour type and orientation are unresolved (slope and ridge both
unknown). Pipeline unsurveyed. These partial masks do not extend the
complete C4220-C4120-C4020 strip. Training-area symbols at Bardia and the
village point in C4419 remain outside the currently supported facility
review kinds; their identities/capacities are not invented.

The lead's second opinion on Sollum blue blocks and gray tied strokes
suggests possible border/rail families, but does not verify either exact
kind. C3921/C4020 rail kinds and border remain unknown. Pricing relevance
is a rules procedure question and does not alter map masks.

Provisional map-0003 mixed-land abstentions: C2822,C2922,C3024,C3124,
C3832,D2917,D3414 have no clear predominant land substrate on direct
re-review. Their surface mask is removed; coastal/domain evidence remains.
C4026 retains its independent unreadable coastal fragment. Owner batch3
may overturn predominance; old decisions and minor substrates are retained
in source-bound review amendments. Earlier clear-cell minor-substrate
audit remains incomplete; no fullmap reinterpretation audit is claimed.


Port-anchor audit ports-0001: Benghazi A4827 and Mersa Matruh D3814
are verified point records. Tobruk port remains unregistered because
its circled anchor overlaps the C4807/C4908 shared side in the2021 image.
The55.3 port chart has no map locator, and scenario C4807 is a garrison
anchor rather than proof of the port hex. Capacity/efficiency, training
and city extent are separate evidence; absent place records remain unknown.

## Alexandria drawing spillover

E3613/E3714 have verified city symbols; E3613 has the circled port. E3713 contains neighboring city-drawing spillover of unresolved terrain significance. Positive records do not establish a closed extent. No Alexandria place_group, capacity or training inventory is inferred; the existing scenario-specific area is unchanged.
