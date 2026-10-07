# airlog-0013: Turn back supply cargo beyond port capacity

- **Cases:** airlog:55.14, airlog:56.27, airlog:56.28
- **Status:** adopted
- **Profile version:** cna-2021-full / cna-2021-dev
- **Decided by:** rules-airlog with neo-sandtable ruling, 2026-10-07
- **Owner review:** reviewed 2026-10-07 by the owner (batch review 2)

## Question
What happens when a fixed convoy reaches a port whose remaining stage capacity is smaller than the shipment?

## Evidence
Case55.14 scales shipments with the port's remaining efficiency. Case56.27 prohibits receiving more than capacity. Case56.28 makes unloading conditional on being possible, without prescribing storage offshore or a later arrival.

## Ruling
Reduce the arriving cargo to the remaining supply budget and treat the excess as turned back. It is not delivered or destroyed and is not carried into a later stage or another lane. Report the reduction only to Axis logistics. Preserve the shipment's supply proportions: multiply each whole-point quantity by the available-weight fraction, rounding downward so no extra point can exceed capacity.

## Rationale
This implements the reduced-shipment language without inventing bombing losses, another shipping lane or a delayed schedule. Whole supply points cannot be created to fill a fractional remainder.

## Affected behaviour and tests
logistics::convoys::arrive applies the shared port budget. A400ton ammo shipment with50tons available delivers12AmmoPoints(48tons), with no later carry-over. Captured destinations still cancel under56.15.
