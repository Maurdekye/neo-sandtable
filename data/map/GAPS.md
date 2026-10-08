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

- Libya, Egypt and their section-restricted selectors have resolved geometric
  memberships from the cited 84-incidence frontier. Known Sea belongs to
  neither country; unknown terrain retains geometric membership. The
  requires_land consumer offers only surveyed land in development and refuses
  incomplete land domains under full rules. No whole-map land survey follows.
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

## Frontier symbol discrepancy: adopted 2021 baseline

Lead rulings on 2026-10-08 adopt the dense blue ticked section-C line as
national Border, distinct from the gray crossed transport through cell centres.
The 2021 map supplies 83 full sides and C4122/C4221 printed only to the shore;
the remaining part is water. All 84 incidences are cited separately in
national-frontier.toml. No artificial seaward wall was added. Map-0005 records
this interpretation and the non-Sea flood fill. C4221 is Libyan; C4122 is
Egyptian. The fresh orientation supplement corrects C3920 to Libya and C3819
to Egypt at a returning bend; the original local source receipt stays intact.

The available 1979 scans contain rulebooks, but no original map. Corresponding
frontier incidence comparison is therefore unavailable; no equivalence is
claimed. Complete reduced overviews of A/B/D/E show no additional national
border at those scales, with no exhaustive native-side absence claim. Source
views and local tracing receipts remain outside this repository. Error rates
and whole-source independent agreement are unmeasured; the lead independently
confirmed only the northern coastal incidence.

Country geometry resolves libya=3878, egypt=3078, map_c_libya=912 and
map_c_or_d_egypt=1797 canonical cells. The 67 source-verified Sea cells have
neither country; all other 6956 cells, including unknown terrain, partition
without a gap. Requires_land filtering does not classify unknown cells.
Old movement reviews, border masks and rail-family abstentions remain
unchanged: this country trace is separate evidence, not a movement-layer
amendment. No other terrain, facility, lake or Italian extent is added.

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


Combined Village/Bir dot anchors are recorded for six cited scen:60.31 places.
The symbol does not resolve the separate village/bir water subtype or a place
extent. Giarabub's named oasis complex is not enumerated from its single dot;
Fort Maddalena's name does not establish fortification geometry or level.
Derna's visible port anchor remains withheld pending explicit coastal-cell
review. These records do not establish garrison placement or movement bounds.


## Corridor terrain abstentions after the map-team source review

The terrain worker retains these 27 original-share deferrals with individual
reasons in reviews/map-terrain-0002,0004..0009.toml and matching notes:
D2030,D2120,D2132,D2223,D2319,D2332,D2516,D2523,D2530,D2730,D2824,D2830,
D3319,D3332,D3421,E2301,E2405,E3210,E3214,E3311,E3312,E3412,E3413,E3414,
E3514,E3614,E3713. No balanced mixed substrate or unmatched gray/green symbol
family is resolved by RGB or regional plausibility. E3713's city-drawing
spillover does not establish a city selector.

E3413/E3414/E3514/E3614 are also unresolved in coastal domain. Their in-cell blue
water has no confidently established marine, lake or river identity; visible
land and apparent water connectivity do not prove a coastal flag. Empty flags
plus known-land notes retain this uncertainty without either a terrain or a
coastal-domain mask. No lake geometry or evidence is inferred. Separate lake
schema/consumer support must land atomically before any lake observations.

Previously recorded parent mixed-substrate and C4026 coastal-fragment gaps are
unchanged. All line/side abstentions remain indexed by their exact batch records.
Pipeline is unsurveyed on the new worker edges; gray tied rail families, contour
terminations/high endpoints and ambiguous marine/road endpoints retain unknown
kind masks. Source observations do not certify a complete control halo or an
engine action. Six Village/Bir dots still do not determine water subtypes or
place extents; Derna and Tobruk port-anchor gaps remain unchanged.

### Three-worker edge-cycle abstentions, 2026-10-07

The exact records in `map-lines-0006`, `map-lines-2-0005` and
`map-terrain-edges-0001` retain 318 unresolved kind observations: pipeline on
all 192 pairs, 38 rail-kind decisions, three border decisions, eighteen
river-kind decisions, 54 slope/ridge decisions, five road decisions and eight
marine-side decisions. These supply no mask. Gray square-block identity,
contour junctions/high endpoints and inland blue-water identity remain
unresolved. No lake absence follows from a marine or river observation.

C3832 and C4026 terrain remain unknown. Reviewed marine sides incident to
C4026 do not classify that cell's land substrate. All 33 corridor terrain gaps
and the four unknown coastal domains listed above are unchanged; no new route,
control halo or unit-action certification is supplied by this edge cycle.

### Second assembled edge cycle, 2026-10-07

The next three immutable worker inputs cover 225 fresh corridor pairs while retaining 288 unresolved kind observations without masks. Pipeline is unknown on all 225; unresolved records also retain exact rail, river, marine endpoint, contour/high-side, road and track uncertainty. After these inputs the raw union records some evidence on 868 corridor pairs and leaves 1,673 entirely untouched. Western/inland/eastern assigned inventories are respectively 344/969, 104/732 and 420/840 observed pairs; these are geographic coverage counts, not full movement-kind acceptance.

All 33 terrain gaps and the unknown lake layer remain open. The new marine sides do not resolve endpoint substrate, inland water identity, city extent, complete control halos or action legality. Previously published abstentions remain immutable; no negative lake coverage is inferred.

### Third assembled edge-cycle abstentions, 2026-10-07

The three immutable inputs `map-terrain-edges-0003`, `map-lines-2-0007` and `map-lines-0008` retain 473 unresolved observations with no masks: pipeline on all 296 pairs, 81 slope and 81 ridge decisions, eight rail-kind identities, five track endpoints and two escarpment decisions. Broad contour bands, incidence near vertices, exact rail identity and high-side uncertainty are preserved in the per-side notes. No lake absence follows from the existing fourteen-kind survey.

The union has some evidence on 1,164 corridor pairs and leaves 1,377 wholly unobserved. All 33 terrain gaps remain open. C3832 remains unclassified on C3732/C3832, C3733/C3832, C3831/C3832 and C3832/C3833; edge facts do not resolve its substrate. No city extent, complete control halo, new route certification or action permission is inferred.

### Fourth assembled edge-cycle abstentions, 2026-10-07

The three new immutable inputs retain 401 unresolved observations with no masks: pipeline on all 300 pairs, 31 slope and 31 ridge decisions, 22 escarpment junction or direction decisions, twelve rail-kind identities and five track endpoints. Partial contour incidence, mixed gray/ochre junctions, source high-side uncertainty and exact route crossings remain explicit in their own records. D3410/D3411 slope and D2019/D2119 partial gray-side uncertainty are retained rather than inferred from neighboring classes.

The union leaves 1,077 corridor pairs wholly unobserved. All 33 terrain gaps and four unknown coastal domains remain unchanged. Marine and contour facts do not resolve lake identity, endpoint substrate, city extent, full control halos or action legality. Historical thirteen-kind completeness does not certify the unsurveyed lake layer.


### Fifth assembled edge-cycle abstentions, 2026-10-07

The three immutable inputs retain 431 unresolved observations without masks: pipeline on all 300 pairs, 42 slope and 42 ridge decisions, 22 escarpment identity or direction decisions, 24 rail-kind identities and one track endpoint. Partial contour incidence and high-side ambiguity remain in the raw records. No lake absence is inferred from the current fourteen-kind survey. The 33 corridor terrain gaps, including four cells with unknown coastal domain, remain unchanged; source-reviewed edges do not classify those cells or certify a route, control halo or action.


### Sixth assembled edge-cycle abstentions, 2026-10-07

The three inputs retain 320 unresolved observations without masks: pipeline on all 260 pairs, 22 slope and 22 ridge decisions, eleven escarpment decisions, two rail-kind identities and three marine endpoints. Exact unresolved identities and reasons remain in each immutable raw review. C3226/C3227 rail-family uncertainty and the authoritative C3124 terrain deferral on C3124/C3223 and C3124/C3224 remain explicit. No missing lake evidence becomes an absence mask, and source-reviewed edges do not resolve any of the 33 corridor terrain gaps or certify routes, halos or actions.


### Seventh assembled edge-cycle abstentions, 2026-10-07

The three raw inputs retain 379 unresolved observations without masks: pipeline on all 260 pairs, 53 slope and 53 ridge decisions, two escarpment decisions, two railroad and two unfinished-railroad identities, two border identities, three road-family decisions and two marine endpoints. Exact identities and reasons remain in the immutable raw reviews. C3019/C3120 and C3028/C3129 tied or coincident line/border symbols stay unresolved. C3024/C3124 and outside-corridor D2705â€“D2710 surface gaps remain authoritative. Eastern whole-share observation completion still leaves 1,260 kind abstentions, including every pipeline. The new seven-edge track strip covers the current thirteen movement kinds only; missing lake evidence, adjacent halo and action legality remain unknown. No edge review resolves the 33 corridor terrain gaps.


### Final fresh corridor-edge inventory limits, 2026-10-07

The four final inputs retain 368 unresolved decisions without masks: pipeline on all 257 pairs, 51 slope and 51 ridge decisions, and three railroad, three unfinished-railroad and three border identities. Exact outcomes and reasons remain in the immutable source reviews. No negative lake mask is inferred. Existing 33 corridor terrain gaps and all unknown outside-corridor endpoint surfaces remain authoritative.

Every corridor physical pair now has some observation, but the following thirteen historic western pilot pairs have no current review or coverage for the listed kinds. These missing records remain unknown, distinct from explicitly authored abstentions. Existing source observations are preserved under the no-repeat physical-pair ruling. Additional missing-kind review remains a separate task before claiming current-kind completeness.

- C3820/C3921: `line:pipeline`, `line:railroad`, `line:road`, `line:track`, `line:unfinished_railroad`, `line:unfinished_road`, `side:border`, `side:ridge`, `side:slope`, `side:wadi`.
- C3821/C3921: `line:pipeline`, `line:railroad`, `line:road`, `line:track`, `line:unfinished_railroad`, `line:unfinished_road`, `side:border`, `side:ridge`, `side:slope`, `side:wadi`.
- C3920/C3921: `line:pipeline`, `line:railroad`, `line:road`, `line:track`, `line:unfinished_railroad`, `line:unfinished_road`, `side:border`, `side:ridge`, `side:slope`, `side:wadi`.
- C3920/C4020: `line:pipeline`, `line:railroad`, `line:road`, `line:track`, `line:unfinished_railroad`, `line:unfinished_road`, `side:border`, `side:ridge`, `side:slope`, `side:wadi`.
- C4019/C4020: `line:pipeline`, `line:railroad`, `line:road`, `line:track`, `line:unfinished_railroad`, `line:unfinished_road`, `side:border`, `side:ridge`, `side:slope`, `side:wadi`.
- C4020/C4021: `line:pipeline`, `line:railroad`, `line:road`, `line:track`, `line:unfinished_railroad`, `line:unfinished_road`, `side:border`, `side:ridge`, `side:slope`, `side:wadi`.
- C4020/C4121: `line:pipeline`, `line:railroad`, `line:road`, `line:unfinished_railroad`, `line:unfinished_road`, `side:border`, `side:ridge`, `side:slope`, `side:wadi`.
- C4320/C4419: `line:pipeline`, `line:railroad`, `line:unfinished_railroad`, `side:all_sea`, `side:border`, `side:escarpment`, `side:major_river`, `side:minor_river`, `side:ridge`, `side:slope`, `side:wadi`.
- C4320/C4420: `line:pipeline`, `line:railroad`, `line:track`, `line:unfinished_railroad`, `line:unfinished_road`, `side:all_sea`, `side:border`, `side:escarpment`, `side:major_river`, `side:minor_river`, `side:ridge`, `side:slope`, `side:wadi`.
- C4321/C4420: `line:pipeline`, `line:railroad`, `line:unfinished_railroad`, `line:unfinished_road`, `side:all_sea`, `side:border`, `side:escarpment`, `side:major_river`, `side:minor_river`, `side:ridge`, `side:slope`, `side:wadi`.
- C4322/C4421: `line:pipeline`, `line:railroad`, `line:road`, `line:unfinished_railroad`, `line:unfinished_road`, `side:all_sea`, `side:border`, `side:escarpment`, `side:major_river`, `side:minor_river`, `side:ridge`, `side:slope`, `side:wadi`.
- C4420/C4421: `line:pipeline`, `line:road`, `line:unfinished_railroad`, `side:all_sea`, `side:border`, `side:escarpment`, `side:major_river`, `side:minor_river`, `side:ridge`, `side:slope`, `side:wadi`.
- C4420/C4521: `line:pipeline`, `line:railroad`, `line:track`, `line:unfinished_railroad`, `line:unfinished_road`, `side:all_sea`, `side:border`, `side:escarpment`, `side:major_river`, `side:minor_river`, `side:ridge`, `side:slope`, `side:wadi`.

### Approved Italian Campaign allocation and phase 2, 2026-10-07

Neo-sandtable approved the 5,379-cell priority digitization extent for
`scen:60.23`, with exact scratch membership SHA256
`96f658ff42f229161b594501c9057e93ccaf828febc372a26346724fc12fc91b`.
It contains 15,638 internal and 312 crossing edges, including 13,373 wholly
unobserved pairs at `62ca87a`. This approval allocates map work; it does not
change legal scenario boundaries or certify scenario support.

Phase 2 explicitly retains the full 7,023-cell published grid, all free D/E
placement choices, and country, facility and off-map resolution. Arbitrary
placement outside the priority set must remain unreviewed and fail closed;
the priority allocation must never silently restrict a legal setup choice.
The full-grid envelope has 20,605 internal edges and no crossing edges.

The Graziani terrain and historic pilot-kind gaps finish first. New Italian
terrain work precedes edges in each area, starting with Tobruk/Bardia,
then Derna/Mechili, Msus/Beda Fomm and Benghazi, Agadabia/El Agheila,
and finally oasis and Egyptian support areas. Only one checked cycle runs
at a time. No new Italian source classifications are claimed here.

### Pipeline state and provisional lake contact, 2026-10-07

The lead ruled that pipelines are scenario, construction and operating state,
not a static map survey requirement. Existing `line:pipeline` records remain
immutable historical evidence. They must carry no required-survey or play
blocking weight in the future compatible map/consumer contract. Relevant
state evidence is `scen:60.2`, `airlog:52.22`, `airlog:52.24`,
`airlog:52.25` and `airlog:40.67`; printed transport alone establishes none
of that state. Current implementation changes remain a separate atomic task.

Provisional `map-0004` proposes lake contact only where printed inland water
covers the shared-side midpoint and a majority of its length. Minor or
endpoint-only contact does not qualify. Ambiguous or unreadable sides remain
Unknown; existing marine and river observations supply no lake absence.
The initial source-only sample has eight potential inland-water contact
sides and four controls, with no published lake masks. The full corridor
candidate count remains unmeasured until the paced candidate inventory.
The rule's water subtype remains unsourced unless printed, and Land's
compatible consumer belongs to a later atomic slot.


### Graziani gap reinspection, 2026-10-08

This entry supersedes the earlier 33-cell and missing-pilot-slot inventories.
Fresh terrain amendments resolve C4026 as clear coastal land, D2223 and D2332
as clear land with minor rough and mountain substrates, and D3332 as coastal
salt marsh with minor clear substrate. The following 29 corridor cells remain
unclassified after fresh full-cell source review:

C3024, C3124, C3832, D2030, D2120, D2132, D2319, D2516, D2523, D2530, D2730, D2824, D2830, D2917, D3319, D3414, D3421, E2301, E2405, E3210, E3214, E3311, E3312, E3412, E3413, E3414, E3514, E3614, E3713.

E3413, E3414, E3514 and E3614 retain unknown coastal domains and empty flags;
they supply neither terrain nor coastal masks. Mixed substrates, ambiguous
water, contour interference and city graphic spillover remain documented in
the new `zz-map-terrain-gap-0001` through `0011` reviews. Outside-corridor
unknowns remain unchanged. Edge observations do not classify these cells.

Fresh partial-kind review `map-lines-0015` records all 142 previously missing
supported-kind slots on thirteen western historic pilot pairs and separately
reviews the historic C4320/C4419 pilot-track abstention. It resolves 118
identities and retains 25 explicit Unknowns: thirteen legacy pipeline slots,
four railroad and four unfinished-railroad identities, two border identities,
and slope/ridge at C4020/C4021. Every exact unresolved identity remains in the
new raw record with no mask. The fresh C4320/C4419 track decision is absent;
the old unresolved pilot input remains unchanged as historical evidence.

The corridor has no never-observed supported-kind slots left. It still has
1,110 unresolved static-kind identities when legacy pipeline slots are
excluded according to the lead's game-state ruling. Historical pipeline
records remain unchanged; consumer and required-layer compatibility changes
are a later atomic task. No lake absence, complete Bardia-Matruh strip,
control halo or unit action legality follows from this inventory.

### First Cyrenaica terrain wave, 2026-10-08

The three 75-cell reviews preserve 15 explicit terrain Unknowns:
C3808, C4008, C4108, C4206, C4305, C4403, C4606, C4702, C4703,
C4710, C4711, C4712, C4713, C4714 and C5004. Reasons remain in each
immutable raw cell record: substrate balance, contour/shoreline obscuration
or unresolved water identity. C3808, C4008 and C4108 have empty flags and no
terrain or coastal-domain masks. Five coastal land fragments C4710..C4714
and C5004 retain their directly reviewed marine coastal domain while their
land substrate remains unknown. No guessed classes replace these deferrals.

The first wave adds 189 known-land cells inside the source-reviewed Libya
partition. Existing country definitions and frontier evidence are unchanged;
new verified Sea is excluded only by regenerating derived memberships.
Unknown geometric members still prevent a complete FULL free-placement domain.
C4807 major-city terrain does not resolve the withheld Tobruk port anchor or
establish a closed city extent, capacity, water, garrison or fortification fact.
Subsequent Cyrenaican cells, later campaign areas and lake/side consumers
remain separate paced work; this terrain wave certifies no new edge or action.


Northern Cyrenaica continuation retains unknown terrain at C3902, C3702,
C3606, C3507, C3406, C3306, C3205, C3908, C3708, C3608, C3508 and C3408.
Their individual review notes record unresolved substrate predominance or
water identity. C3908, C3708, C3608 and C3508 have known visible land but
unresolved coastal domains and empty flags; neither domain nor terrain is
certified there. Other eight deferrals have reviewed noncoastal domains.
Further source allocations remain paced work; this continuation adds no
edge, lake, named-place, facility or complete setup/action certification.


### Standing C-Libya cycle 1 deferrals, 2026-10-08

C2804, C2605, C2314, C2213 and C2718 retain unknown terrain because their
visible land substrates do not have a confidently predominant class. Their
individual raw notes retain both the observed substrates and reasons for
abstention. All five have directly reviewed noncoastal domains, so they
receive only domain masks; no terrain mask is supplied. Earlier Unknowns
remain unchanged.

This terrain-only cycle adds no lake, movement-side, port, place or facility
evidence. Continuous terrain work follows the standing approved phase;
other layers still require their own authorization. Geographic country
membership alone does not make an Unknown terrain cell usable for setup.
