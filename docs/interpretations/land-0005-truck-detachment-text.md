# land-0005 — Garbled text in truck attach/detach case (8.97)

- **Cases:** land:8.97
- **Status:** adopted
- **Profile version:** v1
- **Decided by:** rules-land, 2026-10-06
- **Owner review:** reviewed 2026-10-06 by the owner (batch review 1)

## Question
The second and third sentences of 8.97 are broken (a clause appears to be missing), and a clarification note says a line was dropped.

## Evidence
The clarification says: when a parent unit detaches subsidiary units during Movement and Combat, a number of its attached trucks may go with the detached units. The cohesion restriction (-5 or worse cannot detach all trucks) is stated separately.

## Ruling
Trucks attach to a historically designated unit only in the Organization Phase, except that when the unit is itself attached to a parent in that stage; trucks detach in the Organization Phase, or during Movement and Combat when a parent detaches subsidiary units, in which case any share of the parent's trucks may go with the detached unit. A unit at cohesion -5 or worse cannot detach all of its trucks.

## Rationale
Follows the clarification note in the 2021 text.

## Affected behaviour and tests
Attach/detach validation. Test: detaching a battalion from a brigade in Movement lets the player send any subset of the brigade's trucks along; a -6 cohesion unit may not detach all.
