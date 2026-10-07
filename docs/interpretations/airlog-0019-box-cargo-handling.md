# airlog-0019 - Cargo handling at off-map supply boxes

- **Cases:** land:8.83, land:8.87, land:8.88, airlog:53.24
- **Status:** proposed
- **Profile version:** cna-2021-dev; cna-2021-full
- **Decided by:** rules-airlog, 2026-10-07, per lead ruling
- **Owner review:** pending batch 2; consequential

## Question
Does the off-map cargo-handling movement ban apply everywhere, and how does it survive the division of a carrier whose trucks have handled supplies?

## Evidence
The handling restriction follows the designation of the four Tripoli/Tunisia boxes as supply dumps in 8.88. Off-map movement uses a full active stage's CPA under 8.83. Transit receives unlimited consumption water under 8.87, but 8.88 does not designate Transit positions as dumps. The text does not apportion handling history when trucks divide.

## Ruling
Apply the handling ban only to cargo loaded or unloaded at Tripoli, Tripolitania, Gabes or Tunis. On-map handling retains the Air/Logistics CP rules. Refilling the consuming unit's own tanks alone is not truck cargo handling and does not stamp this ban.

Record the OpStage and exact loaded/unloaded goods totals on each unit or stable truck pool that handles cargo at a named box. That carrier cannot move again during the recorded stage, including map movement, starting an off-map leg, or continuing Transit. Comparing stage identities expires the restriction without erasing history. The record and quantities remain private to the owner.

Transit consumes unlimited water without a stock draw or water attrition. Transit is not a supply dump. Fuel, ammunition and stores come from actual carried holdings and physically traveling members of the same checkpointed group; a departed box and unrelated travel groups are not sources.

Every truck separation checks the original carrier's current-stage record. Until an apportionment procedure exists, full returns Unsupported for a stamped carrier's division. Dev permits division, leaves the original stamp, and gives the owner a private note that the separated trucks are unrestricted. It never copies the original stamp to new parts. This is an explicit development limitation.

The future division procedure will ask the player for stamped truck counts by type and each part's share of stage-loaded goods. Each share must fit its stamped trucks using shared chart capacity; truck and goods totals must be conserved. This input is deferred until a procedure uses it.

## Alternatives
Applying the box ban to all loading would replace the on-map CP procedure. Copying a stamp to every part would restrict trucks that may never have handled cargo. Dropping history during division would erase a full-game restriction. Exact apportionment is the intended completed procedure.

## Affected behaviour and tests
Private carrier records, box loading and unloading, same-stage movement legality, division profile behavior, stage expiry, checkpoint defaults and owner-only visibility. Transit source isolation is also tested by the location-aware fuel ledger; the Land off-map procedure supplies the consumption-water exemption.
