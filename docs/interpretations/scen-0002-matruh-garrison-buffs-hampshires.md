# scen-0002 — Matruh Garrison: 1st Buffs and 1st Hampshires

- **Cases:** scen:60.41, land:4.44b (item 2), land:4.45 (Selby Force sheet, British unassigned infantry sheet)
- **Status:** proposed
- **Profile version:** v0
- **Decided by:** oob (proposed), 2026-10-06
- **Owner review:** pending

## Question
The scenario lists three battalions attached to the Matruh Garrison at D3714 (1st Essex, 1st Durham
Light Infantry, 1st South Staffordshires). Where are the 1st Buffs and the 1st Hampshires?

## Evidence
- land:4.44b item 2 says the 1st Buffs and 1st Hampshires start the campaign and the Italian
  scenarios assigned to the Matruh Garrison formation.
- The British unassigned-infantry OA sheet marks both "D" (deployed at start) with an asterisk meaning
  "attached to the Matruh Garrison".
- The Matruh Garrison sheet says no units may be assigned to it, but up to six may be attached.
- Neither unit appears in any other scenario deployment line.

## Ruling
Both battalions start **attached** (not assigned) to the Matruh Garrison HQ at D3714, giving five
attached battalions (the limit is six).

## Rationale
"Assigned" in the erratum is read loosely, since the sheet forbids assignment; the asterisk gives the
attached reading, and both must start on the map as D-arrival units.

## Affected behaviour and tests
`land_cw.toml` group `cw_matruh` (`att` list); `[[mention]]` records on the Selby Force sheet.
