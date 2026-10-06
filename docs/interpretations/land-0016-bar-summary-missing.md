# land-0016 — Breakdown Adjustment Summary (21.14) is not in the sources

- **Cases:** land:21.12, land:21.14
- **Status:** proposed
- **Profile version:** v1
- **Decided by:** rules-land, 2026-10-06
- **Owner review:** pending

## Question
Case 21.14 refers to a Breakdown Adjustment Summary chart listing the Breakdown Adjustment Rating (BAR) by vehicle type, with an asterisk for types listed individually on the Tank & Gun Characteristics Charts. The 2021 text and the VASSAL chart images contain no such chart, so the general-type BAR values are not available from our sources.

## Evidence
- 21.12 gives two examples only (Italian M 13/40 is 1R, Commonwealth Sherman 0; all German tanks 0 per 21.36) and a correction from 2R to 1R.
- 21.34 gives trucks as 2L in its worked example and Crusader I as 1R.
- The 4.4x Tank and Gun Characteristics charts (units area) carry a BAR per weapon type; the generic list for trucks and armored cars would be the missing part.

## Ruling
The engine reads each tank, gun or truck type's BAR from the units data (Tank and Gun Characteristics Charts, owned by the units area). For truck points and armored recce/car points, whose generic BAR appears only on the missing summary, the units area should record the BAR from the Characteristics Charts if printed there; otherwise the value is a data gap and the example values (trucks 2L) are used until the chart is located. Registered as unresolved with this interpretation.

## Rationale
Avoids inventing values; keeps the Breakdown procedure fully implementable from per-type data.

## Affected behaviour and tests
Breakdown column selection (21.13, 21.32). Test: a truck type with BAR 2L accumulating 35 points in hot weather rolls on the 21-30 column (worked example in 21.34).
