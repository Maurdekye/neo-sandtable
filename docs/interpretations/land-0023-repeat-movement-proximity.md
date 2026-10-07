# land-0023 - Enemy combat units unlock repeated movement

- **Cases:** land:8.23, land:18.0
- **Status:** proposed
- **Profile version:** cna-full-v1 / cna-dev-v1
- **Decided by:** rules-land, 2026-10-07
- **Owner review:** pending

## Question
The two clauses of the repeated-movement restriction use different enemy categories. Should a nearby noncombat unit permit a further move?

## Evidence
land:8.23 first permits another move when an enemy unit is within two hexes. Its following restriction identifies enemy combat units and bars further movement when none is nearby. Reserve status supplies an explicit exception. land:8.21 and land:8.22 give the repeating sequence to whichever side currently phases.

## Ruling
A unit may move again in that half only if its preceding Movement Segment ends within two hexes of an enemy combat unit, or the reserve exception applies. An enemy truck, dump, or other noncombat presence alone does not qualify. The current phasing side may repeat; capability-point spending remains cumulative within the OpStage.

## Rationale
The restrictive clause defines the required category more precisely. This keeps proximity to noncombat logistics from creating an extra movement entitlement.

## Affected behaviour and tests
Cycle and reserve procedures test a combat unit at distances two and three, a noncombat neighbour, the reserve exception, both phasing halves, and retained CP after repetition. The reserve exception is supplied by the reserve procedure.
