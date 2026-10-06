# 0022 — Ordered sequential dice readings

- **Cases:** land:3.1, land:12.42, land:15.0, land:15.73, land:15.75
- **Status:** adopted
- **Profile version:** cna-2021-dev and cna-2021-full
- **Decided by:** neo-sandtable, 2026-10-06
- **Owner review:** pending

## Question

Do the names of the larger and smaller dice identify their physical sizes, or require sorting the rolled values before producing a sequential reading?

## Evidence

The glossary at land:3.1 assigns the decimal places to distinguishable dice. The barrage procedure at land:12.42 names a designated die for the first place. The close-assault examples at land:15.0 and land:15.73 include both an increasing pair producing 25 and a decreasing pair producing 63. Sorting values cannot reproduce both examples.

## Ruling

The engine draws an ordered pair. The first draw supplies the tens digit; the second supplies the units digit. Values are never sorted. Each of the 36 combinations of two six-sided dice can therefore occur.

For close assault, each side rolls its own pair once. The loss lookup uses that pair's sequential reading, and the other results use the sum of those same dice. A separately prescribed prisoner determination remains a separate draw (land:15.73d).

## Rationale

The named dice distinguish physical dice rather than compare rolled values. This reading follows the printed examples and avoids biasing the result table by removing half of its outcomes.

## Affected behaviour and tests

`cna_core::dice::CampaignRng::two_dice_reading` already preserves draw order. Its `two_dice_reading_is_tens_then_units` regression fixes that contract. Barrage, anti-armor and close-assault table bindings use the resulting reading without reordering it. Close-assault procedure tests must also verify that sequential and summed results come from the same pair, with independent pairs for the two sides.