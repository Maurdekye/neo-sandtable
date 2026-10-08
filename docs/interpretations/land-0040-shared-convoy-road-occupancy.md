# land-0040 - Formations and convoys share road space

- **Cases:** land:9.29, land:9.33, land:9.34, land:9.4
- **Status:** adopted
- **Classification:** consequential
- **Profile version:** v1
- **Decided by:** rules-land under neo-sandtable's delegated choice, 2026-10-07
- **Owner review:** reviewed 2026-10-08 by the owner (batch review 4)

## Question

Does the separate treatment of convoys give them a separate road-space quota, or a separate way of computing their contribution to the shared quota?

## Evidence

Case9.29 excludes first-line attached trucks from extra stacking and excludes independent truck convoys from ordinary terrain stacking while retaining their road-space effect. Case9.33 limits vehicle transit through occupied network hexes and distinguishes convoys by reference to9.29. Case9.4 supplies the convoy truck-point block value. Case9.34 excludes off-road counters from network occupancy.

## Ruling

Count stationary on-network real pools against the same five-point road limit used by both moving formations and moving pools. Apply the convoy chart to each pool's actual truck points, then add represented on-network formation values. Exclude the mover, off-road counters and off-road pools. Attached first-line trucks do not receive a second convoy contribution. Ordinary terrain stacking remains unchanged and does not count these pools.

The approved checkpoint representation is a serde-default set of real pool ids in Land movement state. The set is MovementState.pool_on_road; missing membership means OFF. Setup and arrivals therefore start OFF. An executed network edge inserts membership and an executed OFF edge removes it; a zero-edge order preserves it. Full removal or deletion clears membership, and a future split inherits its parent's membership.

## Alternatives and rationale

A separate five-point convoy quota would allow stationary convoys to leave all five formation points available on the same road. That conflicts with the road-space exception in9.29. The selected reading treats the separate wording as a counting method: the convoy block chart replaces formation size, while physical road space stays shared. Counting pools in ordinary terrain stacking would discard9.29's explicit exemption.

## Affected behaviour and tests

One checked occupancy function serves convoy execution and both formation planning and execution. Over-capacity transit is repriced without network benefits and may become prohibited terrain. Tests pin combined values, off-road/excluded/foreign pools, checked overflow, old-checkpoint default and planning/truth parity. Caller activation still requires exact lead readback and the complete convoy transaction proofs.

## Related question: set-up and arrival units (decided)

Owner decision, 2026-10-08 (batch review 4): units placed at set-up or on arrival also start off the network. Like a pool, a unit counts against road space only after an executed move whose last edge used the network leaves it in that hex. The implementation uses serde-default MovementState.on_road alongside pool_on_road: absent unit membership means OFF. Fixed initial placements, buffered set-up placements and canonical arrivals start OFF, clearing any stale membership at committed placement. Executed movement updates every represented member from its last edge; a zero-edge order preserves posture. Planning restores the complete movement state, including posture.

Older checkpoints without on_road load all units OFF, as explicitly approved by the owner. The old inverse off_road field is not an alias: its presence cannot manufacture ON membership. New checkpoints preserve explicit on_road membership. This changes initial road occupancy and can change campaign choices and preset counts; it does not change the five-point limit or terrain prices.
