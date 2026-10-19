# land-0043 — Compute morale once, then charge actual defense

- **Cases:** land:6.21, land:6.22, land:11.21, land:11.22, land:11.24, land:11.27, land:15.96, land:17.21
- **Status:** proposed
- **Classification:** consequential
- **Profile version:** v1
- **Decided by:** neo-sandtable, 2026-10-14
- **Owner review:** pending
- **Lead decision:** md9ed9f017398, recorded in the combat docket; Rules919/920 relay

## Question

Defensive CP can depend on the final assault differential, while morale depends on cohesion
after combat expenditure. Which value is used without repeatedly changing the result?

## Evidence

The extended combat example includes attack/defense CP in its morale inputs. Land:11.27
reduces the cost of certain full defenses; 15.96 can make defense against a probe free.
Land:11.24 preserves the segment caps, including earlier barrage or other combat charges.

## Ruling

Use one calculation pass. Morale-input cohesion reflects all earlier CP/DP and the ordinary
cost of this assault: attacker five CP, defender three; a probe ordinarily costs two for each.
Compute only the incremental amount allowed by the segment cap and earlier segment charges.
The attacker's certain cost may be charged before morale. For the defender, ordinary cost is
solely a calculation input; do not write that hypothetical charge or DP into State or activity.

Draw morale and fix the adjusted differential once. Then derive the actual defender cost:
one CP under 11.27, zero under 15.96, otherwise the ordinary cost. Charge its incremental
amount once, applying DP from that actual expenditure. Existing segment expenditure is not
refunded. Never recalculate morale, change the differential, reroll or charge-and-refund.
Engineering receives only the actual charge.

## Rationale

The lead chose a virtual ordinary-cost morale input and a single actual debit. A fixed-point
iteration or provisional debit/refund would introduce unsourced feedback and activity history.

## Affected behaviour and tests

Uncalled S1 helpers separately provide ordinary cost, capped increment, fatigue preview,
morale input and final actual defense increment. Authored tests put a defender at its CPA:
ordinary full defense previews three DP, but a final differential of -4 charges only one CP;
an adverse probe can preview two and actually charge zero. Earlier barrage charges survive.
The S1 arithmetic tests passed on 2026-10-19. Later callers must prove one actual ledger write
and no morale feedback.
This consequential ruling enters an owner batch only after publication on main.
