# Withdrawal deadlines and missing transport

- **Cases:** land:4.43a, land:20.82, land:20.83, land:20.84, land:20.85
- **Status:** proposed
- **Profile version:** cna-2021-dev / cna-2021-full
- **Decided by:** neo-sandtable, interim implementation instruction, 2026-10-07
- **Owner review:** pending (batch review 2)

## Question
Does failing a scheduled truck-value minimum eliminate a withdrawing formation, or should the formation leave with available trucks? The rules state a consequence for missing location or TOE, but do not give a consequence for the schedule's separate transport minimum.

## Evidence
Land:4.43a prints a full-Logistics truck-value requirement and an abstract-Logistics motorization requirement for Commonwealth withdrawals. Its footnote supplies truck weights; these are stored in integer halves. Land:20.83 permanently eliminates a scheduled unit that misses Cairo/Alexandria or the required TOE by its deadline. Land:20.85 permits a similar substitute with at least three quarters of maximum TOE. No examined case extends the elimination consequence to missing trucks.

## Ruling
The interim implementation offers same-type, same-echelon substitutes at or above the required TOE; infantry also matches the source-bound infantry kind. Replacements are a separate procedure and are not offered here. A unit missing the stated destination or strength deadline is eliminated permanently under 20.83.

For transport shortages, full play stops with Unsupported citing 4.43a/20.83. Development withdraws the eligible units and accompanying trucks, supplements them with eligible empty trucks at Cairo/Alexandria up to the minimum, and sends only the Commonwealth side a note stating the remaining shortage. It assigns no additional elimination penalty.

## Alternatives for owner review
1. Apply the deadline elimination mechanism to a formation that cannot supply its printed minimum transport. This extends 20.83 beyond its explicit location/TOE grounds and requires an owner ruling.
2. Withdraw the eligible formation with available transport, recording the shortage. This preserves the explicit unit deadline without introducing an unstated truck penalty; it is the development reading pending review.

## Non-combat headquarters

A headquarters shell with no numerical TOE of its own does not receive an invented three-quarter strength check. Land:3.33/3.34 distinguish the shell from its attached fighting units. Each selected combat counter is checked against its own printed maximum; a headquarters with numerical tank or gun strength keeps that check. The lead approved this second interim reading on 2026-10-07, and it is included for owner batch 2 review.

## Rationale
The second reading separates a source-defined unit penalty from an unspecified transport consequence. Strict play exposes the gap rather than resolving it silently. Neither reading invents trucks, replacement points or a numerical penalty.

## Affected behaviour and tests
Only the Italian Campaign currently reaches these withdrawals: 17 named counters in three scheduled groups. Graziani ends before the first withdrawal. Tests cover all 17 named counters at their three deadlines, subtree exclusions, substitute thresholds, permanent elimination and owner-only shortage notes.

Headquarters substitutes use the same printed class id, because the broad headquarters type also covers unarmed shells, tank HQs and artillery HQs. A different class requires an explicit equivalent-role record in content; ratings do not supply that equivalence. Infantry substitutes retain the source-bound infantry_kind constraint. A regression excludes tank and unarmed HQs from the candidates for a weak artillery HQ.
