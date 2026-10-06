# map-0001 - VASSAL 2021 numbering versus original seam description

- **Cases:** land:4.1, land:8.19, land:8.8, land:15.35, airlog:34.11, airlog:55.21, scen:60.31, scen:60.41, scen:60.44, scen:60.5
- **Status:** proposed
- **Profile version:** geometry `vassal-2021`, schema 1; no playable rules profile yet
- **Decided by:** cartographer, 2026-10-06 (proposal authorized by neo-sandtable)
- **Owner review:** pending

## Question

The original assembly description refers to successive 01xx/39xx overlap rows,
but the available module and scenario ids describe a northward first axis and
roughly 33 columns on the eastward second axis. Which coordinates can we safely
publish before original sheets are available?

## Evidence

`land:4.1` orders A-E west to east and describes the 01xx/39xx overlap. The local
PNG is explicitly the 2021 newly rendered VASSAL map, not a scan of the original
1979 sheets. It has no embedded hex numbers; the module's grid overlays them.
Its terrain boundaries and hexside features may differ from the original map.

The main board's A-E zone/grid definitions produce 7055 contained locations and
7023 canonical cells. Grid origins, numbering offsets and stagger yield
`r=63-first`, `q=second+east_offset-floor(r/2)`, with east offsets
0,33,66,99,132. The apparent raw offset 133 on E is corrected by that section's
shifted image origin. Source polygons are not published as paths.

Scenario anchors C4807 Tobruk, C4321 Bardia, C4131 Sidi Barrani, D3714 Mersa
Matruh and E3613/E3714 Alexandria fit named hexes on local overlays. Marble Arch A2109 (land:8.19), Aboukir E3815 and Rosetta E4019
(airlog:55.21) also agree; the C3101-C3133 flight distance is 32 hexes
(airlog:34.11). Additional A/B/C/E landmarks and setup ids are recorded in data/map/VERIFICATION.md. Tripoli
is an off-map box, so it supplies no ordinary grid anchor. The Benghazi token
`84827` is garbled; its candidate A4827 is documented separately and not adopted
as a silent scenario correction.

The module exposes 32 duplicate positions at C-D and D-E: boundary `00` names
share the same axial cell as the western section's `33` names, and local centers
lie in the same visible hex despite small registration differences. No original
01xx/39xx aliases can be established from that evidence.

## Ruling (proposed)

Publish the reproducible `vassal-2021` coordinate profile for board and engine
integration, validating against scenario ids. Resolve only duplicate memberships
actually observed in the module. Prefer positive second-axis ids over boundary
`00` ids for canonical names, then west-to-east section order. Reject locations
outside the published mask. Keep the original assembly conflict unresolved; do
not invent aliases from the assembly text or declare original-sheet coverage.

## Rationale

This yields a usable continuous grid without substituting a conflicting assembly
formula for independently located scenario hexes. The choice is explicit and
can be migrated if original sheets later supply better evidence.

## Affected behaviour and tests

`data/map/sections.toml`, `hexes.csv`, `aliases.csv`, `tools/map/geometry.py` and
`generate_grid.py` implement the geometry. Tests cover all canonical round trips,
six setup anchors, neighbour reciprocity and seam continuity, missing ids, and
alias plumbing. Source-only overlays remain outside the repository.

Before terrain claims correspondence to 1979, obtain original sheets and compare
stratified terrain/section samples, boundary hexes, directional features and
referenced facilities; until then record the limitation in GAPS/VERIFICATION.

## Numbering implementation reference

The local module declares VASSAL 3.6.1. The independently consulted current
upstream implementation explains the sideways axis swap, numbering offsets,
zone-height-dependent descending axis and optional row stagger:
[HexGridNumbering](https://github.com/vassalengine/vassal/blob/master/vassal-app/src/main/java/VASSAL/build/module/map/boardPicker/board/mapgrid/HexGridNumbering.java),
[RegularGridNumbering](https://github.com/vassalengine/vassal/blob/master/vassal-app/src/main/java/VASSAL/build/module/map/boardPicker/board/mapgrid/RegularGridNumbering.java),
[Zone](https://github.com/vassalengine/vassal/blob/master/vassal-app/src/main/java/VASSAL/build/module/map/boardPicker/board/mapgrid/Zone.java).
This is a method reference, not an assertion that master is byte-identical to 3.6.1;
scenario/rules landmarks on the local map provide the independent validation.
No upstream implementation code has been copied into this repository.
