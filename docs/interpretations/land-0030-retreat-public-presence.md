# land-0030 - Retreat allowance uses disclosed adjacent presence

- **Cases:** land:13.23, land:13.24, land:3.62
- **Status:** proposed
- **Profile version:** cna-2021-dev / cna-2021-full
- **Decided by:** neo-sandtable, 2026-10-07 (provisional)
- **Owner review:** pending, batch 2

## Question
An adjacent enemy combat unit changes the retreat allowance. Under the current limited-information projection, the observer knows where enemy counters are present but cannot classify their contents. Testing the hidden class during validation would reveal that distinction.

## Evidence
Land:13.23 gives initially adjacent units the broader retreat allowance; land:13.24 applies the other cap. Land:3.62 governs inspection and is the subject of the separate counter-disclosure question. The lead adopted the public-presence test provisionally, following land-0023, while the owner considers that broader question.

## Ruling
At the start of retreat before assault, use adjacent public enemy counter presence. Any enemy counter qualifying as a public stack grants the adjacent allowance. Apply the four-CP or single-hex restriction elsewhere. Preserve this snapshot throughout the window; do not reclassify hidden occupants while responding or adjudicating.

This follows the public-presence approach in [land-0023](land-0023-repeat-movement-proximity.md). An actual supply-dump marker alone is not a land-unit stack.

## Rationale and owner alternative
The provisional reading prevents an action validator from serving as a hidden-class probe. It also avoids a second, undisclosed cap at resolution. The owner is separately considering whether map counter identities should be visible under land:3.62. If counter classes become disclosed, revisit both 0030 and 0023 and use those public classifications instead.

## Tests and delivery boundary
Paired states with combat and noncombat occupants at the same enemy hex must produce identical own-side requests, reachability and validation. Tests also distinguish a genuinely empty adjacent hex and verify that moving the enemy later does not rewrite the snapshot. Unit paths, contact costs and fuel remain normal movement rules. This interpretation does not fill the separate dump-demolition or vehicle-breakdown implementation gaps.
