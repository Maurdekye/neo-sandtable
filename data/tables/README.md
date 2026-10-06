# Charts and tables

Every chart and table of the baseline rules, transcribed as exact, machine-readable data with a
citation. The engine reads these; it never hard-codes a table value.

Schema owner: the lead agent. Each table's internal shape follows the table itself, but the
envelope below is common to all.

## Files

```
data/tables/land/<case>-<slug>.toml       # e.g. 8.37-terrain-effects.toml
data/tables/airlog/<case>-<slug>.toml     # e.g. 49.19-fuel-consumption.toml
data/tables/scen/<case>-<slug>.toml
```

## Envelope

```toml
[table]
id = "land.8.37.terrain_effects"      # <book>.<case>.<snake_slug> — referenced from data/rules
case = "8.37"
title = "Terrain Effects Chart"
src = ["land:8.37", "errata79:8.37"]
transcribed_from = ["vassal:8.37 Terrain Effects Chart.png", "rules-2021:land-2021.layout.txt"]
                                      # source FILE NAMES only, for traceability — never contents
verification = "double"               # double = transcribed twice independently and diffed; single
dice = "none"                         # none | 1d6 | 2d6_sum | 2d6_reading (11–66) | other (explain)
notes = "Paraphrased notes on how to read the table, in our own words."

# Then the table body, shaped to fit the table. Rules for the body:
# - Keys are explicit and typed: integers stay integers; dice ranges are [lo, hi] inclusive.
# - Every row and column the source has is present; no "see source" placeholders.
# - Footnotes become structured fields on the rows/cells they qualify (or a [[footnote]] list
#   with explicit references), not loose prose.
# - Units are in field names (cp_cost, fuel_points, percent).
# - A missing or illegible value is omitted and listed in GAPS.md — never guessed.
```

### Example body (illustrative only — not real values)

```toml
[[row]]
terrain = "clear"
movement_cp = { motorized = 1, non_motorized = 1 }
close_assault_shift = 0
footnotes = []
```

## Verification

Tables are where silent transcription errors hide. For every table:

1. Transcribe it from the chart image (`cna-sources/vassal/extracted/images/<case> <title>.png`)
   and, where the rules text also prints it, from the text.
2. Diff the two (or re-read the image a second time, independently, if there is only one source).
3. Mark `verification = "double"` only when both passes agree; resolve disagreements against the
   1979 scan and note the outcome.
4. Record values that differ between the 2021 retype and the 1979 printing as an interpretation
   (`docs/interpretations/`).
