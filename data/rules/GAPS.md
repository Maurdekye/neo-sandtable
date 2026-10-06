# Rule registry gaps

Items in the rulebook sources that are missing, unreadable, internally inconsistent, or deliberately left
to another area. Interpretation proposals (docs/interpretations) cover contradictions; this file covers
things we could not obtain. Table gaps are in data/tables/GAPS.md.

## Land book

| Case | Gap | Status |
|---|---|---|
| land:21.14 | The Breakdown Adjustment Summary (generic BAR by vehicle type, notably trucks and armored cars) is absent from the retyped text and from the chart images. | Case registered as unresolved; interpretation land-0016. |
| land:20.67 | The Axis Replacement Point Type Limitations Chart is referenced but no chart image exists among the sources; the limits appear to be printed with the 20.66 pool tables. | Case registered as unresolved. |
| land:19.31-19.33 | The Formation Organization Charts are symbolic composition charts (NATO icons) for Commonwealth, Italian and German formations. | Registered as data cases without table files; left for the units area (or a later pass) to encode. |
| land:8.44 | The text sends the reader to "5.33" for abandoned vehicles; no such case exists in the Land book (Section 5 has only 5.1 and 5.2). | Noted in the case summary; the intended rule is the abandonment of vehicles that enter a salt marsh off a track. |
| land:4.2, 4.21-4.24 | The Counter Manifest and sample units (cited by 3.21 and 9.11) are not part of the retyped Land text. | Not registered (not in the source text); units area. |
| land:11.4 | The Land Combat Calculations Summary chart has no case number in the rules text. | Transcribed as a table, cited as land:11.3. |
| unnumbered | "An Example of Combat" after 15.96 and several worked examples in 6.x, 15.x and 21.x are not cases. | Not registered; they are good conformance tests (see docs/interpretations land-0010 and the examples in 11.35, 15.64, 21.34). |

## Notes on source typos handled by the checker

`[7 .1]` (space), `[21.3)` (closing parenthesis) and `[4.43a)]` (errata note, not a case) are printed
irregularly; the checker reads the first two as cases 7.1 and 21.3 and ignores the third (folded into 4.43).
