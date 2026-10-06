# map-0002 - Coastal domain and land substrate

- **Cases:** land:5.2, land:8.37, land:10.21, land:24.73, land:29.46, land:30.14, land:30.21, airlog:33.0, airlog:36.3
- **Status:** adopted
- **Profile version:** geometry `vassal-2021`, map schema 1; no playable rules profile yet
- **Decided by:** neo-sandtable, 2026-10-06
- **Owner review:** reviewed 2026-10-06 by the owner (batch review 1)

## Question

How should a hex containing both land and sea represent terrain when the TEC
lists coastal symbols but has no coastal movement-cost row?

## Evidence

The fleet assignment sequence in land:5.2 / airlog:33.0 permits sea and coastal
locations. Naval stacking distinguishes coastal from all-sea locations
(land:30.14); bombardment operates from coastal locations (land:30.21).
Sandstorms end at the coast (land:29.46). Flying boat construction allows a
coastal hex independently of its terrain (land:24.73; airlog:36.3 also restricts
basins to coastal locations). Thus coastal domain and terrain are distinct.
ZOCs are blocked by all-sea hexsides, among other features (land:10.21).

## Ruling

`terrain=sea` and flag `sea` require an entirely water hex. Any visible land
fragment makes the cell coastal. Publish its visibly identified TEC land
substrate as `terrain` with flags `land|coastal`. If that substrate is
unreadable, keep `terrain=unclassified` and flag `coastal`; do not use majority
pixel color. Facilities remain separate place records, not terrain classes.
No new coastal terrain category or hexes.csv schema change is introduced.

Plan `all_sea` in hexsides.csv for edges wholly in water, including edges
between two coastal cells where bays or inlets separate their land portions.
An endpoint's coastal flag alone never establishes this feature: inspect the
whole drawn edge. Do not infer movement permission or naval capacities from
these map classifications.

## Rationale

Independent coastal and terrain fields preserve the rule distinction and
avoid calling a small land fragment impassable sea because its hex is mostly
blue. C4026 demonstrates this failure of the automatic proposal.

## Affected behaviour and tests

Reviews graziani-0002/0003 and apply_terrain.py publish visible substrate with
coastal flags. C4026 remains explicitly deferred. Tests pin its unknown
substrate, accepted coastal substrate, reject sea/coastal combinations and
require explicit batch-targeted amendments instead of silent overrides.
The first verified port record is at C4022; other facilities and hexsides
remain incomplete. Original 1979 sheet verification remains unavailable.
Later contrary rule evidence requires a superseding interpretation.
