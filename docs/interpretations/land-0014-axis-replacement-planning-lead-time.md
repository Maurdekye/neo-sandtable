# land-0014 — Axis replacement planning lead time (20.63)

- **Cases:** land:20.63, land:20.21, land:20.6
- **Status:** proposed
- **Profile version:** v1
- **Decided by:** rules-land, 2026-10-06
- **Owner review:** pending

## Question
20.63 prints a rewritten rule (schedule replacements at least two weeks ahead on the Axis naval convoys; points for the third week of a month are scheduled in the first week) followed by the original version (plan two Game-Turns ahead in the Naval Convoy Arrival Phase; June III planning arrives in July I). The two versions give different lead times in places, and 20.21 and the pool tables speak of arrival two Game-Turns after planning.

## Evidence
The 2021 text labels the first paragraph "(rewrite)" and the second "(original:)". The pool table notes (20.66) say points planned on a turn arrive two Game-Turns later, which agrees with the rewrite's two-week lead and the original's two-turn lead.

## Ruling
Use the rewrite: replacement points from the pool are scheduled in the Naval Convoy Schedule Phase of a Game-Turn at least two Game-Turns before they arrive, and arrive in any OpStage of the scheduled Game-Turn via the convoy mechanism. The original paragraph is treated as superseded and is not used.

## Rationale
The rewrite is the later text and the tables agree with it.

## Affected behaviour and tests
Replacement planning window. Test: points scheduled in Game-Turn N cannot arrive before Game-Turn N+2.
