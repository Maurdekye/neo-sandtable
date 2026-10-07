# air-0016 - Unarmed aircraft retain normal ratings against opposing fire

- **Cases:** airlog:38.43, airlog:45.0, airlog:45.17
- **Status:** proposed (implemented provisionally)
- **Profile version:** v0.1
- **Decided by:** neo-sandtable (lead), 2026-10-07
- **Owner review:** pending batch 4 (consequential)

## Question

An ammunition shortage sets an aircraft's TacAir to zero in the maintenance rules. The combat rules separately preserve normal ratings when resolving fire against an unarmed aircraft. Which TacAir should the opponent subtract when computing its shot differential?

## Evidence

38.43 connects the squadron ammunition payment to its aircraft's TacAir and ability to fire, assigning zero TacAir when the payment is absent. 45.17 prohibits firing without gun ammunition but retains normal ratings for the effects of opposing fire. 45.0 resolves each shot using the difference between the participating ratings. These instructions conflict if the zero from 38.43 also reduces defensive TacAir.

## Ruling

Provisionally ruled: apply the specific combat instruction in 45.17. An aircraft without gun ammunition cannot fire and consumes no firing dice. Against opposing fire, use its normal mode rating, with applicable pilot, formation and maneuver adjustments established by the combat procedure. The zero in 38.43 governs its own fire only; do not replace its defensive TacAir because of the ammunition shortage.

Alternative: apply the zero TacAir in 38.43 to both sides of the differential. The unarmed aircraft still cannot fire, but its opponent subtracts zero TacAir when resolving a shot against it. Normal non-TacAir ratings continue to apply.

## Rationale

45.17 addresses the exact opposing-fire situation and expressly distinguishes inability to fire from the ratings used against that aircraft. Treating the zero in 38.43 as a restriction on its own fire preserves this specific distinction. The alternative reads the maintenance instruction literally for every use of TacAir.

## Affected behaviour and tests

This is consequential: the readings can change the opponent's differential and kill threshold. The isolated local air::combat::Combatant::differential calculation implements the provisionally ruled reading; the ammunition test verifies no firing dice for an unarmed aircraft and preservation of the opposing-fire differential. There is no gameplay caller, persistent loss, disclosure or live combat-window implementation in this slice. The lead permits caller adoption under this reading, subject to the existing decision-window, privacy and shared-contract requirements. This ruling grants no publication clearance or full combat-phase approval. No squadron ammunition payment, load entitlement or maintenance procedure is established by this calculation helper.
