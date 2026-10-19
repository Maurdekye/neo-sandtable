# land-0051 — Cohesion means round half ties away from zero

- **Cases:** land:6.27, land:6.28, land:6.29, land:11.32
- **Status:** proposed
- **Classification:** consequential
- **Profile version:** v1
- **Decided by:** neo-sandtable, 2026-10-14
- **Owner review:** pending
- **Lead decision:** m4e977cef36e2; Rules928/929 relay

## Question

When several equally large counters set cohesion through an average, how does nearest-whole
rounding handle a result exactly halfway between integers, especially a negative mean?

## Evidence

Land:6.27's example turns the mean of -4, -1 and +3 into -1, establishing nearest-whole
rounding for a non-tie. Land:6.28 applies the calculation within a parent before the outer
formation comparison. Neither example settles exact halves. The positive strength example in
11.32 rounds a half upward; the lead selected its sign-symmetric extension for cohesion means.

## Ruling

Round an averaged cohesion value to the nearest whole point; exact halves go away from zero.
Thus +0.5 becomes +1, -0.5 becomes -1, +1.5 becomes +2 and -1.5 becomes -2. Apply the same
rule at each nested averaging boundary. A single largest counter is not an average and keeps
its exact quarter-point cohesion. Individual stored cohesion does not change.

This does not change land-0021: selecting a morale-table row still floors signed cohesion.
No second morale calculation, State change or additional disclosure follows from this rule.

## Rationale

Half-away-from-zero treats positive and negative means symmetrically. The lead chose it over
half-to-zero, ties-to-even and always-toward-positive-infinity. This is a consequential
lead-decided reading awaiting owner review, not an inferred universal rounding policy.

## Affected behaviour and tests

Uncalled S1 `combat::math::largest_cohesion` now handles ties instead of returning its former
local half-tie error. Authored controls cover both signs at 0.5 and 1.5, nearby non-ties,
singleton quarter values, nested means and the distinct signed morale-row floor. All controls
passed in the ten-test S1 arithmetic run on 2026-10-19; no engine caller, event, state or existing
Unsupported path changed. Submit this
file to the next owner batch only after it exists on main.
