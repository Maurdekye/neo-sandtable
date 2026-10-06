# land-0008 — Self-propelled guns absorbing Anti-Armor damage (14.47)

- **Cases:** land:14.47, land:12.1
- **Status:** adopted
- **Profile version:** v1
- **Decided by:** rules-land, 2026-10-06
- **Owner review:** reviewed 2026-10-06 by the owner (batch review 1)

## Question
Case 14.47 says SP guns absorb Anti-Armor Damage Points at Armor Protection plus Vulnerability; an attached correction says that is wrong and SP guns use Armor Protection only.

## Evidence
The 2021 text prints both; the correction calls the original text an unwarranted developer change and adds that an SP gun that is barraging in the Back position neither absorbs nor is affected by Anti-Armor fire.

## Ruling
SP guns absorb damage at their Armor Protection rating only, like other armored units. An SP gun in a Back barrage position is excluded from Anti-Armor fire altogether (as 14.13 says for Back anti-armor guns).

## Rationale
The correction is explicit and more specific; this is the form D3 takes as baseline.

## Affected behaviour and tests
Damage assignment (14.43). Test: an SP gun with Armor Protection 2 absorbs 2 Damage Points regardless of its Vulnerability; a Back SP gun is not a legal damage sink.
