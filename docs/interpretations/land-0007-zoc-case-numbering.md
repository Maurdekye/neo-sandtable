# land-0007 — Case 10.6 is printed out of order

- **Cases:** land:10.6, land:10.1
- **Status:** adopted
- **Profile version:** v1
- **Decided by:** rules-land, 2026-10-06
- **Owner review:** reviewed 2026-10-06 by the owner (batch review 1)

## Question
The case that requires the non-phasing player to say whether a unit exerts a ZOC is numbered 10.6 but sits inside 10.1 (after 10.15) and before 10.2. Is it a misprint for 10.16?

## Evidence
It appears between 10.15 and 10.2 in the 2021 retype and there is no Section 10.3-10.5 content matching it (10.3 is the holding-off section).

## Ruling
Treat the case as printed (id 10.6) in the registry, but understand it as the last case of 10.1 (functionally 10.16): the non-phasing player must truthfully answer whether a unit exerts a ZOC into a hex when a phasing unit starts or ends movement there.

## Rationale
Keeps the registry id equal to the printed id (completeness check) while documenting the intended placement.

## Affected behaviour and tests
Visibility filter for ZOC queries. Test: a phasing move into a hex adjacent to an enemy that does not exert ZOC (under 10 raw defensive points) is answered "no".
