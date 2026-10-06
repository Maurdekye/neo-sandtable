# land-0012 — Morale Modifier table: reading 56 uncovered in the -4 cohesion row

- **Cases:** land:17.4, land:17.22
- **Status:** proposed
- **Profile version:** v1
- **Decided by:** rules-land, 2026-10-06
- **Owner review:** pending

## Question
In the Morale Modifier Table the Cohesion Level -4 row prints no change 11-33, minus 1 for 34-41, minus 2 for 42-55 and minus 3 for 61-66. Dice reading 56 (a valid sequential reading) has no printed modifier. Every other row covers all 36 readings exactly once.

## Evidence
- The cell values were read twice (the second time at 4x zoom) and are as printed.
- Expected modifier by row: -3 = -0.78, -4 = -1.17 (with reading 56 unassigned), -5 = -1.69. Giving 56 to minus 2 yields -1.22 and to minus 3 yields -1.25; both are smooth, so the progression does not decide it.
- The charts booklet is not in the 1979 scans available, so the original printing could not be consulted.

## Ruling
Reading 56 in the -4 row gives a modifier of -2 (the less severe adjacent result). The printed table is stored unchanged; the engine applies this fill through the interpretation hook for the table.

## Rationale
Ties between equally plausible fills go to the less severe adjacent result; the choice is easy to flip if the owner prefers the other reading.

## Affected behaviour and tests
Morale adjustment lookup (17.22). Test: cohesion -4, reading 56 gives -2; a table-coverage test asserts that every other row covers all readings once.
