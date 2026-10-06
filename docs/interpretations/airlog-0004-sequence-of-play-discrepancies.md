# airlog-0004 — Differences between the Land/Air and Logistics sequences of play

- **Cases:** airlog:33.0, airlog:48.0, airlog:56.32, airlog:56.31, land:5.2, land:20.72, land:20.21, land:20.78A, land:20.78B
- **Status:** proposed
- **Profile version:** v0.1
- **Decided by:** rules-airlog, 2026-10-06
- **Owner review:** pending

## Question
The Land/Air sequence (33.0) and the Logistics sequence (48.0) are two prints of one outline and
disagree in a few places. Graziani uses the full Logistics sequence, so each disagreement needs a
ruling.

## Evidence
1. **Commonwealth replacement lead time.** In the Naval Convoy Arrival phase 33.0 says the CW rolls
   on the Production Table to plan replacements arriving one month later; 48.0 says two months later.
2. **Truck Convoy Movement phase.** 33.0 lets Player A also move replacement points in this phase;
   48.0 lists only trucks, POWs and guards.
3. **Reaction label.** In the Movement segment 33.0 gives reaction movement to Player B (the
   non-phasing player); 48.0 prints Player A, which contradicts its own definition of the phasing player.
4. **Patrol condition.** 33.0 allows patrols when the phasing player has not assaulted this phase;
   48.0 allows them if there has been no combat anywhere in the OpStage.
5. **Coastal shipping timing.** 48.0 puts "Tactical Shipping" (cargo between African ports, Axis
   coastal ships included) in the Organization Phase; 56.32 says Axis coastal ships move only in the
   Truck Convoy Phase.
6. **Rail case reference.** 33.0 points to land 8.7, 48.0 to 8.9, for CW rail movement.

## Ruling
1. Use a ONE-month CW replacement lead time (33.0's reading); 48.0's "two months" is treated as a
   misprint. Land agreement (confirmed by rules-land): land:20.72 plans production one month ahead
   and uses the Production Table of the month of arrival, land:20.21 plans arrival in player-chosen
   stages four Game-Turns ahead, and the land:20.78A/20.78B table notes say four Game-Turns hence;
   four Game-Turns equal one month. The engine takes the lead time from data, not code.
2. Replacement points may be moved in the Truck Convoy phase (33.0 wording).
3. Reaction belongs to the non-phasing player (Player B).
4. Patrols require both conditions: the phasing player made no assault, and no combat has
   occurred in the current OpStage.
5. Coastal ship counters move in the Truck Convoy phase (56.32); the Tactical Shipping segment
   handles the abstract interport cargo transfer subject to port capacities, for both sides, and
   Axis ships take part only by moving in the Truck Convoy phase.
6. Both references mean the Commonwealth Movement/rail case in the Land game; cite land section 8.

## Rationale
Where the two sequences conflict, the more specific rule or the one that agrees with another case
wins; the most restrictive reading is used for patrols so as not to grant a recon advantage.

## Affected behaviour and tests
Phase tables (`opstage.*` timing anchors), CW production planning, patrol eligibility, coastal
shipping. Tests: a patrol after a close assault by either side in the OpStage is rejected; a coastal
ship moved in the Organization Phase is rejected.
