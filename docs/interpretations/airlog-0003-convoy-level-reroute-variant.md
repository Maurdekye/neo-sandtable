# airlog-0003 — Optional rerouting of the Axis convoy level letters

- **Cases:** airlog:56.29 (and airlog:56.4)
- **Status:** proposed
- **Profile version:** v0.1
- **Decided by:** rules-airlog, 2026-10-06
- **Owner review:** pending

## Question
Case 56.29 is an addition offering players an alternative: choose each month's convoy level letter
freely from the multiset of letters that month would otherwise use, with a restriction on repeating
letters above B, and an invitation to devise a rule so the Axis does not get everything at once.
Is it part of the baseline?

## Evidence
The case is marked as an addition and is phrased as a suggestion to players ("may choose",
"experiment and try to formulate some agreeable rule"). It does not define a complete procedure
(the letter multiset per month is not the same as the one-letter-per-month chart).

## Ruling
Not implemented in the first rules profile. The engine uses the Axis Naval Convoy Level Chart as
printed (one letter per month). The Graziani convoy letters (September-October 1940) are B and B.

## Rationale
The text is an unfinished optional variant, not a complete rule; D2 asks for a faithful baseline.

## Affected behaviour and tests
Convoy scheduling reads `airlog.56.4.axis_naval_convoy_level` only; no rerouting action is offered.
