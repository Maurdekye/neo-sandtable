# land-0039 - Convoy Contact at phase departure

- **Cases:** land:8.15, land:8.62, land:8.65, land:8.68, land:10.23, land:10.26, land:10.29
- **Status:** adopted
- **Classification:** consequential
- **Profile version:** v1
- **Decided by:** neo-sandtable via rules-mgr, 2026-10-07 (selected convoy ruling)
- **Owner review:** reviewed 2026-10-08 by the owner (batch review 4)

## Question

Does the convoy departure reference to the earlier movement section exempt supply pools from Contact, or place an Engaged charge on them?

## Evidence

Case10.23 applies departure rules to a unit, which includes convoys; it does not explicitly name convoys. Cases8.15,8.62 and8.65 distinguish leaving enemy control from an Engaged combat formation's greater cost. Case8.68 limits where the placement procedure applies; it does not grant convoys a departure exemption. Cases10.26 and10.29 give friendly combat coverage its explicit control-negating effect. The printed cross-reference is stale relative to the retype's current section numbering.

## Ruling

At the beginning of the convoy's phasing half, an actual pool in enemy control without friendly combat coverage pays two CP when departing. A supply pool does not pay an Engaged formation charge. Determine this through trusted control adjudication at execution, once for its single continuous Move order. Mere friendly noncombat presence does not supply coverage. Respond preparation does not query hidden control.

## Rationale

The lead selects the specific Contact reading and treats8.68 as placement applicability. The alternative convoy exemption would remove the departure requirement in10.23. Applying Engaged status would fabricate combat formation state for a pool. One order per pool avoids extra persistent assessed-pool state.

## Affected behaviour and tests

The Land convoy departure helper returns eight CP quarters or zero, without moving stock or advancing dice. Airlog's reviewed caller must charge it once before the first actual edge and preserve zero-edge, rollback and hidden-control event boundaries. Test uncovered departure, friendly combat coverage, friendly noncombat presence and unchanged Respond state.
