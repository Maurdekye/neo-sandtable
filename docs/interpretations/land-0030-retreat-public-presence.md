# land-0030 - Retreat allowance uses visible printed combat counters

- **Cases:** land:13.23, land:13.24, land:3.62
- **Status:** adopted
- **Profile version:** cna-2021-dev / cna-2021-full
- **Decided by:** owner, 2026-10-07 (batch review 2; fog rules as written)

Owner review: reviewed 2026-10-07 by the owner (batch review 2; fog rules as written).

## Question
The retreat allowance depends on initial adjacency to enemy combat units. Which disclosed facts determine that allowance when a formation counter conceals its component units?

## Evidence
Land:13.23 and land:13.24 distinguish initially adjacent units from other retreating units. Land:3.62 permits inspection of printed counter faces while keeping their contents private. The owner adopted that disclosure contract in batch review 2. This replaces the provisional presence-only ruling.

## Ruling
When retreat before assault opens, collect enemy map counters using `view::is_map_counter`. Each qualifying counter counts as combat only if its own printed identity satisfies `view::printed_combat_face`. A formation uses its counter's printed type; hidden combat members do not grant a noncombat HQ the combat adjacency allowance. Attached contents contribute no separate counter face.

Snapshot adjacency to those combat counters for each eligible own retreating unit. Initial combat adjacency grants the land:13.23 allowance; otherwise apply land:13.24's four-CP or one-hex limit. The recorded adjacency survives answers, enemy changes and checkpoint recovery. Neither validation nor resolution reclassifies a concealed occupant to change that opening allowance.

## Rationale
Printed identity and type are disclosed information. Using them follows the combat-unit distinction without making the retreat validator reveal hidden formation contents, strength or supplies. A noncombat counter's presence alone is insufficient; a supply-dump marker is not a land combat counter. This adopted retreat ruling does not change other proximity procedures.

## Tests and delivery boundary
Real printed combat and HQ identities produce different retreat caps and reachable catalogs. Paired HQ states with and without a hidden combat member retain the same requests, observations, reachability and validation. Rejected over-cap plans leave the complete input game unchanged; accepted plans remain buffered until closure. Tests also retain the opening cap across enemy changes and serialized recovery, then compare execution and events.

Normal movement legality, contact costs, fuel and per-member CP accounting remain in force. This change does not fill optional dump demolition under land:13.25 or other full-profile gaps, and does not adopt fallible query APIs or Engineering activity callbacks.
