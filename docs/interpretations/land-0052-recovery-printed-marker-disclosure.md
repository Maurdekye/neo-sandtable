# land-0052 — Public marker changes during private recovery

- **Cases:** land:3.62, land:21.42, land:21.63, land:22.5
- **Status:** proposed
- **Classification:** consequential
- **Profile version:** v1, pending publication
- **Decided by:** @neo-sandtable, 2026-10-19 (me25575fd8865)
- **Owner review:** pending
- **Amends:** land-0038's owner-adopted unchanged-former-owner-stream clause; historical text retained

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

## Ruling — lead decided, owner review pending

Keep normal public printed-face changes. A former owner receives no special recovery notice,
private equipment counts, supply costs, dice, accounts or provenance merely because it once
owned the captured assets.

Privacy pairs compare worlds with the same permitted public physical outcomes and different
private holdings or histories; their former-owner streams must match. Such a comparison does
not require a removed or moved public marker to remain visible at its previous position.

This explicitly amends the owner-adopted unchanged-stream requirement of land-0038 to permit
normal public reasonless printed-face removal and movement. Its actor-only Notes, physical
conservation and funding rules remain in force. Central Sync, views and the printed-face
contract remain unchanged. Do not retain a stale marker to hide a physical change. The owner
must see this amendment in the next batch; it does not silently replace the historical text.

## Rationale

The proposal preserves the existing public counter contract and does not create a stale or
empty marker to conceal a physical change. It separates public movement/removal from the
private facts that repair accounting consumes.

## Affected behaviour and tests

Three authored controls in recovery/disclosure_contract_tests.rs call production Sync on
fixture changes: public removal, public towing, and equal public removals with different
private contents. They also check that actor-only Notes do not enter the former-owner stream.
They remain unrun and are source-contract controls, not Respond/Advance repair-driver proof.
A first Axis field tank-repair driver now has seven authored flow controls in
recovery/field/tests.rs, including actual Respond/Advance former-owner pairs. These also
remain unrun; no demonstrated gameplay result or projection change is claimed. The lead
approved implementation; the amendment remains pending the owner's next batch review after
publication.
