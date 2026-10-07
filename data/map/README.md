# Map data: coordinate profile vassal-2021

This first milestone supplies the discrete grid for all five A-E sections.
**Terrain coverage: 228 classified cells; 6795 remain unclassified. One port and six city cells (Cairo five, Benghazi one) are recorded. This content is not yet playable.** The local map is a
2021 VASSAL re-rendering, not a scan of the original 1979 sheets. Scenario ids are
our independent numbering anchors. The original seam description conflicts with
this source; see [GAPS.md](GAPS.md) and the [interpretation proposal](../../docs/interpretations/map-0001-grid-numbering.md).

## Numbering and orientation

North is up and east is right on every section, ordered A-E west to east. Hexes
are pointy-top with vertical east/west edges. In `C4218`, `42` is the north axis
(increases northward) and `18` is the local east axis (increases eastward).
The names `printed_first` and `printed_second` preserve this order; they do not
assert that numbers are embedded in the local image (VASSAL overlays them).

For the source-contained ids in this profile:

```text
r = 63 - printed_first
q = printed_second + east_offset - floor(r / 2)
east_offset: A=0, B=33, C=66, D=99, E=132
```

`q` increases east; `r` increases southeast. Offset rows with odd `r` lie half a
hex-width to the east of even rows. At radius `s`, draw regular hex centers at
`x = sqrt(3)*s*(q+r/2)`, `y = 1.5*s*r`. Scan registration constants in
`sections.toml` belong only to the local inspection pipeline; the board draws
from axial coordinates. Floating point pixels never determine game distance.

| Direction | dq | dr |
|---|---:|---:|
| E | 1 | 0 |
| SE | 0 | 1 |
| SW | -1 | 1 |
| W | -1 | 0 |
| NW | 0 | -1 |
| NE | 1 | -1 |

Distance is `max(abs(dq), abs(dr), abs(dq+dr))`. A neighbour outside the published
mask is absent, not a new terrain hex. `tools/map/geometry.py` looks up exact
membership before converting; it rejects id-shaped strings in excluded water,
UI boxes, or outside the image. Example anchors: Tobruk C4807=(66,15), Bardia
C4321=(77,20), C4218=(74,21), C4120=(75,22), Matruh D3714=(100,26),
Alexandria E3613=(132,27).

## Extents, mask and aliases

| Section | Source locations | Canonical hexes | Alias ids |
|---|---:|---:|---:|
| A | 1007 | 1007 | 0 |
| B | 1959 | 1959 | 0 |
| C | 1520 | 1520 | 0 |
| D | 1261 | 1240 | 21 |
| E | 1308 | 1297 | 11 |
| Total | 7055 | 7023 | 32 |

The mask is each VASSAL zone's contained grid centers, including its narrow sea
fringe. It is not a rectangular product of min/max numbers. Some edge locations
have second axis `00`, and E has a few `34` locations. Keep leading zeros in ids;
never discard boundary column 00 by syntax alone. Malta and off-map holding or
transit boxes have separate identities; they are not ordinary A-E axial hexes.

The module has 32 duplicate positions: D0200..D4200 on even first-axis values
resolve to C0233..C4233; E1200..E3200 on even values resolve to D1233..D3233.
Both names must actually occur in source-contained locations before an alias is
recorded. Canonical naming prefers a positive second axis to boundary `00`, then
west-to-east section letter order. No aliases from the unresolved `land:4.1`
01xx/39xx statement are inferred. The lookup plumbing is tested with synthetic
aliases separately from these observed module aliases.

## Published schemas (version 1)

`sections.toml`:
- `schema_version`, `coordinate_profile`, `orientation`,
  `original_printed_seams` (currently `unresolved`), `src` (citation array),
  `geometry_source` (relative local source identity), `build_file_sha256`.
- `[[sections]]`: `id`, `north_axis_constant`, `east_offset`, `first_min/max`,
  `second_min/max`, `hex_count` (source memberships), `canonical_hex_count`,
  `alias_count`. Min/max is descriptive, never a membership test.
- Registration fields per section: `dx`, `dy`, `x0`, `y0`, `max_rows`, `h_off`,
  `v_off`, `stagger`, copied as numeric geometry data. For sideways grids the
  raw axes swap: map x uses `dy` and `y0`, map y uses `dx` and `x0`.
- `[[off_map_boxes]]`: `name`, `hex_id` (empty until linked/verified), `src`
  (citation array). These are a source-zone inventory, not validated transport
  routes, capacities or a claim that every rules-defined area is complete.

`hexes.csv`: `hex_id,section,printed_first,printed_second,q,r,terrain,flags,src`.
One row per canonical hex, sorted by `hex_id`.
The pinned base vocabulary uses TEC row ids: `clear`, `gravel`, `salt_marsh`,
`heavy_vegetation`, `rough`, `mountain`, `delta`, `desert`, `major_city`, `swamp`,
`village_bir_oasis`, plus the legend-only value `sea`. `gravel` corresponds to
legend `rock_gravel`. Sea has no movement-cost row in the TEC. Published classifications currently use
`clear`, `rough`, `sea`, `salt_marsh`, `major_city`; village/bir/oasis markers must not replace the
underlying terrain because their TEC row inherits the other terrain's costs. Axes are decimal integers; ids are
strings. `terrain=unclassified` is a missing value, not a TEC class. Blank flags
mean unknown, not false. Flags are pipe-delimited tokens. `sea` means entirely water. Mixed cells publish their readable land substrate
with `land|coastal`, or `coastal` alone when the substrate remains unreadable.
See [map-0002](../../docs/interpretations/map-0002-coastal-terrain.md).
`port` is supplied by a separate reviewed place record, currently C4022.
`major_city` is also a place-derived flag for the five Cairo city cells and Benghazi A4827.
These flags describe observed map surface, not complete facilities or movement
permission. Missing a flag never establishes the absence of an unreviewed layer. CSV citations
are semicolon-delimited case references. Current `src=land:4.1` identifies the
map/assembly case, with exact geometry provenance in the section metadata; it
does not mean the disputed seam text proves the extracted mask. Classified
terrain additionally cites `land:8.37` and its local verification evidence.

`aliases.csv`: `alias_id,hex_id,src`, sorted by alias. Targets are canonical
records; aliases cannot shadow a canonical id or point to another alias.

`grid-preview.svg`: original regular hexagons constructed only from the axial
records. Section colors distinguish geometry, not terrain. No scan pixels,
source polygons, coastline paths or source text occur in the SVG.

`reviews/*.toml`: immutable batch metadata and explicit per-cell visual decisions.
`[batch]` has `id,coordinate_profile,source_image_sha256,build_file_sha256,
observer,observed_on,verification,proposal_algorithm,selection,notes`. `[[hex]]`
has `hex_id,status,terrain,flags,src,proposed_terrain,note`; status is accepted or
deferred. Only accepted records publish terrain. Deferred records remain
unclassified with a reason. Image hashes bind reviews to exact local evidence;
source changes require re-review. No raster pixels or image crops are stored.
Optional `batch.supersedes` names an earlier batch: only explicitly listed hexes
from that batch are amended; others are retained. Filename sorting defines replay
order. Silent duplicate decisions, invalid targets and duplicate batch ids fail.
Optional `[[place]]` records pin a locally verified facility using `id,name,hex_id,
type,src,note`. These share the batch's exact source hashes and observer metadata.

`terrain-preview.svg`: original flat terrain colors over the regular axial grid.
Unclassified cells are gray, including deferred coastal cells. It is generated
solely from `hexes.csv` and contains no raster or traced coastline.

`places.toml`: `schema_version,coordinate_profile,complete,verification` plus
`[[places]]` with `id,name,hex_id,type,src,note,review_batch`. Currently types
`port` and `major_city` are verified; Cairo city entries share `place_group="cairo"`; Benghazi uses `place_group="benghazi"`. Sollum's port is C4022; its nearby town dot is C4021, which
has not been published as a place. Missing capacities/attributes are unknown.
The replay tool validates the coastal port location and matching city terrain and generates the place file and
its generic SVG marker. This is an incomplete inventory, not all map facilities.

## Movement layers and coverage (schema 1)

`layers.toml` pins `schema_version=1`, `coordinate_profile`, exact source/build
hashes, `line_kinds`, `hexside_kinds`, `cell_layers`, `edge_coverage`,
`unknown_policy` and `verification`. Existing geometry/hex CSV schemas stand.
Source hashes identify the 2021 profile; they do not establish 1979 equivalence.

`line_features.csv`: `from_hex,to_hex,kind,src,review_batch`.
Kinds: `road`, `unfinished_road`, `track`, `railroad`, `unfinished_railroad`,
`pipeline`. Each row connects adjacent canonical hexes through their shared
side. Endpoints are lexicographically sorted (`from_hex < to_hex`); the line is
undirected. A visible road somewhere in both hexes is insufficient: the same
line must cross their shared boundary. Several kinds can coexist on an edge;
one row per kind, no duplicate keys. Retain unfinished kinds as printed;
movement applies land:8.47 and construction state separately. Pipeline is
separate from railway (airlog:52.2); no pipeline is inferred from railroad
presence. All rows have semicolon-separated case citations and a review batch.

`hexsides.csv`: `hex_id,direction,neighbour_id,feature,high_side,src,review_batch`.
Features: `escarpment`, `slope`, `ridge`, `wadi`, `major_river`, `minor_river`,
`border`, `all_sea`. The canonical endpoints are lexicographically sorted;
`hex_id` is the first endpoint, `neighbour_id` the second. Direction is one of
the six published axial directions **from hex_id to neighbour_id**. Store each
physical edge once; queries from the opposite side reuse that record. Multiple
feature types may share it. `high_side` is a required endpoint id for slope or
escarpment and blank for other features. Ascending means moving from the other
endpoint to high_side; descending reverses it. The downhill splash identifies
the lower side (land:8.35); unreadable direction cannot be accepted. Boundary
edges without a neighbour in the grid are not movement connections and are not
invented. `all_sea` means the full shared boundary is water (land:10.21); endpoint
coastal flags do not prove it. Border identity remains unresolved where the
source symbol conflicts with the key. Base feature kinds map to the TEC's
up/down cost rows at movement time; those rows are not extra map features.

`coverage.csv`: `layer,hex_id,neighbour_id,src,review_batch`.
**Each row certifies a complete inspection for exactly one layer and domain.**
The cell layers `terrain` and `coastal` have a blank neighbour. Edge layers are
`line:<kind>` and `side:<feature>`, using the kinds above. Their endpoints are
sorted canonical neighbours. Masks are explicit sets of these rows, never
inferred from a bounding rectangle, a hex's land flag, another layer's mask,
a high classifier score, or the presence of any nearby feature. Every positive
line/side record must have matching coverage. Deferred/ambiguous edges stay
outside the mask; rejected proposals do not certify feature absence.

Inside a particular edge-layer mask, no matching feature row means **verified
none for that kind**. Outside it the answer is **unknown**, even if the file is
empty. Consumers must block or explicitly surface incomplete adjudication;
unknown cannot fall back to clear ground, ordinary CP cost or zero hexside cost.
A road mask says nothing about tracks, pipelines or escarpments. Within coastal
cell coverage the flag is true if `coastal` exists, otherwise false (including
All-Sea cells); outside it coastal status is unknown. Terrain coverage permits
reading the accepted TEC class. The unreadable land fragment C4026 has coastal
coverage but no terrain coverage. Place/facility absence has **no coverage mask
yet**, so a missing place record always remains unknown. A later schema will
pin facility-kind masks before certifying any absence there.

Current publication contains **228 terrain cells, 229 coastal-domain cells** and
**55 surveyed line-kind edges**: 16 road, 21 track, 10 railroad and eight
unfinished-road masks from individually reviewed pilot rows. Nine features are
published (five road, four track). All hexside layers, unfinished railroad and
pipeline remain unknown everywhere. No mask covers the entire work window. `tools/map/layers.py`
validates canonical adjacency, coverage, duplicate keys and directional features;
`feature(family,kind,a,b)` returns a row, `None` for covered absence, or raises
`UnknownCoverage`. It accepts display aliases at the query boundary only.
Review batch ids on surface masks reference the existing source-locked reviews.
Future edge reviews must explicitly accept both presence and absence with exact
source hashes, observer/date, citations and confidence/evidence; proposals alone
are not published. Confidence is a classifier score, not measured accuracy.

### Approved Graziani digitization bounds

`graziani-window.toml` is a reproducible **work-priority window**, not a legal
movement boundary, a national region or a new scenario restriction. Its status
is `approved_digitization_window`, approved by neo-sandtable on 2026-10-06. Use all
source-contained rows with these inclusive printed-second-axis bounds:

| Section | Columns | Canonical memberships before union |
|---|---|---:|
| C | 07 through 33 | 1218 |
| D | 00 through 33 (entire section) | 1261 |
| E | 00 through 14 | 486 |

Resolve source aliases before union: **2933 canonical hexes**, **8547 internal
neighbour edges**, plus **175 edges to grid cells outside the window**. Include
those crossing edges in feature inspection, so a window limit cannot invent a
broken route. The mask includes the source-contained sea fringe for coastal and
All-Sea checking. Land extent is determined by review, not this window. E14
includes Alexandria E3714 as well as E3613; C07 includes Tobruk C4807.
The file pins exact canonical `hex_ids`, counts, section bounds and grid hash;
`scenario_rule=false` keeps this scope distinct from play restrictions.

### Semi-automatic verification plan

Calibrate interior patches for terrain, bands around side midpoints for
hexsides, and corridors between registered centres for connecting lines. Use
multiple sample offsets to tolerate displaced symbols, labels and contour marks.
Assign per-kind confidence and abstain on mixed/overlapping/unrecognized symbols;
preserve unfinished lines and high-side direction as separate decisions. Keep
pixel samples, labelled sheets and overlay images outside the repository.

Review every low-confidence proposal and a reproducibly seeded random sample of
high-confidence positives **and negatives** separately for each kind. Log the
seed, population, sample ids, disagreements, abstentions and corrected records.
Report observed errors with denominator and uncertainty per layer; report false
positives and false negatives separately. If the sample finds systematic misses,
expand review and recalibrate before increasing masks. Existing convenience
terrain samples are not random. The first seeded line pilot and its limitations
are documented in VERIFICATION.md; its sample rates do not establish full-map
accuracy.

## Area and off-map identities (schema 1)

`area-definitions.toml` is the cited selector/identity input. `generate_areas.py`
replays it against published grid membership and aliases to write `areas.toml`.
This does not complete national-region digitization or off-map range data.

`areas.toml` has `schema_version,coordinate_profile,complete,build_file_sha256,
definitions_source`, `[[locations]]`, and `[[areas]]`. Locations have stable
`id,kind,name,off_map,range_status,src`, optional `facility_type,printed_location,
reference_hex,reference_status,departure_hex,note`. All current distances remain
unresolved. `reference_hex` retains a normalized printed locator, never an
ordinary occupancy hex or a verified flight connection. Deversoir and Kabrit
share E(1833) but are distinct locations. An ambiguous printed token must fail
lookup rather than merge facilities. `Off-Map` alone is similarly ambiguous.

Areas have `id,kind,src,membership_status,hex_ids,location_ids` plus selector
fields (`sections`, `country`, `requires_land`) or a `reason` where needed.
Membership status is `resolved`, `unresolved`, or `requires_state`. The empty
arrays on an unresolved area are placeholders, **never an empty legal placement
set**. `tools/map/areas.py` raises explicitly for unresolved regions or dynamic
facilities; callers must preserve this distinction when loading the TOML.
Resolved section selectors include canonical seam cells through alias section
membership, not only the section letter of each canonical name. They define
geometric sets; unit-specific movement/placement restrictions still apply.

| Requested ID | Meaning/status |
|---|---|
| `map_a` through `map_e` | Exact canonical membership of each source section |
| `map_a_or_b`, `map_d_or_e` | Exact section unions, including observed aliases |
| `libya`, `egypt`, `map_c_libya`, `map_c_or_d_egypt` | Stable IDs; unresolved frontier/full-land membership |
| `tripoli`, `tripolitania`, `gabes`, `tunis` | Separate main boxes from land:8.81 |
| `tripoli_tunisia_boxes` | The four boxes above; no transit boxes included |
| `tunisia_boxes` | Gabes and Tunis boxes |
| `italy`, `sicily`, `crete`, `axis_mediterranean_bases` | Separate bases and their union; campaign availability is separate |
| `offmap_abu_seier`, `offmap_deversoir`, `offmap_kabrit` | Off-map facilities with retained printed references |
| `offmap_fayid`, `offmap_ismailia`, `offmap_port_said` | Other distinct off-map facilities |
| `malta` | Symbolic region containing `offmap_malta`; no African-grid hex or individual airfield implied |
| `alexandria`, `helwan` | Cited ordinary hex sets; Helwan is E1430 |
| `benghazi` | Verified city set A4827, source-locked review benghazi-0001; nearby El Berca/Benina excluded |
| `cairo` | Verified city set: E1930, E1931, E1829, E1830, E1730; Helwan remains separate |
| `any_air_facility` | Requires friendly control, construction and capacity state |

Symbolic location IDs are `box_<name>` for the seven main boxes/bases and the
six `offmap_<facility>` IDs, plus `offmap_malta`. Malta has kind `box`, grounded
in airlog:44.11, and printed_location `Malta Box`. Its inset uses a separate scale
and geographic placement, so it supplies no coordinates in the African axial
grid. The `malta` selector resolves to that symbolic location only. Initial
aggregate capacity (scen:60.46) stays in scenario state; individual Maltese fields,
mission target hexes and flight distances still need separate source verification.
These are separate from canonical grid IDs and
carry no invented axial coordinates. `Areas.within(hex,n)` generates a sorted
canonical grid set by integer hex distance, includes the center, resolves source
aliases, rejects invalid radii, and never invents cells outside the grid. It does
not apply dynamic enemy-distance exclusions or facility capacity constraints.

## Regeneration and checks

Python 3.11+; stdlib suffices for data and preview. Run from the repository root:

```powershell
$env:CNA_SOURCES = 'C:\Users\ncola_k8bx\AppData\Roaming\Orgtree v2\data\workspaces\maurdekye-works\cna-sources'
py -3.12 tools/map/generate_grid.py
py -3.12 tools/map/apply_terrain.py
py -3.12 tools/map/publish_layer_schema.py
py -3.12 tools/map/generate_areas.py
py -3.12 -m unittest discover -s tools/map -v
```

`--output <folder>` permits regeneration into a temporary directory for comparison.
Existing terrain/flags/citations are preserved when the grid's membership is
unchanged; a changed membership aborts and requires explicit migration. Source
polygons are held only in memory. The generator does not open the map image.

For local inspection, install Pillow outside the repo and run
`python tools/map/verify_overlay.py --output <scratch-folder-outside-repo>`.
This opens the local re-rendered map and writes overlays to that outside folder.
The script refuses outputs inside either the repository or source directory.
See [VERIFICATION.md](VERIFICATION.md) for actual checks and limits.


Terrain proposals are local-only and require Pillow. Example pilot command:

```powershell
python tools/map/propose_terrain.py --section C --first 35 42 --second 18 27 --output ../runs/terrain/graziani-0001
```

Review every selected drawn hex in the resulting contact sheet, record decisions
in a source-locked batch, then run `apply_terrain.py` (Python 3.11+ stdlib) to
rebuild reviewed classifications and the preview. The proposal algorithm only
recognizes four measured solid colors and can abstain on contours or labels;
it is not a complete terrain classifier. Never publish its output automatically.
The original pilot deferred 11 mixed shoreline cells. Amendment graziani-0002
resolves ten under map-0002; C4026 remains unreadable. Batch graziani-0003
adds 70 inland cells at C first28..34/second18..27.
`apply_terrain.py` rejects source changes, duplicate ids, non-TEC classes and
unreviewed existing classifications that would otherwise be lost.

`publish_layer_schema.py` rechecks local source hashes and visual surface decisions
before regenerating the initial layer files and approved window. It refuses to
erase nonempty edge features or edge coverage; replace this initialization step
with the reviewed-edge replay at the first edge-data milestone.

### Line proposals, audits and accepted replay

`propose_lines.py` samples a band along each shared side and short crossing
corridors. Paired brown strokes propose roads; gray dash/tie patterns propose
tracks and railroad. Its confidence numbers are uncalibrated heuristic scores,
including the score attached to an absence proposal. It never publishes a mask.
Unfinished roads always abstain in this version; unfinished railroad and pipeline
have no classifier. A source-grid line or an obscured crossing can still confuse
the model, so every accepted edge has an individual visual decision.

With Python3.10+ and Pillow, write proposals and inspection sheets outside the
clone. Use a fresh empty output folder: the command refuses to overwrite reviews.

```powershell
python tools/map/propose_lines.py --section C --first 44 48 --second 17 22 --seed 6031 --sample 8 --output ../runs/lines/new-validation
```

The generator writes numeric `proposals.csv`, seeded `audit.csv`, `metadata.json`
and local PNG sheets. Fill the audit's `observed` with present, absent or
unresolved, and explain the visual evidence in `note`. `audit_lines.py <folder>`
checks the exact seeded cohort, unchanged predictions and population counts,
then reports positive/negative errors and abstentions separately. Unresolved
labels are counted and excluded from accuracy denominators; an empty resolved
sample has unknown accuracy rather than zero error.

Only numeric review bundles enter `data/map/line-reviews/<batch>/`: proposals,
audit decisions and metadata. Explicit `publish_reviewed_labels=true`, source
image/build hashes, LF-normalized CSV hashes, review basis and per-kind case citations are
required. This acceptance is for resolved audit rows only, not unsampled model
predictions. `publish_layer_schema.py` checks and replays these bundles with
`line_reviews.py`, generating `line_features.csv` and the matching per-kind
coverage rows. Unresolved or unaudited edges remain unknown. Duplicate batches,
stale sources, altered CSVs, noncanonical/nonadjacent endpoints and removal of
existing surveyed edges fail. Hexside replay is still pending; the publisher
refuses to erase future hexside evidence. Schema1 and consumer query semantics
are unchanged.


### GT1-6 digitization corridor

`graziani-corridor.toml` is the approved, narrower work envelope inside the
approved Graziani window. It is not a movement restriction or an assertion
that any layer is complete. Regenerate with
`py -3.12 tools/map/generate_corridor.py`; canonical membership includes source
section aliases. The stepped printed bounds follow the coastal latitude and
roughly ten rows inland through Matruh, then include the road/rail approach to
Alexandria. The manifest has 834 cells, 2,326 internal edges and 215 crossing
edges. Border, road, railroad and escarpment identities cannot be inferred
from membership; consult each feature's coverage mask.
