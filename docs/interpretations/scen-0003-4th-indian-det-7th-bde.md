# scen-0003 — 4th Indian Division "Det: 7th In Bde" before it has arrived

- **Cases:** scen:60.41, land:4.43a, land:4.45 (4th Indian Division sheet)
- **Status:** proposed
- **Profile version:** v0
- **Decided by:** oob (proposed), 2026-10-06
- **Owner review:** pending

## Question
The scenario deploys the 4th Indian Division at D3615 with "Det: 31st Field Arty and 7th In Bde", but
the 7th Indian Brigade arrives at OpStage 1 of GT 5 (OA sheet and reinforcement schedule agree).

## Evidence
- 4th Indian OA sheet and land:4.43a: 7th Indian Bde HQ and its three battalions arrive 1/5 (OpStage 1,
  GT 5); the 5th and 11th Brigades are "D".
- The 31st Field Artillery is placed separately at C4131, consistent with "Det".
- No scenario line places the 7th Brigade.

## Ruling
Treat the "Det" of the 7th Indian Brigade as a no-op at the start: it is not on the map until its
scheduled arrival (GT 5 OpStage 1, in Cairo). The 4th Indian Division group at D3615 consists of the
HQ with the 5th and 11th Brigades, the Central India Horse and the 25th Field Artillery.

## Rationale
The brigade cannot be detached from the map before it exists; the schedule is the more specific
source about its presence.

## Affected behaviour and tests
`land_cw.toml` group `cw_d3615_4indian` (`det` keeps the printed list; the loader skips units whose
arrival is after the start); `schedules/land_cw.toml` GT 5 row.
