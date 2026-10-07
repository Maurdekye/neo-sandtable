# land-0023 - Public enemy presence unlocks repeated movement

- **Cases:** land:8.23, land:18.0
- **Status:** proposed
- **Profile version:** cna-full-v1 / cna-dev-v1
- **Decided by:** neo-sandtable, 2026-10-07
- **Owner review:** pending

## Question
The two clauses of the repeated-movement restriction use different enemy categories. Should a nearby noncombat unit permit a further move?

## Evidence
land:8.23 first permits another move when an enemy unit is within two hexes. Its following restriction identifies enemy combat units and bars further movement when none is nearby. Reserve status supplies an explicit exception. land:8.21 and land:8.22 give the repeating sequence to whichever side currently phases.

## Ruling
A unit may move again in that half only if its preceding Movement Segment ends within two hexes of any enemy unit on the map, or the reserve exception applies. A nearby enemy noncombat counter qualifies. A dump alone does not count as a unit. The current phasing side may repeat; capability-point spending remains cumulative within the OpStage.

## Rationale
The first clause explicitly permits proximity to any enemy unit. This reading depends only on public stack presence, preserving limited intelligence without introducing an additional disclosure rule.

## Alternative for owner review
The second clause can instead be read to require an enemy combat unit. That would make repeat eligibility a disclosed bit about hidden enemy stack contents. Adopting that alternative would require an explicit disclosure exception under land:3.6. The project initially proposed that narrower reading, then the secrecy audit identified its information channel; this proposal now uses the first clause pending owner review.

## Affected behaviour and tests
Cycle and reserve procedures test an enemy unit at distances two and three, a noncombat neighbour, the reserve exception, both phasing halves, and retained CP after repetition. A shared-harness pair replaces an enemy combat stack with noncombat units at the same public hex and checks every own-side read surface. The reserve procedure supplies its exception.
