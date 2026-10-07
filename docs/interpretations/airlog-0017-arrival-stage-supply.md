# airlog-0017: Supplies for newly arrived land units

- **Cases:** land:20.12, airlog:49.14, airlog:49.16, airlog:51.11, airlog:52.11, airlog:52.13, airlog:52.41, airlog:56.28
- **Status:** adopted
- **Profile version:** cna-2021-full / cna-2021-dev
- **Decided by:** neo-sandtable, rules-airlog and oob, 2026-10-07
- **Owner review:** reviewed 2026-10-07 by the owner (batch review 2)

## Question
How can reinforcements obtain the supplies needed for their permitted movement in the arrival OpStage, when the ordinary organization supply windows have already closed?

## Evidence
Case20.12 permits movement in the arrival stage. The sequence places land arrivals after organization. Cases49.14,51.11 and52.41 require actual supplies rather than an arrival allowance. Case56.28 provides a port stock destination for delivered convoy goods;52.11 and52.13 identify water sources and the CP cost of using them.

## Ruling
After land arrival placements and withdrawals close, deliver the stage's supply convoys. Then always open one simultaneous owner-private batched supply window per side, restricted to the exact successfully placed new unit ids. After both lists close, adjudicate issues and well draws, then always open a second simultaneous well-water allocation window per side. Owners allocate their actual yields, and passing discards unallocated water. Each round includes empty forced-pass requests, answered locally without a model call, so hidden arrivals or draws cannot change the number or timing of requests. Assess remaining shortages only after both second-round requests close.

The window can issue stores, infantry water, activity water and tank fuel from lawful same-location sources, including the unit's first-line holdings. It can draw an actual well at that location, paying the ordinary CP and resolving its hidden condition after accepted lists close. Second-line truck cargo still requires unloading under the ordinary rules; this window does not make it a first-line source.

No stock or allowance is created. An unsupplied reinforcement keeps the printed ration and water restrictions. Shortage finalization applies only to the new units, preserving the assessments of units supplied earlier in the stage. Completed windows and accepted well draws survive checkpoints.

## Rationale and alternative
This lets20.12 operate without granting free supply. The alternative is to wait for the next organization phase, leaving newly arrived units unable to use much of their same-stage movement permission. The timing choice is consequential and requires the owner's review.

## Tests
Real Graziani units consume actual dump supplies and keep older units' ration history unchanged. Passing with no stocks records shortages. Whole-list overdraw and foreign-unit selections reject atomically. Multi-seed owner baselines use actual stock and city wells. Hidden poisoned versus clean well states accept the same answer before finish-time disclosure, and enemy views hide allocations. Paired real-dispatcher transitions, streams and checkpoint replay have identical observer requests, clocks and phases when the other side has a private off-map arrival or requests a well draw.
