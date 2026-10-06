# land-0015 — Cost for engineers and stacked units to enter an enemy minefield

- **Cases:** land:23.21, land:26.24, land:26.21, land:6.3, land:8.37
- **Status:** proposed
- **Profile version:** v1
- **Decided by:** rules-land, 2026-10-06
- **Owner review:** pending

## Question
Three places give different costs for a unit with an Engineer present entering an enemy minefield: 23.21 says six CP for motorized and three for non-motorized units; 26.24 says four additional CP; the Capability Point Cost Summary (6.3) says non-motorized with Engineers 2 + terrain, motorized with Engineers 4 + terrain, non-motorized without 4 + terrain, motorized without CPA + terrain. The Terrain Effects Chart gives +4 (non-motorized) and + CPA (motorized) with a footnote that Engineers reduce the cost.

## Evidence
6.3 is the only source that separates all four cases and its no-Engineer rows match the Terrain Effects Chart. 26.24's four additional CP equals the 6.3 motorized-with-Engineers row; 23.21's six and three match no table.

## Ruling
Use the Capability Point Cost Summary (6.3) for enemy and friendly minefield entry costs, with the Terrain Effects Chart supplying the plain (no Engineer) values. 23.21 and 26.24 are read as informal descriptions of the same Engineer discount.

## Rationale
It is the most detailed and the one that agrees with the chart in the no-Engineer cases.

## Affected behaviour and tests
Minefield entry cost. Test: a non-motorized unit stacked with an Engineer battalion enters an enemy minefield hex of Clear terrain for 2 + 2 CP; a motorized one for 4 + 2; without Engineers 4 + 2 and CPA + 2.
