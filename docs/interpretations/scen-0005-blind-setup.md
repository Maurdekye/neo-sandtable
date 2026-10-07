# Blind simultaneous scenario setup

- **Cases:** scen:59.2, scen:60.31, scen:60.41, land:3.62
- **Status:** adopted
- **Profile version:** cna-2021-dev / cna-2021-full
- **Decided by:** the owner, 2026-10-06
- **Owner review:** reviewed 2026-10-06 by the owner (batch review 1)

## Question
What order and visibility govern player-selected setup positions, and which enemy locations can an exclusion inspect?

## Evidence
Scen:59.2 permits choices among listed hexes and within scenario placement domains, but the setup instructions examined do not establish a side ordering. Land:3.62 exposes stack presence during play while protecting composition. Some setup lines restrict distance to Commonwealth units. The lead proposed this ordering on2026-10-06; the owner adopted it in interpretation batch1.

## Ruling
Both sides submit setup choices privately in one simultaneous window for their seats, using SecretSimultaneous decisions. Newly selected stack positions become public only when the entire window closes, and then reveal presence without composition.

Distance exclusions inspect only enemy positions fixed by the scenario. Freely selected enemy positions are not inputs to an exclusion. Thus an accepted distance choice cannot fail when another private answer arrives later.

Answer validation never reads the opponent's hidden destinations. It may use the answering side's assets, the public map and enemy set-up positions fixed by the scenario. This restriction applies to validation preflight as well as submitted answers.

After the window's answers are collected, the set-up completion procedure adjudicates opposing occupancy conflicts. For each contested hex, the side with the latest accepted placement order reopens its affected placements. Accepted destinations then expose stack presence, so those retry decisions use public occupancy. Unaffected decisions remain accepted. A friendly convoy destination depending solely on a removed placement is rechecked and reopened if necessary. Known stacking limits use land:9; development records a private unassessed-constraint note where the terrain or exemption is unsupported, and full play stops with the governing case.

The lead corrected the original answer-time rejection on 2026-10-07 after review identified a hidden-placement validation oracle. The adopted blind window and fixed-position exclusion policy remain in effect; cross-side adjudication now occurs only at completion, following docs/engine.md section 3.

## Rationale
The fixed scenario positions are common knowledge. Basing exclusions on them gives both sides stable choice domains while allowing independent private setup answers.

## Affected behaviour and tests
The setup procedure buffers new positions until all required decisions close. Its tests cover fixed-position exclusions, private choices before closure, presence-only views after closure, and retry of an opposing occupancy conflict at closure, and identical answer legality when only the enemy's private buffer differs. Implementation is in progress in the setup docket item.
