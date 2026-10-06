# land-0018 — Construction Chart versus the Section 24 case texts

- **Cases:** land:24.17, land:24.35, land:24.44, land:24.73, land:24.83, land:24.84, land:24.9, land:6.3
- **Status:** proposed
- **Profile version:** v1
- **Decided by:** rules-land, 2026-10-06
- **Owner review:** pending

## Question
Several values in the Construction Chart (24.17) differ from the matching case texts, so the engine needs one source of truth per item.

## Evidence
| Item | Case text | Construction Chart |
|---|---|---|
| Minefield terrain | clear, sand/gravel, rough (24.35) | clear, sand/gravel, salt marsh |
| Fortification terrain | not mountain, salt marsh, desert, major city, delta (24.44) | not salt marsh, delta, major city |
| Air facility terrain | clear, rough, major city, desert, sand/gravel (24.73) | clear, major city, desert, sand/gravel |
| New temporary repair facility | 250 Stores, 150 Fuel, three Construction Segments (24.83) | 250 Stores, 50 Fuel, one stage |
| Rebuild one repair-facility level | 50 Stores, 30 Fuel (24.84) | 50 Stores, 10 Fuel |
| Real supply dump | 3 CP, 20 Stores (24.9) | 3 CP, 10 Stores |
| Dummy supply dump | 3 CP (24.9) | 2 CP, no supplies |

The Capability Point Cost Summary (6.3) gives the supply-dump cost as "3/2" for "Real or Dummy/Non-Dump", which agrees with the chart (real 3, fake 2) and not with the case text. All other chart entries (fortification 30 Stores and 3 stages, real minefield 15 Ammo + 15 Stores, dummy 3 Stores, road 2 Stores per hex, railroad 1 Store, airfield 50 Fuel + 100 Stores and 3 stages) agree with the text.

## Ruling
The Construction Chart is the source of truth for supplies, stage counts, CP costs and terrain restrictions, because case 24.13 defers to it for supplies and because 6.3 independently confirms the chart's supply-dump figures. The case texts are used where the chart is silent (for example the rules for pinned builders and storm effects). The 6.3 "3/2" entry is read as real dump 3 CP, fake dump 2 CP. (This resolves the open question flagged on the 6.3 table; the table file itself keeps the printed "3/2".)

## Rationale
The chart is the single consolidated list, and one independent confirmation of its numbers exists. Owners wanting the text values can flip this per item.

## Affected behaviour and tests
Construction validation. Test: a fake supply dump costs 2 CP and no stores; a fortification may be started in Mountain hex under this ruling.
