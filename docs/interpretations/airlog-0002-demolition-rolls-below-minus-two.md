# airlog-0002 — Supply dump demolition rolls outside the printed columns

- **Cases:** airlog:54.14, airlog:54.17 (Supply Dump Demolition Table)
- **Status:** proposed
- **Profile version:** v0.1
- **Decided by:** rules-airlog, 2026-10-06
- **Owner review:** pending

## Question
The die roll is modified by up to four cumulative groups of modifiers (about -4 to +5 or more from
the modifiers alone), but the chart's columns run from -2 to "8 or more". A modified roll of -3 or
lower has no printed column.

## Evidence
The chart prints 0% destroyed for -2, -1 and 0; the errata notes only that the -1 and +7 cells are
0% and 100%.

## Ruling
Any modified roll of -2 or lower destroys 0%. A roll of 8 or more destroys 100% as printed.

## Rationale
Results increase monotonically from -2 and the three lowest printed columns are all 0%, so extending
the lowest column downward is the only continuation consistent with the chart.

## Affected behaviour and tests
Dump demolition; table `airlog.54.17.supply_dump_demolition`. Test: modified roll -5 -> 0%.
