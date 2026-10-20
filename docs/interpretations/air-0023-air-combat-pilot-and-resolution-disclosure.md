# air-0023 - Pilot privacy until combat allocation is final

- **Cases:** land:3.6, airlog:45.22, airlog:45.23, airlog:45.24, airlog:45.4, airlog:45.5
- **Status:** proposed
- **Profile version:** cna-2021-full; encounter disclosure and combat caller pending
- **Decided by:** @neo-sandtable, 2026-10-20; relayed by @rules-mgr
- **Lead decision:** adopted for implementation; owner batch pending
- **Owner review:** pending

## Question

When may an opponent learn a pilot's rating and the detailed calculation for a combat matchup?

## Evidence

45.22 discloses aircraft models but initially withholds pilot ratings. The attacker then allocates its fighters under 45.23. Combat calculations use those pilot modifiers, maneuver adjustments and differentials, so the calculation itself can reveal a previously hidden pilot rating.

## Ruling

Preserve pilot privacy through final allocation. Pilot-zero and pilot-three versions of an otherwise identical encounter must have identical opposing information and decision streams up to that boundary.

After allocation is final, show both sides of the encounter the resolution of each matched plane: model, pilot rating, adjusted TacAir, maneuver adjustment, final differentials, firing order, dice and result. Use labels local to that encounter.

That disclosure grants nothing further: do not expose loads, fuel, squadron membership, persistent plane identities or aircraft outside the encounter. It is an encounter-side disclosure, not a public-board report or an outsider audience grant.

## Rationale

Allocation cannot exploit a hidden pilot rating, while the later combat calculation remains inspectable rather than disclosing its result without the source-required inputs.

## Affected behaviour and tests

No projection, event type or combat caller is implemented here. Planned actual-flow pairs preserve pilot privacy during declaration, route queries and allocation, then assert the exact permitted encounter fields after final allocation. Source-required encounter models and counts remain available for allocation. An unmatched aircraft's pilot rating, adjusted rating and calculation details remain absent unless it is matched; aircraft outside the encounter remain wholly absent. Load, fuel, squadron and persistent-ID facts stay absent throughout. Every .at or .about audience must independently be entitled to its location or identity; board synchronization remains the engine's responsibility. Publish a main SHA for the owner batch only after authorized landing.
