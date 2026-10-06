# airlog-0007 ? Ration shortages and fractional storage losses

- **Cases:** airlog:49.3, airlog:51.12, airlog:51.21, airlog:51.22, airlog:51.23, land:28.15
- **Status:** proposed
- **Profile version:** cna-2021-full / cna-2021-dev
- **Decided by:** rules-airlog, 2026-10-07
- **Owner review:** pending

## Question
The shortage rules do not spell out partial rations, fractional prisoner groups, or the treatment of a fractional fuel balance during storage losses.

## Evidence
Case51.21 penalizes a unit when some of its strength receives insufficient stores. Case51.22 increases infantry losses with consecutive food shortages. Case51.23 permits two stores per strength point only when supplies are insufficient. Case28.15 groups prisoners by hex;51.12 charges one stores point for each group of five. Case49.3 rounds storage losses down, while airlog-0001 retains fractional fuel consumption.

## Ruling
A partial full ration counts as a short week. A completed legal half ration satisfies food needs and imposes its printed movement limits. Half rations are available only when stocks accessible at that unit's location cannot cover its full ration; HQ and engineer flat rates are not halved.

Consecutive short weeks2,4,6 incur infantry losses at2%,4%,6%, respectively; odd weeks incur only the weekly cohesion penalty. Round the loss on the total affected infantry strength in a hex, grouping equal consecutive-shortage histories, then let the owner allocate the resulting infantry losses. Weapons are excluded. Completing either legal ration resets the consecutive-shortage count.

Aggregate prisoner points belonging to the same captor and location before rounding their required stores upward to a whole point. Serve prisoners before guards and units. Separate guards consume two stores each per week.

Evaporation removes whole supply points. A fuel tank keeps its fractional balance: calculate its percentage loss in fuel points, round that loss down, and subtract whole points from its exact tenths balance. Water reserved in radiators is included in the weekly storage loss, while the hot-weather adjustment continues to exclude tanks/radiators under29.34.

## Rationale
The partial-ration reading follows the explicit penalty for an incompletely fed unit. Half rations have their own stated cost and penalties. Hex aggregation avoids making casualties or prisoner consumption depend on how counters are split into bookkeeping groups. Whole-point storage loss preserves the specified rounding without discarding pre-existing fractional fuel.

## Affected behaviour and tests
`logistics::stores` records weekly rations, mandatory prisoner/guard consumption, private shortages, and rounded storage losses. Graziani tests pin full/half/partial rations, repeat protection, source rejection, nearest-source exemptions, checkpoint-compatible state, and enemy secrecy. Infantry attrition allocation is implemented by the separate attrition procedure.
