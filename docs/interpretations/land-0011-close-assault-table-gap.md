# land-0011 — Close Assault table: defender +2 column has a gap at readings 34-36

- **Cases:** land:15.79
- **Status:** adopted
- **Profile version:** v1
- **Decided by:** rules-land, 2026-10-06
- **Owner review:** reviewed 2026-10-06 by the owner (batch review 1)

## Question
In the defender half of the Close Assault Combat Results Table, the +2 column prints the 10% line as 24-33 and the 5% line as 41-52. Dice readings 34, 35 and 36 therefore have no printed result. Every other column of both halves covers all 36 readings exactly once, so this looks like a printing error of the kind the Sept 1979 errata already fixed for the +4 column.

## Evidence
- Transcribed twice from the chart image (and zoomed at 5x): the printed cells are as stated.
- Expected defender loss (percent of Raw points, weighted by dice readings) across neighbouring columns: 0 = 6.94, +1 = 7.08, +3 = 8.89, +4 = 10.56. Filling the three readings at +2 with 10% gives 7.92 (a smooth step between 7.08 and 8.89); filling with 5% gives 7.50; leaving them as zero gives 7.08 (no increase over +1).
- The 1979 scans in the sources contain only the rulebook text, not the Charts and Tables booklet, so the original printing could not be checked.

## Ruling
Readings 34, 35 and 36 in the defender +2 column give a 10% loss. The printed values are kept unchanged in data/tables/land/15.79; the engine applies this fill through the interpretation hook for this table.

## Rationale
The 10% fill produces the smoothest progression of expected losses and matches the pattern in neighbouring columns where the 10% range ends just below the 5% range.

## Affected behaviour and tests
Close Assault loss lookup (15.73a). Test: defender +2 column, reading 35, yields 10%; a table-coverage test asserts that every other column covers all readings once.
