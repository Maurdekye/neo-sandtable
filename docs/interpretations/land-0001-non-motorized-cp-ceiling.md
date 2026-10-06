# land-0001 — Non-motorized units' voluntary CP ceiling: 150% of CPA

- **Cases:** land:8.17
- **Status:** proposed
- **Profile version:** v1
- **Decided by:** rules-land, 2026-10-06
- **Owner review:** pending

## Question
Case 8.17 limits how many CP a non-motorized unit (CPA 10 or less) may voluntarily spend in its own portion of an Operations Stage. Which percentage of base CPA is the ceiling?

## Evidence
- The 2021 retype states 150% (8 may reach 12, 10 may reach 15) and carries a note that 150% was corrected from 50% in the original.
- The 1979 printing (land scan p14) prints "50%" of base CPA, which would be below the CPA itself and contradicts the worked examples (8 to 12, 10 to 15) in the same case.

## Ruling
The ceiling is 150% of base CPA (8 -> 12, 10 -> 15); Reaction and Retreat Before Assault are not voluntary spending in the unit's own portion and do not count. Units at cohesion -26 or worse may not move at all (6.26).

## Rationale
The 2021 text is the baseline (D3); the 1979 figure is a misprint refuted by its own examples.

## Affected behaviour and tests
Movement validation for non-motorized units. Test: an 8-CPA unit may spend 12 CP voluntarily and is refused a 13th; the same unit may react afterwards without that spend counting.
