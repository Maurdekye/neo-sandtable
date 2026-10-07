# air-0003 - Fighter abort uses a ten-point maneuver deficit allowance

- **Cases:** airlog:39.35, airlog:39.37, airlog:40.26
- **Status:** adopted
- **Profile version:** v0.1
- **Decided by:** rules-air, 2026-10-07
- **Owner review:** reviewed 2026-10-07 by the owner (batch review 3)

## Question
The verbal condition permits a maneuver deficit of at most ten, but the printed subtraction instruction and sign examples do not consistently express that condition.

## Evidence
39.35 describes a fighter whose maneuver is no more than ten below the highest opposing CAP fighter. Its arithmetic explanation reverses the sign associated with the sample acceptable values. 40.26 limits voluntary withdrawal by offensive CAP;39.37 prevents escort fighters abandoning a screen they provide.

## Ruling
A fighter meets the maneuver condition when enemy_best_maneuver - own_maneuver <= 10. With no enemy CAP the maneuver condition is met. Offensive-CAP commitments and a screen that must remain still impose their independent restrictions.

## Rationale
This directly encodes the stated ten-point allowance and supplies a consistent boundary. It avoids the literal reversed subtraction admitting the aircraft that the verbal condition excludes.

## Affected behaviour and tests
Planned: deficits9/10/11, positive advantage, absent CAP, offensive CAP and screen-lock tests. All enemy-dependent eligibility is resolved after recorded abort choices, not during answer acceptance. Not implemented by inventory import.
