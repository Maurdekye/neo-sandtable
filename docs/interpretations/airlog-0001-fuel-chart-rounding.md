# airlog-0001 — Fuel burned for CP counts between chart rows

- **Cases:** airlog:49.13, airlog:49.19 (Fuel Consumption Chart)
- **Status:** adopted
- **Profile version:** v0.1
- **Decided by:** rules-airlog, 2026-10-06
- **Owner review:** reviewed 2026-10-06 by the owner (batch review 1)

## Question
Case 49.13 prices fuel per five CP "or fraction thereof" (its own example turns 12 CP into three
groups of five), but the Fuel Consumption Chart prints fractional costs for 1-4 CP (one fifth of the
consumption rate per CP) and lists whole rows only at 5, 10, ... 50 CP. What does a unit pay for a CP
count that is not printed, for example 6 or 12 CP, and what does it pay for 1-4 CP?

## Evidence
The 49.13 text rounds the number of five-CP groups up. The chart is exactly rate x CP / 5 in every
cell, including the fractional cells under 5 CP, and 49.13's note that no fuel is used for CP spent
on non-movement does not address fractions.

## Ruling
Fuel for one movement segment = consumption rate x (CP spent moving, rounded UP to the next row
printed on the chart) / 5, using the chart's 1-4 CP rows as printed for CP counts of 1-4. For
CP above 5, round up to the next multiple of 5. Costs are tracked in tenths of a Fuel Point and the
total drawn from a source is rounded up to a whole Fuel Point at the moment fuel is taken.

## Rationale
It reproduces the 49.13 worked example (12 CP at rate 4 = 12 fuel) and uses every printed chart cell
exactly, while remaining a total function over integer CP. The alternative (strict proportional
rate x CP / 5 for every CP count) would contradict the printed example.

## Affected behaviour and tests
Fuel deduction in movement; table `airlog.49.19.fuel_consumption`. Test: 12 CP at rate 4 = 12;
3 CP at rate 5 = 3; 7 CP at rate 2 rounds to the 10-CP row (4).
