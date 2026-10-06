# land-0006 — Slash entries on the Capability Point Expenditure Summary (6.3)

- **Cases:** land:6.3, land:24.17, land:24.9, land:27.73, land:30.0
- **Status:** proposed
- **Profile version:** v1
- **Decided by:** rules-land, 2026-10-06
- **Owner review:** pending

## Question
Two rows of the CP Expenditure Summary use compressed notation: "Construct a Real or Dummy/Non-Dump Supply Dump: 3/2" and "Commando amphibious landing: 5 or 10 + TEC". What do the slash and the "or" select between?

## Evidence
- The Construction Chart (24.17) lists a Real Supply Dump at 3 CP (plus 10 Stores) and a Fake Supply Dump at 2 CP (no supplies), which matches the 3/2 pairing in order (real, fake).
- Case 24.9's text gives a dummy dump as 3 CP, which disagrees (covered by interpretation land-0018).
- Case 27.73 prices a commando landing at 5 CP per 50 hexes of ship movement that OpStage, plus terrain; 27.83 repeats this for the SAS ("five Capability Points for every 50 Movement Points or fraction thereof").

## Ruling
Supply dump: real 3 CP, fake 2 CP (chart reading; the "Non-Dump" wording is not otherwise explained). Commando landing: 5 CP if the carrying ship moved up to 50 hexes that stage and 10 CP if it moved 51 to 100 (the range limit of 100 sea hexes, 30.15), plus the terrain cost of the landing hex. The table file keeps the printed notation.

## Rationale
Each reading is the only one supported by a second passage in the rules.

## Affected behaviour and tests
CP cost lookup for construction and commando landings. Test: a ship that moved 60 hexes landing Layforce costs 10 CP plus terrain.
