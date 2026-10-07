# land-0023 - Visible combat counters unlock repeated movement

- **Cases:** land:8.23, land:18.0, land:3.62
- **Status:** adopted
- **Profile version:** cna-full-v1 / cna-dev-v1
- **Decided by:** project owner, 2026-10-07 (batch 2; rules-as-written fog)
- **Owner review:** reviewed 2026-10-07 by the owner (batch review 2; fog rules as written)

## Question
The two clauses of the repeated-movement restriction use different enemy categories. Should a nearby noncombat counter permit a further move?

## Evidence
land:8.23 first permits another move when an enemy unit is nearby. Its following restriction identifies enemy combat units and bars further movement when none is within two hexes. Reserve status supplies an explicit exception. land:8.21 and land:8.22 give the repeating sequence to whichever side currently phases. Under land:3.62 the opponent can see the printed face of independent and formation parent map counters; units attached on log sheets and variable contents remain secret.

## Ruling
A unit may move again in that half when its preceding Movement Segment ends within two hexes of a visible enemy map counter whose printed type is a combat type, or when the reserve exception applies. A visible noncombat counter does not qualify. A parent or headquarters qualifies only by its own printed face, irrespective of hidden attached combat units. Capability-point spending remains cumulative within the OpStage.

## Rationale
The operative restriction names combat units. The owner's rules-as-written visibility decision makes the relevant printed counter type public without disclosing attachments, strength, status or any other variable content. A proximity test over those visible faces therefore needs no additional disclosure exception.

## Superseded provisional reading
The earlier secrecy-audit proposal used any enemy unit because the then-current engine exposed only stack presence. The owner rejected that visibility model in batch 2. This adopted reading replaces the temporary any-unit predicate; it does not permit inspecting hidden log-sheet units.

## Affected behaviour and tests
The continual-movement predicate uses the canonical visible-counter query and printed combat classification. Regressions cover distance two versus three, both phasing halves, reserve exceptions and cumulative CP. Privacy pairs retain the same visible parent face while changing its hidden attached combat contents; both worlds must expose identical repeat eligibility.
