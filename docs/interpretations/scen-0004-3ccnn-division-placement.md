# scen-0004 — "3 CCNN Div" at Tripoli has no divisional HQ

- **Cases:** scen:60.31, land:4.45 (3rd CCNN sheet), land:19.27
- **Status:** proposed
- **Profile version:** v0
- **Decided by:** oob (proposed), 2026-10-06
- **Owner review:** pending

## Question
Scenario 60.31 puts "3 CCNN Div (I)" at Tripoli, but the 3rd CCNN OA sheet states the division had
no headquarters and its units are independent, and no "3 CCNN" counter exists in the module.

## Evidence
- OA sheet: 250th Legion HQ (brigade-level) with three battalions, and three battalion-level
  independent units (203rd MG Bn, 214th Artillery Regt, 203rd Engineer Bn; the last asterisked as
  assigned to a parent that never arrived, land:19.27).
- The 64th Catanzaro sheet notes that its 203rd Artillery Regt was originally assigned to the dispersed
  3rd CCNN (distinct from the 214th on the 3rd CCNN sheet).

## Ruling
Place every unit of the 3rd CCNN sheet at Tripoli (the legion with its battalions and the three
independent units); no divisional HQ exists.

## Rationale
It is the only reading consistent with both sources.

## Affected behaviour and tests
`land_axis.toml` group `it_tripoli` (`sheet = "it.3ccnn_div"`).
