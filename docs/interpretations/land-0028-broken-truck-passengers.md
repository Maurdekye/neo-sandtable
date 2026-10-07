# land-0028 - Passengers carried by broken trucks at the movement origin

- **Cases:** land:21.41, land:21.43, land:21.45
- **Status:** proposed
- **Profile version:** cna-full-v1 / cna-dev-v1
- **Decided by:** neo-sandtable, 2026-10-07
- **Owner review:** pending

## Question
How are transported infantry accounted for when the broken truck carrying them is placed at the movement origin, while the working part of their unit has reached its destination?

## Evidence
land:21.41 requires some breakdowns near a larger enemy formation to remain at the origin. land:21.43 keeps a broken truck's cargo with that truck unless available working capacity can take it. land:21.45 permits infantry to dismount for one CP. These cases do not provide a physical counter-splitting procedure for the separated passengers.

## Ruling
Passengers remain embarked with their broken truck as owner-private cargo, referencing their source unit. They are removed from the working body's strength but remain in the campaign. An explicit one-CP dismount or collection into an existing same-hex unit releases them. If a separate physical infantry counter would be necessary, full reports Unsupported land:21.45; dev retains the embarked passengers and privately reports the unsupported split. Supply cargo and broken weapons are handled normally.

## Rationale
The accounting preserves both the origin-placement requirement and the passenger total. It grants neither free travel to the destination nor free dismounting, and does not manufacture a new counter.

## Affected behaviour and tests
Origin and destination records conserve infantry. Checkpoint recovery retains the source-unit reference and embarked count. The opponent sees only any resulting change in stack presence. Dismounting into an existing same-hex unit pays one CP; a required separate counter follows the profile policy above.
