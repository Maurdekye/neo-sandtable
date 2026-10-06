# January 1941 air totals

- **Cases:** airlog:34.84, airlog:34.86
- **Status:** adopted
- **Profile version:** draft
- **Decided by:** oob, 2026-10-07
- **Owner review:** reviewed 2026-10-06 by the owner (batch review 1)

## Question
Does the worked example override the January schedule?

## Evidence
The example totals 59 planes with 2 Wellingtons and 1 Maryland. The chart lists 44 Hurricanes, 12 Blenheims and 1 Wellington, totaling 57.

## Ruling
Use the chart: 44 Hurricane I, 12 Blenheim I, 1 Wellington I; no Maryland in January.

## Rationale
The worked example illustrates distribution; the schedule defines quantities.

## Affected behaviour and tests
air_cw.toml preserves those three counts and coverage.py checks that the interval covers the scenario.
