# land-0019 — Weather Table: seasonal probabilities look inverted

- **Cases:** land:29.6, land:29.61, land:29.1
- **Status:** adopted
- **Profile version:** v1
- **Decided by:** rules-land, 2026-10-06
- **Owner review:** reviewed 2026-10-06 by the owner (batch review 1)

## Question
The errata says the Weather Table was "completely backwards" and that the correct seasonal sequence is the one in 29.1. The VASSAL chart image lists Game-Turn ranges for Fall, Winter, Spring and Summer that agree with 29.1, but the dice results printed in each season row look climatically reversed: Summer has no hot weather and a rainstorm on 53-66, while Winter has hot weather on 24-55 and sandstorms on 56-66 with no rain.

## Evidence
- Row cells (readings): Fall normal 11-42, hot 43-55, sandstorm 56-64, rainstorm 65-66; Winter normal 11-23, hot 24-55, sandstorm 56-66; Spring normal 11-35, hot 36-54, sandstorm 55-61, rainstorm 62-66; Summer normal 11-52, rainstorm 53-66.
- Each row covers all 36 dice readings, so the table is internally consistent as printed.
- The errata locates the fix in the season sequence (the Game-Turn labels), which the retype has applied; it says nothing about the dice cells.
- Other rules treat hot weather as an important summer feature (29.3, 24.21, 21.37), which the Summer row never produces.

## Ruling
Swap the dice rows of Summer and Winter: the cells printed under Summer apply to the Winter
game-turns, and the cells printed under Winter apply to the Summer game-turns. Fall and Spring are
used as printed. The transcribed table in `data/tables/land/29.6` stays exactly as printed; the
lookup applies the swap and cites `interp:land-0019`. Decided by the owner on 2026-10-06 (batch
review 1).

## Rationale
The weather case's own example (a roll of 53 in summer gives hot weather) only works with the rows
swapped; as printed, 53 in summer is a rainstorm. Other rules treat hot weather as a summer
feature (29.3 and the cases it refers to), and the swapped rows match the climate. The September
1979 errata calls the table backwards but only relabels the seasons.

## Affected behaviour and tests
Weather determination (29.1). Test: a Game-Turn in 37-48 with reading 61 yields a rainstorm; a Game-Turn in 13-24 with reading 31 yields hot weather. A coverage test asserts each row covers all readings once.

