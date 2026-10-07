# land-0026 - Replan movement after reaction

- **Cases:** land:8.13, land:8.51, land:9.31, land:9.32
- **Status:** proposed
- **Profile version:** cna-full-v1 / cna-dev-v1
- **Decided by:** neo-sandtable, 2026-10-07
- **Owner review:** pending

## Question
An ordered path supplies movement choices ahead of time. What happens when a defender reacts during that path, especially in an overfull transit hex?

## Evidence
land:8.51 interrupts movement with a defender's reaction. land:9.31 prohibits an overfull endpoint, while land:9.32 permits transit. The printed procedure lets the phasing player choose each following hex after seeing the reaction; it does not commit a future route.

## Ruling
After reaction, the current move remains suspended and its owner chooses a new remaining path for that same moving unit or represented stack. Stopping is offered only where stacking is legal. Accepted reaction is never undone. Subsequent ordered moves wait until this move finishes. If no legal continuation exists, full reports Unsupported land:9.31; dev stops the unit and privately reports unresolved overstacking, without inventing a numeric limit.

## Rationale
An upfront path is a plan for the ordinary sequence of choices. An interrupt restores the choice that would occur in the physical game.

## Affected behaviour and tests
Reaction checkpoints preserve the interrupted membership, queued later orders and suspended seat decisions. Tests cover replanning, legal stopping, overfull transit, recovery, and owner-only pending path details.
