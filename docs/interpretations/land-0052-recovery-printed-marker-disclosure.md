# land-0052 — Public marker changes during private recovery

- **Cases:** land:3.62, land:21.42, land:21.63, land:22.5
- **Status:** proposed
- **Classification:** consequential
- **Profile version:** pending decision; no runtime change
- **Decided by:** pending; proposal by @rules-land, 2026-10-19
- **Owner review:** pending
- **Related interpretation:** land-0038; its adopted ruling is not changed by this proposal

## Question

Does the unchanged former-owner stream requirement prohibit the public face changes caused
by removing a fully repaired captured marker or towing it to another hex, or does it prohibit
additional private recovery information while retaining those public changes?

## Evidence

The RAW printed-face contract exposes the marker's identity and hex under land:3.62/21.42.
The production board includes every marker in its public counter faces. Central Sync emits
UnitRemoved when a face disappears and UnitUpdated when its hex changes, even if the acting
side emits only private Notes.

Adopted land-0038 retains central Sync, limits provenance Notes to the actor, and also requires
captor recovery to leave the former-owner stream unchanged. Taken literally across an action
and no action, those requirements conflict when the physical public marker changes.

## Proposed ruling — awaiting decision

Keep normal public printed-face changes. A former owner receives no special recovery notice,
private equipment counts, supply costs, dice, accounts or provenance merely because it once
owned the captured assets.

Privacy pairs compare worlds with the same permitted public physical outcomes and different
private holdings or histories; their former-owner streams must match. Such a comparison does
not require a removed or moved public marker to remain visible at its previous position.

This would clarify only the unchanged-stream requirement of land-0038. Its actor-only Notes,
physical conservation and funding rules remain in force. The alternative is to require no
former-owner face delta at all, which needs an explicit change to the current public-face
contract and Sync behavior. Neither alternative is implemented here.

## Rationale

The proposal preserves the existing public counter contract and does not create a stale or
empty marker to conceal a physical change. It separates public movement/removal from the
private facts that repair accounting consumes.

## Affected behaviour and tests

Three authored controls in recovery/disclosure_contract_tests.rs call production Sync on
fixture changes: public removal, public towing, and equal public removals with different
private contents. They also check that actor-only Notes do not enter the former-owner stream.
They remain unrun and are source-contract controls, not Respond/Advance repair-driver proof.
No callable repair procedure, projection change or adopted interpretation change is claimed.
