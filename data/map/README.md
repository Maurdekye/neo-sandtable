# Map data: coordinate profile vassal-2021

This first milestone supplies the discrete grid for all five A-E sections.
**Terrain is unclassified; this content is not yet playable.** The local map is a
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
One row per canonical hex, sorted by `hex_id`. Axes are decimal integers; ids are
strings. `terrain=unclassified` is a missing value, not a TEC class. Blank flags
mean unknown, not false. Future flags are pipe-delimited tokens. CSV citations
are semicolon-delimited case references. Current `src=land:4.1` identifies the
map/assembly case, with exact geometry provenance in the section metadata; it
does not mean the disputed seam text proves the extracted mask. Classified
terrain must additionally cite `land:8.37` and its local verification evidence.

`aliases.csv`: `alias_id,hex_id,src`, sorted by alias. Targets are canonical
records; aliases cannot shadow a canonical id or point to another alias.

`grid-preview.svg`: original regular hexagons constructed only from the axial
records. Section colors distinguish geometry, not terrain. No scan pixels,
source polygons, coastline paths or source text occur in the SVG.

Planned terrain deliverables, **not yet published or complete**:
- `hexsides.csv`: canonical `hex_id,direction,neighbour_id,feature,high_side,src`;
  one feature per row. `high_side` records a directional escarpment's high hex,
  blank where inapplicable. Feature vocabulary will be pinned against the TEC.
- `places.toml`: `[[places]]` with `id,name,hex_id,type,src` and sourced attributes;
  `src` is a citation array. Lists of types and attributes will be pinned with
  the first verified facilities. Printed structures and dynamic scenario state
  must remain distinct.

## Regeneration and checks

Python 3.11+; stdlib suffices for data and preview. Run from the repository root:

```powershell
$env:CNA_SOURCES = 'C:\Users\ncola_k8bx\AppData\Roaming\Orgtree v2\data\workspaces\maurdekye-works\cna-sources'
py -3.12 tools/map/generate_grid.py
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
