# air-0001 - Night air combat follows the specific search procedure

- **Cases:** airlog:39.44, airlog:40.91, airlog:41.46, airlog:41.47
- **Status:** proposed
- **Profile version:** v0.1
- **Decided by:** rules-air, 2026-10-07
- **Owner review:** pending (consequential)

## Question
General night-flight text excludes air combat, while the detailed night-bombing cases describe search, night-fighter scramble and air combat after a successful search.

## Evidence
39.44 limits interception and describes night safety broadly. 41.46 permits same-hex searches by CAP and night-fighter scramble; 41.47 says a successful search proceeds to air combat. 40.91 provides a scramble die modifier for night fighters.

## Ruling
Keep the general ban on en-route night interception. Permit same-hex night air combat only through the detailed search procedure: eligible CAP or night scramble must first succeed at search under 41.47. Day missions finish before the night pipeline begins.

## Rationale
The specific mission procedure provides meaningful search and scramble rules; treating the general sentence as overriding it would leave those cases without an operational effect.

## Affected behaviour and tests
Planned: fixed day/night sequence and night search/combat tests covering unsuccessful search, successful CAP search, night-fighter modifiers and no night air ZOC. This inventory foundation does not yet implement night combat.
