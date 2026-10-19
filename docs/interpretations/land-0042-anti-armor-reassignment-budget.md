# land-0042 — Anti-Armor reassignment uses a raw-point budget

- **Cases:** land:11.32, land:11.33, land:11.34, land:14.11, land:14.23, land:14.32, land:15.15, land:15.21, land:15.22, land:15.94, land:16.46
- **Status:** proposed
- **Classification:** consequential
- **Profile version:** v1
- **Decided by:** neo-sandtable, 2026-10-14
- **Owner review:** pending
- **Lead decision:** md9ed9f017398, recorded in the combat docket; Rules919/920 relay

## Question

Does blind Anti-Armor fire against an unarmored force grant abstract assault points, or let
specific surviving equipment change roles? How does an integer allowance constrain that change?

## Evidence

Land:14.23 expresses an allowance in Anti-Armor points. The 15.21 exception links that
allowance to previously committed units. The strength calculations and the extended combat
example use Actual AA points for that expression. Land:16.46 separately excludes dummy armor.

## Ruling

Let A be the Actual AA strength fired by that side into this target exchange, calculated under
11.32–11.34, including 11.33. Eligible surviving whole TOE may change from AA to the already
committed close assault at this hex. Their combined raw AA contribution, using the ratings
actually fired, cannot exceed `10 * floor(A / 2)`. Do not round the candidate contribution
before comparing it; A=1 cannot transfer positive AA strength.

Only surviving points that fired in this particular exchange qualify. Withheld points,
casualties and points fired at another target do not. The target must contain no enemy unit
with an Armor Protection rating: withheld, pinned and RBA arrivals count for that test. A
dummy-only armor situation does not qualify either. For nonphasing fire, check the phasing
commitments against the defended hex.

For the phasing player, the transferred points join its precommitted assault group containing
that target. Nonphasing points join their defense there. With no phasing assault already
committed to that hex, reassignment cannot create one. Transferred TOE use their normal CA
ratings for strength, the small-force exception, combined arms, ratio shifts, absorption and
capture. They require ordinary CA ammunition; earlier AA ammunition remains spent. The
force-assignment probe/full classification and CP obligations remain fixed. Reassignment draws
no adjudication dice and must not expose target eligibility through answer validation.

## Rationale

This preserves the unit-level exception without turning it into a free numerical bonus.
Comparison in raw units prevents rounded candidate strength from exceeding the allowance.
The alternative flat Actual CA grant was rejected by the lead.

## Affected behaviour and tests

S1's uncalled `combat::math::reassignment_allowed` checks budget, survival, local participation,
ammunition and the armor/dummy/existing-assault gates. Source tests cover 10 versus 12 raw at
A=3, A=1, dead/withheld/other-target points and every exclusion. Actual role movement, ammo
spending, unchanged probe classification and disclosure need later caller proofs. No runtime
implementation or owner approval is claimed. Submit this ruling to an owner batch only after
its file exists on main.
