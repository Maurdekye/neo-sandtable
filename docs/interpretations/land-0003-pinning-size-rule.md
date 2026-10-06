# land-0003 — Size comparison for pinning (8.54)

- **Cases:** land:8.54, land:9.2
- **Status:** adopted
- **Profile version:** v1
- **Decided by:** rules-land, 2026-10-06
- **Owner review:** reviewed 2026-10-06 by the owner (batch review 1)

## Question
8.54 says a battalion-size unit can never pin a division and a company-size unit can never pin a brigade or larger, judged by size (stacking points and TOE), and tells players to use common sense or a coin flip when unclear. An engine needs a deterministic test.

## Evidence
The case itself; unit-equivalent rules in 9.2 convert reduced-strength formations to equivalent size.

## Ruling
Compare organization size using the unit-equivalent size of the moving (pinning) unit and of the unit that would be pinned, using stacking points as the first measure; when the two measures disagree, the larger-size reading is used for the pinned unit and the smaller for the pinner. No coin flip: ties favour the non-phasing (pinned) player.

## Rationale
Deterministic and conservative to the unit that wants to escape; the rule's intent is to stop tiny units freezing large formations.

## Affected behaviour and tests
Reaction legality (8.53c). Test: skeleton division (low stacking points) still cannot be pinned by a battalion; a company cannot pin a brigade.
