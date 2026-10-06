# scen-0001 — Benghazi's hex id: A4827 or B4827

- **Cases:** scen:60.31, scen:60.5, land:4.45 (Benghazi Garrison OA sheet)
- **Status:** adopted
- **Profile version:** v0
- **Decided by:** oob (proposed), 2026-10-06
- **Owner review:** reviewed 2026-10-06 by the owner (batch review 1)

## Question
The Italian deployment line in the scenario booklet names "Benghazi (B4827)", but the Benghazi
Garrison OA sheet heading gives "Hex: A4827", and the neighbouring Benina airfield (scen:60.5) is
A4829. Which hex is the garrison in?

## Evidence
- scen:60.31 prints the garrison's hex with section letter B (the booklet page image reads B).
- The Benghazi Garrison OA sheet prints "Hex: A4827" in its heading.
- Benina (A4829) and Soluch (A4130) are in section A, Benghazi lies between them on the coast. Derna
  (B5925), Barce (B5504) and Mechili (B4921) are in section B.

## Ruling
Place the garrison at **A4827**, subject to confirmation by the cartographer that Benghazi is a city
hex in section A. The scenario data records the printed value in a note.

## Rationale
The two sources disagree on one character; geography and the adjacent A-section airfields favour A.
If the map data puts Benghazi at B4827, the scenario file changes one hex id and nothing else.

## Affected behaviour and tests
`data/scenarios/graziani/land_axis.toml` group `it_benghazi`. The Benghazi dump (`ax_benghazi`) is
placed by city name, so it follows the map's Benghazi.
