# airlog-0023 - Owning-player refit success priority

- **Cases:** airlog:38.34, airlog:38.35, airlog:38.38, airlog:38.39
- **Status:** proposed
- **Profile version:** cna-2021-dev; cna-2021-full, pending reviewed maintenance caller activation
- **Decided by:** neo-sandtable (lead), 2026-10-09, Rules461 provisional implementation ruling
- **Owner review:** pending NEXT consequential owner batch
- **Implementation:** provisional reading authorized; live cohort/finish selection is not implemented by this document

## Question

The percentage table determines how many aircraft refit successfully, but does not identify which aircraft receive those successes.

## Evidence

38.34 provides a rounded-up success percentage for the aircraft attempting the squadron method.38.35 supplies assignment and aircraft-nationality modifiers.38.39 offers a distinct individual-plane dice method. None specifies random success identities or a further post-roll selection round for the percentage method.

## Ruling

For each source-valid roll cohort, the owner supplies an exact permutation of its attempted PlaneIds before resolution. Finish computes N successful refits using the bound percentage method, with fractions rounded up, and refits the first N IDs in that priority. A malformed order, repeated ID, omitted attempted ID or foreign identity is not a default priority. Zero successes refit none; complete success refits all attempted IDs.

The optional per-plane method retains the actual two-dice AircraftRefit binding for each specific PlaneId, including its foreign modifier and nationality range. Its successes are the actual IDs whose rolls pass; it does not apply a percentage or choose another default. No duplicated nationality thresholds or standard-method modifier is added to that binding.

## Rationale and alternatives

Predeclared priority preserves owning-player control and a single atomic finish. Random identity selection would introduce an extra rule; a fixed both-Air post-roll choice would require persisted private outcomes and another reviewed transaction/disclosure boundary. Both alternatives are retained but not selected.

## Affected behaviour and tests

Pin exact first-N outcomes, zero/full/fractional counts, aircraft-nationality and foreign-cohort modifiers, full permutation validation, actual per-plane identities, unchanged unattempted planes and stable RNG order. Paired enemy tests cover hidden priority and outcomes with fixed both-Air/pass windows. Respond buffers only; checkpoint/reentry cannot reroll, and late error rolls back whole State/Cx/RNG/events.
