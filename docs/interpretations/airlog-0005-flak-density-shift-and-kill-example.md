# airlog-0005 — Flak density column shift and the +2 TacAir kill example

- **Cases:** airlog:46.0, airlog:46.3, airlog:46.4, airlog:45.3 (worked examples), airlog:45.5
- **Status:** adopted
- **Profile version:** v0.1
- **Decided by:** rules-airlog, 2026-10-06
- **Owner review:** reviewed 2026-10-06 by the owner (batch review 1)

## Question
1. The AA table notes (and the identical Flak Adjustment Chart, 46.4) shift the flak-point column to the
   right "for each twelve aircraft exceeding twelve", with the example that 24 aircraft shift one column.
   The text of 46.0 calls the same adjustment an AA density adjustment based on the total number of enemy
   non-fighter planes. What happens with counts that are not multiples of twelve (for example 23, 30, 36)?
2. The first worked example in the air-to-air section (a CR 42 against a Hurricane I) says a plane at a final
   differential of +2 needs 11-22 to kill, but the TacAir Kill table gives 16 at +2 (22 is the +3 entry).
   The second example (+5 needs 11-26) agrees with the table.

## Evidence
The notes' wording ("a multiple of twelve", "for each twelve aircraft exceeding twelve") and the sample
(24 -> one column) fit the formula shift = (n - 12) / 12 rounded down for n of 12 or more, and 0 below
12. The 46.4 chart is declared wrong by the addenda in favour of the 46.3 notes, but contains the same
sentence. The +2 example sentence is not otherwise corrected.

## Ruling
1. Column shift = max(0, floor((n - 12) / 12)) to the right, where n is the number of bomber-class and
   transport-class planes in a non-fighter target group (fighter-class planes in the group count as
   they are not separate); so 12-23 planes shift 0, 24-35 shift 1, 36-47 shift 2, and so on.
2. The Kill table value governs; the example's "11-22" is treated as a typo for 11-16.

## Rationale
It reproduces the one number the text gives and is monotone and total over all group sizes. For the
second point the table is the authoritative data and is internally consistent with the other example.

## Affected behaviour and tests
Flak column selection; air combat kill thresholds. Tests: a 24-plane group shifts one column; a 23-plane
group shifts none; a +2 differential kills on readings up to 16 only.
