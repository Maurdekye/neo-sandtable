# land-0046 — Casualty floors, owner remainder and raw-point capture

- **Cases:** land:15.77, land:15.82, land:15.83, land:15.84, land:15.85, land:15.86
- **Status:** proposed
- **Classification:** consequential
- **Profile version:** v1
- **Decided by:** neo-sandtable, 2026-10-14
- **Owner review:** pending
- **Lead decision:** md9ed9f017398, recorded in the combat docket; Rules919/920 relay

## Question

How are fractional loss shares assigned across origins, what captures count, and how much
avoidable excess may the owner select when indivisible TOE have different ratings?

## Evidence

Land:15.83 sets the raw loss calculation and proportional allocation. Land:15.86c and the
extended example use raw strength for capture: 33% of ten raw losses requires four raw captured,
which two infantry TOE rated two can satisfy. Land:15.82 adds retreat-shortfall losses.

## Ruling

Let L be the already rounded raw loss obligation, R the raw casualty basis and R_i the part
from origin i. Each origin must absorb at least `floor(L * R_i / R)`. The force must absorb
at least L overall. The owner chooses where the remaining obligation goes; the engine must
not impose a largest-remainder or deterministic tie-selection policy.

When the affected force stands or retreats together, add ten percentage points per missing
hex to its CRT percentage, multiply once by its casualty basis and round once under 15.83c.
For split groups, calculate each short group's extra obligation on that group's raw strength
separately, with the same attacker/defender/overrun rounding; that group absorbs the extra.

Capture requires `ceil(percent * raw_loss_obligation / 100)`, including retreat-related losses.
Captured TOE are a subset of the actual casualties of this assault. Each absorbs its normal
offensive/defensive raw rating, as applicable. Selecting a captured TOE does not remove it twice.

Both the casualty selection and its captured subset must be minimal: every selected TOE must
be needed to meet some applicable obligation. If dropping a single selected TOE leaves all
obligations satisfied, the choice is invalid. Necessary overshoot caused by indivisible ratings
is permitted. Gratuitous extra casualties or surrender are not, including excess prisoners
that would burden the captor. All loss minima and maxima must be checked together.

## Rationale

Floors preserve each origin's required share while retaining the player's remainder choice.
Raw-point capture matches the specific procedure/example. Minimality distinguishes unavoidable
rounding excess from deliberate over-allocation; a largest-remainder allocator was rejected.

## Affected behaviour and tests

S1 contains uncalled pure helpers for raw loss rounding, per-origin floors, short-retreat loss,
capture obligation, generic minimal-selection bounds and minimal captured subsets. The later
caller must include every applicable obligation in its contribution matrix, splitting captured
and uncaptured buckets where their weights differ, and verify all physical eligibility.
Authored hand tests cover 63 raw at 5% -> four losses, 70 at 15% -> ten defensive losses,
33% of ten -> four captured, owner placement of a fractional remainder, necessary overshoot,
superfluous TOE and a captured subset exceeding casualties. S1 arithmetic tests passed on
2026-10-19. No
gameplay loss selection is active; batch submission waits for this file to be on main.
